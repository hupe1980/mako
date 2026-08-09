//! Integration tests for mako-gabi-gas Allocation workflow (ALOCAT).
//!
//! Verifies:
//! - All ALLOCATION_PIDS (90001, 90002, 90003) route to `"gabi-gas-allocation"`.
//! - AllocationType derived correctly from each PID.
//! - Happy path for each of the three allocation types.
//! - Duplicate `ReceiveAlocat` on a non-New state is rejected.
//! - Invalid PID is rejected.
//! - Independent gas days result in separate process streams (state boundary).

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::MessageRef,
    version::WorkflowId,
};
use mako_gabi_gas::allocation::AllocationVersion;
use mako_gabi_gas::{
    ALLOCATION_PIDS, AllocationCommand, AllocationState, AllocationType,
    FINAL_ALOCAT_DEADLINE_LABEL, GaBiGasAllocationWorkflow, GasDay,
};

// ── Helpers ───────────────────────────────────────────────────────────────────────────────────

fn make_process() -> Process<GaBiGasAllocationWorkflow, InMemoryEventStore> {
    Process::new(
        InMemoryEventStore::new(),
        TenantId::new(),
        WorkflowId::new("gabi-gas-allocation", "FV2025-10-01"),
    )
}

fn receive_alocat(synthetic_pid: u32, gas_day: &str) -> AllocationCommand {
    AllocationCommand::ReceiveAlocat {
        synthetic_pid,
        sender_eic: "11XFNB-SENDTEST1".to_owned(),
        receiver_eic: "11XBKV-RECVTEST2".to_owned(),
        gas_day: GasDay::parse(gas_day).expect("valid gas day"),
        version: AllocationVersion::Initial,
        allocated_quantity: None,
        clearing_number: Some("CLR-2025-001".to_owned()),
        message_ref: MessageRef::new("ALOCAT-2025-001"),
    }
}

