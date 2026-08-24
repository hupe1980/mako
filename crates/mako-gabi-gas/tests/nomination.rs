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

fn send_nomination(pruefidentifikator: u32) -> NominationCommand {
    send_nomination_of(pruefidentifikator, None)
}

fn send_nomination_of(
    pruefidentifikator: u32,
    nominated_kwh: Option<rust_decimal::Decimal>,
) -> NominationCommand {
    NominationCommand::SendNomination {
        pruefidentifikator,
        sender_eic: "11XBKV-SENDTESTU".to_owned(),
        receiver_eic: "11XFNB-RECVTESTT".to_owned(),
        gas_day: mako_gabi_gas::GasDay::parse("2025-01-15").unwrap(),
        nomination_ref: MessageRef::new("NOMINT-2025-001"),
        nominated_kwh,
    }
}

fn receive_nomres(acceptance: NomresAcceptance) -> NominationCommand {
    receive_nomres_of(acceptance, None)
}

fn receive_nomres_of(
    acceptance: NomresAcceptance,
    confirmed_kwh: Option<rust_decimal::Decimal>,
) -> NominationCommand {
    NominationCommand::ReceiveNomres {
        nomres_ref: MessageRef::new("NOMRES-2025-001"),
        acceptance,
        gas_day: mako_gabi_gas::GasDay::parse("2025-01-15").unwrap(),
        confirmed_kwh,
        rejection_reason: None,
    }
}

fn receive_nomres_rejected(reason: &str) -> NominationCommand {
    NominationCommand::ReceiveNomres {
        nomres_ref: MessageRef::new("NOMRES-2025-001"),
        acceptance: NomresAcceptance::Rejected,
        gas_day: mako_gabi_gas::GasDay::parse("2025-01-15").unwrap(),
        confirmed_kwh: None,
        rejection_reason: Some(reason.to_owned()),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────────────────────

/// NOMRES deadline label must be `"gabi-gas-nomres-response-deadline"`.
#[test]
fn nomres_deadline_label_is_canonical() {
    assert_eq!(NOMRES_DEADLINE_LABEL, "gabi-gas-nomres-response-deadline",);
}

/// All NOMINATION_PIDS (70030–70039) route to `"gabi-gas-nomination"`.
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

/// Physical-point PIDs (70030 NOMINT, 70035 NOMRES) derive counterparty = Fnb.
#[test]
fn counterparty_from_pid_fnb() {
    assert_eq!(
        NominationCounterparty::from_pid(70030),
        Some(NominationCounterparty::Fnb)
    );
    assert_eq!(
        NominationCounterparty::from_pid(70035),
        Some(NominationCounterparty::Fnb)
    );
}

/// Virtual-trading-point PIDs (70031 NOMINT, 70037 NOMRES) derive counterparty = Mgv.
#[test]
fn counterparty_from_pid_mgv() {
    assert_eq!(
        NominationCounterparty::from_pid(70031),
        Some(NominationCounterparty::Mgv)
    );
    assert_eq!(
        NominationCounterparty::from_pid(70037),
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

    // Step 1: send nomination (PID 70030 = physical point)
    proc.execute(send_nomination(70030)).await.unwrap();
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

    proc.execute(send_nomination(70031)).await.unwrap();
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

    proc.execute(send_nomination(70030)).await.unwrap();
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

    proc.execute(send_nomination(70030)).await.unwrap();
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

    proc.execute(send_nomination(70030)).await.unwrap();
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

    proc.execute(send_nomination(70030)).await.unwrap();
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
    proc.execute(send_nomination(70030)).await.unwrap();
    let result = proc.execute(send_nomination(70030)).await;
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

// ── Curtailment detection ────────────────────────────────────────────────────

/// A NOMRES that confirms less than was nominated is a curtailment, even when it
/// says "Bestätigung".
///
/// NOMRES carries no status segment. The document-name code says only *that* the
/// nomination was confirmed, so a partial acceptance shows up nowhere except in
/// the numbers — and recording it as full leaves the BKV's portfolio short by the
/// difference with nothing pointing at it.
#[tokio::test]
async fn a_reduced_confirmation_is_recorded_as_a_curtailment() {
    use rust_decimal::Decimal;

    let proc = make_process();
    proc.execute(send_nomination_of(70030, Some(Decimal::from(24_000))))
        .await
        .unwrap();
    // The counterparty confirms — but only 18 000 of the 24 000 nominated.
    proc.execute(receive_nomres_of(
        NomresAcceptance::Accepted,
        Some(Decimal::from(18_000)),
    ))
    .await
    .unwrap();

    match proc.state().await.unwrap() {
        NominationState::PartiallyAccepted(data) => {
            let q = data.quantity.expect("the nominated quantity is on file");
            assert_eq!(q.submitted_kwh, Decimal::from(24_000));
            assert_eq!(q.accepted_kwh, Some(Decimal::from(18_000)));
            assert_eq!(q.curtailed_kwh, Some(Decimal::from(6_000)));
            assert!(q.is_curtailed());
        }
        other => panic!("a reduced confirmation must not read as full acceptance: {other:?}"),
    }
}

/// A confirmation of the full amount stays a full acceptance.
#[tokio::test]
async fn a_full_confirmation_is_not_mistaken_for_a_curtailment() {
    use rust_decimal::Decimal;

    let proc = make_process();
    proc.execute(send_nomination_of(70030, Some(Decimal::from(24_000))))
        .await
        .unwrap();
    proc.execute(receive_nomres_of(
        NomresAcceptance::Accepted,
        Some(Decimal::from(24_000)),
    ))
    .await
    .unwrap();

    match proc.state().await.unwrap() {
        NominationState::Accepted(data) => {
            let q = data.quantity.expect("quantity on file");
            assert_eq!(q.accepted_kwh, Some(Decimal::from(24_000)));
            assert!(!q.is_curtailed());
        }
        other => panic!("expected Accepted, got {other:?}"),
    }
}

/// Confirming *more* than was nominated is not a curtailment.
///
/// It is a counterparty defect, but silently reclassifying it as partial
/// acceptance would be a second one.
#[tokio::test]
async fn an_over_confirmation_is_not_treated_as_a_curtailment() {
    use rust_decimal::Decimal;

    let proc = make_process();
    proc.execute(send_nomination_of(70030, Some(Decimal::from(24_000))))
        .await
        .unwrap();
    proc.execute(receive_nomres_of(
        NomresAcceptance::Accepted,
        Some(Decimal::from(30_000)),
    ))
    .await
    .unwrap();
    assert!(matches!(
        proc.state().await.unwrap(),
        NominationState::Accepted(_)
    ));
}

/// With no figures to compare, the stated acceptance stands.
///
/// A message whose quantities could not be integrated must not be guessed at in
/// either direction.
#[tokio::test]
async fn an_unquantified_confirmation_keeps_its_stated_acceptance() {
    let proc = make_process();
    proc.execute(send_nomination(70030)).await.unwrap();
    proc.execute(receive_nomres(NomresAcceptance::Accepted))
        .await
        .unwrap();
    assert!(matches!(
        proc.state().await.unwrap(),
        NominationState::Accepted(_)
    ));
}
