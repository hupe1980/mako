//! Integration tests for GeLi Gas Sperrung / Entsperrung workflows.
//!
//! Covers both sides of the gas disconnection / reconnection process:
//!
//! - **`geli-gas-sperrung-lf`** — LF-side: LF initiates a Sperrauftrag (ORDERS 17115)
//!   or Entsperrauftrag (ORDERS 17117), then awaits the GNB's ORDRSP.
//! - **`geli-gas-sperrung-nb`** — GNB-side: GNB receives the Anweisung from LF,
//!   optionally forwards to gMSB (ORDERS 17116), receives gMSB response
//!   (ORDRSP 19118/19119), and confirms execution to the LF.
//!
//! # APERAK Frist
//!
//! the window the Prüfidentifikator publishes — 1 Werktag for the
//! Sperr-/Entsperrauftrag. Saturdays, Sundays and federal public
//! holidays are not Werktage.
//!
//! # Regulatory basis
//!
//! BK7-24-01-009 (Beschluss 12.09.2025) — GeLi Gas 3.0.

use mako_engine::process::RunOptions;
use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL, GasSperrungLfCommand, GasSperrungLfState,
    GasSperrungNbCommand, GasSperrungNbState, GeliGasSperrungLfWorkflow, GeliGasSperrungNbWorkflow,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn lf_process() -> Process<GeliGasSperrungLfWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("geli-gas-sperrung-lf", "FV2025-10-01"),
    )
}

fn nb_process() -> Process<GeliGasSperrungNbWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("geli-gas-sperrung-nb", "FV2025-10-01"),
    )
}

fn lf_gnb() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn malo() -> MaLo {
    MaLo::new("DE00123456789012345678901234567890")
}

fn msg(r: &str) -> MessageRef {
    MessageRef::new(r)
}

fn pid(n: u32) -> Pruefidentifikator {
    Pruefidentifikator::new(n).unwrap_or_else(|_| panic!("PID {n} must be valid"))
}

// ── LF-side tests ─────────────────────────────────────────────────────────────

/// New → AuftragGesendet: initiate a Gas-Sperrauftrag (ORDERS 17115).
#[tokio::test]
async fn lf_initiate_sperrauftrag_transitions_to_auftrag_gesendet() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-001"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("InitiateSperrung must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::AuftragGesendet(_)),
        "expected AuftragGesendet, got: {state:?}",
    );
}

/// New → AuftragGesendet: initiate a Gas-Entsperrauftrag (ORDERS 17117).
#[tokio::test]
async fn lf_initiate_entsperrauftrag_transitions_to_auftrag_gesendet() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17117),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-ENTSPERR-001"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("InitiateSperrung (Entsperrauftrag) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::AuftragGesendet(_)),
        "expected AuftragGesendet for Entsperrauftrag, got: {state:?}",
    );
}

/// Happy path: New → AuftragGesendet → OrdrspBestaetigt.
#[tokio::test]
async fn lf_happy_path_sperrauftrag_bestaetigt() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-002"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("initiate");

    // GNB confirms (ORDRSP 19116)
    p.run(
        GasSperrungLfCommand::ReceiveOrdrsp {
            pid: pid(19116),
            is_confirmed: true,
            message_ref: msg("GNB-ORDRSP-001"),
            sender: lf_gnb(),
            reason: None,
        },
        RunOptions::new(),
    )
    .await
    .expect("ReceiveOrdrsp (Bestätigung) must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::OrdrspBestaetigt(_)),
        "expected OrdrspBestaetigt, got: {state:?}",
    );
    assert!(state.is_terminal(), "OrdrspBestaetigt must be terminal");
}

/// LF-side: GNB rejects Sperrauftrag → OrdrspAbgelehnt.
#[tokio::test]
async fn lf_rejection_transitions_to_ordrsp_abgelehnt() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-003"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("initiate");

    p.run(
        GasSperrungLfCommand::ReceiveOrdrsp {
            pid: pid(19117),
            is_confirmed: false,
            message_ref: msg("GNB-ORDRSP-002"),
            sender: lf_gnb(),
            reason: Some("Keine Zuständigkeit für diese Marktlokation".to_owned()),
        },
        RunOptions::new(),
    )
    .await
    .expect("ReceiveOrdrsp (Ablehnung) must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::OrdrspAbgelehnt { .. }),
        "expected OrdrspAbgelehnt, got: {state:?}",
    );
    assert!(state.is_terminal(), "OrdrspAbgelehnt must be terminal");
}

