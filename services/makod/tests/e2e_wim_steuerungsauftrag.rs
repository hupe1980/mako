//! End-to-end test: WiM Steuerungsauftrag — grid remote control command
//! (BDEW API-Webdienste Strom `controlMeasuresV1`).
//!
//! Models the MSB (Messstellenbetreiber) side of the REST-based remote control
//! command workflow.  An NB or LF sends a `konfiguration` or `initialZustand`
//! command to the MSB; the MSB confirms or rejects within **5 Werktage**.
//!
//! Unlike AS4-based processes, no EDIFACT wire bytes are exchanged here —
//! the workflow is driven by REST JSON events mapped to domain commands.
//!
//! # Regulatory basis
//!
//! - **BDEW API-Guideline 1.0a** — API-Webdienste Strom, `controlMeasuresV1.yaml`
//! - **BK6-18-032** — WiM timeline: **5 Werktage** (Saturday counts as Werktag;
//!   Sunday and public holidays do not)
//!
//! # Lifecycle trace (Konfiguration — positive)
//!
//! ```text
//!   NB/LF REST caller                  MSB (this workflow)
//!   ─────────────────────────────────────────────────────
//!   POST /controlMeasuresV1
//!     konfiguration (TX-001, 7.5 kW)
//!                        ──────────► ReceiveKonfiguration
//!                                        state: Received
//!   POST /api/v1/commands
//!     wim.steuerungsauftrag.bestaetigen
//!                        ──────────► SendEndantwortPositiv
//!                                        state: Completed
//!   ─────────────────────────────────────────────────────
//! ```
//!
//! # Lifecycle trace (InitialZustand — negative)
//!
//! ```text
//!   NB/LF REST caller                  MSB (this workflow)
//!   ─────────────────────────────────────────────────────
//!   POST /controlMeasuresV1
//!     initialZustand (TX-002)
//!                        ──────────► ReceiveInitialZustand
//!                                        state: Received
//!   POST /api/v1/commands
//!     wim.steuerungsauftrag.ablehnen
//!                        ──────────► SendEndantwortNegativ
//!                                        state: Rejected
//!   ─────────────────────────────────────────────────────
//! ```

use mako_engine::{
    event_store::InMemoryEventStore,
    ids::{DeadlineId, TenantId},
    process::Process,
    types::MarktpartnerCode,
    version::WorkflowId,
};
use mako_wim::{
    STEUERUNGSAUFTRAG_DEADLINE_LABEL, SteuerungsCommandType, SteuerungsauftragCommand,
    SteuerungsauftragState, WimSteuerungsauftragWorkflow,
};

// ── Constants ─────────────────────────────────────────────────────────────────

const MSB_ID: &str = "9900357000004"; // Messstellenbetreiber (MSB) GLN
const NB_ID: &str = "4012345000023"; // Netzbetreiber (command sender) GLN
const LOCATION_ID: &str = "E0000000000000000001"; // NeLo / SR location ID
const FV: &str = "FV2025-10-01";

// ── Mock MSB backend ──────────────────────────────────────────────────────────

/// Simulates the **MSB's** REST control-command workflow.
struct MockMsb {
    process: Process<WimSteuerungsauftragWorkflow, InMemoryEventStore>,
}

impl MockMsb {
    fn new() -> Self {
        Self {
            process: Process::new(
                InMemoryEventStore::new(),
                TenantId::from_party_id(MSB_ID),
                WorkflowId::new("wim-steuerungsauftrag", FV),
            ),
        }
    }

    /// NB/LF sent a `konfiguration` (power-cap) command.
    async fn receive_konfiguration(
        &self,
        tx_id: &str,
        max_power_kw: &str,
        execution_time_until: Option<&str>,
    ) {
        self.process
            .execute(SteuerungsauftragCommand::ReceiveKonfiguration {
                tx_id: tx_id.to_owned(),
                sender_gln: MarktpartnerCode::new(NB_ID),
                location_id: LOCATION_ID.to_owned(),
                execution_time_from: "2025-01-15T06:00:00Z".to_owned(),
                max_power_kw: max_power_kw.to_owned(),
                execution_time_until: execution_time_until.map(str::to_owned),
            })
            .await
            .expect("ReceiveKonfiguration");
    }

    /// NB/LF sent an `initialZustand` (reset) command.
    async fn receive_initial_zustand(&self, tx_id: &str) {
        self.process
            .execute(SteuerungsauftragCommand::ReceiveInitialZustand {
                tx_id: tx_id.to_owned(),
                sender_gln: MarktpartnerCode::new(NB_ID),
                location_id: LOCATION_ID.to_owned(),
                execution_time_from: "2025-01-15T06:00:00Z".to_owned(),
            })
            .await
            .expect("ReceiveInitialZustand");
    }

