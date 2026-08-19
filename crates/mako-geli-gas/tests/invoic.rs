//! Integration tests for the GeLi Gas AWH Sperrprozesse INVOIC billing workflow.
//!
//! The gas network operator (GNB/VNB) sends an INVOIC 31011 to the supplier
//! (LFN/LFA) for services rendered during the gas disconnection/reconnection
//! process (AWH = Abrechnungswürdige Handlungen from Sperrprozesse Gas).
//!
//! # State machine
//!
//! ```text
//! ── Recipient (LFN/LFA) ───────────────────────────────────────────────────
//! New ──ReceiveInvoic──► InvoicReceived ──[valid]──► ValidationPassed ──SettleInvoice──► Settled
//!                                        ╰──[invalid]──► Rejected      ╰─DisputeInvoice──► Disputed
//!
//! ── Issuer (GNB/VNB) ──────────────────────────────────────────────────────
//! New ──SendInvoic──► InvoicSent ──ReceiveRemadv 33001──► PaymentConfirmed
//!                                ╰─ReceiveRemadv 33002/3/4──► PaymentDisputed
//!
//! Any active state ──TimeoutExpired──► Rejected
//! ```
//!
//! # Regulatory basis
//!
//! BK7-24-01-009 — GeLi Gas 3.0 (Beschluss 12.09.2025).
//! PID 31011 belongs to GeLi Gas (NB → LF billing for AWH), not GaBi Gas.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GeliGasSperrprozesseInvoicCommand, GeliGasSperrprozesseInvoicState,
    GeliGasSperrprozesseInvoicWorkflow, SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL,
};

// ── Helpers ────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GeliGasSperrprozesseInvoicWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("geli-gas-sperrprozesse-invoic", "FV2025-10-01"),
    )
}

fn gnb_gln() -> MarktpartnerCode {
    MarktpartnerCode::new("9900357000004")
}

fn lf_mp_id() -> MarktpartnerCode {
    MarktpartnerCode::new("4012345000023")
}

fn msg(r: &str) -> MessageRef {
    MessageRef::new(r)
}

fn receive_invoic_cmd(validation_passed: bool) -> GeliGasSperrprozesseInvoicCommand {
    GeliGasSperrprozesseInvoicCommand::ReceiveInvoic {
        pid: Pruefidentifikator::new(31011).unwrap(),
        sender: gnb_gln(),
        recipient: lf_mp_id(),
        invoice_ref: msg("INV-2025-001"),
        document_date: "20250601".to_owned(),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["INVOIC AHB segment MOA+77 missing mandatory net amount".to_owned()]
        },
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

/// New → ValidationPassed: valid INVOIC 31011 received and validated.
#[tokio::test]
async fn receive_valid_invoic_transitions_to_validation_passed() {
    let p = make_process();

    p.execute(receive_invoic_cmd(true))
        .await
        .expect("ReceiveInvoic (valid) must succeed from New");

    let state = p.state().await.expect("state");
    assert!(
        matches!(
            state,
            GeliGasSperrprozesseInvoicState::ValidationPassed(_)
                | GeliGasSperrprozesseInvoicState::InvoicReceived(_)
        ),
        "expected InvoicReceived or ValidationPassed after valid INVOIC, got: {state:?}",
    );
}

/// New → Rejected: invalid INVOIC 31011 rejected immediately.
#[tokio::test]
async fn receive_invalid_invoic_transitions_to_rejected() {
    let p = make_process();

    p.execute(receive_invoic_cmd(false))
        .await
        .expect("ReceiveInvoic (invalid) must not return Err — emits Rejected event");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasSperrprozesseInvoicState::Rejected { .. }),
        "expected Rejected after failed validation, got: {state:?}",
    );
}

