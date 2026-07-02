//! GaBi Gas TRANOT workflow — transport notification (PID 90051).
//!
//! # Process overview
//!
//! The **TRANOT** message (*Transport Notification / Transportbenachrichtigung*)
//! notifies a market participant about transport conditions, restrictions, or
//! capacity changes affecting their balance group or delivery point. The
//! notification is informational — no formal response is required.
//!
//! ```text
//! FNB / VNB ──(TRANOT 90051)──→  BKV / GH / MGV
//! ```
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ TransportNotificationReceived   [terminal — no response required]
//! ```
//!
//! # Synthetic Prüfidentifikator
//!
//! | PID   | Message | Direction              | Content                         |
//! |-------|---------|------------------------|---------------------------------|
//! | 90051 | TRANOT  | FNB/VNB → BKV/GH/MGV  | Transport notification          |
//!
//! # Regulatory basis
//!
//! - **DVGW G 685 / G 2000** — gas transport notification obligations
//! - **Kooperationsvereinbarung Gas (KoV)** — capacity constraint reporting
//! - **BNetzA GaBi Gas 2.0 (BK7-14-020)** — regulatory framework

use mako_engine::{
    error::WorkflowError,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── Synthetic PID ─────────────────────────────────────────────────────────────

/// Synthetic PID for the TRANOT transport notification message.
pub const TRANOT_PID: u32 = 90051;

/// All PIDs handled by this workflow.
pub const TRANOT_PIDS: &[u32] = &[TRANOT_PID];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-tranot";

// ── Notification type ─────────────────────────────────────────────────────────

/// Type of transport notification.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportNotificationType {
    /// Capacity restriction or curtailment imposed on a delivery point.
    CapacityRestriction,
    /// Force majeure event affecting transport capability.
    ForceMajeure,
    /// Planned maintenance affecting transport.
    PlannedMaintenance,
    /// Other notification type (raw code preserved).
    Other(String),
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a TRANOT transport notification is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransportNotificationData {
    /// Synthetic PID (90051).
    pub synthetic_pid: u32,
    /// EIC code of the sending FNB/VNB.
    pub sender_eic: String,
    /// EIC code of the receiving party (BKV / GH / MGV).
    pub receiver_eic: String,
    /// Affected period start (from DTM qualifier 2).
    pub period_start: String,
    /// Affected period end (from DTM qualifier 3).
    pub period_end: Option<String>,
    /// Type of transport notification.
    pub notification_type: TransportNotificationType,
    /// Document reference from the TRANOT message (BGM element 1).
    pub document_ref: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas TRANOT workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum TransportNotificationEvent {
    /// TRANOT transport notification received and recorded.
    TransportNotificationReceived {
        /// Synthetic PID (90051).
        synthetic_pid: u32,
        /// EIC code of the sender.
        sender_eic: String,
        /// EIC code of the receiver.
        receiver_eic: String,
        /// Affected period start.
        period_start: String,
        /// Affected period end (if bounded).
        period_end: Option<String>,
        /// Type of notification.
        notification_type: TransportNotificationType,
        /// Document reference.
        document_ref: MessageRef,
    },
}

impl EventPayload for TransportNotificationEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::TransportNotificationReceived { .. } => "TransportNotificationReceived",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Process state for the GaBi Gas TRANOT workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TransportNotificationState {
    /// Initial state — no TRANOT received yet.
    New,
    /// TRANOT received and recorded (terminal).
    Received(TransportNotificationData),
}

impl TransportNotificationState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Received(_) => "Received",
        }
    }
}

impl Default for TransportNotificationState {
    fn default() -> Self {
        Self::New
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas TRANOT workflow.
#[derive(Clone)]
pub enum TransportNotificationCommand {
    /// Inbound TRANOT (PID 90051) received from FNB or VNB.
    ReceiveTransportNotification {
        /// Must be 90051.
        synthetic_pid: u32,
        /// EIC code of the sender (FNB / VNB).
        sender_eic: String,
        /// EIC code of the receiver (BKV / GH / MGV).
        receiver_eic: String,
        /// Affected period start (DTM qualifier 2).
        period_start: String,
        /// Affected period end (DTM qualifier 3, if bounded).
        period_end: Option<String>,
        /// Type of notification.
        notification_type: TransportNotificationType,
        /// Document reference from BGM element 1.
        document_ref: MessageRef,
    },
}

impl CommandPayload for TransportNotificationCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas TRANOT receive-and-record workflow (PID 90051).
///
/// Records inbound transport notifications from FNB/VNB. No outbound response
/// is required — the workflow transitions immediately to `Received` (terminal).
pub struct GaBiGasTransportNotificationWorkflow;

impl Workflow for GaBiGasTransportNotificationWorkflow {
    type State = TransportNotificationState;
    type Event = TransportNotificationEvent;
    type Command = TransportNotificationCommand;

    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            TransportNotificationEvent::TransportNotificationReceived {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                period_start,
                period_end,
                notification_type,
                document_ref,
            } => TransportNotificationState::Received(TransportNotificationData {
                synthetic_pid: *synthetic_pid,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                period_start: period_start.clone(),
                period_end: period_end.clone(),
                notification_type: notification_type.clone(),
                document_ref: document_ref.clone(),
            }),
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        let TransportNotificationCommand::ReceiveTransportNotification {
            synthetic_pid,
            sender_eic,
            receiver_eic,
            period_start,
            period_end,
            notification_type,
            document_ref,
        } = command;

        if !matches!(state, TransportNotificationState::New) {
            return Err(WorkflowError::invalid_state("New", state.label()));
        }
        if synthetic_pid != TRANOT_PID {
            return Err(WorkflowError::rejected(format!(
                "expected TRANOT PID {TRANOT_PID}, got {synthetic_pid}",
            )));
        }

        Ok(
            vec![TransportNotificationEvent::TransportNotificationReceived {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                period_start,
                period_end,
                notification_type,
                document_ref,
            }]
            .into(),
        )
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{types::MessageRef, workflow::Workflow};

    use super::*;

    fn make_ref() -> MessageRef {
        MessageRef::new("TRANOT-0001")
    }

    #[test]
    fn workflow_name_is_stable() {
        assert_eq!(WORKFLOW_NAME, "gabi-gas-tranot");
    }

    #[test]
    fn tranot_pid_is_90051() {
        assert_eq!(TRANOT_PID, 90051);
        assert!(TRANOT_PIDS.contains(&90051));
    }

    #[test]
    fn receive_transport_notification_transitions_new_to_received() {
        let state = TransportNotificationState::New;
        let cmd = TransportNotificationCommand::ReceiveTransportNotification {
            synthetic_pid: 90051,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            period_start: "2026-01-15T06:00:00+01:00".to_owned(),
            period_end: Some("2026-01-15T18:00:00+01:00".to_owned()),
            notification_type: TransportNotificationType::CapacityRestriction,
            document_ref: make_ref(),
        };
        let output = GaBiGasTransportNotificationWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasTransportNotificationWorkflow::apply);
        assert!(matches!(next, TransportNotificationState::Received(_)));
    }

    #[test]
    fn wrong_pid_returns_error() {
        let state = TransportNotificationState::New;
        let cmd = TransportNotificationCommand::ReceiveTransportNotification {
            synthetic_pid: 90041, // wrong PID
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            period_start: "2026-01-15T06:00:00+01:00".to_owned(),
            period_end: None,
            notification_type: TransportNotificationType::ForceMajeure,
            document_ref: make_ref(),
        };
        assert!(GaBiGasTransportNotificationWorkflow::handle(&state, cmd).is_err());
    }
}
