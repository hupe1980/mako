//! Integration tests for mako-gabi-gas Allocation workflow (ALOCAT).
//!
//! Verifies:
//! - All ALLOCATION_PIDS (70001–70023) route to `"gabi-gas-allocation"`.
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

fn receive_alocat(pruefidentifikator: u32, gas_day: &str) -> AllocationCommand {
    AllocationCommand::ReceiveAlocat {
        pruefidentifikator,
        sender_eic: "11XFNB-SENDTESTE".to_owned(),
        receiver_eic: "11XBKV-RECVTEST8".to_owned(),
        gas_day: GasDay::parse(gas_day).expect("valid gas day"),
        version: AllocationVersion::Initial,
        allocated_quantity: None,
        clearing_number: Some("CLR-2025-001".to_owned()),
        message_ref: MessageRef::new("ALOCAT-2025-001"),
    }
}

/// Same as [`receive_alocat`] but with an explicit §47 Ziffer 1 KoV XV version.
fn receive_alocat_versioned(
    pruefidentifikator: u32,
    gas_day: &str,
    version: AllocationVersion,
) -> AllocationCommand {
    AllocationCommand::ReceiveAlocat {
        pruefidentifikator,
        sender_eic: "11XFNB-SENDTESTE".to_owned(),
        receiver_eic: "11XBKV-RECVTEST8".to_owned(),
        gas_day: GasDay::parse(gas_day).expect("valid gas day"),
        version,
        allocated_quantity: None,
        clearing_number: Some("CLR-2025-001".to_owned()),
        message_ref: MessageRef::new("ALOCAT-2025-001"),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────────────────────

/// All ALLOCATION_PIDS (70001–70023) route to `"gabi-gas-allocation"`.
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

/// PID 70001 derives AllocationType::NbAnMgv.
#[test]
fn allocation_type_from_pid_70001() {
    assert_eq!(
        AllocationType::from_pid(70001),
        Some(AllocationType::NbAnMgv)
    );
}

/// PID 70013 derives AllocationType::MgvAnBkv.
#[test]
fn allocation_type_from_pid_70013() {
    assert_eq!(
        AllocationType::from_pid(70013),
        Some(AllocationType::MgvAnBkv)
    );
}

/// PID 70011 derives AllocationType::EnbAnbAnNb.
#[test]
fn allocation_type_from_pid_70011() {
    assert_eq!(
        AllocationType::from_pid(70011),
        Some(AllocationType::EnbAnbAnNb)
    );
}

/// Unknown PID returns None from `from_pid`.
#[test]
fn allocation_type_from_pid_unknown_returns_none() {
    assert_eq!(AllocationType::from_pid(12345), None);
}

/// Happy path — SLP allocation, NB an MGV (PID 70001).
///
/// ```text
/// New → AllocationReceived
/// ```
#[tokio::test]
async fn nb_an_mgv_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    match state {
        AllocationState::Recorded(data) => {
            assert_eq!(data.pruefidentifikator, 70001);
            assert_eq!(data.allocation_type, AllocationType::NbAnMgv);
            assert_eq!(data.gas_day, GasDay::parse("2025-01-15").unwrap());
            assert_eq!(data.clearing_number.as_deref(), Some("CLR-2025-001"));
        }
        other => panic!("expected Recorded, got {}", other.label()),
    }
}

/// Happy path — SLP allocation, MGV an BKV (PID 70013).
#[tokio::test]
async fn mgv_an_bkv_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(70013, "20250201"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    assert!(matches!(state, AllocationState::Recorded(ref d) if d.pruefidentifikator == 70013));
}

/// Happy path — korrigierte Mengenmeldung NKP, ENB/ANB an NB (PID 70011).
#[tokio::test]
async fn enb_an_nb_alocat_received() {
    let proc = make_process();

    proc.execute(receive_alocat(70011, "20250115"))
        .await
        .unwrap();

    let state = proc.state().await.unwrap();
    assert!(matches!(state, AllocationState::Recorded(ref d) if d.pruefidentifikator == 70011));
}

/// A second ALOCAT for the same gas day is a **correction**, not a duplicate:
/// §47 Ziffer 1 KoV XV admits corrections until the binding final allocation.
#[tokio::test]
async fn a_correction_supersedes_the_initial_allocation() {
    let proc = make_process();
    proc.execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();

    proc.execute(receive_alocat_versioned(
        70001,
        "20250115",
        AllocationVersion::Correction(1),
    ))
    .await
    .expect("§47 Ziffer 1 KoV XV admits corrections after the initial allocation");

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
    proc.execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();
    proc.execute(receive_alocat_versioned(
        70001,
        "20250115",
        AllocationVersion::Final,
    ))
    .await
    .unwrap();

    assert!(proc.state().await.unwrap().is_settled());

    let result = proc
        .execute(receive_alocat_versioned(
            70001,
            "20250115",
            AllocationVersion::Correction(2),
        ))
        .await;
    assert!(
        result.is_err(),
        "§47 Ziffer 1 KoV XV admits no correction after the binding final allocation"
    );
}

/// The §47 KoV XV final-allocation window closing with no final allocation is recorded, so the
/// unsettled imbalance is visible in the event log rather than merely absent.
#[tokio::test]
async fn final_allocation_window_closing_without_a_final_is_recorded() {
    let proc = make_process();
    proc.execute(receive_alocat(70001, "20250115"))
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

/// The missed §47 Ziffer 1 KoV XV obligation must leave the platform, not just the state.
///
/// A `FinalOverdue` stream that raises no notification is indistinguishable
/// from a healthy one to everything outside makod: the imbalance cannot be
/// settled and only a Clearingfall with the FNB/MGV resolves it, so the fact
/// has to reach the operator. The outbox entry is what `OutboxErpWorker` turns
/// into `de.gabi.alocat.missing`.
#[tokio::test]
async fn a_closed_window_enqueues_the_alocat_missing_notification() {
    use mako_engine::workflow::Workflow as _;

    // Drive the state through the real command path, then hand it to `handle`
    // directly — `Process::execute` persists the outbox rather than returning it.
    let proc = make_process();
    proc.execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();
    let state = proc.state().await.unwrap();

    let out = GaBiGasAllocationWorkflow::handle(
        &state,
        AllocationCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: FINAL_ALOCAT_DEADLINE_LABEL.into(),
        },
    )
    .expect("the window closing is not an error");

    assert_eq!(out.events.len(), 1, "one FinalAllocationOverdue event");
    assert_eq!(
        out.outbox.len(),
        1,
        "the missed obligation must be notified, not only recorded"
    );

    let notice = &out.outbox[0];
    // This string is the contract with `map_message_type_to_erp_event` in
    // makod; changing it silently drops the notification on the floor.
    assert_eq!(notice.message_type.as_ref(), "GabiFinalAllocationOverdue");
    assert_eq!(notice.recipient.as_ref(), "11XBKV-RECVTEST8");
    assert_eq!(notice.payload["sender_eic"], "11XFNB-SENDTESTE");
    assert_eq!(
        notice.payload["deadline_label"],
        FINAL_ALOCAT_DEADLINE_LABEL
    );
    assert!(
        notice.payload.get("gas_day").is_some(),
        "the Clearingfall is opened for a specific gas day"
    );
}

/// A settled gas day raises neither the event nor the notification.
#[tokio::test]
async fn a_settled_gas_day_enqueues_no_notification() {
    use mako_engine::workflow::Workflow as _;

    let proc = make_process();
    proc.execute(receive_alocat_versioned(
        70001,
        "20250115",
        AllocationVersion::Final,
    ))
    .await
    .unwrap();
    let state = proc.state().await.unwrap();

    let out = GaBiGasAllocationWorkflow::handle(
        &state,
        AllocationCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: FINAL_ALOCAT_DEADLINE_LABEL.into(),
        },
    )
    .expect("idempotent no-op");

    assert!(out.events.is_empty());
    assert!(
        out.outbox.is_empty(),
        "a settled gas day must not raise a Clearingfall notification"
    );
}

