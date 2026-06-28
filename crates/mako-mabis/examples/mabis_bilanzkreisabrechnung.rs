//! # MABIS Bilanzkreisabrechnung — mako-mabis end-to-end example
//!
//! Demonstrates the full write→store→read cycle for a MABIS
//! "Bilanzkreisabrechnung Strom" (PID 13003) process using the `mako-engine`
//! event-sourced runtime and `mako-mabis` domain logic.
//!
//! ## Process flow (BKV perspective)
//!
//! 1. BIKO sends `Abrechnungssummenzeitreihe` to BKV.
//! 2. BKV inspects the data and must send a `Prüfmitteilung` within **1 Werktag**
//!    (MaBiS BK6-24-174 §13.8).
//! 3. BIKO sends `Datenstatus` confirming settlement.
//!
//! ## Frist
//!
//! The `PRUEFMITTEILUNG_DEADLINE_LABEL` deadline (1 Werktag) must be registered
//! in the deadline store immediately after step 1. If it fires before step 2,
//! the process transitions to `DeadlineExpired`.
//!
//! ## Run
//!
//! ```text
//! cargo run --example mabis_bilanzkreisabrechnung -p mako-mabis
//! ```

use mako_engine::{
    builder::EngineBuilder,
    event_store::{EventStore, InMemoryEventStore},
    ids::TenantId,
    outbox::{InMemoryOutboxStore, OutboxMessage, OutboxStore},
    projection::ProjectionRunner,
    registry::{InMemoryProcessRegistry, ProcessRegistry, RegistryKey},
    snapshot::InMemorySnapshotStore,
    types::{BikoId, BillingPeriod, BkvId, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    BillingCommand, BillingProjection, BillingState, BillingVersion, DataStatus,
    MabisBillingWorkflow, PRUEFMITTEILUNG_DEADLINE_LABEL,
};

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔════════════════════════════════════════════════════════════╗");
    println!("║  mako-mabis — Bilanzkreisabrechnung Strom example          ║");
    println!("╚════════════════════════════════════════════════════════════╝");
    println!();

    let ctx = EngineBuilder::new()
        .with_event_store(InMemoryEventStore::new())
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_outbox_store(InMemoryOutboxStore::new())
        .with_registry(InMemoryProcessRegistry::new())
        .build();

    const SNAP_INTERVAL: u64 = 2;

    // One stream per billing period per Bilanzkreis.
    let process = ctx.spawn::<MabisBillingWorkflow>(
        TenantId::new(),
        WorkflowId::new("mabis-billing", "FV2025-10-01"),
    );

    println!("  Stream : {}", process.stream_id());
    println!();

    // ── Step 1: Receive Abrechnungssummenzeitreihe from BIKO ──────────────────
    println!("[1/4] Receiving Abrechnungssummenzeitreihe from BIKO (vorläufig, 2025-09)...");
    println!("  (In production: triggered by inbound MSCONS from BIKO)");

    let envs = process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(13003).expect("13003 is a valid PID"),
            billing_period: BillingPeriod::new("2025-09"),
            bkv_id: BkvId::new("BKV-DE-001"),
            biko_id: BikoId::new("BIKO-DE-001"),
            version: BillingVersion::Vorlaeufig,
            message_ref: MessageRef::new("MSCONS-BKA-2025-09-001"),
        })
        .await?;

    for env in &envs {
        println!(
            "  ✓ {} (seq {}, schema_v{})",
            env.event_type, env.sequence_number, env.schema_version
        );
    }

    println!();
    println!(
        "  [deadline] In production: register {PRUEFMITTEILUNG_DEADLINE_LABEL} deadline for 1 Werktag after receiving."
    );
    println!("  [deadline] fristen::add_werktage(received_date, 1, HolidayCalendar::BdewMaKo)");

    let state = process.state().await?;
    assert!(
        matches!(&state, BillingState::SummenzeitreiheReceived(_)),
        "expected SummenzeitreiheReceived, got: {state:?}"
    );
    println!("  Status : {}", state.status_str());

    // ── Step 2: Send positive Prüfmitteilung ──────────────────────────────────
    println!();
    println!("[2/4] Sending positive Prüfmitteilung to BIKO (accepts the billing)...");
    println!("  (Must be sent within 1 Werktag of step 1 — MaBiS BK6-24-174 §13.8)");

    let pruef_envs = process
        .execute(BillingCommand::SendPruefmitteilungPositiv {
            message_ref: MessageRef::new("PRUEF-POS-2025-09-001"),
        })
        .await?;

    for env in &pruef_envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    // Enqueue outbound Prüfmitteilung for transport layer (AS4).
    let pruef_env = &pruef_envs[0];
    ctx.outbox_store()
        .enqueue(&[OutboxMessage::new(
            process.stream_id().clone(),
            pruef_env.process_id,
            pruef_env.tenant_id,
            pruef_env.correlation_id,
            pruef_env.conversation_id,
            pruef_env.event_id,
            "PRUEFMITTEILUNG",
            "BIKO-DE-001",
            serde_json::json!({
                "message_ref":  "PRUEF-POS-2025-09-001",
                "billing_period": "2025-09",
                "accepted":     true,
            }),
        )])
        .await?;
    println!(
        "  [outbox] Prüfmitteilung queued ({} pending)",
        ctx.outbox_store().len().await?
    );

    let snapped = process
        .take_snapshot(ctx.snapshot_store(), SNAP_INTERVAL)
        .await?;
    println!(
        "  [snap] Snapshot taken: {snapped} (event count {})",
        process.event_count().await?
    );

    // ── Step 3: Receive Datenstatus from BIKO ─────────────────────────────────
    println!();
    println!("[3/4] Receiving Datenstatus from BIKO (confirms settlement)...");

    let datenstatus_envs = process
        .execute(BillingCommand::ReceiveDatastatus {
            data_status: DataStatus::AbgerechtneteDaten,
        })
        .await?;

    for env in &datenstatus_envs {
        println!("  ✓ {} (seq {})", env.event_type, env.sequence_number);
    }

    let state = process.state_with_snapshot(ctx.snapshot_store()).await?;
    println!("  Status : {}", state.status_str());
    assert_eq!(state.status_str(), "Settled");

    // ── Step 4: State + projections ───────────────────────────────────────────
    println!();
    println!("[4/4] Inspecting state and projections...");

    if let BillingState::Settled(d) = &state {
        println!("  Billing period : {}", d.billing_period);
        println!("  BKV            : {}", d.bkv_id);
        println!("  BIKO           : {}", d.biko_id);
        println!("  Version        : {:?}", d.version);
        println!("  PID            : {}", d.pruefidentifikator);
    }

    println!();
    println!("  [4b] Full-replay projection (BillingProjection)...");
    let all_events = ctx.event_store().load(process.stream_id()).await?;
    let mut proj = BillingProjection::default();
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
    println!("  [4c] Incremental catch-up projection...");
    let mut partial = BillingProjection::default();
    // Replay up to first event only (SummenzeitreiheReceived)
    ProjectionRunner::run(&mut partial, &all_events[..1]);
    println!(
        "  Partial cursor (after SummenzeitreiheReceived): seq {}",
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
    println!("[+] Guard: receiving a second Abrechnungssummenzeitreihe on a Settled process...");
    let guard_err = process
        .execute(BillingCommand::ReceiveSummenzeitreihe {
            pid: Pruefidentifikator::new(13003).expect("valid"),
            billing_period: BillingPeriod::new("2025-09"),
            bkv_id: BkvId::new("BKV-DE-001"),
            biko_id: BikoId::new("BIKO-DE-001"),
            version: BillingVersion::Vorlaeufig,
            message_ref: MessageRef::new("MSCONS-BKA-2025-09-DUP"),
        })
        .await
        .unwrap_err();
    assert!(
        guard_err
            .as_workflow_error()
            .is_some_and(|we| we.is_invalid_state())
    );
    println!("  ✓ Rejected (invalid state): {guard_err}");

    println!();
    println!("[+] Guard: registry lookup...");
    ctx.registry()
        .register(
            process.tenant_id(),
            &RegistryKey::from_static("billing-2025-09"),
            process.identity(),
        )
        .await?;
    let found = ctx
        .registry()
        .lookup(
            process.tenant_id(),
            &RegistryKey::from_static("billing-2025-09"),
        )
        .await?
        .expect("must be registered");
    assert_eq!(found.process_id, process.process_id());
    let resumed = ctx.resume::<MabisBillingWorkflow>(found);
    println!(
        "  ✓ Resumed process event count: {}",
        resumed.event_count().await?
    );

    println!();
    println!("[+] Guard: outbox delivery and drain...");
    let pending = ctx.outbox_store().pending_now(10).await?;
    assert_eq!(pending.len(), 1, "expected one pending Prüfmitteilung");
    ctx.outbox_store()
        .acknowledge(pending[0].message_id)
        .await?;
    println!(
        "  ✓ Outbox drained ({} remaining)",
        ctx.outbox_store().len().await?
    );

    println!();
    println!("══════════════════════════════════════════════════════════════════");
    println!("  All checks passed — mako-mabis round-trip OK.");
    println!("══════════════════════════════════════════════════════════════════");

    Ok(())
}
