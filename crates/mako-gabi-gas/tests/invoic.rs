//! Integration tests for mako-gabi-gas INVOIC billing (PID 31010 only).
//!
//! Verifies:
//! - PID routing: 31010 routes to `"gabi-gas-invoic"`.
//! - Happy path: INVOIC received → ValidationPassed → Settled.
//! - Validation failure leads to `Rejected`.
//! - Timeout fires `DeadlineExpired` and transitions to `Rejected`.
//! - The settlement deadline label matches the exported constant.
//!
//! Note: PID 31011 (Rechnung sonstige Leistung, AWH Sperrprozesse Gas) belongs
//! to `mako-geli-gas`, not `mako-gabi-gas`.

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gabi_gas::{
    GABI_GAS_INVOIC_PIDS, GaBiGasInvoicCommand, GaBiGasInvoicState, GaBiGasInvoicWorkflow,
    INVOIC_SETTLEMENT_WINDOW_LABEL,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn make_process() -> Process<GaBiGasInvoicWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gabi-gas-invoic", "FV2025-10-01"),
    )
}

fn receive_invoic(pid: u32, validation_passed: bool) -> GaBiGasInvoicCommand {
    GaBiGasInvoicCommand::ReceiveInvoic {
        pid: Pruefidentifikator::new(pid).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        recipient: MarktpartnerCode::new("9900357000004"),
        invoice_ref: MessageRef::new("INVOIC-GABI-001"),
        document_date: "20250115".to_owned(),
        validation_passed,
        validation_errors: if validation_passed {
            vec![]
        } else {
            vec!["Pflichtfeld BGM:1225 fehlt".to_owned()]
        },
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Settlement deadline label must be `"gabi-gas-invoic-settlement-deadline"`.
#[test]
fn settlement_label_matches_constant() {
    assert_eq!(
        INVOIC_SETTLEMENT_WINDOW_LABEL,
        "gabi-gas-invoic-settlement-deadline",
    );
}

/// PID 31010 (Kapazitätsrechnung) routes to `"gabi-gas-invoic"`.
#[test]
fn pid_31010_routes_to_gabi_gas_invoic() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_gabi_gas::GaBiGasModule;

    let mut router = PidRouter::new();
    GaBiGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    assert_eq!(
        router.route(31010),
        Some("gabi-gas-invoic"),
        "31010 must route to gabi-gas-invoic"
    );
}

/// All `GABI_GAS_INVOIC_PIDS` route to `"gabi-gas-invoic"`.
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

/// Happy path: INVOIC received (valid) → ValidationPassed → Settled.
#[tokio::test]
async fn happy_path_invoic_31010() {
    let process = make_process();

    // Step 1: Receive valid INVOIC 31010.
    process
        .execute(receive_invoic(31010, true))
        .await
        .expect("ReceiveInvoic should succeed");

    let state = process.state().await.expect("state after ReceiveInvoic");
    assert!(
        matches!(state, GaBiGasInvoicState::ValidationPassed(_)),
        "must be ValidationPassed after valid INVOIC, got: {state:?}"
    );

    // Step 2: Settle invoice.
    process
        .execute(GaBiGasInvoicCommand::SettleInvoice)
        .await
        .expect("SettleInvoice should succeed");

    let state = process.state().await.expect("state after SettleInvoice");
    assert!(
        matches!(state, GaBiGasInvoicState::Settled(_)),
        "must be Settled after SettleInvoice, got: {state:?}"
    );
}

/// Validation failure transitions directly to `Rejected`.
#[tokio::test]
async fn validation_failure_leads_to_rejected() {
    let process = make_process();
    process
        .execute(receive_invoic(31010, false))
        .await
        .expect("ReceiveInvoic failure must not panic");
    let state = process.state().await.expect("state");
    assert!(
        matches!(state, GaBiGasInvoicState::Rejected { .. }),
        "validation failure must transition to Rejected, got: {state:?}"
    );
}

/// Settlement deadline timeout leads to `Rejected`.
#[tokio::test]
async fn timeout_leads_to_rejected() {
    let process = make_process();
    process
        .execute(receive_invoic(31010, true))
        .await
        .expect("step 1 ok");

    process
        .execute(GaBiGasInvoicCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: INVOIC_SETTLEMENT_WINDOW_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired must not panic");

    let state = process.state().await.expect("state");
    assert!(
        matches!(state, GaBiGasInvoicState::Rejected { .. }),
        "deadline timeout must transition to Rejected, got: {state:?}"
    );
}

/// TimeoutExpired on already-Rejected is absorbed without error.
#[tokio::test]
async fn timeout_on_rejected_is_absorbed() {
    let process = make_process();
    process.execute(receive_invoic(31010, false)).await.unwrap(); // → Rejected

    let result = process
        .execute(GaBiGasInvoicCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: INVOIC_SETTLEMENT_WINDOW_LABEL.into(),
        })
        .await;
    assert!(
        result.is_ok(),
        "TimeoutExpired on Rejected must be absorbed"
    );
}
