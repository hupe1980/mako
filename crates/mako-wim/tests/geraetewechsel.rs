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
//! APERAK Frist: **5 Werktage** (WiM Strom, BK6-24-174). Saturdays, Sundays and
//! federal holidays are not Werktage.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    projection::ProjectionRunner,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    version::WorkflowId,
    workflow::Workflow,
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
        received_at: time::OffsetDateTime::now_utc(),
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

// ── Outbound MSB-Wechsel order (InitiateDeviceChange) ─────────────────────────

fn initiate_cmd(pid: u32) -> DeviceChangeCommand {
    DeviceChangeCommand::InitiateDeviceChange {
        pid: Pruefidentifikator::new(pid).expect("test pid must be in range"),
        sender: MarktpartnerCode::new("9900357000004"),
        receiver: MarktpartnerCode::new("4012345000023"),
        melo_id: MeLo::new("DE00123456789012345678901234567890"),
        process_date: "20260801".to_owned(),
        message_ref: MessageRef::new("WIM-GW-OUT-001"),
    }
}

/// An ERP-initiated order moves `New → AuftragGesendet` and emits a UTILMD
/// outbox entry addressed to the counterparty.
#[tokio::test]
async fn initiate_device_change_spawns_outbound_order() {
    let p = make_process();

    let output = p
        .execute_and_collect(initiate_cmd(55_042))
        .await
        .expect("InitiateDeviceChange must succeed from New");

    let (_events, outbox) = output;
    assert_eq!(outbox.len(), 1, "exactly one outbox entry");
    let entry = &outbox[0];
    assert_eq!(entry.message_type.as_ref(), "UTILMD");
    assert_eq!(entry.recipient.as_ref(), "4012345000023");

    // The renderer requires these keys for a WiM UTILMD; assert the contract.
    let p_json = &entry.payload;
    assert_eq!(p_json["pid"], 55_042);
    assert_eq!(p_json["sender"], "9900357000004");
    assert_eq!(p_json["receiver"], "4012345000023");
    assert_eq!(p_json["melo"], "DE00123456789012345678901234567890");
    assert_eq!(p_json["process_date"], "20260801");
    assert_eq!(p_json["direction"], "outbound");

    assert_eq!(
        p.state().await.unwrap().status_str(),
        "AuftragGesendet",
        "state must record that we sent an order, not that we received one"
    );
}

/// All four WiM MSB-Wechsel PIDs are accepted.
#[tokio::test]
async fn initiate_device_change_accepts_all_wim_pids() {
    for pid in [55_039_u32, 55_042, 55_051, 55_168] {
        let p = make_process();
        p.execute(initiate_cmd(pid))
            .await
            .unwrap_or_else(|e| panic!("PID {pid} must be accepted: {e}"));
        assert_eq!(p.state().await.unwrap().status_str(), "AuftragGesendet");
    }
}

/// A non-WiM PID is rejected before any event is written.
#[tokio::test]
async fn initiate_device_change_rejects_foreign_pid() {
    let p = make_process();
    let err = p
        .execute(initiate_cmd(55_001))
        .await
        .expect_err("PID 55001 is GPKE Lieferbeginn, not a WiM MSB-Wechsel");
    assert!(
        err.to_string().contains("55039"),
        "error must name the valid PIDs, got: {err}"
    );
}

/// An outbound order cannot be issued on a stream that already has history.
#[tokio::test]
async fn initiate_device_change_requires_new_state() {
    let p = make_process();
    p.execute(receive_utilmd_cmd(true))
        .await
        .expect("inbound UTILMD sets up prior state");

    let err = p
        .execute(initiate_cmd(55_042))
        .await
        .expect_err("must not issue an outbound order over an inbound process");
    assert!(
        err.to_string().contains("New"),
        "error must mention the expected state, got: {err}"
    );
}

