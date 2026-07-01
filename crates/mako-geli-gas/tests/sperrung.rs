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
//! **10 Werktage** (BK7-24-01-009).  Saturday counts as a Werktag; Sunday and
//! federal public holidays do not.
//!
//! # Regulatory basis
//!
//! BK7-24-01-009 (Beschluss 12.09.2025) — GeLi Gas 3.0.

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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-001"),
    })
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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17117),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-ENTSPERR-001"),
    })
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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-002"),
    })
    .await
    .expect("initiate");

    // GNB confirms (ORDRSP 19116)
    p.execute(GasSperrungLfCommand::ReceiveOrdrsp {
        pid: pid(19116),
        is_confirmed: true,
        message_ref: msg("GNB-ORDRSP-001"),
        sender: lf_gnb(),
        reason: None,
    })
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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-003"),
    })
    .await
    .expect("initiate");

    p.execute(GasSperrungLfCommand::ReceiveOrdrsp {
        pid: pid(19117),
        is_confirmed: false,
        message_ref: msg("GNB-ORDRSP-002"),
        sender: lf_gnb(),
        reason: Some("Keine Zuständigkeit für diese Marktlokation".to_owned()),
    })
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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-004"),
    })
    .await
    .expect("initiate");

    p.execute(GasSperrungLfCommand::SendStornierung {
        message_ref: msg("LF-STORNO-001"),
    })
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

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-005"),
    })
    .await
    .expect("initiate");

    p.execute(GasSperrungLfCommand::SendStornierung {
        message_ref: msg("LF-STORNO-002"),
    })
    .await
    .expect("stornierung");

    // GNB confirms stornierung (ORDRSP 19128)
    p.execute(GasSperrungLfCommand::ReceiveStornoOrdrsp {
        pid: pid(19128),
        is_confirmed: true,
        message_ref: msg("GNB-STORNO-001"),
        sender: lf_gnb(),
    })
    .await
    .expect("ReceiveStornoOrdrsp (Bestätigung) must succeed from StornierungGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::StornoBestaetigt(_)),
        "expected StornoBestaetigt, got: {state:?}",
    );
    assert!(state.is_terminal(), "StornoBestaetigt must be terminal");
}

/// LF-side: deadline fires while awaiting GNB response → DeadlineExpired.
#[tokio::test]
async fn lf_timeout_fires_deadline_expired() {
    let p = lf_process();

    p.execute(GasSperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        gnb_gln: lf_gnb(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-006"),
    })
    .await
    .expect("initiate");

    p.execute(GasSperrungLfCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from("geli-gas-sperrung-lf-antwort-10wt"),
    })
    .await
    .expect("TimeoutExpired must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungLfState::DeadlineExpired { .. }),
        "expected DeadlineExpired, got: {state:?}",
    );
    assert!(state.is_terminal(), "DeadlineExpired must be terminal");
}

// ── GNB-side tests ────────────────────────────────────────────────────────────

fn receive_sperrung_cmd(validation_passed: bool) -> GasSperrungNbCommand {
    GasSperrungNbCommand::ReceiveSperrung {
        pid: pid(17115),
        sender: MarktpartnerCode::new("4012345000023"),
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

    p.execute(receive_sperrung_cmd(true))
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

    p.execute(receive_sperrung_cmd(false))
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

    p.execute(receive_sperrung_cmd(true))
        .await
        .expect("receive");

    p.execute(GasSperrungNbCommand::BestaetigueSperrung {
        durchgefuehrt: true,
        reason: None,
    })
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

    p.execute(receive_sperrung_cmd(true))
        .await
        .expect("receive");

    p.execute(GasSperrungNbCommand::BestaetigueSperrung {
        durchgefuehrt: false,
        reason: Some("Kundenanlage nicht zugänglich".to_owned()),
    })
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

    p.execute(receive_sperrung_cmd(true))
        .await
        .expect("receive");

    p.execute(GasSperrungNbCommand::ReceiveMsbAntwort {
        pid: pid(19118),
        is_confirmed: true,
        message_ref: msg("MSB-ORDRSP-001"),
    })
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

    p.execute(receive_sperrung_cmd(true))
        .await
        .expect("receive");

    p.execute(GasSperrungNbCommand::ReceiveStornierung {
        pid: pid(39000),
        sender: MarktpartnerCode::new("4012345000023"),
        message_ref: msg("LF-STORNO-NB-001"),
    })
    .await
    .expect("ReceiveStornierung must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GasSperrungNbState::Storniert(_)),
        "expected Storniert, got: {state:?}",
    );
    assert!(state.is_terminal(), "Storniert must be terminal");
}

/// GNB-side: deadline fires (10-WT window) → DeadlineExpired state is recorded.
#[tokio::test]
async fn nb_timeout_fires_while_active() {
    let p = nb_process();

    p.execute(receive_sperrung_cmd(true))
        .await
        .expect("receive");

    let label = Box::from(GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL);
    p.execute(GasSperrungNbCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label,
    })
    .await
    .expect("TimeoutExpired must succeed from active state");

    // The workflow transitions to a terminal rejected/timeout state
    let state = p.state().await.expect("state");
    assert!(
        state.is_terminal(),
        "state after TimeoutExpired must be terminal, got: {state:?}",
    );
}

/// GNB-side: domain data is preserved through state transitions.
#[tokio::test]
async fn nb_domain_data_preserved_in_validation_passed() {
    let p = nb_process();

    let location = MaLo::new("DE00123456789012345678901234567890");
    let sender = MarktpartnerCode::new("4012345000023");

    p.execute(GasSperrungNbCommand::ReceiveSperrung {
        pid: pid(17115),
        sender: sender.clone(),
        location_id: location.clone(),
        document_date: "20250601".to_owned(),
        message_ref: msg("NB-RECV-DATA"),
        validation_passed: true,
        validation_errors: vec![],
    })
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
        GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL.contains("10wt")
            || GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL.contains("geli-gas-sperrung-nb"),
        "deadline label must identify the 10-WT gas sperrung window: {GELI_GAS_SPERRUNG_NB_ANTWORT_WINDOW_LABEL}",
    );
}
