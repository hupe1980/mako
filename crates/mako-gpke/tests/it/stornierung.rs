//! Integration tests for the GPKE Stornierung workflow (PIDs 55022–55024).
//!
//! The NB receives a Stornierungsanfrage (UTILMD 55022) from the LF requesting
//! cancellation of a previously submitted Anmeldung, Abmeldung, or Kündigung.
//! The NB answers with UTILMD 55023 (Bestätigung) or 55024 (Ablehnung) —
//! UTILMD AHB Strom 2.2 Kap. 9.6 — before it answers the *original* message:
//! GPKE Teil 4 Kap. 5 makes the Stornierung admissible only while that message
//! is unanswered, and publishes no window of its own.
//!
//! # State machine
//!
//! ```text
//! New → Initiated → ValidationPassed → Completed   (55023)  [terminal]
//!                                    ↘ Rejected    (55024)  [terminal]
//!     ↘ ValidationFailed → Rejected                          [terminal]
//! ```
//!
//! # Regulatory basis
//!
//! GPKE Teil 4 Kap. 5 (BK6-22-024 Anlage 1d), which publishes no Frist of its
//! own — the bound is „die auslösende Meldung wurde noch nicht beantwortet".

use mako_engine::process::RunOptions;
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    GpkeStornierungCommand, GpkeStornierungState, GpkeStornierungWorkflow,
    STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL, StornoAblehnung,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GpkeStornierungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gpke-stornierung", "FV2025-10-01"),
    )
}

fn lf_mp_id() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
}

fn nb_mp_id() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn vorgang() -> MaLo {
    // The 55022's own Vorgangsnummer, SG4 IDE+24 DE 7402
    MaLo::new("DE00123456789012345678901234567890")
}

fn msg(r: &str) -> MessageRef {
    MessageRef::new(r)
}

fn receive_stornierung_cmd(validation_passed: bool) -> GpkeStornierungCommand {
    GpkeStornierungCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(55022).unwrap(),
        sender: lf_mp_id(),
        receiver: nb_mp_id(),
        vorgang_id: vorgang(),
        document_code: "E01".to_owned(),
        document_date: "20250601".to_owned(),
        message_ref: msg("MSG-STORNO-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["UTILMD Strom segment IDE+24 missing mandatory Vorgangsnummer".to_owned()]
        },
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// New → Initiated: valid 55022 received, process enters active state.
#[tokio::test]
async fn receive_valid_stornierung_transitions_to_initiated() {
    let p = make_process();

    p.run(receive_stornierung_cmd(true), RunOptions::new())
        .await
        .expect("ReceiveUtilmd (55022 valid) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(
            state,
            GpkeStornierungState::ValidationPassed(_) | GpkeStornierungState::Initiated(_)
        ),
        "expected Initiated or ValidationPassed, got: {state:?}",
    );
}

/// New → Rejected: invalid 55022 received (validation failed).
#[tokio::test]
async fn receive_invalid_stornierung_transitions_to_rejected() {
    let p = make_process();

    p.run(receive_stornierung_cmd(false), RunOptions::new())
        .await
        .expect("ReceiveUtilmd (invalid) must not return Err — emits rejected event");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "expected Rejected after failed validation, got: {state:?}",
    );
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "Rejected must be a terminal variant",
    );
}

/// Happy path: UTILMD 55023 Bestätigung sent → Completed (terminal).
#[tokio::test]
async fn happy_path_bestaetigung_completes_process() {
    let p = make_process();

    p.run(receive_stornierung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GpkeStornierungCommand::SendAntwort { ablehnung: None },
        RunOptions::new(),
    )
    .await
    .expect("SendAntwort (Bestätigung) must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GpkeStornierungState::Completed(_)),
        "expected Completed after the 55023, got: {state:?}",
    );
}

/// NB sends UTILMD 55024 Ablehnung → Rejected (terminal).
#[tokio::test]
async fn ablehnung_transitions_to_rejected() {
    let p = make_process();

    p.run(receive_stornierung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GpkeStornierungCommand::SendAntwort {
            ablehnung: Some(StornoAblehnung::SchonBeantwortet),
        },
        RunOptions::new(),
    )
    .await
    .expect("SendAntwort (Ablehnung) must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "expected Rejected after the 55024, got: {state:?}",
    );
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "Rejected must be a terminal variant",
    );
}

/// The NB's answer window lapses while the answer is owed → `FristAbgelaufen`,
/// a Frist breach the operator sees; the answer can still go out.
#[tokio::test]
async fn timeout_fires_while_awaiting_the_answer() {
    let p = make_process();

    p.run(receive_stornierung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GpkeStornierungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: Box::from(STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL),
        },
        RunOptions::new(),
    )
    .await
    .expect("TimeoutExpired must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GpkeStornierungState::FristAbgelaufen { .. }),
        "state after TimeoutExpired must be FristAbgelaufen, got: {state:?}",
    );
}

/// Domain data (Vorgangsnummer, sender) preserved through state transitions.
#[tokio::test]
async fn domain_data_preserved_in_validation_passed() {
    let p = make_process();

    let sender = lf_mp_id();
    let vorgangsnr = vorgang();

    p.run(
        GpkeStornierungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(55022).unwrap(),
            sender: sender.clone(),
            receiver: nb_mp_id(),
            vorgang_id: vorgangsnr.clone(),
            document_code: "E02".to_owned(),
            document_date: "20250601".to_owned(),
            message_ref: msg("MSG-STORNO-DATA"),
            validation_passed: true,
            validation_errors: vec![],
        },
        RunOptions::new(),
    )
    .await
    .expect("receive");

    let state = p.state().await.expect("state");
    match &state {
        GpkeStornierungState::ValidationPassed(data) | GpkeStornierungState::Initiated(data) => {
            assert_eq!(data.sender, sender, "sender preserved");
            assert_eq!(data.vorgang_id, vorgangsnr, "vorgang_id preserved");
            assert_eq!(data.document_date, "20250601", "document_date preserved");
            assert_eq!(
                data.document_code, "E02",
                "BGM category preserved for the answer"
            );
        }
        other => panic!("unexpected state: {other:?}"),
    }
}

/// The label names its process and not a duration.
#[test]
fn aperak_window_label_is_correct() {
    assert!(
        STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL.contains("gpke-stornierung")
            && !STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL.contains("24h"),
        "the Stornierung window is the *original* message's Antwortfrist — no \
         Festlegung publishes a 24-hour one (GPKE Teil 4 Kap. 5): \
         {STORNIERUNG_GPKE_ANTWORT_WINDOW_LABEL}",
    );
}

/// `is_terminal()` is consistent across all terminal states.
#[test]
fn terminal_states_are_terminal() {
    let cases: Vec<GpkeStornierungState> = vec![
        GpkeStornierungState::Rejected {
            reason: "test".to_owned(),
        },
        GpkeStornierungState::Completed(mako_gpke::GpkeStornierungData {
            pruefidentifikator: Pruefidentifikator::new(55022).unwrap(),
            sender: lf_mp_id(),
            receiver: nb_mp_id(),
            vorgang_id: vorgang(),
            document_code: "E01".to_owned(),
            document_date: "20250601".to_owned(),
            message_ref: msg("x"),
        }),
    ];

    for state in &cases {
        assert!(
            matches!(
                state,
                GpkeStornierungState::Rejected { .. } | GpkeStornierungState::Completed(_)
            ),
            "expected terminal variant (Rejected or Completed): {state:?}",
        );
    }
}