/// The projection surfaces the outbound order distinctly from an inbound one.
#[tokio::test]
async fn projection_tracks_outbound_order() {
    let store = InMemoryEventStore::new();
    let p: Process<WimDeviceChangeWorkflow, _> = Process::new(
        store.clone(),
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
    );

    p.execute(initiate_cmd(55_039)).await.unwrap();

    let mut projection = DeviceChangeProjection::default();
    let events = store.all_events().await;
    ProjectionRunner::run(&mut projection, &events);

    let record = projection.records.values().next().unwrap();
    assert_eq!(record.status(), "AuftragGesendet");
    assert_eq!(record.event_count(), 1);
}

// ── Counterparty answer closes the outbound order (ReceiveAntwort) ────────────

fn antwort_cmd(pid: u32, reason: Option<&str>) -> DeviceChangeCommand {
    DeviceChangeCommand::ReceiveAntwort {
        pid: Pruefidentifikator::new(pid).expect("test pid must be in range"),
        sender: MarktpartnerCode::new("4012345000023"),
        message_ref: MessageRef::new("WIM-GW-ANTWORT-001"),
        reason: reason.map(str::to_owned),
    }
}

/// A Bestätigung moves `AuftragGesendet → AuftragBestaetigt` and the order can
/// then complete on the physical device swap.
#[tokio::test]
async fn antwort_bestaetigung_closes_the_order() {
    let p = make_process();
    p.execute(initiate_cmd(55_042)).await.unwrap();

    p.execute(antwort_cmd(55_043, None))
        .await
        .expect("55043 confirms a 55042 order");
    assert_eq!(p.state().await.unwrap().status_str(), "AuftragBestaetigt");

    p.execute(DeviceChangeCommand::Complete {
        device_id: DeviceId::new("ZHR-77777777"),
    })
    .await
    .expect("Complete must be reachable from AuftragBestaetigt");
    assert_eq!(p.state().await.unwrap().status_str(), "Completed");
}

/// An Ablehnung rejects the process and carries the counterparty's reason.
#[tokio::test]
async fn antwort_ablehnung_rejects_with_reason() {
    let p = make_process();
    p.execute(initiate_cmd(55_042)).await.unwrap();

    p.execute(antwort_cmd(55_044, Some("MeLo unbekannt")))
        .await
        .expect("55044 rejects a 55042 order");

    match p.state().await.unwrap() {
        DeviceChangeState::Rejected { reason } => {
            assert_eq!(reason, "MeLo unbekannt", "counterparty reason must survive");
        }
        other => panic!("expected Rejected, got {}", other.status_str()),
    }
}

/// Every request PID is closed by its own Bestätigung/Ablehnung pair.
#[tokio::test]
async fn antwort_pairs_match_their_request_pid() {
    for (antwort, request, confirmed) in mako_wim::DEVICE_CHANGE_ANTWORT_PIDS.iter().copied() {
        let p = make_process();
        p.execute(initiate_cmd(request)).await.unwrap();
        p.execute(antwort_cmd(antwort, Some("x")))
            .await
            .unwrap_or_else(|e| panic!("{antwort} must answer {request}: {e}"));

        let expected = if confirmed {
            "AuftragBestaetigt"
        } else {
            "Rejected"
        };
        assert_eq!(
            p.state().await.unwrap().status_str(),
            expected,
            "PID {antwort} (confirmed={confirmed}) answering {request}"
        );
    }
}

/// An answer belonging to a *different* request must not close this order.
///
/// Without this guard a 55043 (Anmeldung confirmed) would silently close a
/// 55039 (Kündigung) order and the audit trail would record the wrong outcome.
#[tokio::test]
async fn antwort_for_a_different_request_is_rejected() {
    let p = make_process();
    p.execute(initiate_cmd(55_039)).await.unwrap();

    let err = p
        .execute(antwort_cmd(55_043, None))
        .await
        .expect_err("55043 answers 55042, not 55039");
    let msg = err.to_string();
    assert!(
        msg.contains("55042") && msg.contains("55039"),
        "error must name both the answered and the sent request, got: {msg}"
    );
    assert_eq!(
        p.state().await.unwrap().status_str(),
        "AuftragGesendet",
        "state must be unchanged after a mismatched answer"
    );
}

