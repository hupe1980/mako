//! Integration tests for the GeLi Gas Datenabruf workflow.
//!
//! Handles inbound ORDERS messages requesting Gas-specific metered values:
//! - PID 17103: Anfrage Abrechnungsbrennwert / Zustandszahl (LF → GNB/MSB)
//! - PID 17104: Anfrage MSB Gas an NB Strom (MSB Gas → NB Strom)
//!
//! Rejection responses: ORDRSP 19103 / 19104.
//! Data delivery is signalled via `NotifyDatenGeliefert` (actual data arrives via MSCONS).
//!
//! # State machine
//!
//! ```text
//! New ──ReceiveAnfrage──► AnfrageGesendet ──ReceiveAblehnung──► Abgelehnt
//!                                          ──NotifyDatenGeliefert──► DatenErhalten
//!                                          ──TimeoutExpired──► DeadlineExpired
//! ```
//!
//! # Regulatory basis
//!
//! BK7-24-01-009 — GeLi Gas 3.0. APERAK Frist: **10 Werktage**.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GELI_GAS_DATENABRUF_WORKFLOW_NAME, GeliGasDatanabrufCommand, GeliGasDatanabrufState,
    GeliGasDatanabrufWorkflow,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GeliGasDatanabrufWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("geli-gas-datenabruf", "FV2025-10-01"),
    )
}

fn lf_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
}

fn nb_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn msg(r: &str) -> MessageRef {
    MessageRef::new(r)
}

fn pid(n: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(n).unwrap_or_else(|_| panic!("PID {n} must be valid"))
}

fn receive_anfrage(anfrage_pid: u32) -> GeliGasDatanabrufCommand {
    GeliGasDatanabrufCommand::ReceiveAnfrage {
        pid: pid(anfrage_pid),
        sender: lf_gln(),
        receiver: nb_gln(),
        message_ref: msg("ORDERS-17103-001"),
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// New → AnfrageGesendet: ORDERS 17103 (Brennwert) received.
#[tokio::test]
async fn receive_brennwert_anfrage_transitions_to_anfrage_gesendet() {
    let p = make_process();

    p.execute(receive_anfrage(17103))
        .await
        .expect("ReceiveAnfrage (17103) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::AnfrageGesendet { .. }),
        "expected AnfrageGesendet, got: {state:?}",
    );
}

/// New → AnfrageGesendet: ORDERS 17104 (MSB Gas → NB Strom) received.
#[tokio::test]
async fn receive_msb_gas_anfrage_transitions_to_anfrage_gesendet() {
    let p = make_process();

    p.execute(GeliGasDatanabrufCommand::ReceiveAnfrage {
        pid: pid(17104),
        sender: MarktpartnerCode::new("8888888000001"), // MSB Gas
        receiver: nb_gln(),
        message_ref: msg("ORDERS-17104-001"),
    })
    .await
    .expect("ReceiveAnfrage (17104) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::AnfrageGesendet { .. }),
        "expected AnfrageGesendet for 17104, got: {state:?}",
    );
}

/// AnfrageGesendet → Abgelehnt: ORDRSP 19103 rejection received.
#[tokio::test]
async fn receive_rejection_19103_transitions_to_abgelehnt() {
    let p = make_process();

    p.execute(receive_anfrage(17103)).await.expect("anfrage");

    p.execute(GeliGasDatanabrufCommand::ReceiveAblehnung {
        pid: pid(19103),
        sender: nb_gln(),
        message_ref: msg("ORDRSP-19103-001"),
    })
    .await
    .expect("ReceiveAblehnung (19103) must succeed from AnfrageGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::Abgelehnt),
        "expected Abgelehnt, got: {state:?}",
    );
}

/// AnfrageGesendet → Abgelehnt: ORDRSP 19104 rejection received.
#[tokio::test]
async fn receive_rejection_19104_transitions_to_abgelehnt() {
    let p = make_process();

    p.execute(receive_anfrage(17104)).await.expect("anfrage");

    p.execute(GeliGasDatanabrufCommand::ReceiveAblehnung {
        pid: pid(19104),
        sender: nb_gln(),
        message_ref: msg("ORDRSP-19104-001"),
    })
    .await
    .expect("ReceiveAblehnung (19104) must succeed from AnfrageGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::Abgelehnt),
        "expected Abgelehnt for 19104, got: {state:?}",
    );
}

/// AnfrageGesendet → DatenErhalten: MSCONS data arrived.
#[tokio::test]
async fn notify_daten_geliefert_transitions_to_daten_erhalten() {
    let p = make_process();

    p.execute(receive_anfrage(17103)).await.expect("anfrage");

    p.execute(GeliGasDatanabrufCommand::NotifyDatenGeliefert)
        .await
        .expect("NotifyDatenGeliefert must succeed from AnfrageGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::DatenErhalten),
        "expected DatenErhalten, got: {state:?}",
    );
}

/// AnfrageGesendet → DeadlineExpired: 10-WT window fires.
#[tokio::test]
async fn timeout_fires_deadline_expired() {
    let p = make_process();

    p.execute(receive_anfrage(17103)).await.expect("anfrage");

    p.execute(GeliGasDatanabrufCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from("geli-gas-datenabruf-antwort"),
    })
    .await
    .expect("TimeoutExpired must succeed from AnfrageGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::DeadlineExpired),
        "expected DeadlineExpired, got: {state:?}",
    );
}

/// Sender and PID are preserved in AnfrageGesendet state.
#[tokio::test]
async fn anfrage_data_preserved_in_anfrage_gesendet() {
    let p = make_process();

    let sender = lf_gln();

    p.execute(GeliGasDatanabrufCommand::ReceiveAnfrage {
        pid: pid(17103),
        sender: sender.clone(),
        receiver: nb_gln(),
        message_ref: msg("ORDERS-DATA"),
    })
    .await
    .expect("anfrage");

    let state = p.state().await.expect("state");
    match &state {
        GeliGasDatanabrufState::AnfrageGesendet {
            pid: state_pid,
            sender: state_sender,
        } => {
            assert_eq!(*state_pid, pid(17103), "PID preserved");
            assert_eq!(*state_sender, sender, "sender preserved");
        }
        other => panic!("unexpected state: {other:?}"),
    }
}

/// Deadline fires on Abgelehnt state — already terminal, stays Abgelehnt.
#[tokio::test]
async fn timeout_on_already_abgelehnt_is_idempotent() {
    let p = make_process();

    p.execute(receive_anfrage(17103)).await.expect("anfrage");

    p.execute(GeliGasDatanabrufCommand::ReceiveAblehnung {
        pid: pid(19103),
        sender: nb_gln(),
        message_ref: msg("ORDRSP-19103-002"),
    })
    .await
    .expect("reject");

    // Fire deadline on already-Abgelehnt process
    p.execute(GeliGasDatanabrufCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from("geli-gas-datenabruf-antwort"),
    })
    .await
    .expect("TimeoutExpired on Abgelehnt must not fail");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasDatanabrufState::Abgelehnt),
        "Abgelehnt must be preserved when deadline fires after rejection, got: {state:?}",
    );
}

/// Workflow name constant is correct.
#[test]
fn workflow_name_is_geli_gas_datenabruf() {
    assert_eq!(GELI_GAS_DATENABRUF_WORKFLOW_NAME, "geli-gas-datenabruf");
}
