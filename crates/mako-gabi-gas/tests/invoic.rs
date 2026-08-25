//! What the GaBi Gas billing family chooses.
//!
//! The state machine itself is [`mako_invoic`]'s and is tested there, once, in
//! `mako-invoic/tests/state_machine.rs`. These tests cover the family: its PID
//! set, its routing, and the roles a GaBi Gas deployment actually plays.
//!
//! Regulatory basis: BK7-24-01-008 (GaBi Gas 2.1).

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::TenantId,
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gabi_gas::{
    GABI_GAS_INVOIC_PIDS, GaBiGasInvoicWorkflow, INVOIC_SETTLEMENT_WINDOW_LABEL,
    INVOIC_WORKFLOW_NAME,
};
use mako_invoic::{InvoicCommand, InvoicFamily};

fn make_process() -> Process<GaBiGasInvoicWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new(INVOIC_WORKFLOW_NAME, "FV2025-10-01"),
    )
}

#[test]
fn settlement_label_matches_constant() {
    assert_eq!(
        INVOIC_SETTLEMENT_WINDOW_LABEL,
        "gabi-gas-invoic-settlement-deadline",
    );
}

/// Every GaBi Gas billing PID routes to the family's workflow.
#[test]
fn all_invoic_pids_route_to_gabi_gas_invoic() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_gabi_gas::GaBiGasModule;

    let mut router = PidRouter::new();
    GaBiGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    for &pid in GABI_GAS_INVOIC_PIDS {
        assert_eq!(
            router.route(pid),
            Some("gabi-gas-invoic"),
            "PID {pid} must route to gabi-gas-invoic"
        );
    }
}

/// **GaBi Gas receives invoices; it does not issue them.**
///
/// All three PIDs arrive *at* the roles this platform plays — the BKV receives
/// the Kapazitätsrechnung, the MGV the aggregated MMM-Rechnung — and nothing
/// here renders one, so the issuer leg must stay shut.
///
/// Accepting an inbound REMADV would invert the direction of the conversation:
/// after *receiving* an invoice this platform is the one that sends it.
#[test]
fn gabi_gas_is_a_payer_only_family() {
    // The capabilities are `const`, so this is a compile-time statement about
    // the family rather than a runtime check.
    const {
        assert!(
            !mako_gabi_gas::GaBiGasInvoic::SENDS_INVOIC,
            "nothing in this platform issues a GaBi Gas invoice"
        );
        assert!(
            mako_gabi_gas::GaBiGasInvoic::ANSWERS_COMDIS,
            "COMDIS 29001 is inbound for a payer — the invoicer refusing our REMADV"
        );
    }
}

#[tokio::test]
async fn the_issuer_leg_is_refused() {
    let p = make_process();
    let err = p
        .execute(InvoicCommand::SendInvoic {
            pid: Pruefidentifikator::new(31010).unwrap(),
            sender: MarktpartnerCode::new("9900357000004"),
            recipient: MarktpartnerCode::new("4012345000023"),
            document_date: "20260601".to_owned(),
            invoice_ref: MessageRef::new("INV-1"),
        })
        .await
        .expect_err("GaBi Gas does not issue invoices");
    assert!(format!("{err}").contains("issuer role"), "{err}");
}

#[tokio::test]
async fn an_inbound_remadv_is_refused() {
    let p = make_process();
    let err = p
        .execute(InvoicCommand::ReceiveRemadv {
            pid: Pruefidentifikator::new(33002).unwrap(),
            remadv_ref: MessageRef::new("REM-1"),
            sender: MarktpartnerCode::new("4012345000023"),
        })
        .await
        .expect_err("after receiving an invoice this platform sends the REMADV");
    assert!(format!("{err}").contains("never issues"), "{err}");
}

/// No REMADV PID is routed to this family — the inbound direction does not
/// exist for the roles modelled here.
#[test]
fn no_remadv_pid_routes_to_gabi_gas_invoic() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_gabi_gas::GaBiGasModule;

    let mut router = PidRouter::new();
    GaBiGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    for pid in mako_invoic::REMADV_PIDS {
        assert_ne!(
            router.route(*pid),
            Some("gabi-gas-invoic"),
            "REMADV {pid} must not route to a family that issues no invoices"
        );
    }
}
