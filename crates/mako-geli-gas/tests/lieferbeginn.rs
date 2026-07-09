//! Integration tests for the GeLi Gas supplier-change workflow (PIDs 44001–44021).
//!
//! Covers the full write→store→read cycle using `InMemoryEventStore` — no
//! SlateDB required. Tests exercise the happy-path lifecycle, validation
//! failures, deadline wiring, idempotent deadline absorption, and the
//! `GasSupplierChangeProjection` read-model.
//!
//! # State machine under test
//!
//! ```text
//! New → Initiated → ValidationPassed → AntwortGesendet → Active (44001)
//!                 ↘ Rejected (validation failure)
//!                                    ↘ Rejected (negative Antwort)
//!      ↘ Rejected (deadline fired on any non-terminal state)
//! ```
//!
//! # Regulatory context
//!
//! APERAK Frist: **10 Werktage** (GeLi Gas BNetzA BK7-24-01-009).
//! Saturday counts as a Werktag; Sunday and federal holidays do not.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    projection::ProjectionRunner,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GasSupplierChangeCommand, GasSupplierChangeProjection, GasSupplierChangeState,
    GeliGasSupplierChangeWorkflow,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GeliGasSupplierChangeWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    )
}

fn receive_utilmd_cmd(validation_passed: bool) -> GasSupplierChangeCommand {
    GasSupplierChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(44_001).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        malo_id: MaLo::new("DE00123456789012345678901234567890"),
        document_date: "20250115".to_owned(),
        process_date: "20250301".to_owned(),
        message_ref: MessageRef::new("MSG-GAS-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["UTILMD G segment IDE missing mandatory Z18 Marktlokation reference".to_owned()]
        },
        received_at: time::OffsetDateTime::now_utc(),
    }
}

// ── Happy-path lifecycle ───────────────────────────────────────────────────────

/// Full GeLi Gas Lieferbeginn lifecycle:
/// New → Initiated → ValidationPassed → AntwortGesendet → Active.
#[tokio::test]
async fn happy_path_full_lifecycle() {
    let p = make_process();

    // Step 1: Receive valid UTILMD G → Initiated + ValidationPassed
    p.execute(receive_utilmd_cmd(true))
        .await
        .expect("ReceiveUtilmd with valid message must succeed");

    let state = p.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, GasSupplierChangeState::ValidationPassed(_)),
        "process must be ValidationPassed after valid UTILMD G, got: {state:?}",
    );

    // Step 2: Send positive Antwort → AntwortGesendet
    p.execute(GasSupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations: vec![],
    })
    .await
    .expect("SendAntwort accepted must succeed from ValidationPassed");

    let state = p.state().await.expect("state after SendAntwort");
    assert!(
        matches!(state, GasSupplierChangeState::AntwortGesendet { .. }),
        "process must be AntwortGesendet after positive Antwort, got: {state:?}",
    );

    // Step 3: Activate → Active
    p.execute(GasSupplierChangeCommand::Activate)
        .await
        .expect("Activate must succeed from AntwortGesendet(LieferbeginnGas)");

    let state = p.state().await.expect("state after Activate");
    assert!(
        matches!(state, GasSupplierChangeState::Active(_)),
        "process must be Active after Activate, got: {state:?}",
    );
}

// ── Validation failure ─────────────────────────────────────────────────────────

/// When the UTILMD G fails validation, the workflow transitions to `Rejected`.
#[tokio::test]
async fn validation_failure_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(false))
        .await
        .expect("ReceiveUtilmd with invalid message must still succeed as command");

    let state = p.state().await.expect("state after failed validation");
    assert!(
        matches!(state, GasSupplierChangeState::Rejected { .. }),
        "process must be Rejected after validation failure, got: {state:?}",
    );
}

// ── Negative Antwort ───────────────────────────────────────────────────────────

/// A negative `SendAntwort` from `ValidationPassed` transitions to `Rejected`.
#[tokio::test]
async fn negative_antwort_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(true)).await.unwrap();

    p.execute(GasSupplierChangeCommand::SendAntwort {
        accepted: false,
        reason: Some("Marktlokation nicht im Versorgungsgebiet".to_owned()),
        obligations: vec![],
    })
    .await
    .expect("Negative SendAntwort must succeed from ValidationPassed");

    let state = p.state().await.expect("state after negative Antwort");
    assert!(
        matches!(state, GasSupplierChangeState::Rejected { .. }),
        "process must be Rejected after negative Antwort, got: {state:?}",
    );
}

// ── PID guard ─────────────────────────────────────────────────────────────────

/// An unsupported PID (e.g. 55001 — GPKE, not GeLi Gas) must be rejected.
#[tokio::test]
async fn unsupported_pid_is_rejected() {
    let p = make_process();

    let result = p
        .execute(GasSupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55_001).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            malo_id: MaLo::new("DE00123456789012345678901234567890"),
            document_date: "20250115".to_owned(),
            process_date: "".to_owned(),
            message_ref: MessageRef::new("MSG-001"),
            validation_passed: true,
            validation_errors: vec![],
            received_at: time::OffsetDateTime::now_utc(),
        })
        .await;

    assert!(
        result.is_err(),
        "GPKE PID 55001 must be rejected by GeLi Gas PID guard"
    );
}

