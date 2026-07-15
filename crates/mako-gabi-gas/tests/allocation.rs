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
    event_store::InMemoryEventStore, ids::TenantId, process::Process, types::MessageRef,
    version::WorkflowId,
};
use mako_gabi_gas::allocation::AllocationVersion;
use mako_gabi_gas::{
    ALLOCATION_PIDS, AllocationCommand, AllocationState, AllocationType, GaBiGasAllocationWorkflow,
    GasDay,
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
        AllocationState::AllocationReceived(data) => {
            assert_eq!(data.synthetic_pid, 90001);
            assert_eq!(data.allocation_type, AllocationType::FnbDailyToBkv);
            assert_eq!(data.gas_day, GasDay::parse("2025-01-15").unwrap());
            assert_eq!(data.clearing_number.as_deref(), Some("CLR-2025-001"));
        }
        other => panic!("expected AllocationReceived, got {}", other.label()),
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
    assert!(
        matches!(state, AllocationState::AllocationReceived(ref d) if d.synthetic_pid == 90002)
    );
}

/// Happy path — VNB sub-daily allocation (PID 90003) received.
#[tokio::test]
async fn vnb_sub_daily_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(90003, "20250115"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    assert!(
        matches!(state, AllocationState::AllocationReceived(ref d) if d.synthetic_pid == 90003)
    );
}

/// Duplicate `ReceiveAlocat` on an already-received stream is rejected.
#[tokio::test]
async fn duplicate_receive_alocat_rejected() {
    let proc = make_process();
    proc.execute(receive_alocat(90001, "20250115"))
        .await
        .unwrap();

    let result = proc.execute(receive_alocat(90001, "20250115")).await;
    assert!(result.is_err(), "duplicate ReceiveAlocat must be rejected");
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
    assert!(matches!(s1, AllocationState::AllocationReceived(_)));
    assert!(matches!(s2, AllocationState::AllocationReceived(_)));
}
