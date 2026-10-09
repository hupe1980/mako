//! Deadline wiring regression tests for GPKE workflows.
//!
//! Verifies that when a deadline fires (`TimeoutExpired` command) on a GPKE
//! workflow process, the workflow records the lapse as `FristAbgelaufen`, and
//! that a deadline firing on a process whose window is already closed is
//! absorbed harmlessly.
//!
//! These tests do **not** require SlateDB or a full engine runtime — they use
//! `InMemoryEventStore` and run commands directly via `Process::run`.

use mako_engine::process::RunOptions;
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
        transaktionsgrund_ergaenzung: None,
        tranchengroesse: None,
        pid: Pruefidentifikator::new(55001).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        location_id: MaLo::new("DE00123456789012345678901234567890"),
        process_date: "2025-10-01".to_owned(),
        transaktionsgrund: None,
        bilanzkreis: Some("11XBK-LF-------9".to_owned()),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// When a `TimeoutExpired` deadline fires on a `Pending` process, the workflow
/// records `FristAbgelaufen` naming the lapsed window.
///
/// If the NB does not answer inside the window `mako_fristen::antwort` resolved
/// for the Prüfidentifikator — a clock time on the first Werktag after the ÜT,
/// never a flat duration — the lapse is the NB's Frist breach. GPKE Teil 2
/// gives it no outcome, so the Anmeldung is not rejected: it stays open for the
/// NB's late answer, and the operator sees the breach.
#[tokio::test]
async fn timeout_on_pending_is_a_frist_breach() {
    let p = make_lf_anmeldung();

    // Spawn the process in `Pending` state by issuing an Initiate command.
    p.run(initiate_cmd(), RunOptions::new())
        .await
        .expect("Initiate must succeed");

    let state = p.state().await.expect("state after Initiate");
    assert!(
        matches!(state, LfAnmeldungState::Pending(_)),
        "process must be Pending after Initiate, got: {state:?}",
    );

    // Simulate the deadline scheduler firing TimeoutExpired.
    let deadline_id = DeadlineId::new();
    p.run(
        LfAnmeldungCommand::TimeoutExpired {
            deadline_id,
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        },
        RunOptions::new(),
    )
    .await
    .expect("TimeoutExpired on Pending must succeed");

    let state = p.state().await.expect("state after TimeoutExpired");
    assert!(
        matches!(state, LfAnmeldungState::FristAbgelaufen { .. }),
        "process must be FristAbgelaufen after timeout, got: {state:?}",
    );
}

/// A `TimeoutExpired` deadline firing on a process whose window already lapsed
/// (late delivery) must be absorbed harmlessly — the process stays
/// `FristAbgelaufen` and no error is raised.
///
/// This is the idempotent-deadline contract: deadline store may deliver the
/// same `TimeoutExpired` twice if the first delivery caused a `VersionConflict`
/// and the scheduler retried without first checking the current state.
#[tokio::test]
async fn a_second_timeout_is_absorbed() {
    let p = make_lf_anmeldung();

    p.run(initiate_cmd(), RunOptions::new())
        .await
        .expect("Initiate must succeed");

    let deadline_id = DeadlineId::new();
    // First TimeoutExpired — transitions Pending → FristAbgelaufen.
    p.run(
        LfAnmeldungCommand::TimeoutExpired {
            deadline_id,
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        },
        RunOptions::new(),
    )
    .await
    .expect("first TimeoutExpired must succeed");

    // Second TimeoutExpired — the window already lapsed; must be a no-op.
    p.run(
        LfAnmeldungCommand::TimeoutExpired {
            deadline_id,
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        },
        RunOptions::new(),
    )
    .await
    .expect("late/duplicate TimeoutExpired must not error");

    let state = p.state().await.expect("state after duplicate timeout");
    assert!(
        matches!(state, LfAnmeldungState::FristAbgelaufen { .. }),
        "process must still be FristAbgelaufen, got: {state:?}",
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

    p.run(initiate_cmd(), RunOptions::new())
        .await
        .expect("Initiate must succeed");

    // NB accepts — transitions Pending → Active.
    p.run(
        LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55003).unwrap(),
            accepted: true,
            reason: None,
            response_ref: MessageRef::new("NB-ACCEPT-001"),
        },
        RunOptions::new(),
    )
    .await
    .expect("HandleAntwort(accepted) must succeed");

    let state = p.state().await.expect("state after HandleAntwort");
    assert!(
        matches!(state, LfAnmeldungState::Active(_)),
        "process must be Active after accepted HandleAntwort, got: {state:?}",
    );

    // Late-firing deadline after acceptance — must be absorbed.
    let deadline_id = DeadlineId::new();
    p.run(
        LfAnmeldungCommand::TimeoutExpired {
            deadline_id,
            label: NB_RESPONSE_WINDOW_LABEL.into(),
        },
        RunOptions::new(),
    )
    .await
    .expect("late TimeoutExpired on Active must not error");

    let state = p.state().await.expect("state after late timeout");
    assert!(
        matches!(state, LfAnmeldungState::Active(_)),
        "process must still be Active after late timeout, got: {state:?}",
    );
}
