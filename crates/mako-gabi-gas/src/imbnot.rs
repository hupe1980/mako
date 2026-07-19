//! GaBi Gas IMBNOT workflow — imbalance notification (PID 90041).
//!
//! # Process overview
//!
//! The **IMBNOT** message (*Imbalance Notification*) is sent by the FNB or MGV
//! to the BKV to notify them of an imbalance in their balance group for a given
//! gas day. The notification is informational — no formal response is required.
//! The BKV must reconcile the imbalance through the balance group adjustment
//! process.
//!
//! ```text
//! FNB / MGV ──(IMBNOT 90041)──→  BKV
//! ```
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ ImbalanceReceived   [terminal — no response required]
//! ```
//!
//! # Synthetic Prüfidentifikator
//!
//! | PID   | Message | Direction         | Content                    |
//! |-------|---------|-------------------|----------------------------|
//! | 90041 | IMBNOT  | FNB/MGV → BKV     | Imbalance notification     |
//!
//! # Regulatory basis
//!
//! - **DVGW G 685 / G 2000** — gas balance group rules
//! - **Kooperationsvereinbarung Gas (KoV)** — imbalance reporting obligations
//! - **BNetzA GaBi Gas 2.1 (BK7-24-01-008)** — regulatory framework for gas balancing

use mako_engine::{
    error::WorkflowError,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use rust_decimal::Decimal;

use crate::domain::GasDay;

// ── Synthetic PID ─────────────────────────────────────────────────────────────

/// Synthetic PID for the IMBNOT imbalance notification message.
pub const IMBNOT_PID: u32 = 90041;

/// All PIDs handled by this workflow.
pub const IMBNOT_PIDS: &[u32] = &[IMBNOT_PID];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-imbnot";

// ── Imbalance direction ───────────────────────────────────────────────────────

/// Direction of the imbalance (long or short position).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImbalanceDirection {
    /// BKV consumed more gas than nominated (short position).
    Short,
    /// BKV consumed less gas than nominated (long position).
    Long,
    /// Direction not determinable from available data.
    Unknown,
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when an IMBNOT imbalance notification is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImbalanceData {
    /// Synthetic PID (90041).
    pub synthetic_pid: u32,
    /// EIC code of the sender (FNB / MGV).
    pub sender_eic: String,
    /// EIC code of the receiving BKV.
    pub receiver_eic: String,
    /// Gas day for which the imbalance is reported.
    pub gas_day: GasDay,
    /// Direction of the imbalance.
    pub direction: ImbalanceDirection,
    /// Imbalance quantity in kWh_Hs (from QTY segment).
    ///
    /// Stored as `Decimal` per DVGW G 685 — sub-kWh precision required.
    pub quantity_kwh: Option<Decimal>,
    /// Document reference from the IMBNOT message (BGM element 1).
    pub document_ref: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas IMBNOT workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ImbalanceEvent {
    /// IMBNOT imbalance notification received and recorded.
    ImbalanceReceived {
        /// Synthetic PID (90041).
        synthetic_pid: u32,
        /// EIC code of the sender (FNB / MGV).
        sender_eic: String,
        /// EIC code of the receiving BKV.
        receiver_eic: String,
        /// Gas day.
        gas_day: GasDay,
        /// Direction of the imbalance.
        direction: ImbalanceDirection,
        /// Imbalance quantity in kWh_Hs (Decimal — DVGW G 685 sub-kWh precision).
        quantity_kwh: Option<Decimal>,
        /// Document reference.
        document_ref: MessageRef,
    },
}

impl EventPayload for ImbalanceEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::ImbalanceReceived { .. } => "ImbalanceReceived",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Process state for the GaBi Gas IMBNOT workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum ImbalanceState {
    /// Initial state — no IMBNOT received yet.
    #[default]
    New,
    /// IMBNOT received and recorded (terminal).
    Received(ImbalanceData),
}

impl ImbalanceState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Received(_) => "Received",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas IMBNOT workflow.
#[derive(Clone)]
pub enum ImbalanceCommand {
    /// Inbound IMBNOT (PID 90041) received from FNB or MGV.
    ReceiveImbalanceNotification {
        /// Must be 90041.
        synthetic_pid: u32,
        /// EIC code of the sender (FNB / MGV).
        sender_eic: String,
        /// EIC code of the receiving BKV.
        receiver_eic: String,
        /// Gas day (from DTM 137).
        gas_day: GasDay,
        /// Direction of the imbalance.
        direction: ImbalanceDirection,
        /// Imbalance quantity in kWh_Hs (Decimal — DVGW G 685).
        quantity_kwh: Option<Decimal>,
        /// Document reference from BGM element 1.
        document_ref: MessageRef,
    },
}

impl CommandPayload for ImbalanceCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas IMBNOT receive-and-record workflow (PID 90041).
///
/// Records inbound imbalance notifications from FNB/MGV. No outbound response
/// is required — the workflow transitions immediately to `Received` (terminal).
pub struct GaBiGasImbalanceWorkflow;

impl Workflow for GaBiGasImbalanceWorkflow {
    type State = ImbalanceState;
    type Event = ImbalanceEvent;
    type Command = ImbalanceCommand;

    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ImbalanceEvent::ImbalanceReceived {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                direction,
                quantity_kwh,
                document_ref,
            } => ImbalanceState::Received(ImbalanceData {
                synthetic_pid: *synthetic_pid,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                direction: *direction,
                quantity_kwh: *quantity_kwh,
                document_ref: document_ref.clone(),
            }),
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        let ImbalanceCommand::ReceiveImbalanceNotification {
            synthetic_pid,
            sender_eic,
            receiver_eic,
            gas_day,
            direction,
            quantity_kwh,
            document_ref,
        } = command;

        if !matches!(state, ImbalanceState::New) {
            return Err(WorkflowError::invalid_state("New", state.label()));
        }
        if synthetic_pid != IMBNOT_PID {
            return Err(WorkflowError::rejected(format!(
                "expected IMBNOT PID {IMBNOT_PID}, got {synthetic_pid}",
            )));
        }

        Ok(vec![ImbalanceEvent::ImbalanceReceived {
            synthetic_pid,
            sender_eic,
            receiver_eic,
            gas_day,
            direction,
            quantity_kwh,
            document_ref,
        }]
        .into())
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{types::MessageRef, workflow::Workflow};

    use super::*;

    fn make_ref() -> MessageRef {
        MessageRef::new("IMBNOT-0001")
    }

    #[test]
    fn workflow_name_is_stable() {
        assert_eq!(WORKFLOW_NAME, "gabi-gas-imbnot");
    }

    #[test]
    fn imbnot_pid_is_90041() {
        assert_eq!(IMBNOT_PID, 90041);
        assert!(IMBNOT_PIDS.contains(&90041));
    }

    #[test]
    fn receive_imbalance_transitions_new_to_received() {
        let state = ImbalanceState::New;
        let cmd = ImbalanceCommand::ReceiveImbalanceNotification {
            synthetic_pid: 90041,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: GasDay::parse("2026-01-15").unwrap(),
            direction: ImbalanceDirection::Short,
            quantity_kwh: Some(rust_decimal_macros::dec!(-15000)),
            document_ref: make_ref(),
        };
        let output = GaBiGasImbalanceWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasImbalanceWorkflow::apply);
        assert!(matches!(next, ImbalanceState::Received(_)));
    }

    #[test]
    fn wrong_pid_returns_error() {
        let state = ImbalanceState::New;
        let cmd = ImbalanceCommand::ReceiveImbalanceNotification {
            synthetic_pid: 90031, // wrong PID
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: GasDay::parse("2026-01-15").unwrap(),
            direction: ImbalanceDirection::Long,
            quantity_kwh: None,
            document_ref: make_ref(),
        };
        assert!(GaBiGasImbalanceWorkflow::handle(&state, cmd).is_err());
    }
}
