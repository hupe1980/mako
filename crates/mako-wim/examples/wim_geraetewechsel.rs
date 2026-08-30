//! # WiM Gerätewechsel — mako-wim + edi-energy end-to-end example
//!
//! Demonstrates the full write→store→read cycle for a WiM "Gerätewechsel"
//! (PID 55042 — Anmeldung MSB, nMSB → NB) process using the `mako-engine` event-sourced runtime,
//! `mako-wim` domain logic, and `edi-energy` for EDIFACT parsing.
//!
//! ## Key difference from GPKE
//!
//! WiM runs **two clocks on one inbound order**, and the example arms both:
//!
//! - the technical APERAK acknowledgement — **45 minutes** for a Strom UTILMD
//!   received on a Werktag (`fristen::aperak_strom_due_at`), and
//! - the **business Antwortfrist** — 3 / 5 / 7 / 1 Werktage per PID
//!   (`antwort_frist_werktage`), answered with a UTILMD on the answer PID.
//!
//! Sending the APERAK does not discharge the Antwortfrist. `Complete` refuses
//! to run from `AperakSent` for exactly that reason: an acknowledged order
//! whose business answer never went out is the failure the split makes visible.
//!
//! ## Architecture boundary demonstrated
//!
//! ```text
//! edi-energy (transport boundary)          mako-wim (pure domain)
//! ─────────────────────────────────────    ────────────────────────────
//! Platform::parse(raw_bytes)             → DeviceChangeCommand { pid, … }
//! msg.validate()                         → WimDeviceChangeWorkflow::handle()
//! extract sender/receiver/melo           → pure, no I/O, deterministic
//! ```
//!
//! ## Deadline helpers — reference table
//!
//! | Process | Frist | Helper |
//! |---|---|---|
//! | GPKE Lieferbeginn | 11:00 Uhr des 1. WT nach dem ÜT | `mako_fristen::antwort` |
//! | WiM MSB-Wechsel (Antwort) | 3 / 5 / 7 / 1 WT je PID | `mako_wim::antwort_frist_werktage` |
//! | APERAK Strom | 45 Minuten (UTILMD/ORDERS am Werktag) | `fristen::aperak_strom_due_at` |
//! | APERAK Gas | nächster WT 12:00, bzw. 3 WT auf einem Initialprozess | `fristen::aperak_gas_due_at` |
//!
//! ## Run
//!
//! ```text
//! cargo run --example wim_geraetewechsel -p mako-wim
//! ```

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::{EventStore, InMemoryEventStore},
    ids::TenantId,
    inbox::{InMemoryInboxStore, InboxStore, inbox_key},
    outbox::{InMemoryOutboxStore, OutboxMessage, OutboxStore},
    projection::ProjectionRunner,
    registry::{InMemoryProcessRegistry, ProcessRegistry, RegistryKey},
    snapshot::InMemorySnapshotStore,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::CommandContext,
};
use mako_fristen::{self as fristen};
use mako_wim::{DeviceChangeCommand, DeviceChangeProjection, WimDeviceChangeWorkflow};

// ── EDIFACT fixture ───────────────────────────────────────────────────────────
//
// Minimal WiM UTILMD Anmeldung Messstellenbetrieb (PID 55042 — Anmeldung MSB, nMSB → NB).
// BGM+E01 is used for WiM Anmeldung in UTILMD S2.x.
// NAD+MS = neuer Messstellenbetreiber (nMSB, sender)
// NAD+MR = Netzbetreiber (NB, receiver)
// - IDE+24 = Messlokation identifier (24 qualifier for WiM MSB, per AHB-55042)
// - MeLo ID: 51238696781 (11-char format, [A-Z0-9]{11})

