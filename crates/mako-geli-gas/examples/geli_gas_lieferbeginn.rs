//! # GeLi Gas Lieferbeginn — mako-geli-gas + edi-energy end-to-end example
//!
//! Demonstrates the full write→store→read cycle for a GeLi Gas "Lieferbeginn
//! Gas" (PID 44001 — Lieferbeginn Gas Anmeldung, GL → GNB) process using the `mako-engine` event-sourced runtime,
//! `mako-geli-gas` domain logic, and `edi-energy` for EDIFACT parsing.
//!
//! ## Key difference from GPKE and WiM
//!
//! GeLi Gas uses a **10-Werktage** APERAK deadline. The object is a
//! Marktlokation (MaLo), not a Messlokation (MeLo). The grid operator is
//! called Gasnetzbetreiber (GNB), not Netzbetreiber (NB).
//!
//! ## Deadline helpers — reference table
//!
//! | Process | Frist | Helper |
//! |---|---|---|
//! | GPKE Lieferbeginn Strom | 24 h wall-clock | `fristen::add_hours(24)` |
//! | WiM Gerätewechsel | 5 Werktage | `fristen::add_werktage(5, BdewMaKo)` |
//! | GeLi Gas Lieferbeginn | **10 Werktage** | `fristen::add_werktage(10, BdewMaKo)` |
//!
//! ## Run
//!
//! ```text
//! cargo run --example geli_gas_lieferbeginn -p mako-geli-gas
//! ```

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::{EventStore, InMemoryEventStore},
    fristen::{self, HolidayCalendar},
    ids::TenantId,
    inbox::{InMemoryInboxStore, InboxStore, inbox_key},
    outbox::{InMemoryOutboxStore, OutboxMessage, OutboxStore},
    projection::ProjectionRunner,
    registry::{InMemoryProcessRegistry, ProcessRegistry, RegistryKey},
    snapshot::InMemorySnapshotStore,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::CommandContext,
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeProjection, GeliGasSupplierChangeWorkflow,
};