    /// MSB sends the final positive response (command executed successfully).
    async fn send_endantwort_positiv(&self, reference_id: &str) {
        self.process
            .execute(SteuerungsauftragCommand::SendEndantwortPositiv {
                reference_id: reference_id.to_owned(),
            })
            .await
            .expect("SendEndantwortPositiv");
    }

    /// MSB sends the final negative response (command could not be executed).
    async fn send_endantwort_negativ(&self, reason: Option<&str>) {
        self.process
            .execute(SteuerungsauftragCommand::SendEndantwortNegativ {
                reason: reason.map(str::to_owned),
            })
            .await
            .expect("SendEndantwortNegativ");
    }

    async fn state(&self) -> SteuerungsauftragState {
        self.process.state().await.unwrap()
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// WiM Steuerungsauftrag — happy path: Konfiguration received and confirmed.
///
/// Lifecycle: New → Received → Completed.
/// Models bounded power cap (7.5 kW, with end time).
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_konfiguration_positive() {
    let msb = MockMsb::new();

    msb.receive_konfiguration("TX-STEU-001", "7.5", Some("2025-01-20T06:00:00Z"))
        .await;

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Received(d) => {
            assert_eq!(d.tx_id, "TX-STEU-001");
            assert_eq!(d.sender_gln.as_str(), NB_ID);
            assert_eq!(d.location_id, LOCATION_ID);
            assert_eq!(d.command_type, SteuerungsCommandType::Konfiguration);
            assert_eq!(d.max_power_kw.as_deref(), Some("7.5"));
            assert_eq!(
                d.execution_time_until.as_deref(),
                Some("2025-01-20T06:00:00Z")
            );
        }
        _ => panic!("expected Received; got: {state:?}"),
    }

    msb.send_endantwort_positiv("REF-EXEC-001").await;

    let state = msb.state().await;
    match state {
        SteuerungsauftragState::Completed(d) => {
            assert_eq!(d.tx_id, "TX-STEU-001");
            assert_eq!(d.command_type, SteuerungsCommandType::Konfiguration);
        }
        _ => panic!("expected Completed; got: {state:?}"),
    }
}

/// WiM Steuerungsauftrag — happy path: Konfiguration unbounded (no end time).
///
/// The `execution_time_until` is optional — permanent power caps omit it.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_konfiguration_unbounded() {
    let msb = MockMsb::new();

    // No execution_time_until — permanent power cap
    msb.receive_konfiguration("TX-STEU-PERM", "4.6", None).await;

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Received(d) => {
            assert_eq!(d.max_power_kw.as_deref(), Some("4.6"));
            assert!(
                d.execution_time_until.is_none(),
                "unbounded konfiguration must have no execution_time_until"
            );
        }
        _ => panic!("expected Received; got: {state:?}"),
    }

    msb.send_endantwort_positiv("REF-EXEC-PERM").await;
    assert!(matches!(
        msb.state().await,
        SteuerungsauftragState::Completed(_)
    ));
}

/// WiM Steuerungsauftrag — happy path: InitialZustand received and confirmed.
///
/// `initialZustand` resets a smart meter to unlimited operation.
/// Lifecycle: New → Received → Completed.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_initial_zustand_positive() {
    let msb = MockMsb::new();

    msb.receive_initial_zustand("TX-INIT-001").await;

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Received(d) => {
            assert_eq!(d.tx_id, "TX-INIT-001");
            assert_eq!(d.command_type, SteuerungsCommandType::InitialZustand);
            assert!(
                d.max_power_kw.is_none(),
                "InitialZustand must not carry max_power_kw"
            );
            assert!(
                d.execution_time_until.is_none(),
                "InitialZustand must not carry execution_time_until"
            );
        }
        _ => panic!("expected Received; got: {state:?}"),
    }

    msb.send_endantwort_positiv("REF-INIT-001").await;

    assert!(
        matches!(msb.state().await, SteuerungsauftragState::Completed(_)),
        "must be Completed after positive response"
    );
}

/// WiM Steuerungsauftrag — negative path: Konfiguration rejected by MSB.
///
/// The MSB determines the command cannot be executed (e.g., meter offline).
/// Lifecycle: New → Received → Rejected.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_konfiguration_negative() {
    let msb = MockMsb::new();

    msb.receive_konfiguration("TX-STEU-NEG", "5.0", None).await;

    msb.send_endantwort_negativ(Some("iMSB-Gerät DE000LALA1234 nicht erreichbar"))
        .await;

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Rejected { tx_id, reason } => {
            assert_eq!(tx_id.as_deref(), Some("TX-STEU-NEG"));
            assert!(
                reason.contains("DE000LALA1234"),
                "rejection reason must be preserved; got: {reason:?}"
            );
        }
        _ => panic!("expected Rejected; got: {state:?}"),
    }
}