const UTILMD_GERAETEWECHSEL: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+WIM-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055042::+9'\
DTM+137:202501150800?+00:303'\
RFF+Z13:WIM-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+24+51238696781::'\
UNT+8+MSG-001'\
UNZ+1+WIM-2025-001'";

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  mako-wim — WiM Gerätewechsel end-to-end example           ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();

    // ── Infrastructure via EngineBuilder ──────────────────────────────────────
    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_outbox_store(InMemoryOutboxStore::new())
        .with_deadline_store(InMemoryDeadlineStore::new())
        .with_registry(InMemoryProcessRegistry::new())
        .build();

    let inbox = InMemoryInboxStore::new();

    // Snapshot every 2 events (low threshold for illustration;
    // production value is typically 100–200).
    const SNAP_INTERVAL: u64 = 2;

    let process = ctx.spawn::<WimDeviceChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    println!("  Stream : {}", process.stream_id());
    println!();

    // ── Step 1: Parse + validate EDIFACT — transport boundary ─────────────────
    println!("[1/7] Parsing EDIFACT bytes with edi-energy...");

    let msg = Platform::with_all_profiles().parse(UTILMD_GERAETEWECHSEL)?;
    let msg_type = msg
        .try_message_type()
        .map(|t| t.as_str().to_owned())
        .unwrap_or_else(|| "Unknown".to_owned());
    let release = msg.detect_release()?.as_str().to_owned();
    let pid = Pruefidentifikator::new(msg.detect_pruefidentifikator()?.as_u32())
        .map_err(|e| anyhow::anyhow!(e))?;
    let msg_ref = MessageRef::new(msg.message_ref());

    let (sender, receiver, melo_id, device_id, document_date) = if let AnyMessage::Utilmd(u) = &msg
    {
        (
            MarktpartnerCode::new(
                u.sender()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or_default(),
            ),
            MarktpartnerCode::new(
                u.receiver()
                    .and_then(|n| n.party_id.as_deref())
                    .unwrap_or_default(),
            ),
            MeLo::new(
                u.transactions()
                    .first()
                    .and_then(|tx| tx.ide.object_id.as_deref())
                    .unwrap_or_default(),
            ),
            // device_id from LOC+172 — use a fallback if not parsed
            DeviceId::new("ZHR-12345678"),
            u.dtm()
                .iter()
                .find(|d| d.is_document_date())
                .and_then(|d| d.value.clone())
                .unwrap_or_default(),
        )
    } else {
        unreachable!("fixture is always UTILMD")
    };

    let report = msg.validate()?;
    let validation_passed = report.is_valid();
    let validation_errors: Vec<String> = report
        .errors()
        .iter()
        .map(|i| {
            if let Some(rid) = i.rule_id() {
                format!("[{rid}] {i}")
            } else {
                format!("{i}")
            }
        })
        .collect();

    println!("  ✓ Message type : {msg_type}");
    println!("  ✓ Release      : {release}");
    println!("  ✓ PID          : {pid}  (WiM Anmeldung MSB — nMSB → NB)");
    println!("  ✓ Sender (nMSB): {sender}");
    println!("  ✓ Receiver (NB): {receiver}");
    println!("  ✓ MeLo         : {melo_id}");
    println!("  ✓ Device       : {device_id}");
    println!(
        "  ✓ Validation   : {} ({} issues)",
        if validation_passed {
            "passed"
        } else {
            "failed"
        },
        validation_errors.len()
    );

    // ── Step 2: Inbox deduplication ───────────────────────────────────────────
    println!();
    println!("[2/7] Inbox deduplication...");

    let key = inbox_key(sender.as_str(), msg_ref.as_str()).map_err(|e| anyhow::anyhow!(e))?;
    if !inbox.accept(&key).await? {
        println!("  ✗ DUPLICATE — idempotency key: {key}");
        return Ok(());
    }
    println!("  ✓ New message accepted — key: {key}");

    // ── Step 3: ReceiveUtilmd — domain command (pure, no I/O) ────────────────
    println!();
    println!("[3/7] Dispatching ReceiveUtilmd...");

    let envs = process
        .execute(DeviceChangeCommand::ReceiveUtilmd {
            transaktionsgrund: Some("E03".to_owned()),
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            melo_id: melo_id.clone(),
            device_id: device_id.clone(),
            document_date: document_date.clone(),
            message_ref: msg_ref.clone(),
            vorgangsnummer: Some("VG-TEST-001".to_owned()),
            process_date: Some("20260401".to_owned()),
            validation_passed,
            validation_errors: validation_errors.clone(),
            received_at: time::OffsetDateTime::now_utc(),
        })
        .await?;

    for env in &envs {
        println!(
            "  ✓ {} (seq {}, schema_v{})",
            env.event_type, env.sequence_number, env.schema_version
        );
    }

    let utilmd_conversation_id = envs[0].conversation_id;
    let utilmd_event_id = envs[0].event_id;

    // ── APERAK sending window (APERAK AHB 1.1 §2.4.1) ────────────────────────
    //
    // The technical acknowledgement, on its own clock: **45 minutes** for a
    // Strom UTILMD received on a Werktag, Sonntag 12:00 after a Saturday. It is
    // not the business Antwortfrist, which is 3 / 5 / 7 / 1 Werktage per PID.
    let received_at = time::OffsetDateTime::now_utc();
    let aperak_due_at = fristen::aperak_strom_due_at(received_at);

    let aperak_deadline = Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        process.tenant_id(),
        process.workflow_id().clone(),
        fristen::APERAK_STROM_WINDOW_LABEL,
        aperak_due_at,
    );
    let aperak_deadline_id = aperak_deadline.deadline_id();
    ctx.deadline_store().register(&aperak_deadline).await?;
    println!(
        "  [deadline] APERAK window registered (45 min — due {aperak_due_at}, id: {}…)",
        &aperak_deadline_id.to_string()[..8]
    );

    // ── Business Antwortfrist (WiM Teil 1, per PID) ──────────────────────────
    //
    // The second clock. 55042 „Anmeldung MSB" answers in 5 Werktagen; the
    // number is read off the PID rather than assumed, because the four WiM
    // MSB-Wechsel chapters give 3 / 5 / 7 / 1 WT.
    let antwort_wt = mako_wim::antwort_frist_werktage(pid.as_u32())
        .expect("55042 is a WiM MSB-Wechsel Prüfidentifikator");
    let antwort_due_at =
        fristen::deadline_at_werktage(received_at, antwort_wt, fristen::HolidayCalendar::BdewMaKo);
    let antwort_deadline = Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        process.tenant_id(),
        process.workflow_id().clone(),
        mako_wim::GERAETEWECHSEL_ANTWORT_FRIST_WINDOW_LABEL,
        antwort_due_at,
    );
    let antwort_deadline_id = antwort_deadline.deadline_id();
    ctx.deadline_store().register(&antwort_deadline).await?;
    println!(
        "  [deadline] Antwortfrist registered ({antwort_wt} WT — due {antwort_due_at}, id: {}…)",
        &antwort_deadline_id.to_string()[..8]
    );

    ctx.registry()
        .register(
            process.tenant_id(),
            &RegistryKey::from_conversation_and_sender(utilmd_conversation_id, sender.as_str()),
            process.identity(),
        )
        .await?;
    println!(
        "  [registry] Registered under conversation_id {}…",
        &utilmd_conversation_id.to_string()[..8]
    );

    // ── Step 4: DispatchAperak ────────────────────────────────────────────────
    println!();
    println!("[4/7] Dispatching positive APERAK (same conversation as UTILMD)...");

    let aperak_ctx = CommandContext::new(
        envs[0].tenant_id,
        envs[0].process_id,
        envs[0].workflow_id.clone(),
    )
    .with_conversation(utilmd_conversation_id)
    .with_causation(utilmd_event_id.into());

    let aperak_envs = process
        .execute_with(
            DeviceChangeCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
            aperak_ctx,
        )
        .await?;

    for env in &aperak_envs {
        println!(
            "  ✓ {} (seq {}, conv {}…)",
            env.event_type,
            env.sequence_number,
            &env.conversation_id.to_string()[..8]
        );
    }

    let aperak_env = &aperak_envs[0];
    ctx.outbox_store()
        .enqueue(&[OutboxMessage::new(
            process.stream_id().clone(),
            aperak_env.process_id,
            aperak_env.tenant_id,
            aperak_env.correlation_id,
            aperak_env.conversation_id,
            aperak_env.event_id,
            "APERAK",
            receiver.as_str(),
            serde_json::json!({
                "positive":       true,
                "message_ref":    "APERAK-WIM-001",
                "in_response_to": aperak_env.correlation_id.to_string(),
            }),
        )])
        .await?;
    ctx.deadline_store().cancel(aperak_deadline_id).await?;
    println!(
        "  [outbox] APERAK queued ({} pending)",
        ctx.outbox_store().len().await?
    );
    println!("  [deadline] 45-Minuten APERAK window cancelled (acknowledged in time)");
    println!("  [deadline] Antwortfrist still armed — the APERAK is not the answer");

    // ── Step 5: DispatchAntwort — the business Bestätigung ───────────────────
    //
    // UTILMD 55043 „Bestätigung Anmeldung MSB" carrying `SG4 STS+E01` with a
    // code from this PID's Entscheidungsbaum — `E_0201` „Anmeldung
    // Messstellenbetrieb prüfen", where `E15` is the Zustimmung ohne
    // Korrekturen. This — not the APERAK — is what the Festlegung means by
    // „Antwort", and it is what closes the Antwortfrist.
    println!();
    println!("[5/7] Dispatching the business Antwort (UTILMD 55043 Bestätigung)...");

    let antwort_envs = process
        .execute(DeviceChangeCommand::DispatchAntwort {
            bestaetigt: true,
            antwort_code: "E15".to_owned(),
            bemerkung: None,
            abweichender_termin: None,
        })
        .await?;
    for env in &antwort_envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }
    ctx.deadline_store().cancel(antwort_deadline_id).await?;
    println!("  [deadline] Antwortfrist cancelled (Bestätigung sent in time)");

    // ── Step 6: Complete ──────────────────────────────────────────────────────
    println!();
    println!("[6/7] Completing device change (meter swap confirmed)...");

    let complete_envs = process
        .execute(DeviceChangeCommand::Complete {
            device_id: DeviceId::new("ZHR-99999999"),
        })
        .await?;
    for env in &complete_envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    let snapped = process
        .take_snapshot(ctx.snapshot_store(), SNAP_INTERVAL)
        .await?;
    println!(
        "  [snap] Snapshot taken: {snapped} (event count {})",
        process.event_count().await?
    );

    // ── Step 6: State + projections ───────────────────────────────────────────
    println!();
    println!("[7/7] Inspecting typed process state...");

    let state = process.state_with_snapshot(ctx.snapshot_store()).await?;
    println!("  Status              : {}", state.status_str());
    // Access typed data from the enum variant — no unwrap() required.
    if let mako_wim::DeviceChangeState::ValidationPassed(ref data)
    | mako_wim::DeviceChangeState::AperakSent(ref data)
    | mako_wim::DeviceChangeState::Completed(ref data)
    | mako_wim::DeviceChangeState::Initiated(ref data) = state
    {
        println!("  MeLo                : {}", data.melo_id);
        println!("  Incoming MSB (GLN)  : {}", data.incoming_msb);
        println!("  Grid operator (GLN) : {}", data.grid_operator);
        println!("  Device ID           : {}", data.device_id);
        println!("  Prüfidentifikator   : {}", data.pruefidentifikator);
    }

    println!();
    println!("  [6b] Full-replay projection (DeviceChangeProjection)...");
    let all_events = ctx.event_store().load(process.stream_id()).await?;
    let mut proj = DeviceChangeProjection::default();
    ProjectionRunner::run(&mut proj, &all_events);
    if let Some(rec) = proj.records.get(process.stream_id().as_str()) {
        println!(
            "  Status: {}  (events: {}, cursor seq: {})",
            rec.status(),
            rec.event_count(),
            proj.last_seq
        );
    }

    println!();
    println!("  [6c] Incremental catch-up projection...");
    let mut partial = DeviceChangeProjection::default();
    ProjectionRunner::run(&mut partial, &all_events[..2]);
    println!(
        "  Partial cursor (after ReceiveUtilmd): seq {}",
        partial.last_seq
    );
    ProjectionRunner::catch_up(&mut partial, &all_events);
    if let Some(rec) = partial.records.get(process.stream_id().as_str()) {
        println!(
            "  After catch-up: seq {} — status: {}",
            partial.last_seq,
            rec.status()
        );
    }

    // ── Guards ────────────────────────────────────────────────────────────────
    println!();
    println!("[+] Guard: stale ReceiveUtilmd on completed process is rejected...");
    let guard_err = process
        .execute(DeviceChangeCommand::ReceiveUtilmd {
            transaktionsgrund: Some("E03".to_owned()),
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            melo_id: melo_id.clone(),
            device_id: device_id.clone(),
            document_date: document_date.clone(),
            message_ref: msg_ref.clone(),
            vorgangsnummer: Some("VG-TEST-001".to_owned()),
            process_date: Some("20260401".to_owned()),
            validation_passed: true,
            validation_errors: vec![],
            received_at: time::OffsetDateTime::now_utc(),
        })
        .await
        .unwrap_err();
    assert!(
        guard_err
            .as_workflow_error()
            .is_some_and(|we| we.is_invalid_state())
    );
    println!("  ✓ Rejected: {guard_err}");

    println!();
    println!("[+] Guard: AS4 retry duplicate is rejected by inbox...");
    assert!(!inbox.accept(&key).await?);
    println!("  ✓ Duplicate UTILMD rejected");

    println!();
    println!("[+] Guard: outbox delivery and drain...");
    let pending = ctx.outbox_store().pending_now(10).await?;
    assert_eq!(pending.len(), 1, "expected one pending APERAK");
    ctx.outbox_store()
        .acknowledge(pending[0].message_id)
        .await?;
    println!(
        "  ✓ Outbox drained ({} remaining)",
        ctx.outbox_store().len().await?
    );

    println!();
    println!("[+] Guard: no overdue deadlines after cancellation...");
    assert!(ctx.deadline_store().due_now(10).await?.deadlines.is_empty());
    println!("  ✓ No overdue deadlines");

    println!();
    println!("[+] Guard: registry lookup by conversation_id...");
    let found = ctx
        .registry()
        .lookup(
            process.tenant_id(),
            &RegistryKey::from_conversation_and_sender(utilmd_conversation_id, sender.as_str()),
        )
        .await?
        .expect("must be registered");
    assert_eq!(found.process_id, process.process_id());
    let resumed = ctx.resume::<WimDeviceChangeWorkflow>(found);
    println!(
        "  ✓ Resumed process event count: {}",
        resumed.event_count().await?
    );

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("  All checks passed — mako-wim + edi-energy round-trip OK.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
