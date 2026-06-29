//! Integration tests for the WiM Messstellenbetrieb (PIDs 55039, 55042, 55051, 55168) workflow.
//!
//! Covers the full write→store→read cycle using `InMemoryEventStore` — no
//! SlateDB required. Tests exercise the happy-path lifecycle, validation
//! failures, deadline wiring, idempotent deadline absorption, and the
//! `DeviceChangeProjection` read-model.
//!
//! # State machine under test
//!
//! ```text
//! New → Initiated → ValidationPassed → AperakSent → Completed
//!                 ↘ Rejected (validation failure)
//!                                    ↘ Rejected (negative APERAK)
//!      ↘ Rejected (deadline fired on any non-terminal state)
//! ```
//!
//! # Regulatory context
//!
//! APERAK Frist: **5 Werktage** (WiM BNetzA BK6-18-032).  Saturday counts as a
//! Werktag; Sunday and federal holidays do not.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    projection::ProjectionRunner,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_wim::{
    DeviceChangeCommand, DeviceChangeProjection, DeviceChangeState, WimDeviceChangeWorkflow,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<WimDeviceChangeWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    )
}

fn receive_utilmd_cmd(validation_passed: bool) -> DeviceChangeCommand {
    DeviceChangeCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(55_042).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        melo_id: MeLo::new("DE00123456789012345678901234567890"),
        device_id: DeviceId::new("MSB-DEVICE-001"),
        document_date: "2025-01-15".to_owned(),
        message_ref: MessageRef::new("MSG-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["UTILMD segment RFF missing mandatory Z13 reference".to_owned()]
        },
    }
}

// ── Happy-path lifecycle ───────────────────────────────────────────────────────

/// Full WiM Gerätewechsel lifecycle:
/// New → Initiated → ValidationPassed → AperakSent → Completed.
///
/// Verifies that each `execute()` call persists events and that subsequent
/// `state()` calls reconstruct the correct variant from the in-memory store.
#[tokio::test]
async fn happy_path_full_lifecycle() {
    let p = make_process();

    // Step 1: Receive valid UTILMD → Initiated + ValidationPassed
    p.execute(receive_utilmd_cmd(true))
        .await
        .expect("ReceiveUtilmd with valid message must succeed");

    let state = p.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, DeviceChangeState::ValidationPassed(_)),
        "process must be ValidationPassed after valid UTILMD, got: {state:?}",
    );

    // Step 2: Dispatch positive APERAK → AperakSent
    p.execute(DeviceChangeCommand::DispatchAperak {
        positive: true,
        reason: None,
    })
    .await
    .expect("DispatchAperak must succeed from ValidationPassed");

    let state = p.state().await.expect("state after DispatchAperak");
    assert!(
        matches!(state, DeviceChangeState::AperakSent(_)),
        "process must be AperakSent after positive APERAK, got: {state:?}",
    );

    // Step 3: Mark device change complete → Completed
    p.execute(DeviceChangeCommand::Complete {
        device_id: DeviceId::new("MSB-DEVICE-001"),
    })
    .await
    .expect("Complete must succeed from AperakSent");

    let state = p.state().await.expect("state after Complete");
    assert!(
        matches!(state, DeviceChangeState::Completed(_)),
        "process must be Completed after Complete command, got: {state:?}",
    );
}

// ── Validation failure ─────────────────────────────────────────────────────────

/// When the UTILMD fails EDIFACT profile validation, the workflow must
/// transition to `Rejected` (not `ValidationPassed`).
///
/// Regulatory context: NB is obliged to send a negative CONTRL within the
/// WiM acceptance window (5 Werktage) — it must never silently proceed with a
/// syntactically invalid message.
#[tokio::test]
async fn validation_failure_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(false))
        .await
        .expect("ReceiveUtilmd with invalid message must still succeed as a command");

    let state = p.state().await.expect("state after failed validation");
    assert!(
        matches!(state, DeviceChangeState::Rejected { .. }),
        "process must be Rejected after validation failure, got: {state:?}",
    );
}

// ── Negative APERAK ────────────────────────────────────────────────────────────

/// A negative APERAK dispatched from `ValidationPassed` transitions to
/// `Rejected`, not `AperakSent`.
///
/// This covers the path where the UTILMD is syntactically valid but the NB
/// applies a business-rule rejection (e.g. metering point not in grid area).
#[tokio::test]
async fn negative_aperak_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(true))
        .await
        .expect("ReceiveUtilmd must succeed");

    p.execute(DeviceChangeCommand::DispatchAperak {
        positive: false,
        reason: Some("Messlokation nicht im Netzgebiet".to_owned()),
    })
    .await
    .expect("Negative DispatchAperak must succeed from ValidationPassed");

    let state = p.state().await.expect("state after negative APERAK");
    assert!(
        matches!(state, DeviceChangeState::Rejected { .. }),
        "process must be Rejected after negative APERAK, got: {state:?}",
    );
}

// ── Deadline wiring ────────────────────────────────────────────────────────────

