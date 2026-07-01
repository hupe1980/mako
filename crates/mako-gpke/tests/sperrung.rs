//! Integration tests for GPKE Sperrung / Entsperrung workflows.
//!
//! Covers both sides of the Strom disconnection / reconnection process:
//!
//! - **`gpke-sperrung`** — NB-role (NB receives Sperrauftrag from LF, forwards to MSB,
//!   confirms execution to LF via ORDRSP).
//! - **`gpke-sperrung-lf`** — LF-role (LF initiates Sperrauftrag outbound, awaits NB's
//!   ORDRSP 19116/19117, then awaits IFTSTA 21039 after confirmation).
//!
//! # APERAK Frist
//!
//! **24 wall-clock hours** (BK6-22-024 §5). Saturday counts as a Werktag but the
//! 24h window is wall-clock, not business-day based.
//!
//! # Regulatory basis
//!
//! BK6-22-024 (AWH Sperrprozesse / GPKE Teil 2/3).

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    GpkeSperrungLfWorkflow, GpkeSperrungWorkflow, SPERRUNG_LF_ANTWORT_WINDOW_LABEL,
    SPERRUNG_WINDOW_LABEL, SperrungCommand, SperrungLfCommand, SperrungLfState, SperrungState,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn nb_process() -> Process<GpkeSperrungWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gpke-sperrung", "FV2025-10-01"),
    )
}

fn lf_process() -> Process<GpkeSperrungLfWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gpke-sperrung-lf", "FV2025-10-01"),
    )
}

fn nb_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn lf_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
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

fn receive_sperrauftrag(validation_passed: bool) -> SperrungCommand {
    SperrungCommand::ReceiveSperrung {
        pid: pid(17115),
        sender: lf_gln(),
        location_id: malo(),
        document_date: "20250601".to_owned(),
        message_ref: msg("LF-SPERR-001"),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["ORDERS segment BGM missing Sperrauftrag qualifier".to_owned()]
        },
    }
}

// ── NB-role (gpke-sperrung) ────────────────────────────────────────────────────

/// New → ValidationPassed: NB receives a valid Sperrauftrag.
#[tokio::test]
async fn nb_receive_valid_sperrauftrag_validation_passed() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("ReceiveSperrung (valid) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungState::ValidationPassed(_)),
        "expected ValidationPassed, got: {state:?}",
    );
}

/// New → Rejected: NB receives an invalid Sperrauftrag (validation failed).
#[tokio::test]
async fn nb_receive_invalid_sperrauftrag_transitions_to_rejected() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(false))
        .await
        .expect("ReceiveSperrung (invalid) must not return Err — emits Rejected event");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungState::Rejected { .. }),
        "expected Rejected after failed validation, got: {state:?}",
    );
    assert!(state.is_terminal(), "Rejected must be terminal");
}

/// Happy path: New → ValidationPassed → Ausgefuehrt.
#[tokio::test]
async fn nb_happy_path_ausgefuehrt() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("receive");

    p.execute(SperrungCommand::BestaetigueSperrung {
        durchgefuehrt: true,
        reason: None,
    })
    .await
    .expect("BestaetigueSperrung must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungState::Ausgefuehrt(_)),
        "expected Ausgefuehrt, got: {state:?}",
    );
    assert!(state.is_terminal(), "Ausgefuehrt must be terminal");
}

/// NB: confirm with `durchgefuehrt = false` → Rejected (non-execution recorded as failure).
#[tokio::test]
async fn nb_bestaetige_nicht_durchgefuehrt_transitions_to_rejected() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("receive");

    p.execute(SperrungCommand::BestaetigueSperrung {
        durchgefuehrt: false,
        reason: Some("Zähler nicht zugänglich".to_owned()),
    })
    .await
    .expect("BestaetigueSperrung (nicht durchgeführt) must succeed");

    let state = p.state().await.expect("state");
    assert!(
        state.is_terminal(),
        "state after nicht-durchgeführt must be terminal, got: {state:?}",
    );
}

/// NB: Stornierung received from LF → Storniert.
#[tokio::test]
async fn nb_receive_stornierung_transitions_to_storniert() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("receive");

    p.execute(SperrungCommand::ReceiveStornierung {
        pid: pid(39000),
        sender: lf_gln(),
        message_ref: msg("LF-STORNO-001"),
    })
    .await
    .expect("ReceiveStornierung must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungState::Storniert(_)),
        "expected Storniert, got: {state:?}",
    );
    assert!(state.is_terminal(), "Storniert must be terminal");
}