/// `on_deadline` is what tells makod whether the obligation was really missed.
///
/// The §47 Ziffer 1 KoV XV window is registered when the *first* ALOCAT arrives and is
/// never cancelled, so the deadline fires for **every** gas day — including the
/// ones that settled normally. `makod`'s `dispatch_deadline` routes through
/// `execute_timeout_with_retry` and raises its error-level REGULATORY ALERT
/// only when this hook produced a command. If the hook stopped discriminating,
/// every healthy gas day would page the operator at M+2.
#[tokio::test]
async fn on_deadline_is_silent_for_a_gas_day_that_settled() {
    use mako_engine::{deadline::Deadline, ids::ProcessId, workflow::Workflow as _};

    fn window(label: &str) -> Deadline {
        Deadline::new(
            mako_engine::ids::StreamId::new("gabi-gas-allocation-20250115"),
            ProcessId::new(),
            TenantId::new(),
            WorkflowId::new("gabi-gas-allocation", "FV2025-10-01"),
            label,
            time::OffsetDateTime::now_utc(),
        )
    }

    // Settled — the final allocation is on file, nothing is overdue.
    let settled = make_process();
    settled
        .execute(receive_alocat_versioned(
            70001,
            "20250115",
            AllocationVersion::Final,
        ))
        .await
        .unwrap();
    assert!(
        GaBiGasAllocationWorkflow::on_deadline(
            &window(FINAL_ALOCAT_DEADLINE_LABEL),
            &settled.state().await.unwrap(),
        )
        .is_none(),
        "a settled gas day must not raise the §47 Ziffer 1 KoV XV alert",
    );

    // Unsettled — only the initial allocation arrived.
    let unsettled = make_process();
    unsettled
        .execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();
    assert!(
        GaBiGasAllocationWorkflow::on_deadline(
            &window(FINAL_ALOCAT_DEADLINE_LABEL),
            &unsettled.state().await.unwrap(),
        )
        .is_some(),
        "a gas day with no binding final allocation must raise the alert",
    );
}

/// A settled gas day must not raise an overdue event when the window closes.
#[tokio::test]
async fn a_settled_gas_day_does_not_go_overdue() {
    let proc = make_process();
    proc.execute(receive_alocat_versioned(
        70001,
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
        .execute(receive_alocat(70001, "20250115"))
        .await
        .unwrap();
    proc2
        .execute(receive_alocat(70001, "20250116"))
        .await
        .unwrap();

    let s1 = proc1.state().await.unwrap();
    let s2 = proc2.state().await.unwrap();
    assert!(matches!(s1, AllocationState::Recorded(_)));
    assert!(matches!(s2, AllocationState::Recorded(_)));
}