/// LF-side: send Stornierung before GNB responds → StornierungGesendet.
#[tokio::test]
async fn lf_stornierung_before_ordrsp() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-004"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("initiate");

    p.run(
        GasSperrungLfCommand::SendStornierung {
            message_ref: msg("LF-STORNO-001"),
        },
        RunOptions::new(),
    )
    .await
    .expect("SendStornierung must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::StornierungGesendet(_)),
        "expected StornierungGesendet, got: {state:?}",
    );
}

/// LF-side: full Stornierung accepted path → StornoBestaetigt.
#[tokio::test]
async fn lf_stornierung_accepted_path() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-005"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("initiate");

    p.run(
        GasSperrungLfCommand::SendStornierung {
            message_ref: msg("LF-STORNO-002"),
        },
        RunOptions::new(),
    )
    .await
    .expect("stornierung");

    // GNB confirms stornierung (ORDRSP 19128)
    p.run(
        GasSperrungLfCommand::ReceiveStornoOrdrsp {
            pid: pid(19128),
            is_confirmed: true,
            message_ref: msg("GNB-STORNO-001"),
            sender: lf_gnb(),
        },
        RunOptions::new(),
    )
    .await
    .expect("ReceiveStornoOrdrsp (Bestätigung) must succeed from StornierungGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::StornoBestaetigt(_)),
        "expected StornoBestaetigt, got: {state:?}",
    );
    assert!(state.is_terminal(), "StornoBestaetigt must be terminal");
}

/// LF-side: the GNB's answer window lapses → `FristAbgelaufen`, still open for
/// the GNB's late ORDRSP.
#[tokio::test]
async fn lf_timeout_is_a_frist_breach() {
    let p = lf_process();

    p.run(
        GasSperrungLfCommand::InitiateSperrung {
            pid: pid(17115),
            gnb_gln: lf_gnb(),
            location_id: malo(),
            message_ref: msg("LF-SPERR-006"),
            gesendet_am: time::OffsetDateTime::UNIX_EPOCH,
        },
        RunOptions::new(),
    )
    .await
    .expect("initiate");

    p.run(
        GasSperrungLfCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: Box::from(mako_geli_gas::sperrung_lf::ANTWORT_WINDOW_LABEL),
        },
        RunOptions::new(),
    )
    .await
    .expect("TimeoutExpired must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::FristAbgelaufen { .. }),
        "expected FristAbgelaufen, got: {state:?}",
    );
    assert!(
        !state.is_terminal(),
        "a lapsed window leaves the Gas-Sperrauftrag waiting for the GNB"
    );
}

// ── GNB-side tests ────────────────────────────────────────────────────────────

fn receive_sperrung_cmd(validation_passed: bool) -> GasSperrungNbCommand {
    GasSperrungNbCommand::ReceiveSperrung {
        pid: pid(17115),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: lf_gnb(),
        location_id: malo(),
        document_date: "20250601".to_owned(),
        message_ref: msg("NB-RECV-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["ORDERS segment BGM missing mandatory reference qualifier".to_owned()]
        },
    }
}

/// New → ValidationPassed: GNB receives a valid Sperrauftrag.
#[tokio::test]
async fn nb_receive_valid_sperrauftrag_validation_passed() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("ReceiveSperrung (valid) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::ValidationPassed(_)),
        "expected ValidationPassed, got: {state:?}",
    );
}

/// New → Rejected: GNB receives an invalid Sperrauftrag (validation failed).
#[tokio::test]
async fn nb_receive_invalid_sperrauftrag_transitions_to_rejected() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(false), RunOptions::new())
        .await
        .expect("ReceiveSperrung (invalid) must not return Err — it emits Rejected event");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::Rejected { .. }),
        "expected Rejected after failed validation, got: {state:?}",
    );
    assert!(state.is_terminal(), "Rejected must be terminal");
}

/// Happy path: New → ValidationPassed → Ausgefuehrt.
#[tokio::test]
async fn nb_happy_path_ausgefuehrt() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GasSperrungNbCommand::BestaetigueSperrung {
            durchgefuehrt: true,
            reason: None,
        },
        RunOptions::new(),
    )
    .await
    .expect("BestaetigueSperrung must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::Ausgefuehrt(_)),
        "expected Ausgefuehrt, got: {state:?}",
    );
    assert!(state.is_terminal(), "Ausgefuehrt must be terminal");
}