/// WiM Steuerungsauftrag — negative path: InitialZustand rejected without reason.
///
/// The `reason` field is optional.  An omitted reason must still produce a
/// non-empty default rejection string.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_initial_zustand_negative_no_reason() {
    let msb = MockMsb::new();

    msb.receive_initial_zustand("TX-INIT-NEG").await;
    msb.send_endantwort_negativ(None).await;

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Rejected { tx_id, reason } => {
            assert_eq!(tx_id.as_deref(), Some("TX-INIT-NEG"));
            assert!(
                !reason.is_empty(),
                "rejection must have a non-empty default reason"
            );
        }
        _ => panic!("expected Rejected; got: {state:?}"),
    }
}

/// WiM Steuerungsauftrag — guard: second `ReceiveKonfiguration` on non-New state
/// is rejected.
///
/// The process must enforce the `New` precondition to prevent duplicate
/// command injection.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_duplicate_receive_rejected() {
    let msb = MockMsb::new();

    msb.receive_konfiguration("TX-DUP-001", "6.0", None).await;
    assert!(matches!(
        msb.state().await,
        SteuerungsauftragState::Received(_)
    ));

    let result = msb
        .process
        .execute(SteuerungsauftragCommand::ReceiveKonfiguration {
            tx_id: "TX-DUP-002".to_owned(),
            sender_gln: MarktpartnerCode::new(NB_ID),
            location_id: LOCATION_ID.to_owned(),
            execution_time_from: "2025-01-16T06:00:00Z".to_owned(),
            max_power_kw: "6.0".to_owned(),
            execution_time_until: None,
        })
        .await;

    assert!(
        result.is_err(),
        "second ReceiveKonfiguration on non-New state must return error"
    );
    assert!(
        matches!(msb.state().await, SteuerungsauftragState::Received(_)),
        "state must remain Received after rejected duplicate"
    );
}

/// WiM Steuerungsauftrag — guard: 5-Werktage deadline fires → Rejected.
///
/// If the MSB does not send a final response within the 5-Werktage window
/// (BK6-18-032), the deadline fires and the process transitions to `Rejected`.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_deadline_fires_rejected() {
    let msb = MockMsb::new();

    msb.receive_konfiguration("TX-TIMEOUT-001", "3.7", None)
        .await;

    let deadline_id = DeadlineId::new();
    msb.process
        .execute(SteuerungsauftragCommand::TimeoutExpired {
            deadline_id,
            label: STEUERUNGSAUFTRAG_DEADLINE_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired must be accepted from Received");

    let state = msb.state().await;
    match &state {
        SteuerungsauftragState::Rejected { tx_id, reason } => {
            assert_eq!(tx_id.as_deref(), Some("TX-TIMEOUT-001"));
            assert!(
                reason.contains(STEUERUNGSAUFTRAG_DEADLINE_LABEL),
                "rejection reason must name the deadline label; got: {reason:?}"
            );
        }
        _ => panic!("expected Rejected after deadline; got: {state:?}"),
    }
}

/// WiM Steuerungsauftrag — late deadline absorbed on Completed process.
///
/// A late-firing deadline on an already-Completed process must be a no-op.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_late_deadline_absorbed_on_completed() {
    let msb = MockMsb::new();

    msb.receive_konfiguration("TX-LATE-001", "5.0", None).await;
    msb.send_endantwort_positiv("REF-LATE-001").await;
    assert!(matches!(
        msb.state().await,
        SteuerungsauftragState::Completed(_)
    ));

    let deadline_id = DeadlineId::new();
    msb.process
        .execute(SteuerungsauftragCommand::TimeoutExpired {
            deadline_id,
            label: STEUERUNGSAUFTRAG_DEADLINE_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired on Completed must be absorbed without error");

    assert!(
        matches!(msb.state().await, SteuerungsauftragState::Completed(_)),
        "state must remain Completed after late deadline"
    );
}

/// WiM Steuerungsauftrag — late deadline absorbed on Rejected process.
///
/// Same absorption guarantee as the Completed case — terminal states are immune.
#[tokio::test]
async fn e2e_wim_steuerungsauftrag_late_deadline_absorbed_on_rejected() {
    let msb = MockMsb::new();

    msb.receive_initial_zustand("TX-LATE-REJ").await;
    msb.send_endantwort_negativ(Some("Gerät offline")).await;
    assert!(matches!(
        msb.state().await,
        SteuerungsauftragState::Rejected { .. }
    ));

    let deadline_id = DeadlineId::new();
    msb.process
        .execute(SteuerungsauftragCommand::TimeoutExpired {
            deadline_id,
            label: STEUERUNGSAUFTRAG_DEADLINE_LABEL.into(),
        })
        .await
        .expect("TimeoutExpired on Rejected must be absorbed without error");

    assert!(
        matches!(msb.state().await, SteuerungsauftragState::Rejected { .. }),
        "state must remain Rejected after late deadline"
    );
}