/// A non-Antwort PID is not accepted as an answer.
#[tokio::test]
async fn antwort_rejects_non_antwort_pid() {
    let p = make_process();
    p.execute(initiate_cmd(55_042)).await.unwrap();
    let err = p
        .execute(antwort_cmd(55_001, None))
        .await
        .expect_err("55001 is GPKE Lieferbeginn, not a WiM Antwort");
    assert!(err.to_string().contains("55043"), "got: {err}");
}

/// An answer with no open order cannot be applied.
#[tokio::test]
async fn antwort_requires_an_open_order() {
    let p = make_process();
    let err = p
        .execute(antwort_cmd(55_043, None))
        .await
        .expect_err("no order was sent");
    assert!(err.to_string().contains("AuftragGesendet"), "got: {err}");
}

/// Once answered the state has left `AuftragGesendet`, so the response deadline
/// yields no command.
#[tokio::test]
async fn answered_order_absorbs_the_response_deadline() {
    let p = make_process();
    p.execute(initiate_cmd(55_042)).await.unwrap();
    p.execute(antwort_cmd(55_043, None)).await.unwrap();

    let state = p.state().await.unwrap();
    let deadline = mako_engine::deadline::Deadline::new(
        p.stream_id().clone(),
        p.process_id(),
        TenantId::new(),
        WorkflowId::new("wim-device-change", "FV2025-10-01"),
        mako_wim::AUFTRAG_ANTWORT_WINDOW_LABEL,
        time::OffsetDateTime::now_utc(),
    );
    assert!(
        WimDeviceChangeWorkflow::on_deadline(&deadline, &state).is_none(),
        "an answered order must not be timed out"
    );
}

/// The Antwortfrist differs per process and must not be flattened to one value.
///
/// BK6-24-174 WiM Teil 1: Kap. 2.2.2 Nr. 2 (3 WT), Kap. 2.3.2 Nr. 2 (5 WT),
/// Kap. 2.4.2 Nr. 2 (7 WT), Kap. 2.4.2 Nr. 4 (1 WT).
#[test]
fn antwortfrist_is_per_process_not_flat() {
    assert_eq!(mako_wim::antwort_frist_werktage(55_039), Some(3));
    assert_eq!(mako_wim::antwort_frist_werktage(55_042), Some(5));
    assert_eq!(mako_wim::antwort_frist_werktage(55_051), Some(7));
    assert_eq!(mako_wim::antwort_frist_werktage(55_168), Some(1));
    assert_eq!(mako_wim::antwort_frist_werktage(55_001), None);

    // Every request PID must have a Frist, or the dispatcher rejects the order.
    for pid in mako_wim::DEVICE_CHANGE_PIDS {
        assert!(
            mako_wim::antwort_frist_werktage(*pid).is_some(),
            "PID {pid} has no Antwortfrist"
        );
    }
}

/// Every response PID pairs with a request PID that has a Frist, and each
/// request has exactly one Bestätigung and one Ablehnung.
#[test]
fn antwort_pid_table_is_complete_and_consistent() {
    for (antwort, request, _) in mako_wim::DEVICE_CHANGE_ANTWORT_PIDS.iter().copied() {
        assert!(
            mako_wim::DEVICE_CHANGE_PIDS.contains(&request),
            "Antwort {antwort} references unknown request {request}"
        );
        assert_eq!(
            mako_wim::antwort_pid_meaning(antwort),
            Some((request, mako_wim::antwort_pid_meaning(antwort).unwrap().1))
        );
    }
    for req in mako_wim::DEVICE_CHANGE_PIDS {
        let ja = mako_wim::DEVICE_CHANGE_ANTWORT_PIDS
            .iter()
            .filter(|(_, r, c)| r == req && *c)
            .count();
        let nein = mako_wim::DEVICE_CHANGE_ANTWORT_PIDS
            .iter()
            .filter(|(_, r, c)| r == req && !*c)
            .count();
        assert_eq!(ja, 1, "request {req} needs exactly one Bestätigung");
        assert_eq!(nein, 1, "request {req} needs exactly one Ablehnung");
    }
}