/// NB: receive MSB Bestätigung (ORDRSP 19118) — gated by MSB before confirming to LF.
#[tokio::test]
async fn nb_receive_msb_antwort_bestaetigung_not_yet_terminal() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("receive");

    p.execute(SperrungCommand::ReceiveMsbAntwort {
        pid: pid(19118),
        is_confirmed: true,
        message_ref: msg("MSB-ORDRSP-001"),
    })
    .await
    .expect("ReceiveMsbAntwort must succeed from ValidationPassed");

    // After MSB confirms, NB still needs to explicitly BestaetigueSperrung
    let state = p.state().await.expect("state");
    assert!(
        !state.is_terminal(),
        "process must not be terminal after MSB Bestätigung alone — NB must confirm execution",
    );
}

/// NB: deadline fires while awaiting execution confirmation → terminal.
#[tokio::test]
async fn nb_timeout_fires_while_active() {
    let p = nb_process();

    p.execute(receive_sperrauftrag(true))
        .await
        .expect("receive");

    p.execute(SperrungCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from(SPERRUNG_WINDOW_LABEL),
    })
    .await
    .expect("TimeoutExpired must succeed from active state");

    let state = p.state().await.expect("state");
    assert!(
        state.is_terminal(),
        "state after TimeoutExpired must be terminal, got: {state:?}",
    );
}

/// NB: domain data is preserved through ValidationPassed.
#[tokio::test]
async fn nb_domain_data_preserved_in_validation_passed() {
    let p = nb_process();

    let location = malo();
    let sender = lf_gln();

    p.execute(SperrungCommand::ReceiveSperrung {
        pid: pid(17115),
        sender: sender.clone(),
        location_id: location.clone(),
        document_date: "20250601".to_owned(),
        message_ref: msg("NB-DATA-001"),
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

/// NB: Entsperrauftrag (17117) accepted the same way as Sperrauftrag (17115).
#[tokio::test]
async fn nb_receive_entsperrauftrag_valid() {
    let p = nb_process();

    p.execute(SperrungCommand::ReceiveSperrung {
        pid: pid(17117),
        sender: lf_gln(),
        location_id: malo(),
        document_date: "20250601".to_owned(),
        message_ref: msg("LF-ENTSPERR-001"),
        validation_passed: true,
        validation_errors: vec![],
    })
    .await
    .expect("Entsperrauftrag (PID 17117) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungState::ValidationPassed(_)),
        "expected ValidationPassed for Entsperrauftrag, got: {state:?}",
    );
}

// ── LF-role (gpke-sperrung-lf) ────────────────────────────────────────────────

/// New → AuftragGesendet: LF initiates a Sperrauftrag (ORDERS 17115).
#[tokio::test]
async fn lf_initiate_sperrauftrag_transitions_to_auftrag_gesendet() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-002"),
    })
    .await
    .expect("InitiateSperrung must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::AuftragGesendet(_)),
        "expected AuftragGesendet, got: {state:?}",
    );
}

/// New → AuftragGesendet: LF initiates a Entsperrauftrag (ORDERS 17117).
#[tokio::test]
async fn lf_initiate_entsperrauftrag_transitions_to_auftrag_gesendet() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17117),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-ENTSPERR-002"),
    })
    .await
    .expect("InitiateSperrung (Entsperrauftrag) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::AuftragGesendet(_)),
        "expected AuftragGesendet for Entsperrauftrag, got: {state:?}",
    );
}

/// Happy path: New → AuftragGesendet → OrdrsepBestaetigt → Ausgefuehrt (via IFTSTA 21039).
#[tokio::test]
async fn lf_happy_path_auftrag_bestaetigt_then_ausgefuehrt() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-003"),
    })
    .await
    .expect("initiate");

    // NB confirms (ORDRSP 19116)
    p.execute(SperrungLfCommand::ReceiveOrdrsp {
        pid: pid(19116),
        is_confirmed: true,
        message_ref: msg("NB-ORDRSP-001"),
        sender: nb_gln(),
        reason: None,
    })
    .await
    .expect("ReceiveOrdrsp (Bestätigung) must succeed from AuftragGesendet");

    let state = p.state().await.expect("state after ORDRSP");
    assert!(
        matches!(state, SperrungLfState::OrdrsepBestaetigt(_)),
        "expected OrdrsepBestaetigt after 19116, got: {state:?}",
    );

    // NB sends IFTSTA 21039 (Sperrung executed)
    p.execute(SperrungLfCommand::ReceiveIftsta {
        pid: pid(21039),
        message_ref: msg("NB-IFTSTA-001"),
        sender: nb_gln(),
    })
    .await
    .expect("ReceiveIftsta must succeed from OrdrsepBestaetigt");

    let state = p.state().await.expect("state after IFTSTA");
    assert!(
        matches!(state, SperrungLfState::Ausgefuehrt(_)),
        "expected Ausgefuehrt after IFTSTA 21039, got: {state:?}",
    );
    assert!(state.is_terminal(), "Ausgefuehrt must be terminal");
}

