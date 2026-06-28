//! Deadline wiring regression tests for GPKE workflows.
//!
//! Verifies that when a deadline fires (`TimeoutExpired` command) on a GPKE
//! workflow process, the workflow transitions to the correct terminal state
//! (`Rejected`) and any late-firing deadlines on already-terminal processes
//! are absorbed harmlessly.
//!
//! These tests do **not** require SlateDB or a full engine runtime — they use
//! `InMemoryEventStore` and execute commands directly via `Process::execute`.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    GpkeLfAnmeldungWorkflow, LfAnmeldungCommand, LfAnmeldungState, NB_RESPONSE_WINDOW_LABEL,
};

/// Build a fresh `GpkeLfAnmeldungWorkflow` process backed by an in-memory store.
fn make_lf_anmeldung() -> Process<GpkeLfAnmeldungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
    )
}

/// Helper to build an `LfAnmeldungCommand::InitiateAnmeldung` with minimal valid data.
fn initiate_cmd() -> LfAnmeldungCommand {
    LfAnmeldungCommand::InitiateAnmeldung {
        pid: Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        location_id: MaLo::new("DE00123456789012345678901234567890"),
        process_date: "2025-10-01".to_owned(),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// When a `TimeoutExpired` deadline fires on a `Pending` process, the workflow
/// must transition to `Rejected` with a reason derived from the deadline label.
///
/// This validates the critical regulatory path: if the NB does not respond
/// within the 24-hour GPKE APERAK window, the process must self-close and the
/// ERP must receive an `AperakTimeout` outcome.
#[tokio::test]
async fn timeout_on_pending_transitions_to_rejected() {
    let p = make_lf_anmeldung();

    // Spawn the process in `Pending` state by issuing an Initiate command.
    p.execute(initiate_cmd())
        .await
        .expect("Initiate must succeed");

    let state = p.state().await.expect("state after Initiate");
    assert!(
        matches!(state, LfAnmeldungState::Pending(_)),
        "process must be Pending after Initiate, got: {state:?}",
    );

    // Simulate the deadline scheduler firing TimeoutExpired.
    let deadline_id = DeadlineId::new();
    p.execute(LfAnmeldungCommand::TimeoutExpired {
        deadline_id,
        label: NB_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .expect("TimeoutExpired on Pending must succeed");

    let state = p.state().await.expect("state after TimeoutExpired");
    assert!(
        matches!(state, LfAnmeldungState::Rejected { .. }),
        "process must be Rejected after timeout, got: {state:?}",
    );
}

/// A `TimeoutExpired` deadline firing on an already-`Rejected` process (late
/// delivery) must be absorbed harmlessly — the process stays `Rejected` and
/// no error is raised.
///
/// This is the idempotent-deadline contract: deadline store may deliver the
/// same `TimeoutExpired` twice if the first delivery caused a `VersionConflict`
/// and the scheduler retried without first checking the current state.
#[tokio::test]
async fn timeout_on_rejected_is_absorbed() {
    let p = make_lf_anmeldung();

    p.execute(initiate_cmd())
        .await
        .expect("Initiate must succeed");

    let deadline_id = DeadlineId::new();
    // First TimeoutExpired — transitions Pending → Rejected.
    p.execute(LfAnmeldungCommand::TimeoutExpired {
        deadline_id,
        label: NB_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .expect("first TimeoutExpired must succeed");

    // Second TimeoutExpired — already Rejected; must be a no-op.
    p.execute(LfAnmeldungCommand::TimeoutExpired {
        deadline_id,
        label: NB_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .expect("late/duplicate TimeoutExpired on Rejected must not error");

    let state = p.state().await.expect("state after duplicate timeout");
    assert!(
        matches!(state, LfAnmeldungState::Rejected { .. }),
        "process must still be Rejected, got: {state:?}",
    );
}

/// Verify that a process which received a positive NB response (`HandleAntwort`
/// with `accepted: true`) and reached `Active` state correctly absorbs a late
/// `TimeoutExpired` without changing state.
///
/// This covers the race where the NB responds just after the deadline timer
/// fires in the scheduler.
#[tokio::test]
async fn timeout_on_active_is_noop() {
    let p = make_lf_anmeldung();

    p.execute(initiate_cmd())
        .await
        .expect("Initiate must succeed");

    // NB accepts — transitions Pending → Active.
    p.execute(LfAnmeldungCommand::HandleAntwort {
        response_pid: Pruefidentifikator::new(55003).unwrap(),
        accepted: true,
        reason: None,
        response_ref: MessageRef::new("NB-ACCEPT-001"),
    })
    .await
    .expect("HandleAntwort(accepted) must succeed");

    let state = p.state().await.expect("state after HandleAntwort");
    assert!(
        matches!(state, LfAnmeldungState::Active(_)),
        "process must be Active after accepted HandleAntwort, got: {state:?}",
    );

    // Late-firing deadline after acceptance — must be absorbed.
    let deadline_id = DeadlineId::new();
    p.execute(LfAnmeldungCommand::TimeoutExpired {
        deadline_id,
        label: NB_RESPONSE_WINDOW_LABEL.into(),
    })
    .await
    .expect("late TimeoutExpired on Active must not error");

    let state = p.state().await.expect("state after late timeout");
    assert!(
        matches!(state, LfAnmeldungState::Active(_)),
        "process must still be Active after late timeout, got: {state:?}",
    );
}