/// Happy path: INVOIC received → ValidationPassed → Settled.
#[tokio::test]
async fn happy_path_invoic_settled() {
    let p = make_process();

    p.execute(receive_invoic_cmd(true))
        .await
        .expect("receive invoic");

    p.execute(GeliGasSperrprozesseInvoicCommand::SettleInvoice)
        .await
        .expect("SettleInvoice must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasSperrprozesseInvoicState::Settled(_)),
        "expected Settled, got: {state:?}",
    );
}

/// LFN/LFA disputes the invoice → Disputed (terminal).
#[tokio::test]
async fn invoice_disputed() {
    let p = make_process();

    p.execute(receive_invoic_cmd(true))
        .await
        .expect("receive invoic");

    p.execute(GeliGasSperrprozesseInvoicCommand::DisputeInvoice {
        reason: "Sperrauftrag war nicht berechtigt — GNB war nicht zuständig".to_owned(),
    })
    .await
    .expect("DisputeInvoice must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(state, GeliGasSperrprozesseInvoicState::Disputed { .. }),
        "expected Disputed, got: {state:?}",
    );
}

/// Settlement deadline fires before LFN/LFA responds → terminal.
#[tokio::test]
async fn timeout_fires_before_settlement() {
    let p = make_process();

    p.execute(receive_invoic_cmd(true))
        .await
        .expect("receive invoic");

    p.execute(GeliGasSperrprozesseInvoicCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: Box::from(SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL),
    })
    .await
    .expect("TimeoutExpired must succeed from ValidationPassed");

    let state = p.state().await.expect("state");
    assert!(
        matches!(
            state,
            GeliGasSperrprozesseInvoicState::Rejected { .. }
                | GeliGasSperrprozesseInvoicState::Settled(_)
                | GeliGasSperrprozesseInvoicState::Disputed { .. }
        ),
        "state after TimeoutExpired must be a terminal variant, got: {state:?}",
    );
}

/// Domain data (sender, recipient, invoice_ref) preserved through transitions.
#[tokio::test]
async fn domain_data_preserved_in_validation_passed() {
    let p = make_process();

    let sender = gnb_gln();
    let recipient = lf_mp_id();
    let inv_ref = msg("INV-DATA-001");

    p.execute(GeliGasSperrprozesseInvoicCommand::ReceiveInvoic {
        pid: Pruefidentifikator::new(31011).unwrap(),
        sender: sender.clone(),
        recipient: recipient.clone(),
        invoice_ref: inv_ref.clone(),
        document_date: "20250601".to_owned(),
        validation_passed: true,
        validation_errors: vec![],
    })
    .await
    .expect("receive");

    let state = p.state().await.expect("state");
    let data = match &state {
        GeliGasSperrprozesseInvoicState::ValidationPassed(d)
        | GeliGasSperrprozesseInvoicState::InvoicReceived(d)
        | GeliGasSperrprozesseInvoicState::Settled(d) => d,
        GeliGasSperrprozesseInvoicState::Disputed { data, .. } => data,
        other => panic!("expected data-bearing state, got: {other:?}"),
    };

    assert_eq!(data.sender, sender, "sender preserved");
    assert_eq!(data.recipient, recipient, "recipient preserved");
    assert_eq!(data.invoice_ref, inv_ref, "invoice_ref preserved");
    assert_eq!(data.document_date, "20250601", "document_date preserved");
}

/// PID 31011 belongs to GeLi Gas (not GaBi Gas) — workflow is correctly named.
#[test]
fn workflow_name_is_geli_gas() {
    use mako_geli_gas::GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME;
    assert_eq!(
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
        "geli-gas-sperrprozesse-invoic"
    );
    assert!(
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME.starts_with("geli-gas"),
        "PID 31011 belongs to geli-gas, not gabi-gas",
    );
}

/// Settlement deadline label identifies the GeLi Gas billing window.
#[test]
fn settlement_label_is_set() {
    assert!(
        !SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL.is_empty(),
        "SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL must be non-empty",
    );
}

// ── Issuer role (GNB/VNB) ──────────────────────────────────────────────────────

