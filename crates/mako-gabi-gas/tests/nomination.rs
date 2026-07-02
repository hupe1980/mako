//! Integration tests for mako-gabi-gas Nomination workflow (NOMINT/NOMRES).
//!
//! Verifies:
//! - NOMRES deadline label is canonical.
//! - All NOMINATION_PIDS route to `"gabi-gas-nomination"`.
//! - Happy path: `SendNomination` → `NominationSent` → `ReceiveNomres(Accepted)` → `Accepted`.
//! - Partial accept path: `SendNomination` → `ReceiveNomres(PartiallyAccepted)` → `PartiallyAccepted`.
//! - Rejection path: `SendNomination` → `ReceiveNomres(Rejected)` → `Rejected`.
//! - Deadline expiry: `SendNomination` → `NomresDeadlineExpired` → `DeadlineExpired`.
//! - Late deadline after NOMRES is silently absorbed (no events emitted).
//! - Invalid PID is rejected.
//! - Duplicate `SendNomination` on a non-New state is rejected.
//! - FNB vs MGV counterparty assignment.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::MessageRef,
    version::WorkflowId,
};
use mako_gabi_gas::{
    GaBiGasNominationWorkflow, NOMINATION_PIDS, NOMRES_DEADLINE_LABEL, NominationCommand,
    NominationCounterparty, NominationState, NomresAcceptance,
};

// ── Helpers ───────────────────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GaBiGasNominationWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gabi-gas-nomination", "FV2025-10-01"),
    )
}

fn send_nomination(synthetic_pid: u32) -> NominationCommand {
    NominationCommand::SendNomination {
        synthetic_pid,
        sender_eic: "11XBKV-SENDTEST1".to_owned(),
        receiver_eic: "11XFNB-RECVTEST2".to_owned(),
        gas_day: "20250115".to_owned(),
        nomination_ref: MessageRef::new("NOMINT-2025-001"),
    }
}

fn receive_nomres(acceptance: NomresAcceptance) -> NominationCommand {
    NominationCommand::ReceiveNomres {
        nomres_ref: MessageRef::new("NOMRES-2025-001"),
        acceptance,
        gas_day: "20250115".to_owned(),
        rejection_reason: None,
    }
}

fn receive_nomres_rejected(reason: &str) -> NominationCommand {
    NominationCommand::ReceiveNomres {
        nomres_ref: MessageRef::new("NOMRES-2025-001"),
        acceptance: NomresAcceptance::Rejected,
        gas_day: "20250115".to_owned(),
        rejection_reason: Some(reason.to_owned()),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────────────────────

/// NOMRES deadline label must be `"gabi-gas-nomres-response-deadline"`.
#[test]
fn nomres_deadline_label_is_canonical() {
    assert_eq!(NOMRES_DEADLINE_LABEL, "gabi-gas-nomres-response-deadline",);
}

/// All NOMINATION_PIDS (90011, 90012, 90021, 90022) route to `"gabi-gas-nomination"`.
#[test]
fn all_nomination_pids_route_correctly() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_gabi_gas::GaBiGasModule;

    let mut router = PidRouter::new();
    GaBiGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    for &pid in NOMINATION_PIDS {
        assert_eq!(
            router.route(pid),
            Some("gabi-gas-nomination"),
            "PID {pid} must route to gabi-gas-nomination"
        );
    }
}

/// FNB PIDs (90011 NOMINT, 90021 NOMRES) derive counterparty = Fnb.
#[test]
fn counterparty_from_pid_fnb() {
    assert_eq!(
        NominationCounterparty::from_pid(90011),
        Some(NominationCounterparty::Fnb)
    );
    assert_eq!(
        NominationCounterparty::from_pid(90021),
        Some(NominationCounterparty::Fnb)
    );
}

/// MGV PIDs (90012 NOMINT, 90022 NOMRES) derive counterparty = Mgv.
#[test]
fn counterparty_from_pid_mgv() {
    assert_eq!(
        NominationCounterparty::from_pid(90012),
        Some(NominationCounterparty::Mgv)
    );
    assert_eq!(
        NominationCounterparty::from_pid(90022),
        Some(NominationCounterparty::Mgv)
    );
}

/// Unknown PID returns None from `from_pid`.
#[test]
fn counterparty_from_pid_unknown_returns_none() {
    assert_eq!(NominationCounterparty::from_pid(12345), None);
}

/// Happy path — BKV sends NOMINT to FNB; FNB accepts in full.
///
/// ```text
/// New → NominationSent → Accepted
/// ```
#[tokio::test]
async fn nomination_to_fnb_accepted_happy_path() {
    let proc = make_process();

    // Step 1: send nomination (PID 90011 = BKV → FNB)
    proc.execute(send_nomination(90011)).await.unwrap();
    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, NominationState::NominationSent(_)),
        "state must be NominationSent after SendNomination, got: {state:?}"
    );

    // Step 2: FNB confirms in full
    proc.execute(receive_nomres(NomresAcceptance::Accepted))
        .await
        .unwrap();
    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, NominationState::Accepted(_)),
        "state must be Accepted after full NOMRES, got: {state:?}"
    );
}

