//! Integration tests for MaBiS Clearingliste workflows (PIDs 55065, 55069, 55070).
//!
//! Covers receive-and-validate logic using `InMemoryEventStore`.
//!
//! # State machine under test
//!
//! ```text
//! New
//!  └─ ClearinglisteErhalten ──── (ValidationPassed) ──→ ValidationPassed (terminal)
//!                           └─── (ValidationFailed) ──→ ValidationFailed (terminal)
//! ```
//!
//! # Regulatory basis
//!
//! BNetzA BK6-24-174 Anlage 3 MaBiS — Clearingverfahren.
//! No outbound APERAK required from the receiving party.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{BillingPeriod, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_mabis::{
    ClearinglisteCommand, ClearinglisteKind, ClearinglisteState, MabisClearinglisteWorkflow,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_process() -> Process<MabisClearinglisteWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("mabis-clearingliste", "FV2025-10-01"),
    )
}

fn receive_cmd(pid: u32, validation_passed: bool) -> ClearinglisteCommand {
    ClearinglisteCommand::ReceiveClearingliste {
        pid: Pruefidentifikator::new(pid).unwrap(),
        sender: MarktpartnerCode::new("9900000000001"),
        receiver: MarktpartnerCode::new("9900000000002"),
        billing_period: BillingPeriod::new("2025-09"),
        document_date: "20251001".to_string(),
        message_ref: MessageRef::new(format!("UTILMD-CL-{pid}-001")),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["AHB profile not yet imported".to_string()]
        },
    }
}

// ── PID 55069: Clearingliste DZR (BIKO → NB/ÜNB) ─────────────────────────────

/// Happy path for PID 55069 (Clearingliste DZR): validation passes.
#[tokio::test]
async fn pid_55069_validation_passed() {
    let p = make_process();

    p.execute(receive_cmd(55069, true))
        .await
        .expect("ReceiveClearingliste PID 55069 must succeed");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationPassed(_)),
        "expected ValidationPassed, got: {state:?}",
    );

    if let ClearinglisteState::ValidationPassed(data) = state {
        assert_eq!(data.pruefidentifikator.as_u32(), 55069);
        assert_eq!(data.kind, ClearinglisteKind::ClearinglisteDzr);
        assert_eq!(data.billing_period, BillingPeriod::new("2025-09"));
        assert_eq!(data.document_date, "20251001");
    }
}

/// Validation failure path for PID 55069.
#[tokio::test]
async fn pid_55069_validation_failed() {
    let p = make_process();

    p.execute(receive_cmd(55069, false))
        .await
        .expect("ReceiveClearingliste PID 55069 must succeed even when validation fails");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationFailed { .. }),
        "expected ValidationFailed, got: {state:?}",
    );

    if let ClearinglisteState::ValidationFailed { reason } = state {
        assert!(
            reason.contains("AHB profile"),
            "reason should mention AHB profile, got: {reason}",
        );
    }
}

// ── PID 55070: Clearingliste BAS (BIKO → BKV) ────────────────────────────────

/// Happy path for PID 55070 (Clearingliste BAS): validation passes.
#[tokio::test]
async fn pid_55070_validation_passed() {
    let p = make_process();

    p.execute(receive_cmd(55070, true))
        .await
        .expect("ReceiveClearingliste PID 55070 must succeed");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationPassed(_)),
        "expected ValidationPassed, got: {state:?}",
    );

    if let ClearinglisteState::ValidationPassed(data) = state {
        assert_eq!(data.pruefidentifikator.as_u32(), 55070);
        assert_eq!(data.kind, ClearinglisteKind::ClearinglisteBas);
    }
}

/// Validation failure path for PID 55070.
#[tokio::test]
async fn pid_55070_validation_failed() {
    let p = make_process();

    p.execute(receive_cmd(55070, false))
        .await
        .expect("ReceiveClearingliste PID 55070 must succeed even when validation fails");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationFailed { .. }),
        "expected ValidationFailed, got: {state:?}",
    );
}

// ── PID 55065: Lieferantenclearingliste (NB → LF) ────────────────────────────

/// Happy path for PID 55065 (Lieferantenclearingliste): validation passes.
#[tokio::test]
async fn pid_55065_validation_passed() {
    let p = make_process();

    p.execute(receive_cmd(55065, true))
        .await
        .expect("ReceiveClearingliste PID 55065 must succeed");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationPassed(_)),
        "expected ValidationPassed, got: {state:?}",
    );

    if let ClearinglisteState::ValidationPassed(data) = state {
        assert_eq!(data.pruefidentifikator.as_u32(), 55065);
        assert_eq!(data.kind, ClearinglisteKind::Lieferantenclearingliste);
    }
}

/// Validation failure path for PID 55065.
#[tokio::test]
async fn pid_55065_validation_failed() {
    let p = make_process();

    p.execute(receive_cmd(55065, false))
        .await
        .expect("ReceiveClearingliste PID 55065 must succeed even when validation fails");

    let state = p.state().await.expect("state after receive");
    assert!(
        matches!(state, ClearinglisteState::ValidationFailed { .. }),
        "expected ValidationFailed, got: {state:?}",
    );
}