fn send_invoic_cmd(pid: u32) -> GeliGasSperrprozesseInvoicCommand {
    GeliGasSperrprozesseInvoicCommand::SendInvoic {
        pid: Pruefidentifikator::new(pid).expect("valid PID"),
        sender: gnb_gln(),
        recipient: lf_mp_id(),
        document_date: "20260131".to_owned(),
        invoice_ref: msg("AWH-2026-000001"),
    }
}

fn remadv_cmd(pid: u32) -> GeliGasSperrprozesseInvoicCommand {
    GeliGasSperrprozesseInvoicCommand::ReceiveRemadv {
        pid: Pruefidentifikator::new(pid).expect("valid PID"),
        remadv_ref: msg("REMADV-0001"),
        sender: lf_mp_id(),
    }
}

/// The GNB issues a 31011 and the payer confirms it.
///
/// This leg did not exist: the workflow modelled only the LFG receiving an
/// invoice, so a gas network operator could settle an AWH invoice and had
/// nowhere to dispatch it.
#[tokio::test]
async fn the_issuer_records_an_outbound_invoice_and_its_confirmation() {
    let p = make_process();
    p.execute(send_invoic_cmd(31011))
        .await
        .expect("the GNB may issue a 31011");
    assert!(matches!(
        p.state().await.expect("state"),
        GeliGasSperrprozesseInvoicState::InvoicSent(_)
    ));

    p.execute(remadv_cmd(33001))
        .await
        .expect("the payer confirms");
    let GeliGasSperrprozesseInvoicState::PaymentConfirmed(data) = p.state().await.expect("state")
    else {
        panic!(
            "expected PaymentConfirmed, got {:?}",
            p.state().await.expect("state")
        );
    };
    assert_eq!(data.invoice_ref, msg("AWH-2026-000001"));
    assert_eq!(data.sender, gnb_gln());
}

/// 33001 is the only Zahlungsbestätigung — 33002, 33003 and 33004 are all
/// Abweisungen, and 33003/33004 are itemised rejections rather than partial
/// payments. Treating any of them as a confirmation books money that never came.
#[tokio::test]
async fn only_33001_confirms_payment() {
    for pid in [33002, 33003, 33004] {
        let p = make_process();
        p.execute(send_invoic_cmd(31011)).await.expect("issue");
        p.execute(remadv_cmd(pid)).await.expect("answer");
        let GeliGasSperrprozesseInvoicState::PaymentDisputed { remadv_pid, .. } =
            p.state().await.expect("state")
        else {
            panic!(
                "REMADV {pid} must not confirm payment: {:?}",
                p.state().await.expect("state")
            );
        };
        assert_eq!(remadv_pid.as_u32(), pid);
    }
}

/// The issuer leg accepts only its own Prüfidentifikator.
#[tokio::test]
async fn the_issuer_leg_refuses_a_foreign_pid() {
    let p = make_process();
    assert!(
        p.execute(send_invoic_cmd(31009)).await.is_err(),
        "31009 is the MSB-Rechnung and belongs to mako-wim"
    );
}

/// A REMADV cannot answer an invoice this deployment never issued.
#[tokio::test]
async fn a_remadv_without_an_outbound_invoice_is_refused() {
    let p = make_process();
    assert!(p.execute(remadv_cmd(33001)).await.is_err());
}

/// A duplicate REMADV does not overwrite the answer that already landed.
#[tokio::test]
async fn a_late_remadv_does_not_overwrite_the_first_answer() {
    let p = make_process();
    p.execute(send_invoic_cmd(31011)).await.expect("issue");
    p.execute(remadv_cmd(33001)).await.expect("confirm");
    p.execute(remadv_cmd(33002))
        .await
        .expect("a late Abweisung is accepted, not an error");
    assert!(
        matches!(
            p.state().await.expect("state"),
            GeliGasSperrprozesseInvoicState::PaymentConfirmed(_)
        ),
        "the first answer stands: {:?}",
        p.state().await.expect("state")
    );
}