// ── Deadline wiring ────────────────────────────────────────────────────────────

/// When the 10-Werktage deadline fires on a `ValidationPassed` process,
/// the workflow must transition to `Rejected`.
#[tokio::test]
async fn aperak_deadline_timeout_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(true)).await.unwrap();

    let deadline_id = DeadlineId::new();
    p.execute(GasSupplierChangeCommand::TimeoutExpired {
        deadline_id,
        label: "geli-gas-response-10-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on ValidationPassed must succeed");

    let state = p.state().await.expect("state after timeout");
    assert!(
        matches!(state, GasSupplierChangeState::Rejected { .. }),
        "process must be Rejected after deadline, got: {state:?}",
    );
}

/// A deadline firing on an already-`Rejected` process must be absorbed harmlessly.
#[tokio::test]
async fn deadline_on_rejected_is_absorbed() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(false)).await.unwrap();

    let deadline_id = DeadlineId::new();
    p.execute(GasSupplierChangeCommand::TimeoutExpired {
        deadline_id,
        label: "geli-gas-response-10-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on already-Rejected must be absorbed");

    let state = p.state().await.expect("state after late deadline");
    assert!(
        matches!(state, GasSupplierChangeState::Rejected { .. }),
        "process must remain Rejected after absorbed deadline, got: {state:?}",
    );
}

/// A deadline firing on an already-`Active` process must be absorbed harmlessly.
#[tokio::test]
async fn deadline_on_active_is_absorbed() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(true)).await.unwrap();
    p.execute(GasSupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations: vec![],
    })
    .await
    .unwrap();
    p.execute(GasSupplierChangeCommand::Activate).await.unwrap();

    let deadline_id = DeadlineId::new();
    p.execute(GasSupplierChangeCommand::TimeoutExpired {
        deadline_id,
        label: "geli-gas-response-10-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on Active must be absorbed");

    let state = p
        .state()
        .await
        .expect("state after late deadline on Active");
    assert!(
        matches!(state, GasSupplierChangeState::Active(_)),
        "process must remain Active after absorbed deadline, got: {state:?}",
    );
}

// ── Invalid state transitions ──────────────────────────────────────────────────

/// `SendAntwort` from `New` state must return an `InvalidState` error.
#[tokio::test]
async fn send_antwort_from_new_is_rejected() {
    let p = make_process();

    let result = p
        .execute(GasSupplierChangeCommand::SendAntwort {
            accepted: true,
            reason: None,
            obligations: vec![],
        })
        .await;

    assert!(result.is_err(), "SendAntwort on New must return Err");
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Verify that `GasSupplierChangeProjection` correctly tracks a full lifecycle.
#[tokio::test]
async fn projection_tracks_full_lifecycle() {
    let store = InMemoryEventStore::new();
    let p: Process<GeliGasSupplierChangeWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    );

    p.execute(receive_utilmd_cmd(true)).await.unwrap();
    p.execute(GasSupplierChangeCommand::SendAntwort {
        accepted: true,
        reason: None,
        obligations: vec![],
    })
    .await
    .unwrap();
    p.execute(GasSupplierChangeCommand::Activate).await.unwrap();

    let events = store.all_events().await;
    let mut projection = GasSupplierChangeProjection::default();
    ProjectionRunner::run(&mut projection, &events);

    assert_eq!(
        projection.records.len(),
        1,
        "exactly one stream in projection"
    );

    let record = projection.records.values().next().unwrap();
    assert_eq!(
        record.status(),
        "Active",
        "projection status must be Active"
    );
    // Initiated + ValidationPassed + AntwortGesendet + Activated = 4 events
    assert_eq!(record.event_count(), 4, "4 events in the stream");
    let data = record.active_data().expect("record must be Active");
    let _ = data.malo_id;
    let _ = data.sender;
    let _ = data.pruefidentifikator;
}

/// Verify that `GasSupplierChangeProjection` correctly tracks a rejection.
#[tokio::test]
async fn projection_tracks_rejected_process() {
    let store = InMemoryEventStore::new();
    let p: Process<GeliGasSupplierChangeWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("geli-gas-supplier-change", "FV2025-10-01"),
    );

    p.execute(receive_utilmd_cmd(false)).await.unwrap();

    let events = store.all_events().await;
    let mut projection = GasSupplierChangeProjection::default();
    ProjectionRunner::run(&mut projection, &events);

    let record = projection.records.values().next().unwrap();
    assert_eq!(
        record.status(),
        "Rejected",
        "projection status must be Rejected"
    );
    // Initiated + Rejected = 2 events
    assert_eq!(record.event_count(), 2, "2 events in the stream");
}
