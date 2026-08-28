//! What the GeLi Gas AWH Sperrprozesse billing family chooses.
//!
//! The state machine itself is [`mako_invoic`]'s and is tested there, once, in
//! `mako-invoic/tests/state_machine.rs`. These tests cover the family: its PID,
//! its routing, and the roles a GeLi Gas deployment plays.
//!
//! Regulatory basis: BK7-24-01-009 — GeLi Gas 3.0 (Beschluss 12.09.2025).
//! PID 31011 belongs to GeLi Gas (NB → LF billing for AWH), not GaBi Gas.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_geli_gas::{
    GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME, GeliGasSperrprozesseInvoicWorkflow,
    SPERRPROZESSE_INVOIC_PID, SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL,
};
use mako_invoic::{InvoicCommand, InvoicFamily};

fn make_process() -> Process<GeliGasSperrprozesseInvoicWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new(GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME, "FV2025-10-01"),
    )
}

/// PID 31011 belongs to GeLi Gas, not GaBi Gas — the workflow name says so.
#[test]
fn workflow_name_is_geli_gas() {
    assert_eq!(
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME,
        "geli-gas-sperrprozesse-invoic"
    );
    assert!(
        GELI_GAS_SPERRPROZESSE_INVOIC_WORKFLOW_NAME.starts_with("geli-gas"),
        "PID 31011 belongs to geli-gas, not gabi-gas",
    );
}

#[test]
fn settlement_label_is_set() {
    assert!(
        !SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL.is_empty(),
        "SPERRPROZESSE_INVOIC_SETTLEMENT_LABEL must be non-empty",
    );
}

#[test]
fn pid_31011_routes_to_the_family() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_geli_gas::GeliGasModule;

    let mut router = PidRouter::new();
    GeliGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    assert_eq!(
        router.route(SPERRPROZESSE_INVOIC_PID.as_u32()),
        Some("geli-gas-sperrprozesse-invoic"),
    );
}

/// Both roles ship — a GNB deployment issues the invoice, an LFG receives one —
/// and the AWH process publishes no COMDIS leg.
#[test]
fn both_roles_ship_and_there_is_no_comdis_leg() {
    const {
        assert!(mako_geli_gas::GeliGasSperrprozesseInvoic::SENDS_INVOIC);
        assert!(!mako_geli_gas::GeliGasSperrprozesseInvoic::ANSWERS_COMDIS);
    }
}

#[tokio::test]
async fn a_comdis_is_refused() {
    let p = make_process();
    p.execute(InvoicCommand::ReceiveInvoic {
        pid: SPERRPROZESSE_INVOIC_PID,
        sender: MarktpartnerCode::new("9900357000004"),
        recipient: MarktpartnerCode::new("4012345000023"),
        invoice_ref: MessageRef::new("INV-2026-001"),
        document_date: "20260601".to_owned(),
        validation_passed: true,
        validation_errors: vec![],
        rechnung: None,
        bestellung_ref: None,
        rechnungstyp: None,
    })
    .await
    .expect("a valid 31011 is accepted");

    let err = p
        .execute(InvoicCommand::ReceiveComdis {
            comdis_ref: MessageRef::new("COM-1"),
        })
        .await
        .expect_err("the AWH Sperrprozesse process publishes no COMDIS leg");
    assert!(format!("{err}").contains("COMDIS"), "{err}");
}

#[tokio::test]
async fn a_foreign_pid_is_refused_on_both_legs() {
    let foreign = Pruefidentifikator::new(31010).unwrap();

    let p = make_process();
    assert!(
        p.execute(InvoicCommand::ReceiveInvoic {
            pid: foreign,
            sender: MarktpartnerCode::new("9900357000004"),
            recipient: MarktpartnerCode::new("4012345000023"),
            invoice_ref: MessageRef::new("INV-1"),
            document_date: "20260601".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
            rechnung: None,
            bestellung_ref: None,
            rechnungstyp: None,
        })
        .await
        .is_err(),
        "31010 is the GaBi Gas Kapazitätsrechnung, not an AWH invoice"
    );

    let p = make_process();
    assert!(
        p.execute(InvoicCommand::SendInvoic {
            pid: foreign,
            sender: MarktpartnerCode::new("9900357000004"),
            recipient: MarktpartnerCode::new("4012345000023"),
            document_date: "20260601".to_owned(),
            invoice_ref: MessageRef::new("INV-1"),
        })
        .await
        .is_err()
    );
}