/// Happy path — BKV sends NOMINT to MGV; MGV accepts in full.
#[tokio::test]
async fn nomination_to_mgv_accepted_happy_path() {
    let proc = make_process();

    proc.execute(send_nomination(90012)).await.unwrap();
    proc.execute(receive_nomres(NomresAcceptance::Accepted))
        .await
        .unwrap();
    let state = proc.state().await.unwrap();
    assert!(matches!(state, NominationState::Accepted(_)));
}

/// Partial acceptance path — FNB curtails submitted quantities.
#[tokio::test]
async fn nomination_partially_accepted() {
    let proc = make_process();

    proc.execute(send_nomination(90011)).await.unwrap();
    proc.execute(receive_nomres(NomresAcceptance::PartiallyAccepted))
        .await
        .unwrap();
    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, NominationState::PartiallyAccepted(_)),
        "state must be PartiallyAccepted, got: {state:?}"
    );
}

/// Rejection path — FNB rejects the nomination.
#[tokio::test]
async fn nomination_rejected_by_fnb() {
    let proc = make_process();

    proc.execute(send_nomination(90011)).await.unwrap();
    proc.execute(receive_nomres_rejected("Kapazitätslimit überschritten"))
        .await
        .unwrap();
    let state = proc.state().await.unwrap();
    match state {
        NominationState::Rejected { reason, .. } => {
            assert_eq!(reason, "Kapazitätslimit überschritten");
        }
        other => panic!("expected Rejected, got {}", other.label()),
    }
}

/// Deadline expiry — no NOMRES received before D-1 15:00.
#[tokio::test]
async fn nomres_deadline_expires() {
    let proc = make_process();

    proc.execute(send_nomination(90011)).await.unwrap();
    proc.execute(NominationCommand::NomresDeadlineExpired {
        deadline_id: DeadlineId::new(),
        label: NOMRES_DEADLINE_LABEL.to_owned(),
    })
    .await
    .unwrap();
    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, NominationState::DeadlineExpired(_)),
        "state must be DeadlineExpired, got: {state:?}"
    );
}

/// Late deadline fired after NOMRES already received → no-op (no new events).
#[tokio::test]
async fn late_deadline_after_accepted_is_absorbed() {
    let proc = make_process();

    proc.execute(send_nomination(90011)).await.unwrap();
    proc.execute(receive_nomres(NomresAcceptance::Accepted))
        .await
        .unwrap();

    // Late deadline should be silently ignored
    proc.execute(NominationCommand::NomresDeadlineExpired {
        deadline_id: DeadlineId::new(),
        label: NOMRES_DEADLINE_LABEL.to_owned(),
    })
    .await
    .unwrap();
    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, NominationState::Accepted(_)),
        "state must still be Accepted after late deadline, got: {state:?}"
    );
}

/// Invalid PID on `SendNomination` is rejected.
#[tokio::test]
async fn send_nomination_with_invalid_pid_rejected() {
    let proc = make_process();
    let result = proc.execute(send_nomination(12345)).await;
    assert!(result.is_err(), "invalid PID must be rejected");
}

/// `SendNomination` on a non-New state is rejected.
#[tokio::test]
async fn duplicate_send_nomination_rejected() {
    let proc = make_process();
    proc.execute(send_nomination(90011)).await.unwrap();
    let result = proc.execute(send_nomination(90011)).await;
    assert!(result.is_err(), "second SendNomination must be rejected");
}

/// `ReceiveNomres` on a New state (before any NOMINT) is rejected.
#[tokio::test]
async fn receive_nomres_before_nomination_rejected() {
    let proc = make_process();
    let result = proc
        .execute(receive_nomres(NomresAcceptance::Accepted))
        .await;
    assert!(
        result.is_err(),
        "ReceiveNomres before SendNomination must be rejected"
    );
}
