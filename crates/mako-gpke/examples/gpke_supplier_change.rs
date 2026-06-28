//! # GPKE Lieferbeginn Strom — mako-gpke + edi-energy end-to-end example
//!
//! Demonstrates the full write→store→read cycle for a GPKE "Lieferbeginn
//! Strom" (PID 55001) process using the `mako-engine` event-sourced runtime,
//! `mako-gpke` domain logic, and `edi-energy` for EDIFACT parsing.
//!
//! ## Architecture boundary demonstrated
//!
//! ```text
//! edi-energy (transport boundary)          mako-gpke (pure domain)
//! ─────────────────────────────────────    ────────────────────────────
//! Platform::parse(raw_bytes)             → SupplierChangeCommand { pid, … }
//! msg.validate()                         → GpkeSupplierChangeWorkflow::handle()
//! extract sender/receiver/location       → pure, no I/O, deterministic
//! ```
//!
//! ## Run
//!
//! ```text
//! cargo run --example gpke_supplier_change -p mako-gpke
//! ```

use edi_energy::{AnyMessage, EdiEnergyMessage, Platform};
use mako_engine::{
    builder::EngineBuilder,
    deadline::{Deadline, DeadlineStore, InMemoryDeadlineStore},
    event_store::{EventStore, InMemoryEventStore},
    fristen,
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
use mako_gpke::{
    GpkeSupplierChangeWorkflow, SupplierChangeCommand, SupplierChangeProjection, post_acceptance,
};

// ── EDIFACT fixture ───────────────────────────────────────────────────────────

const UTILMD_LIEFERBEGINN: &[u8] = b"\
UNB+UNOC:3+4012345000023:14+9900357000004:14+240115:0800+INTER-2024-001'\
UNH+MSG-001+UTILMD:D:11A:UN:S2.1'\
BGM+E01:::+00055001::+9'\
DTM+137:20240115:102'\
RFF+Z13:REF-2024-001'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
IDE+Z19+51238696781::'\
UNT+8+MSG-001'\
UNZ+1+INTER-2024-001'";

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  mako-gpke — GPKE Lieferbeginn Strom end-to-end example    ║");
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

    let process = ctx.spawn::<GpkeSupplierChangeWorkflow>(
        TenantId::new(),
        WorkflowId::new("gpke-supplier-change", "FV2024-10-01"),
    );

    println!("  Stream : {}", process.stream_id());
    println!();

    // ── Step 1: Parse + validate EDIFACT — transport boundary ─────────────────
    // All I/O and parsing happens here. The domain command carries domain
    // values only — no raw bytes cross the boundary.
    println!("[1/6] Parsing EDIFACT bytes with edi-energy...");

    let msg = Platform::with_all_profiles().parse(UTILMD_LIEFERBEGINN)?;
    let msg_type = msg
        .try_message_type()
        .map(|t| t.as_str().to_owned())
        .unwrap_or_else(|| "Unknown".to_owned());
    let release = msg.detect_release()?.as_str().to_owned();
    let pid = Pruefidentifikator::new(msg.detect_pruefidentifikator()?.as_u32())
        .map_err(|e| anyhow::anyhow!(e))?;
    let msg_ref = MessageRef::new(msg.message_ref());

    let (sender, receiver, location, document_date, process_date) =
        if let AnyMessage::Utilmd(u) = &msg {
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
                u.transactions()
                    .first()
                    .and_then(|t| t.dtm.iter().find(|d| d.is_period_start()))
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
    println!("  ✓ PID          : {pid}  (Lieferbeginn Strom)");
    println!("  ✓ Sender       : {sender}  (new supplier)");
    println!("  ✓ Receiver     : {receiver}  (grid operator)");
    println!("  ✓ Location     : {location}  (Messlokation Z19)");
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
        .execute(SupplierChangeCommand::ReceiveUtilmd {
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            location_id: location.clone(),
            document_date: document_date.clone(),
            process_date: process_date.clone(),
            message_ref: msg_ref.clone(),
            validation_passed,
            validation_errors: validation_errors.clone(),
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

    // Register 24h APERAK deadline (GPKE BK6-22-024: wall-clock, not Werktage)
    let aperak_deadline = Deadline::new(
        process.stream_id().clone(),
        process.process_id(),
        process.tenant_id(),
        process.workflow_id().clone(),
        "aperak-response-window",
        fristen::add_hours(time::OffsetDateTime::now_utc(), 24),
    );
    let aperak_deadline_id = aperak_deadline.deadline_id();
    ctx.deadline_store().register(&aperak_deadline).await?;
    println!(
        "  [deadline] APERAK window registered (24h, id: {}…)",
        &aperak_deadline_id.to_string()[..8]
    );

    // Register under (sender GLN, conversation_id) to disambiguate across
    // multiple senders that may independently assign the same conversation ID.
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

    // ── Step 4: SendAntwort ──────────────────────────────────────────────
    println!();
    println!("[4/6] Sending positive UTILMD Antwort (55003 — Bestätigung Lieferbeginn)...");

    let antwort_ctx = CommandContext::new(
        envs[0].tenant_id,
        envs[0].process_id,
        envs[0].workflow_id.clone(),
    )
    .with_conversation(utilmd_conversation_id)
    .with_causation(utilmd_event_id.into()); // From<EventId> for CausationId

    let aperak_envs = process
        .execute_with(
            SupplierChangeCommand::SendAntwort {
                accepted: true,
                reason: None,
                // Build GPKE Teil 3/4 post-acceptance obligations via the
                // domain helper (MSCONS 13015; ORDERS 17134 omitted — no MSB
                // GLN available in this example).
                obligations: post_acceptance::lieferbeginn_obligations(
                    pid.as_u32(),
                    &location,
                    &sender,
                    None,
                ),
            },
            antwort_ctx,
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
                "message_ref":    "APERAK-001",
                "in_response_to": aperak_env.correlation_id.to_string(),
            }),
        )])
        .await?;
    ctx.deadline_store().cancel(aperak_deadline_id).await?;
    println!(
        "  [outbox] APERAK queued ({} pending)",
        ctx.outbox_store().len().await?
    );
    println!("  [deadline] Response-window cancelled (APERAK dispatched in time)");

    // ── Step 5: Activate ──────────────────────────────────────────────────────
    println!();
    println!("[5/6] Activating supply relationship...");

    let activate_envs = process.execute(SupplierChangeCommand::Activate).await?;
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
    println!("  Status              : {}", state.label());
    if let Some(data) = state.initiated_data() {
        println!("  Location (Messlok.) : {}", data.location_id);
        println!("  New supplier (GLN)  : {}", data.new_supplier);
        println!("  Grid operator (GLN) : {}", data.grid_operator);
        println!("  Prüfidentifikator   : {}", data.pruefidentifikator);
    }

    println!();
    println!("  [6b] Full-replay projection (SupplierChangeProjection)...");
    let all_events = ctx.event_store().load(process.stream_id()).await?;
    let mut proj = SupplierChangeProjection::default();
    ProjectionRunner::run(&mut proj, &all_events);
    if let Some(rec) = proj.records.get(process.stream_id().as_str()) {
        println!(
            "  Status: {}  (events: {}, cursor seq: {})",
            rec.status, rec.event_count, proj.last_seq
        );
    }

    println!();
    println!("  [6c] Incremental catch-up projection...");
    let mut partial = SupplierChangeProjection::default();
    ProjectionRunner::run(&mut partial, &all_events[..2]);
    println!(
        "  Partial cursor (after ReceiveUtilmd): seq {}",
        partial.last_seq
    );
    ProjectionRunner::catch_up(&mut partial, &all_events);
    if let Some(rec) = partial.records.get(process.stream_id().as_str()) {
        println!(
            "  After catch-up: seq {} — status: {}",
            partial.last_seq, rec.status
        );
    }

    // ── Guards ────────────────────────────────────────────────────────────────
    println!();
    println!("[+] Guard: stale ReceiveUtilmd on completed process is rejected...");
    let guard_err = process
        .execute(SupplierChangeCommand::ReceiveUtilmd {
            pid,
            sender: sender.clone(),
            receiver: receiver.clone(),
            location_id: location.clone(),
            document_date: document_date.clone(),
            process_date: process_date.clone(),
            message_ref: msg_ref.clone(),
            validation_passed: true,
            validation_errors: vec![],
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
    let resumed = ctx.resume::<GpkeSupplierChangeWorkflow>(found);
    println!(
        "  ✓ Resumed process event count: {}",
        resumed.event_count().await?
    );

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("  All checks passed — mako-gpke + edi-energy round-trip OK.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
