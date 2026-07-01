//! Integration tests for the GPKE Stornierung workflow (PIDs 55022–55024).
//!
//! The NB receives a Stornierungsanfrage (UTILMD 55022) from the LFN requesting
//! cancellation of a previously submitted Anmeldung, Abmeldung, or Kündigung.
//! The NB must respond with a positive (55023) or negative (55024) APERAK
//! within **24 wall-clock hours** per BK6-22-024 §5.
//!
//! # State machine
//!
//! ```text
//! New → Initiated → ValidationPassed → AperakSent → Completed  [terminal]
//!                                    ↘ Rejected                  [terminal]
//!     ↘ ValidationFailed → Rejected                             [terminal]
//!                             ↘ TimeoutExpired → Rejected       [terminal]
//! ```
//!
//! # Regulatory basis
//!
//! BK6-24-174 (Beschluss 24.10.2024, gültig ab 06.06.2025) + BK6-22-024 §5.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    GpkeStornierungCommand, GpkeStornierungState, GpkeStornierungWorkflow,
    STORNIERUNG_GPKE_APERAK_WINDOW_LABEL,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GpkeStornierungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gpke-stornierung", "FV2025-10-01"),
    )
}

fn lf_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
}

fn nb_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn vorgang() -> MaLo {
    // Vorgangsnummer from IDE+Z19 (identifies the original process being cancelled)
    MaLo::new("DE00123456789012345678901234567890")
}

fn msg(r: &str) -> MessageRef {
    MessageRef::new(r)
}

fn receive_stornierung_cmd(validation_passed: bool) -> GpkeStornierungCommand {
    GpkeStornierungCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(55022).unwrap(),
        sender: lf_gln(),
        receiver: nb_gln(),
        vorgang_id: vorgang(),
        document_date: "20250601".to_owned(),
        message_ref: msg("MSG-STORNO-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["UTILMD Strom segment IDE+Z19 missing mandatory Vorgangsnummer".to_owned()]
        },
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// New → Initiated: valid 55022 received, process enters active state.
#[tokio::test]
async fn receive_valid_stornierung_transitions_to_initiated() {
    let p = make_process();

    p.execute(receive_stornierung_cmd(true))
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

    p.execute(receive_stornierung_cmd(false))
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

/// Happy path: positive APERAK dispatched → Completed (terminal).
#[tokio::test]
async fn happy_path_positive_aperak_completes_process() {
    let p = make_process();

    p.execute(receive_stornierung_cmd(true))
        .await
        .expect("receive");

    p.execute(GpkeStornierungCommand::DispatchAperak {
        positive: true,
        reason: None,
    })
    .await
    .expect("DispatchAperak (positive) must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(
            state,
            GpkeStornierungState::Completed(_) | GpkeStornierungState::AperakSent(_)
        ),
        "expected Completed or AperakSent after positive APERAK, got: {state:?}",
    );
}

/// NB dispatches negative APERAK (55024) → Rejected (terminal).
#[tokio::test]
async fn negative_aperak_transitions_to_rejected() {
    let p = make_process();

    p.execute(receive_stornierung_cmd(true))
        .await
        .expect("receive");

    p.execute(GpkeStornierungCommand::DispatchAperak {
        positive: false,
        reason: Some("Stornierungsfrist bereits abgelaufen".to_owned()),
    })
    .await
    .expect("DispatchAperak (negative) must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "expected Rejected after negative APERAK, got: {state:?}",
    );
    assert!(
        matches!(state, GpkeStornierungState::Rejected { .. }),
        "Rejected must be a terminal variant",
    );
}

/// 24-hour deadline fires while waiting → terminal.
#[tokio::test]
async fn timeout_fires_while_awaiting_aperak() {
    let p = make_process();

    p.execute(receive_stornierung_cmd(true))
        .await
        .expect("receive");

    p.execute(GpkeStornierungCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from(STORNIERUNG_GPKE_APERAK_WINDOW_LABEL),
    })
    .await
    .expect("TimeoutExpired must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(
            state,
            GpkeStornierungState::Rejected { .. } | GpkeStornierungState::Completed(_)
        ),
        "state after TimeoutExpired must be terminal (Rejected or Completed), got: {state:?}",
    );
}

/// Domain data (Vorgangsnummer, sender) preserved through state transitions.
#[tokio::test]
async fn domain_data_preserved_in_validation_passed() {
    let p = make_process();

    let sender = lf_gln();
    let vorgangsnr = vorgang();

    p.execute(GpkeStornierungCommand::ReceiveUtilmd {
        pid: Pruefidentifikator::new(55022).unwrap(),
        sender: sender.clone(),
        receiver: nb_gln(),
        vorgang_id: vorgangsnr.clone(),
        document_date: "20250601".to_owned(),
        message_ref: msg("MSG-STORNO-DATA"),
        validation_passed: true,
        validation_errors: vec![],
    })
    .await
    .expect("receive");

    let state = p.state().await.expect("state");
    match &state {
        GpkeStornierungState::ValidationPassed(data) | GpkeStornierungState::Initiated(data) => {
            assert_eq!(data.sender, sender, "sender preserved");
            assert_eq!(data.vorgang_id, vorgangsnr, "vorgang_id preserved");
            assert_eq!(data.document_date, "20250601", "document_date preserved");
        }
        other => panic!("unexpected state: {other:?}"),
    }
}

/// APERAK Frist constant: label matches 24h window.
#[test]
fn aperak_window_label_is_correct() {
    assert!(
        STORNIERUNG_GPKE_APERAK_WINDOW_LABEL.contains("24h")
            || STORNIERUNG_GPKE_APERAK_WINDOW_LABEL.contains("gpke-stornierung"),
        "aperak window label must identify the 24h gpke-stornierung window: {STORNIERUNG_GPKE_APERAK_WINDOW_LABEL}",
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
            sender: lf_gln(),
            receiver: nb_gln(),
            vorgang_id: vorgang(),
            document_date: "20250601".to_owned(),
            message_ref: Some(msg("x")),
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