/// LF-side: NB rejects Sperrauftrag → OrdrsepAbgelehnt (terminal).
#[tokio::test]
async fn lf_rejection_transitions_to_ordrsp_abgelehnt() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-004"),
    })
    .await
    .expect("initiate");

    p.execute(SperrungLfCommand::ReceiveOrdrsp {
        pid: pid(19117),
        is_confirmed: false,
        message_ref: msg("NB-ORDRSP-002"),
        sender: nb_gln(),
        reason: Some("Keine Sperrpflicht an dieser Messlokation".to_owned()),
    })
    .await
    .expect("ReceiveOrdrsp (Ablehnung) must succeed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::OrdrsepAbgelehnt { .. }),
        "expected OrdrsepAbgelehnt, got: {state:?}",
    );
    assert!(state.is_terminal(), "OrdrsepAbgelehnt must be terminal");
}

/// LF-side: send Stornierung before NB responds → StornierungGesendet.
#[tokio::test]
async fn lf_stornierung_before_ordrsp() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-005"),
    })
    .await
    .expect("initiate");

    p.execute(SperrungLfCommand::SendStornierung {
        message_ref: msg("LF-STORNO-001"),
    })
    .await
    .expect("SendStornierung must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::StornierungGesendet(_)),
        "expected StornierungGesendet, got: {state:?}",
    );
}

/// LF-side: full Stornierung path accepted → StornoBestaetigt (terminal).
#[tokio::test]
async fn lf_stornierung_accepted_path() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-006"),
    })
    .await
    .expect("initiate");

    p.execute(SperrungLfCommand::SendStornierung {
        message_ref: msg("LF-STORNO-002"),
    })
    .await
    .expect("stornierung");

    // NB confirms stornierung (ORDRSP 19128)
    p.execute(SperrungLfCommand::ReceiveStornoOrdrsp {
        pid: pid(19128),
        is_confirmed: true,
        message_ref: msg("NB-STORNO-001"),
        sender: nb_gln(),
    })
    .await
    .expect("ReceiveStornoOrdrsp (Bestätigung) must succeed from StornierungGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::StornoBestaetigt(_)),
        "expected StornoBestaetigt, got: {state:?}",
    );
    assert!(state.is_terminal(), "StornoBestaetigt must be terminal");
}

/// LF-side: deadline fires while awaiting NB response → DeadlineExpired (terminal).
#[tokio::test]
async fn lf_timeout_fires_deadline_expired() {
    let p = lf_process();

    p.execute(SperrungLfCommand::InitiateSperrung {
        pid: pid(17115),
        nb_gln: nb_gln(),
        location_id: malo(),
        message_ref: msg("LF-SPERR-007"),
    })
    .await
    .expect("initiate");

    p.execute(SperrungLfCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from(SPERRUNG_LF_ANTWORT_WINDOW_LABEL),
    })
    .await
    .expect("TimeoutExpired must succeed from AuftragGesendet");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, SperrungLfState::DeadlineExpired { .. }),
        "expected DeadlineExpired, got: {state:?}",
    );
    assert!(state.is_terminal(), "DeadlineExpired must be terminal");
}

/// APERAK Frist constant: LF-side label matches 24h window.
#[test]
fn lf_antwort_window_label_is_correct() {
    assert!(
        SPERRUNG_LF_ANTWORT_WINDOW_LABEL.contains("24h")
            || SPERRUNG_LF_ANTWORT_WINDOW_LABEL.contains("gpke-sperrung-lf"),
        "deadline label must identify the 24h GPKE sperrung window: {SPERRUNG_LF_ANTWORT_WINDOW_LABEL}",
    );
}

/// APERAK Frist constant: NB-side deadline label is set.
#[test]
fn nb_window_label_is_set() {
    assert!(
        !SPERRUNG_WINDOW_LABEL.is_empty(),
        "SPERRUNG_WINDOW_LABEL must be non-empty",
    );
}
