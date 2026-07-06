//! WiM Preisliste — PRICAT price-list publication by MSB/NB.
//!
//! Covers the PRICAT process per BDEW WiM documentation:
//! the MSB or NB publishes its service price list to counterparties.
//!
//! This module implements the **receiving-party perspective**: the system
//! receives an inbound PRICAT price-list message and records it.
//!
//! # Prüfidentifikatoren (PRICAT AHB 2.1)
//!
//! | PID   | Process name (AHB)                              | Direction     |
//! |-------|-------------------------------------------------|---------------|
//! | 27001 | Übermittlung der Ausgleichsenergiepreise         | BIKO → NB/LF  |
//! | 27002 | Preisblatt MSB-Leistungen                       | MSB → NB/LF   |
//! | 27003 | Preisblatt NB-Leistungen                        | NB → LF/MSB   |
//!
//! # Regulatory basis
//!
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom
//! - **PRICAT AHB 2.1** — EDI@Energy message format
//! - No APERAK response required (informational publish, not a two-way exchange)

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-preisliste";

/// Inbound PRICAT Prüfidentifikatoren handled by [`WimPreislisteWorkflow`].
///
/// | PID   | Process (AHB)                               | Direction    |
/// |-------|---------------------------------------------|--------------|
/// | 27001 | Übermittlung Ausgleichsenergiepreise         | BIKO → NB/LF |
/// | 27002 | Preisblatt MSB-Leistungen                   | MSB → NB/LF  |
/// | 27003 | Preisblatt NB-Leistungen                    | NB → LF/MSB  |
pub const PRICAT_PIDS: &[u32] = &[27001, 27002, 27003];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Preisliste workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PreislisteEvent {
    /// PRICAT price-list message received.
    PreislisteErhalten {
        /// GLN of the sender (MSB or NB or BIKO).
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (27001–27003).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Message recorded (no counterparty response required).
    Aufgezeichnet,
    /// Message rejected (validation failure).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl EventPayload for PreislisteEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::PreislisteErhalten { .. } => "PreislisteErhalten",
            Self::ValidationPassed { .. } => "PreislisteValidationPassed",
            Self::Aufgezeichnet => "PreislisteAufgezeichnet",
            Self::Rejected { .. } => "PreislisteRejected",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `PreislisteErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreislisteData {
    /// GLN of the price-list publisher.
    pub sender: MarktpartnerCode,
    /// GLN of the recipient.
    pub receiver: MarktpartnerCode,
    /// BDEW Prüfidentifikator (27001–27003).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a WiM Preisliste process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → Aufgezeichnet
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum PreislisteState {
    /// No events yet.
    New,
    /// PRICAT received; awaiting recording.
    Eingegangen(PreislisteData),
    /// Price list recorded.
    Aufgezeichnet(PreislisteData),
    /// Process rejected.
    Rejected {
        /// Reason.
        reason: String,
    },
}

impl Default for PreislisteState {
    fn default() -> Self {
        Self::New
    }
}

impl PreislisteState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::Aufgezeichnet(_) => "Aufgezeichnet",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Preisliste workflow.
#[derive(Clone)]
pub enum PreislisteCommand {
    /// Inbound PRICAT received.
    ReceivePricat {
        /// BDEW Prüfidentifikator (27001–27003).
        pid: Pruefidentifikator,
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation passed.
        validation_passed: bool,
        /// Validation errors.
        validation_errors: Vec<String>,
    },
    /// Record the price list as acknowledged.
    Aufzeichnen,
    /// A registered deadline fired.
    ///
    /// PRICAT is a publish-only workflow with no active deadlines; this
    /// command is a no-op in all terminal states. It satisfies the engine
    /// contract that every workflow must handle `TimeoutExpired`.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for PreislisteCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Preisliste workflow (PRICAT, PIDs 27001–27003).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<WimPreislisteWorkflow>(
///     tenant_id,
///     WorkflowId::new("wim-preisliste", "FV2025-10-01"),
/// );
/// ```
pub struct WimPreislisteWorkflow;

impl Workflow for WimPreislisteWorkflow {
    type State = PreislisteState;
    type Event = PreislisteEvent;
    type Command = PreislisteCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            PreislisteEvent::PreislisteErhalten {
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => PreislisteState::Eingegangen(PreislisteData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            PreislisteEvent::ValidationPassed { .. } => state, // stays Eingegangen
            PreislisteEvent::Aufgezeichnet => match state {
                PreislisteState::Eingegangen(data) => PreislisteState::Aufgezeichnet(data),
                other => other,
            },
            PreislisteEvent::Rejected { reason } => PreislisteState::Rejected {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            PreislisteCommand::ReceivePricat {
                pid,
                sender,
                receiver,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, PreislisteState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !PRICAT_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected PRICAT PID (27001–27003), got {pid}",
                    )));
                }
                let mut events = vec![PreislisteEvent::PreislisteErhalten {
                    sender,
                    receiver,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(PreislisteEvent::ValidationPassed { message_ref });
                } else {
                    events.push(PreislisteEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            PreislisteCommand::Aufzeichnen => {
                if !matches!(state, PreislisteState::Eingegangen(_)) {
                    return Err(WorkflowError::invalid_state("Eingegangen", state.label()));
                }
                Ok(vec![PreislisteEvent::Aufgezeichnet].into())
            }

            // PRICAT is a publish-only workflow with no deadlines; a fired
            // timeout is always a no-op.
            PreislisteCommand::TimeoutExpired { .. } => Ok(vec![].into()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::workflow::Workflow;

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    #[test]
    fn pricat_27002_happy_path() {
        let out = WimPreislisteWorkflow::handle(
            &PreislisteState::New,
            PreislisteCommand::ReceivePricat {
                pid: pid(27002),
                sender: mcod("9900357000004"),
                receiver: mcod("4012345000023"),
                message_ref: mref("PRICAT-001"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 2); // PreislisteErhalten + ValidationPassed
        let state = out
            .events
            .iter()
            .fold(PreislisteState::New, WimPreislisteWorkflow::apply);
        assert!(matches!(state, PreislisteState::Eingegangen(_)));

        let out = WimPreislisteWorkflow::handle(&state, PreislisteCommand::Aufzeichnen).unwrap();
        let state = out.events.iter().fold(state, WimPreislisteWorkflow::apply);
        assert!(matches!(state, PreislisteState::Aufgezeichnet(_)));
    }

    #[test]
    fn pricat_wrong_pid_rejected() {
        let result = WimPreislisteWorkflow::handle(
            &PreislisteState::New,
            PreislisteCommand::ReceivePricat {
                pid: pid(55001),
                sender: mcod("9900357000004"),
                receiver: mcod("4012345000023"),
                message_ref: mref("X"),
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(result.is_err());
    }
}