/// Same as [`receive_alocat`] but with an explicit KoV §6.4 version.
fn receive_alocat_versioned(
    synthetic_pid: u32,
    gas_day: &str,
    version: AllocationVersion,
) -> AllocationCommand {
    AllocationCommand::ReceiveAlocat {
        synthetic_pid,
        sender_eic: "11XFNB-SENDTEST1".to_owned(),
        receiver_eic: "11XBKV-RECVTEST2".to_owned(),
        gas_day: GasDay::parse(gas_day).expect("valid gas day"),
        version,
        allocated_quantity: None,
        clearing_number: Some("CLR-2025-001".to_owned()),
        message_ref: MessageRef::new("ALOCAT-2025-001"),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────────────────────

/// All ALLOCATION_PIDS (90001, 90002, 90003) route to `"gabi-gas-allocation"`.
#[test]
fn all_allocation_pids_route_correctly() {
    use mako_engine::{builder::EngineModule, marktrolle::DeploymentRoles, pid_router::PidRouter};
    use mako_gabi_gas::GaBiGasModule;

    let mut router = PidRouter::new();
    GaBiGasModule.register_pids_with_roles(&mut router, &DeploymentRoles::all());
    for &pid in ALLOCATION_PIDS {
        assert_eq!(
            router.route(pid),
            Some("gabi-gas-allocation"),
            "PID {pid} must route to gabi-gas-allocation"
        );
    }
}

/// PID 90001 derives AllocationType::FnbDailyToBkv.
#[test]
fn allocation_type_from_pid_90001() {
    assert_eq!(
        AllocationType::from_pid(90001),
        Some(AllocationType::FnbDailyToBkv)
    );
}

/// PID 90002 derives AllocationType::MgvMonthlyToBkv.
#[test]
fn allocation_type_from_pid_90002() {
    assert_eq!(
        AllocationType::from_pid(90002),
        Some(AllocationType::MgvMonthlyToBkv)
    );
}

/// PID 90003 derives AllocationType::VnbSubDailyToFnb.
#[test]
fn allocation_type_from_pid_90003() {
    assert_eq!(
        AllocationType::from_pid(90003),
        Some(AllocationType::VnbSubDailyToFnb)
    );
}

/// Unknown PID returns None from `from_pid`.
#[test]
fn allocation_type_from_pid_unknown_returns_none() {
    assert_eq!(AllocationType::from_pid(12345), None);
}

/// Happy path — FNB daily allocation (PID 90001) received.
///
/// ```text
/// New → AllocationReceived
/// ```
#[tokio::test]
async fn fnb_daily_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    match state {
        AllocationState::Recorded(data) => {
            assert_eq!(data.synthetic_pid, 90001);
            assert_eq!(data.allocation_type, AllocationType::FnbDailyToBkv);
            assert_eq!(data.gas_day, GasDay::parse("2025-01-15").unwrap());
            assert_eq!(data.clearing_number.as_deref(), Some("CLR-2025-001"));
        }
        other => panic!("expected Recorded, got {}", other.label()),
    }
}

/// Happy path — MGV monthly allocation (PID 90002) received.
#[tokio::test]
async fn mgv_monthly_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(90002, "20250201"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    assert!(matches!(state, AllocationState::Recorded(ref d) if d.synthetic_pid == 90002));
}

/// Happy path — VNB sub-daily allocation (PID 90003) received.
#[tokio::test]
async fn vnb_sub_daily_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(90003, "20250115"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    assert!(matches!(state, AllocationState::Recorded(ref d) if d.synthetic_pid == 90003));
}

/// A second ALOCAT for the same gas day is a **correction**, not a duplicate:
/// KoV §6.4 admits corrections until the binding final allocation.
#[tokio::test]
async fn a_correction_supersedes_the_initial_allocation() {
    let proc = make_process();
    proc.execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();

    proc.execute(receive_alocat_versioned(
        90001,
        "20250115",
        AllocationVersion::Correction(1),
    ))
    .await
    .expect("KoV §6.4 admits corrections after the initial allocation");

    let state = proc.state().await.unwrap();
    assert_eq!(
        state.latest().map(|d| d.version),
        Some(AllocationVersion::Correction(1)),
        "the correction must supersede the initial allocation"
    );
    assert!(!state.is_settled(), "a correction is not the binding final");
}

/// The binding final allocation settles the gas day; nothing may follow it.
#[tokio::test]
async fn no_correction_is_admissible_after_the_final_allocation() {
    let proc = make_process();
    proc.execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();
    proc.execute(receive_alocat_versioned(
        90001,
        "20250115",
        AllocationVersion::Final,
    ))
    .await
    .unwrap();

    assert!(proc.state().await.unwrap().is_settled());

    let result = proc
        .execute(receive_alocat_versioned(
            90001,
            "20250115",
            AllocationVersion::Correction(2),
        ))
        .await;
    assert!(
        result.is_err(),
        "KoV §6.4 admits no correction after the binding final allocation"
    );
}

/// The KoV §6.4 M+2 window closing with no final allocation is recorded, so the
/// unsettled imbalance is visible in the event log rather than merely absent.
#[tokio::test]
async fn final_allocation_window_closing_without_a_final_is_recorded() {
    let proc = make_process();
    proc.execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();

    proc.execute(AllocationCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: FINAL_ALOCAT_DEADLINE_LABEL.into(),
    })
    .await
    .unwrap();

    let state = proc.state().await.unwrap();
    assert!(matches!(state, AllocationState::FinalOverdue(_)));
}

/// A settled gas day must not raise an overdue event when the window closes.
#[tokio::test]
async fn a_settled_gas_day_does_not_go_overdue() {
    let proc = make_process();
    proc.execute(receive_alocat_versioned(
        90001,
        "20250115",
        AllocationVersion::Final,
    ))
    .await
    .unwrap();

    proc.execute(AllocationCommand::TimeoutExpired {
        deadline_id: DeadlineId::new(),
        label: FINAL_ALOCAT_DEADLINE_LABEL.into(),
    })
    .await
    .unwrap();

    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, AllocationState::Recorded(_)),
        "a settled stream must stay Recorded, not go FinalOverdue"
    );
}

/// Invalid PID on `ReceiveAlocat` is rejected.
#[tokio::test]
async fn receive_alocat_with_invalid_pid_rejected() {
    let proc = make_process();
    let result = proc.execute(receive_alocat(99999, "20250115")).await;
    assert!(result.is_err(), "invalid PID must be rejected");
}

/// Two independent gas days can each be received in separate process streams.
///
/// Each gas day creates its own process stream in the event store; state is
/// independent.
#[tokio::test]
async fn independent_gas_days_are_independent_streams() {
    let proc1 = make_process();
    let proc2 = make_process();

    proc1
        .execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();
    proc2
        .execute(receive_alocat(90001, "20250116"))
        .await
        .unwrap();

    let s1 = proc1.state().await.unwrap();
    let s2 = proc2.state().await.unwrap();
    assert!(matches!(s1, AllocationState::Recorded(_)));
    assert!(matches!(s2, AllocationState::Recorded(_)));
}