// ── Guard: unknown PID rejected ───────────────────────────────────────────────

/// A PID not in the Clearingliste set must be rejected deterministically.
#[tokio::test]
async fn unknown_pid_rejected() {
    let p = make_process();

    let result = p
        .execute(ClearinglisteCommand::ReceiveClearingliste {
            pid: Pruefidentifikator::new(55001).unwrap(), // GPKE, not clearingliste
            sender: MarktpartnerCode::new("9900000000001"),
            receiver: MarktpartnerCode::new("9900000000002"),
            billing_period: BillingPeriod::new("2025-09"),
            document_date: "20251001".to_string(),
            message_ref: MessageRef::new("UTILMD-WRONG-001"),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await;

    assert!(
        result.is_err(),
        "ReceiveClearingliste with PID 55001 must be rejected",
    );
}

// ── Guard: double-receive rejected ───────────────────────────────────────────

/// A second ReceiveClearingliste on the same stream must be rejected
/// (wrong-state guard). The workflow is terminal after the first receive.
#[tokio::test]
async fn double_receive_rejected() {
    let p = make_process();

    p.execute(receive_cmd(55069, true))
        .await
        .expect("first receive must succeed");

    let result = p.execute(receive_cmd(55069, true)).await;

    assert!(
        result.is_err(),
        "second ReceiveClearingliste on the same stream must be rejected",
    );
}

// ── Kind derivation ───────────────────────────────────────────────────────────

/// `ClearinglisteKind::from_pid` must map PIDs correctly.
#[test]
fn kind_from_pid_mapping() {
    assert_eq!(
        ClearinglisteKind::from_pid(55065),
        Some(ClearinglisteKind::Lieferantenclearingliste),
    );
    assert_eq!(
        ClearinglisteKind::from_pid(55069),
        Some(ClearinglisteKind::ClearinglisteDzr),
    );
    assert_eq!(
        ClearinglisteKind::from_pid(55070),
        Some(ClearinglisteKind::ClearinglisteBas),
    );
    assert_eq!(ClearinglisteKind::from_pid(55001), None);
    assert_eq!(ClearinglisteKind::from_pid(99999), None);
    assert_eq!(ClearinglisteKind::from_pid(0), None);
}

/// `ClearinglisteKind::process_name` must return descriptive names.
#[test]
fn kind_process_names() {
    assert_eq!(
        ClearinglisteKind::Lieferantenclearingliste.process_name(),
        "Lieferantenclearingliste",
    );
    assert_eq!(
        ClearinglisteKind::ClearinglisteDzr.process_name(),
        "Clearingliste DZR",
    );
    assert_eq!(
        ClearinglisteKind::ClearinglisteBas.process_name(),
        "Clearingliste BAS",
    );
}

// ── Multiple billing periods ──────────────────────────────────────────────────

/// Distinct billing periods can be passed in; each creates a new stream.
#[tokio::test]
async fn different_billing_periods_are_independent_streams() {
    // Stream 1: 2025-09
    let p1 = make_process();
    p1.execute(ClearinglisteCommand::ReceiveClearingliste {
        pid: Pruefidentifikator::new(55069).unwrap(),
        sender: MarktpartnerCode::new("9900000000001"),
        receiver: MarktpartnerCode::new("9900000000002"),
        billing_period: BillingPeriod::new("2025-09"),
        document_date: "20251001".to_string(),
        message_ref: MessageRef::new("UTILMD-CL-55069-2025-09"),
        validation_passed: true,
        validation_errors: vec![],
    })
    .await
    .expect("stream 1 receive must succeed");

    // Stream 2: 2025-10 (separate InMemoryEventStore / WorkflowId)
    let p2 = make_process();
    p2.execute(ClearinglisteCommand::ReceiveClearingliste {
        pid: Pruefidentifikator::new(55069).unwrap(),
        sender: MarktpartnerCode::new("9900000000001"),
        receiver: MarktpartnerCode::new("9900000000002"),
        billing_period: BillingPeriod::new("2025-10"),
        document_date: "20251101".to_string(),
        message_ref: MessageRef::new("UTILMD-CL-55069-2025-10"),
        validation_passed: true,
        validation_errors: vec![],
    })
    .await
    .expect("stream 2 receive must succeed");

    // Each stream lands in ValidationPassed with its own billing period
    let s1 = p1.state().await.unwrap();
    let s2 = p2.state().await.unwrap();

    if let ClearinglisteState::ValidationPassed(d1) = &s1 {
        assert_eq!(d1.billing_period, BillingPeriod::new("2025-09"));
    } else {
        panic!("stream 1 must be ValidationPassed, got: {s1:?}");
    }

    if let ClearinglisteState::ValidationPassed(d2) = &s2 {
        assert_eq!(d2.billing_period, BillingPeriod::new("2025-10"));
    } else {
        panic!("stream 2 must be ValidationPassed, got: {s2:?}");
    }
}