/// When the 5-Werktage APERAK deadline fires on a `ValidationPassed` process,
/// the workflow must transition to `Rejected`.
///
/// This validates the core regulatory path: if the NB does not dispatch an
/// APERAK within 5 Werktage of the UTILMD receipt, the process self-closes.
#[tokio::test]
async fn aperak_deadline_timeout_rejects_process() {
    let p = make_process();

    p.execute(receive_utilmd_cmd(true))
        .await
        .expect("ReceiveUtilmd must succeed");

    let state = p.state().await.expect("state after ReceiveUtilmd");
    assert!(
        matches!(state, DeviceChangeState::ValidationPassed(_)),
        "expected ValidationPassed before timeout, got: {state:?}",
    );

    let deadline_id = DeadlineId::new();
    p.execute(DeviceChangeCommand::TimeoutExpired {
        deadline_id,
        label: "wim-aperak-5-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on ValidationPassed must succeed");

    let state = p.state().await.expect("state after TimeoutExpired");
    assert!(
        matches!(state, DeviceChangeState::Rejected { .. }),
        "process must be Rejected after deadline, got: {state:?}",
    );
}

/// A deadline firing on an already-`Rejected` process must be absorbed
/// harmlessly (idempotent-deadline contract).
///
/// The deadline store may deliver the same `TimeoutExpired` twice if the first
/// delivery caused a `VersionConflict` and the scheduler retried without first
/// checking current state.
#[tokio::test]
async fn deadline_on_rejected_is_absorbed() {
    let p = make_process();

    // Validation failure → directly Rejected
    p.execute(receive_utilmd_cmd(false))
        .await
        .expect("ReceiveUtilmd with invalid message must succeed");

    let deadline_id = DeadlineId::new();
    p.execute(DeviceChangeCommand::TimeoutExpired {
        deadline_id,
        label: "wim-aperak-5-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on already-Rejected must be absorbed");

    let state = p.state().await.expect("state after late deadline");
    assert!(
        matches!(state, DeviceChangeState::Rejected { .. }),
        "process must remain Rejected after absorbed deadline, got: {state:?}",
    );
}

/// A deadline firing on a `Completed` process must be absorbed harmlessly.
///
/// This covers the race between the deadline scheduler delivering a late
/// `TimeoutExpired` and the process having already reached `Completed`.
#[tokio::test]
async fn deadline_on_completed_is_absorbed() {
    let p = make_process();

    // Drive to Completed
    p.execute(receive_utilmd_cmd(true)).await.unwrap();
    p.execute(DeviceChangeCommand::DispatchAperak {
        positive: true,
        reason: None,
    })
    .await
    .unwrap();
    p.execute(DeviceChangeCommand::Complete {
        device_id: DeviceId::new("MSB-DEVICE-001"),
    })
    .await
    .unwrap();

    let deadline_id = DeadlineId::new();
    p.execute(DeviceChangeCommand::TimeoutExpired {
        deadline_id,
        label: "wim-aperak-5-werktage".into(),
    })
    .await
    .expect("TimeoutExpired on Completed must be absorbed");

    let state = p
        .state()
        .await
        .expect("state after late deadline on Completed");
    assert!(
        matches!(state, DeviceChangeState::Completed(_)),
        "process must remain Completed after absorbed late deadline, got: {state:?}",
    );
}

// ── Invalid state transitions ──────────────────────────────────────────────────

/// Dispatching an APERAK from a non-`ValidationPassed` state (here `New`)
/// must be rejected by the workflow with an `InvalidState` error.
#[tokio::test]
async fn aperak_from_new_is_rejected() {
    let p = make_process();

    let result = p
        .execute(DeviceChangeCommand::DispatchAperak {
            positive: true,
            reason: None,
        })
        .await;

    assert!(
        result.is_err(),
        "DispatchAperak on New state must return Err",
    );
}

// ── Read-model projection ──────────────────────────────────────────────────────

/// Verify that `DeviceChangeProjection` correctly tracks lifecycle transitions
/// and event counts across a full happy-path run.
#[tokio::test]
async fn projection_tracks_full_lifecycle() {
    let store = InMemoryEventStore::new();
    let p: Process<WimDeviceChangeWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    p.execute(receive_utilmd_cmd(true)).await.unwrap();
    p.execute(DeviceChangeCommand::DispatchAperak {
        positive: true,
        reason: None,
    })
    .await
    .unwrap();
    p.execute(DeviceChangeCommand::Complete {
        device_id: DeviceId::new("MSB-DEVICE-001"),
    })
    .await
    .unwrap();

    // Run the projection over all stored events.
    let mut projection = DeviceChangeProjection::default();
    let events = store.all_events().await;
    ProjectionRunner::run(&mut projection, &events);

    // Exactly one stream must be present in the read model.
    assert_eq!(
        projection.records.len(),
        1,
        "exactly one stream in projection"
    );

    let record = projection.records.values().next().unwrap();
    assert_eq!(
        record.status(),
        "Completed",
        "projection status must be Completed"
    );
    // Initiated + ValidationPassed + AperakDispatched + Completed = 4 events
    assert_eq!(record.event_count(), 4, "4 events in the stream");
    let data = record
        .active_data()
        .expect("record must be Active after lifecycle completion");
    assert!(
        !data.melo_id.as_str().is_empty(),
        "melo_id must be populated"
    );
    assert!(
        !data.incoming_msb.as_str().is_empty(),
        "incoming_msb must be populated"
    );
    assert!(
        !data.device_id.as_str().is_empty(),
        "device_id must be populated"
    );
}

/// Verify that the projection correctly tracks a rejection (validation failure).
#[tokio::test]
async fn projection_tracks_rejected_process() {
    let store = InMemoryEventStore::new();
    let p: Process<WimDeviceChangeWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    p.execute(receive_utilmd_cmd(false)).await.unwrap();

    let mut projection = DeviceChangeProjection::default();
    let events = store.all_events().await;
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