// ── EDIFACT fixture ───────────────────────────────────────────────────────────
//
// Minimal GeLi Gas UTILMD G Lieferbeginn Anmeldung (PID 44001 — Lieferbeginn Gas, GL → GNB).
// BGM+E01 is used for GeLi Gas Anmeldung in UTILMD G.
// NAD+MS = neuer Gaslieferant (sender)
// NAD+MR = Gasnetzbetreiber (receiver)
// IDE+Z19 = Marktlokation identifier (MaLo, not MeLo — key gas/electricity difference)// - MaLo ID: 52695662085 (11-char format, [A-Z0-9]{11})
const UTILMD_LIEFERBEGINN_GAS: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+250115:0800+GELI-2025-001'\
UNH+MSG-001+UTILMD:D:11A:UN:G1.1'\
BGM+E01:::+00044001::+9'\
DTM+137:20250115:102'\
RFF+Z13:GELI-REF-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+52695662085::'\
UNT+8+MSG-001'\
UNZ+1+GELI-2025-001'";

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  mako-geli-gas — GeLi Gas Lieferbeginn end-to-end example  ║");
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

    const SNAP_INTERVAL: u64 = 2;

    let process = ctx.spawn::<GeliGasSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    );

    println!("  Stream : {}", process.stream_id());
    println!();

    // ── Step 1: Parse + validate EDIFACT — transport boundary ─────────────────
    println!("[1/6] Parsing EDIFACT bytes with edi-energy...");

    let msg = Platform::with_all_profiles().parse(UTILMD_LIEFERBEGINN_GAS)?;
    let msg_type = msg
        .try_message_type()
        .map(|t| t.as_str().to_owned())
        .unwrap_or_else(|| "Unknown".to_owned());
    let release = msg.detect_release()?.as_str().to_owned();
    let pid = Pruefidentifikator::new(msg.detect_pruefidentifikator()?.as_u32())
        .map_err(|e| anyhow::anyhow!(e))?;
    let msg_ref = MessageRef::new(msg.message_ref());

    let (sender, receiver, malo_id, document_date) = if let AnyMessage::Utilmd(u) = &msg {
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
            MaLo::new(
                u.transactions()
                    .first()
                    .and_then(|tx| tx.ide.object_id.as_deref())
                    .unwrap_or_default(),
            ),
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

    println!("  ✓ Message type    : {msg_type}");
    println!("  ✓ Release         : {release}");
    println!("  ✓ PID             : {pid}  (GeLi Gas Lieferbeginn Gas Anmeldung — GL → GNB)");
    println!("  ✓ Sender (GL)     : {sender}  (neuer Gaslieferant)");
    println!("  ✓ Receiver (GNB)  : {receiver}  (Gasnetzbetreiber)");
    println!("  ✓ MaLo            : {malo_id}");
    println!(
        "  ✓ Validation      : {} ({} issues)",
        if validation_passed {
            "passed"
        } else {
            "failed"
        },
        validation_errors.len()
    );

    // ── Step 2: Inbox deduplication ───────────────────────────────────────────
    println!();
    println!("[2/6] Inbox deduplication...");

    let key = inbox_key(sender.as_str(), msg_ref.as_str()).map_err(|e| anyhow::anyhow!(e))?;
    if !inbox.accept(&key).await? {
        println!("  ✗ DUPLICATE — idempotency key: {key}");
        return Ok(());
    }
    println!("  ✓ New message accepted — key: {key}");

    // ── Step 3: ReceiveUtilmd — domain command (pure, no I/O) ────────────────
    println!();
    println!("[3/6] Dispatching ReceiveUtilmd...");

    let envs = process
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            malo_id: malo_id.clone(),
            document_date: document_date.clone(),
            process_date: String::new(),
            message_ref: msg_ref.clone(),
            validation_passed,
            validation_errors: validation_errors.clone(),
            received_at: time::OffsetDateTime::now_utc(),
            bilanzierungsmethode: None,
            fallgruppe: None,
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

    // ── 10-Werktage APERAK deadline (GeLi Gas / BNetzA BK7) ──────────────────
    //
    // GeLi Gas uses a 10-Werktage APERAK window — double the WiM Frist and
    // significantly longer than the GPKE 24h wall-clock window.
    //
    // Frist reference:
    //   GPKE:     fristen::add_hours(now, 24)                → 24 h
    //   WiM:      fristen::add_werktage(today, 5, BdewMaKo)   → 5 Werktage
    //   GeLi Gas: fristen::add_werktage(today, 10, BdewMaKo)  → 10 Werktage ← here
    let received_date = time::OffsetDateTime::now_utc().date();
    let aperak_due_date = fristen::add_werktage(received_date, 10, HolidayCalendar::BdewMaKo);
    let aperak_due_at = aperak_due_date.midnight().assume_utc();

    let aperak_deadline = Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        process.tenant_id(),
        process.workflow_id().clone(),
        "aperak-response-window",
        aperak_due_at,
    );
    let aperak_deadline_id = aperak_deadline.deadline_id();
    ctx.deadline_store().register(&aperak_deadline).await?;
    println!(
        "  [deadline] APERAK window registered (10 Werktage — due {aperak_due_date}, id: {}…)",
        &aperak_deadline_id.to_string()[..8]
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

    // ── Step 4: SendAntwort ───────────────────────────────────────────────────
    println!();
    println!("[4/6] Sending positive Antwort (same conversation as UTILMD)...");

    let aperak_ctx = CommandContext::new(
        envs[0].tenant_id,
        envs[0].process_id,
        envs[0].workflow_id.clone(),
    )
    .with_conversation(utilmd_conversation_id)
    .with_causation(utilmd_event_id.into());

    let aperak_envs = process
        .execute_with(
            GasSupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                obligations: vec![],
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
                "message_ref":    "APERAK-GELI-001",
                "in_response_to": aperak_env.correlation_id.to_string(),
            }),
        )])
        .await?;
    ctx.deadline_store().cancel(aperak_deadline_id).await?;
    println!(
        "  [outbox] APERAK queued ({} pending)",
        ctx.outbox_store().len().await?
    );
    println!("  [deadline] 10-Werktage window cancelled (APERAK dispatched in time)");

    // ── Step 5: Activate ──────────────────────────────────────────────────────
    println!();
    println!("[5/6] Activating gas supply relationship...");

    let activate_envs = process.execute(GasSupplierChangeCommand::Activate).await?;
    for env in &activate_envs {
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
    println!("[6/6] Inspecting typed process state...");

    let state = process.state_with_snapshot(ctx.snapshot_store()).await?;
    println!("  Status               : {}", state.status_str());
    // Access typed data from the enum variant — no unwrap() required.
    if let mako_geli_gas::GasSupplierChangeState::ValidationPassed(ref data)
    | mako_geli_gas::GasSupplierChangeState::AntwortGesendet { ref data, .. }
    | mako_geli_gas::GasSupplierChangeState::Active(ref data)
    | mako_geli_gas::GasSupplierChangeState::Initiated(ref data) = state
    {
        println!("  MaLo (Marktlok.)     : {}", data.malo_id);
        println!("  Sender (GLN)         : {}", data.sender);
        println!("  Receiver (GLN)       : {}", data.receiver);
        println!("  Prüfidentifikator    : {}", data.pruefidentifikator);
    }

    println!();
    println!("  [6b] Full-replay projection (GasSupplierChangeProjection)...");
    let all_events = ctx.event_store().load(process.stream_id()).await?;
    let mut proj = GasSupplierChangeProjection::default();
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
    let mut partial = GasSupplierChangeProjection::default();
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
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            malo_id: malo_id.clone(),
            document_date: document_date.clone(),
            process_date: String::new(),
            message_ref: msg_ref.clone(),
            validation_passed: true,
            validation_errors: vec![],
            received_at: time::OffsetDateTime::now_utc(),
            bilanzierungsmethode: None,
            fallgruppe: None,
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
    println!("  ✓ Duplicate UTILMD G rejected");

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
    let resumed = ctx.resume::<GeliGasSupplierChangeWorkflow>(found);
    println!(
        "  ✓ Resumed process event count: {}",
        resumed.event_count().await?
    );

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("  All checks passed — mako-geli-gas + edi-energy round-trip OK.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