/// GNB-side: confirm with `durchgefuehrt = false` → Rejected (non-execution recorded as failure).
///
/// When the GNB cannot carry out the Sperrung, the process is rejected — not
/// considered executed. `Ausgefuehrt` only applies when `durchgefuehrt = true`.
#[tokio::test]
async fn nb_bestaetige_nicht_durchgefuehrt_transitions_to_rejected() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GasSperrungNbCommand::BestaetigueSperrung {
            durchgefuehrt: false,
            reason: Some("Kundenanlage nicht zugänglich".to_owned()),
        },
        RunOptions::new(),
    )
    .await
    .expect("BestaetigueSperrung (nicht durchgeführt) must succeed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::Rejected { .. }),
        "expected Rejected when durchgefuehrt=false, got: {state:?}",
    );
    assert!(state.is_terminal(), "Rejected must be terminal");
}

/// GNB-side: receive gMSB Bestätigung (ORDRSP 19118) is recorded.
#[tokio::test]
async fn nb_receive_msb_antwort_bestaetigung() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GasSperrungNbCommand::ReceiveMsbAntwort {
            pid: pid(19118),
            is_confirmed: true,
            message_ref: msg("MSB-ORDRSP-001"),
        },
        RunOptions::new(),
    )
    .await
    .expect("ReceiveMsbAntwort (Bestätigung) must succeed from ValidationPassed");

    // After gMSB confirms, GNB must still explicitly BestaetigueSperrung
    let state = p.state().await.expect("state");
    assert!(
        !state.is_terminal(),
        "process must not be terminal after gMSB Bestätigung alone — GNB must confirm execution",
    );
}

/// GNB-side: Stornierung received from LF → Storniert.
#[tokio::test]
async fn nb_receive_stornierung_transitions_to_storniert() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    p.run(
        GasSperrungNbCommand::ReceiveStornierung {
            pid: pid(39000),
            sender: MarktpartnerCode::new("4012345000023"),
            message_ref: msg("LF-STORNO-NB-001"),
        },
        RunOptions::new(),
    )
    .await
    .expect("ReceiveStornierung must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::Storniert(_)),
        "expected Storniert, got: {state:?}",
    );
    assert!(state.is_terminal(), "Storniert must be terminal");
}

/// GNB-side: its own answer window lapses → `FristAbgelaufen`, a Frist breach
/// the operator sees; the Sperrauftrag is still owed its answer.
#[tokio::test]
async fn nb_timeout_is_a_frist_breach() {
    let p = nb_process();

    p.run(receive_sperrung_cmd(true), RunOptions::new())
        .await
        .expect("receive");

    let label = Box::from(GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL);
    p.run(
        GasSperrungNbCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label,
        },
        RunOptions::new(),
    )
    .await
    .expect("TimeoutExpired must succeed from active state");

    let state = p.state().await.expect("state");
    assert!(
        state.label() == "FristAbgelaufen" && !state.is_terminal(),
        "state after TimeoutExpired must be FristAbgelaufen, got: {state:?}",
    );
}

/// GNB-side: domain data is preserved through state transitions.
#[tokio::test]
async fn nb_domain_data_preserved_in_validation_passed() {
    let p = nb_process();

    let location = MaLo::new("DE00123456789012345678901234567890");
    let sender = MarktpartnerCode::new("4012345000023");

    p.run(
        GasSperrungNbCommand::ReceiveSperrung {
            pid: pid(17115),
            sender: sender.clone(),
            receiver: lf_gnb(),
            location_id: location.clone(),
            document_date: "20250601".to_owned(),
            message_ref: msg("NB-RECV-DATA"),
            validation_passed: true,
            validation_errors: vec![],
        },
        RunOptions::new(),
    )
    .await
    .expect("receive");

    let state = p.state().await.expect("state");
    let data = state
        .sperrung_data()
        .expect("ValidationPassed must have domain data");

    assert_eq!(data.location_id, location, "location_id preserved");
    assert_eq!(data.sender, sender, "sender preserved");
    assert_eq!(data.document_date, "20250601", "document_date preserved");
}

/// APERAK Frist constant: the GNB-side deadline label matches the 10-WT window.
#[test]
fn nb_antwort_window_label_is_correct() {
    assert!(
        GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL.contains("geli-gas-sperrung-nb")
            && !GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL.contains("10wt"),
        "the Gas Sperrung answer window is 1 Werktag on the Sparte-neutral 17115/17117 \
         row, not the 10 Werktage the old label advertised from a Festlegung that has \
         no Sperrprozess in it: {GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL}",
    );
}
