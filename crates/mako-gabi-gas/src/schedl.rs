//! GaBi Gas SCHEDL workflow — day-ahead transport schedule (PID 90031).
//!
//! # Process overview
//!
//! The **SCHEDL** message (*Schedulingnachricht*) is used in DVGW gas transport
//! to exchange day-ahead or intraday transport schedules between market participants
//! (BKV, FNB, MGV). A SCHEDL notification is informational: the receiver records
//! it and no formal response message is required.
//!
//! ```text
//! Sender (BKV / FNB / MGV) ──(SCHEDL 90031)──→  Receiver
//! ```
//!
//! # State machine
//!
//! ```text
//! New
//!  └─ ScheduleReceived   [terminal — no response required]
//! ```
//!
//! # Synthetic Prüfidentifikator
//!
//! | PID   | Message | Direction          | Content                       |
//! |-------|---------|--------------------|-------------------------------|
//! | 90031 | SCHEDL  | sender → receiver  | Transport schedule / Fahrplan |
//!
//! # Regulatory basis
//!
//! - **DVGW G 685 / G 2000** — technical rules for gas transport scheduling
//! - **Kooperationsvereinbarung Gas (KoV)** — scheduling obligations between BKV and FNB/MGV
//! - **BNetzA GaBi Gas 2.1 (BK7-24-01-008)** — regulatory framework

use mako_engine::{
    error::WorkflowError,
    types::MessageRef,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use crate::domain::GasDay;

// ── Synthetic PID ─────────────────────────────────────────────────────────────

/// Synthetic PID for the SCHEDL transport schedule message.
pub const SCHEDL_PID: u32 = 90031;

/// All PIDs handled by this workflow.
pub const SCHEDL_PIDS: &[u32] = &[SCHEDL_PID];

/// Workflow key for PID router registration.
pub const WORKFLOW_NAME: &str = "gabi-gas-schedl";

// ── Domain data ───────────────────────────────────────────────────────────────

/// Data captured when a SCHEDL transport schedule is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchedlData {
    /// Synthetic PID (90031).
    pub synthetic_pid: u32,
    /// EIC code of the sender (BKV / FNB / MGV).
    pub sender_eic: String,
    /// EIC code of the receiver.
    pub receiver_eic: String,
    /// Gas day for this transport schedule.
    pub gas_day: GasDay,
    /// Document reference from the SCHEDL message (BGM element 1).
    pub document_ref: MessageRef,
}

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the GaBi Gas SCHEDL workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SchedlEvent {
    /// SCHEDL transport schedule received and recorded.
    ScheduleReceived {
        /// Synthetic PID (90031).
        synthetic_pid: u32,
        /// EIC code of the sender.
        sender_eic: String,
        /// EIC code of the receiver.
        receiver_eic: String,
        /// Gas day for this transport schedule.
        gas_day: GasDay,
        /// Document reference.
        document_ref: MessageRef,
    },
}

impl EventPayload for SchedlEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::ScheduleReceived { .. } => "SchedlScheduleReceived",
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Process state for the GaBi Gas SCHEDL workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum SchedlState {
    /// Initial state — no SCHEDL received yet.
    #[default]
    New,
    /// SCHEDL received and recorded (terminal).
    Received(SchedlData),
}

impl SchedlState {
    fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Received(_) => "Received",
        }
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands for the GaBi Gas SCHEDL workflow.
#[derive(Clone)]
pub enum SchedlCommand {
    /// Inbound SCHEDL (PID 90031) received from sender.
    ReceiveSchedule {
        /// Must be 90031.
        synthetic_pid: u32,
        /// EIC code of the sender (BKV / FNB / MGV).
        sender_eic: String,
        /// EIC code of the receiver.
        receiver_eic: String,
        /// Gas day for this transport schedule.
        gas_day: GasDay,
        /// Document reference from BGM element 1.
        document_ref: MessageRef,
    },
}

impl CommandPayload for SchedlCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GaBi Gas SCHEDL receive-and-record workflow (PID 90031).
///
/// Records inbound transport schedule notifications. No outbound response
/// is required — the workflow transitions immediately to `Received` (terminal).
pub struct GaBiGasSchedlWorkflow;

impl Workflow for GaBiGasSchedlWorkflow {
    type State = SchedlState;
    type Event = SchedlEvent;
    type Command = SchedlCommand;

    fn apply(_state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SchedlEvent::ScheduleReceived {
                synthetic_pid,
                sender_eic,
                receiver_eic,
                gas_day,
                document_ref,
            } => SchedlState::Received(SchedlData {
                synthetic_pid: *synthetic_pid,
                sender_eic: sender_eic.clone(),
                receiver_eic: receiver_eic.clone(),
                gas_day: *gas_day,
                document_ref: document_ref.clone(),
            }),
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        let SchedlCommand::ReceiveSchedule {
            synthetic_pid,
            sender_eic,
            receiver_eic,
            gas_day,
            document_ref,
        } = command;

        if !matches!(state, SchedlState::New) {
            return Err(WorkflowError::invalid_state("New", state.label()));
        }
        if synthetic_pid != SCHEDL_PID {
            return Err(WorkflowError::rejected(format!(
                "expected SCHEDL PID {SCHEDL_PID}, got {synthetic_pid}",
            )));
        }

        Ok(vec![SchedlEvent::ScheduleReceived {
            synthetic_pid,
            sender_eic,
            receiver_eic,
            gas_day,
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
        MessageRef::new("SCHEDL-0001")
    }

    #[test]
    fn workflow_name_is_stable() {
        assert_eq!(WORKFLOW_NAME, "gabi-gas-schedl");
    }

    #[test]
    fn schedl_pid_is_90031() {
        assert_eq!(SCHEDL_PID, 90031);
        assert!(SCHEDL_PIDS.contains(&90031));
    }

    #[test]
    fn receive_schedule_transitions_new_to_received() {
        let state = SchedlState::New;
        let cmd = SchedlCommand::ReceiveSchedule {
            synthetic_pid: 90031,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: crate::domain::GasDay::parse("2026-01-15").unwrap(),
            document_ref: make_ref(),
        };
        let output = GaBiGasSchedlWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(output.events.len(), 1);
        let next = output
            .events
            .iter()
            .fold(state, GaBiGasSchedlWorkflow::apply);
        assert!(matches!(next, SchedlState::Received(_)));
    }

    #[test]
    fn wrong_pid_returns_error() {
        let state = SchedlState::New;
        let cmd = SchedlCommand::ReceiveSchedule {
            synthetic_pid: 90011, // wrong PID
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: crate::domain::GasDay::parse("2026-01-15").unwrap(),
            document_ref: make_ref(),
        };
        assert!(GaBiGasSchedlWorkflow::handle(&state, cmd).is_err());
    }

    #[test]
    fn duplicate_receive_returns_error() {
        let state = SchedlState::Received(SchedlData {
            synthetic_pid: 90031,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: crate::domain::GasDay::parse("2026-01-15").unwrap(),
            document_ref: make_ref(),
        });
        let cmd = SchedlCommand::ReceiveSchedule {
            synthetic_pid: 90031,
            sender_eic: "21X000000001368S".to_owned(),
            receiver_eic: "21X000000001369Q".to_owned(),
            gas_day: crate::domain::GasDay::parse("2026-01-15").unwrap(),
            document_ref: make_ref(),
        };
        assert!(GaBiGasSchedlWorkflow::handle(&state, cmd).is_err());
    }
}
