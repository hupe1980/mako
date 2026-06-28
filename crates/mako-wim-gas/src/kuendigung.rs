//! WiM Gas Kündigung MSB Gas — termination workflow (PIDs 44022–44024).
//!
#![allow(missing_docs)]
//! The grid operator (NB) sends a UTILMD G Kündigung to the existing gas
//! metering-point operator (MSBA) announcing termination of the MSB service.
//! The MSBA must respond with an APERAK within **10 Werktage** (BK7-24-01-009).
//!
//! # Regulatory basis
//!
//! - **BNetzA BK7-24-01-009** — GeLi Gas 3.0 / WiM Gas ruling
//! - **BDEW/VKU/GEODE/FNBGas AWH WiM Gas V2.0** (2025-08-04)
//!
//! # PID table
//!
//! | PID | Process | Direction |
//! |---|---|---|
//! | 44022 | Kündigung MSB Gas (Anfrage NB) | NB → MSBA |
//! | 44023 | Bestätigung Kündigung MSB Gas | MSBA → NB |
//! | 44024 | Ablehnung Kündigung MSB Gas | MSBA → NB |

use std::collections::HashMap;

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name.
pub const WORKFLOW_NAME: &str = "wim-gas-kuendigung";

/// Deadline label for the 10-Werktage APERAK window (BK7-24-01-009).
pub const APERAK_WINDOW_LABEL: &str = "wim-gas-kuendigung-aperak-10-werktage";

/// PIDs handled by this workflow (NB → MSBA Kündigung).
pub const KUENDIGUNG_PIDS: &[u32] = &[
    44022, // Kündigung MSB Gas (Anfrage NB → MSBA)
    44023, // Bestätigung Kündigung
    44024, // Ablehnung Kündigung
];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas Kündigung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimGasKuendigungEvent {
    /// Process initiated by a valid UTILMD G Kündigung message.
    Initiated {
        malo_id: MaLo,
        sender: MarktpartnerCode,
        receiver: MarktpartnerCode,
        document_date: String,
        message_ref: MessageRef,
        pruefidentifikator: Pruefidentifikator,
    },
    ValidationPassed {
        message_ref: MessageRef,
    },
    AperakDispatched {
        positive: bool,
        reason: Option<String>,
    },
    Completed,
    Rejected {
        reason: String,
    },
    DeadlineExpired {
        deadline_id: DeadlineId,
        label: Box<str>,
    },
}

impl EventPayload for WimGasKuendigungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "WimGasKuendigungInitiated",
            Self::ValidationPassed { .. } => "WimGasKuendigungValidationPassed",
            Self::AperakDispatched { .. } => "WimGasKuendigungAperakDispatched",
            Self::Completed => "WimGasKuendigungCompleted",
            Self::Rejected { .. } => "WimGasKuendigungRejected",
            Self::DeadlineExpired { .. } => "WimGasKuendigungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WimGasKuendigungData {
    pub malo_id: MaLo,
    pub sender: MarktpartnerCode,
    pub receiver: MarktpartnerCode,
    pub document_date: String,
    pub pruefidentifikator: Pruefidentifikator,
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

/// State of a WiM Gas Kündigung process.
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Completed
///                                    ↘ Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum WimGasKuendigungState {
    New,
    Initiated(WimGasKuendigungData),
    ValidationPassed(WimGasKuendigungData),
    AperakSent(WimGasKuendigungData),
    Completed(WimGasKuendigungData),
    Rejected { reason: String },
}

impl Default for WimGasKuendigungState {
    fn default() -> Self {
        Self::New
    }
}

impl WimGasKuendigungState {
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AperakSent(_) => "AperakSent",
            Self::Completed(_) => "Completed",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

#[derive(Clone)]
pub enum WimGasKuendigungCommand {
    ReceiveUtilmd {
        pid: Pruefidentifikator,
        sender: MarktpartnerCode,
        receiver: MarktpartnerCode,
        malo_id: MaLo,
        document_date: String,
        message_ref: MessageRef,
        validation_passed: bool,
        validation_errors: Vec<String>,
    },
    DispatchAperak {
        positive: bool,
        reason: Option<String>,
    },
    Complete,
    TimeoutExpired {
        deadline_id: DeadlineId,
        label: Box<str>,
    },
}

impl CommandPayload for WimGasKuendigungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas Kündigung MSB Gas workflow (PIDs 44022–44024).
pub struct WimGasKuendigungWorkflow;

impl Workflow for WimGasKuendigungWorkflow {
    type State = WimGasKuendigungState;
    type Event = WimGasKuendigungEvent;
    type Command = WimGasKuendigungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                APERAK_WINDOW_LABEL,
                WimGasKuendigungState::Initiated(_)
                | WimGasKuendigungState::ValidationPassed(_)
                | WimGasKuendigungState::AperakSent(_),
            ) => Some(WimGasKuendigungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimGasKuendigungEvent::Initiated {
                malo_id,
                sender,
                receiver,
                document_date,
                message_ref,
                pruefidentifikator,
            } => WimGasKuendigungState::Initiated(WimGasKuendigungData {
                malo_id: malo_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),
            WimGasKuendigungEvent::ValidationPassed { .. } => {
                if let WimGasKuendigungState::Initiated(data) = state {
                    WimGasKuendigungState::ValidationPassed(data)
                } else {
                    state
                }
            }
            WimGasKuendigungEvent::AperakDispatched { positive, .. } => match state {
                WimGasKuendigungState::ValidationPassed(data) => {
                    if *positive {
                        WimGasKuendigungState::AperakSent(data)
                    } else {
                        WimGasKuendigungState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            WimGasKuendigungEvent::Completed => {
                if let WimGasKuendigungState::AperakSent(data) = state {
                    WimGasKuendigungState::Completed(data)
                } else {
                    state
                }
            }
            WimGasKuendigungEvent::Rejected { reason } => WimGasKuendigungState::Rejected {
                reason: reason.clone(),
            },
            WimGasKuendigungEvent::DeadlineExpired { label, .. } => match state {
                WimGasKuendigungState::Completed(_) | WimGasKuendigungState::Rejected { .. } => {
                    state
                }
                _ => WimGasKuendigungState::Rejected {
                    reason: format!("deadline expired: {label}"),
                },
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            WimGasKuendigungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WimGasKuendigungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !KUENDIGUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported WiM Gas Kündigung PID {pid} (expected one of: {KUENDIGUNG_PIDS:?})",
                    )));
                }
                let mut events = vec![WimGasKuendigungEvent::Initiated {
                    malo_id,
                    sender,
                    receiver,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(WimGasKuendigungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(WimGasKuendigungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            WimGasKuendigungCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    WimGasKuendigungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                let mut payload = serde_json::json!({
                    "pid":      data.pruefidentifikator.as_u32(),
                    "malo":     data.malo_id.as_str(),
                    "sender":   data.sender.as_str(),
                    "receiver": data.receiver.as_str(),
                    "positive": positive,
                });
                if let Some(ref mr) = data.message_ref {
                    payload["orig_message_ref"] = serde_json::Value::String(mr.as_str().to_owned());
                }
                if let Some(ref r) = reason {
                    payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry = PendingOutbox::new("Aperak", data.sender.as_str(), payload);
                Ok(WorkflowOutput::with_outbox(
                    vec![WimGasKuendigungEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            WimGasKuendigungCommand::Complete => {
                if !matches!(state, WimGasKuendigungState::AperakSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AperakSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![WimGasKuendigungEvent::Completed].into())
            }

            WimGasKuendigungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    WimGasKuendigungState::Completed(_) | WimGasKuendigungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let mut outbox: Vec<PendingOutbox> = vec![];
                let data_opt = match &state {
                    WimGasKuendigungState::Initiated(d)
                    | WimGasKuendigungState::ValidationPassed(d)
                    | WimGasKuendigungState::AperakSent(d) => Some(d),
                    _ => None,
                };
                if let Some(data) = data_opt {
                    outbox.push(PendingOutbox::new(
                        "AperakTimeout",
                        data.sender.as_str(),
                        serde_json::json!({
                            "pid":            data.pruefidentifikator.as_u32(),
                            "malo":           data.malo_id.as_str(),
                            "deadline_label": label.as_ref(),
                            "deadline_id":    deadline_id,
                        }),
                    ));
                }
                let event = WimGasKuendigungEvent::DeadlineExpired { deadline_id, label };
                if outbox.is_empty() {
                    Ok(vec![event].into())
                } else {
                    Ok(WorkflowOutput::with_outbox(vec![event], outbox))
                }
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Gas Kündigung process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum WimGasKuendigungRecord {
    /// No `Initiated` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `Initiated` event applied; process fields now available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl WimGasKuendigungRecord {
    /// Current lifecycle status label, suitable for logging and serialisation.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Domain data for this record if it has been initiated, or `None` if `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<WimGasKuendigungRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                malo_id,
                pruefidentifikator,
                ..
            } => Some(WimGasKuendigungRecordData {
                malo_id,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`WimGasKuendigungRecord`].
#[derive(Debug, Clone, Copy)]
pub struct WimGasKuendigungRecordData<'a> {
    /// Marktlokation EIC code.
    pub malo_id: &'a MaLo,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for WimGasKuendigungRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

#[derive(Debug, Default)]
pub struct WimGasKuendigungProjection {
    pub records: HashMap<String, WimGasKuendigungRecord>,
    pub last_seq: u64,
}

impl Projection for WimGasKuendigungProjection {
    fn name(&self) -> &'static str {
        "WimGasKuendigungProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<WimGasKuendigungEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            WimGasKuendigungRecord::New { event_count }
            | WimGasKuendigungRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            WimGasKuendigungEvent::Initiated {
                malo_id,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = WimGasKuendigungRecord::Active {
                    status: "Initiated",
                    malo_id,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            WimGasKuendigungEvent::ValidationPassed { .. } => {
                if let WimGasKuendigungRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            WimGasKuendigungEvent::AperakDispatched { positive, .. } => {
                if let WimGasKuendigungRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            WimGasKuendigungEvent::Completed => {
                if let WimGasKuendigungRecord::Active { status, .. } = record {
                    *status = "Completed";
                }
            }
            WimGasKuendigungEvent::Rejected { .. } => {
                if let WimGasKuendigungRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            WimGasKuendigungEvent::DeadlineExpired { .. } => {
                if let WimGasKuendigungRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
        }
    }

    fn last_sequence(&self) -> Option<u64> {
        if self.last_seq > 0 {
            Some(self.last_seq)
        } else {
            None
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::workflow::Workflow;

    fn kuendigung_cmd(pid: u32, validation_passed: bool) -> WimGasKuendigungCommand {
        WimGasKuendigungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("9900357000004"),
            receiver: MarktpartnerCode::new("4012345000023"),
            malo_id: MaLo::new("51238696781"),
            document_date: "20251001".to_owned(),
            message_ref: MessageRef::new("00001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["err".into()]
            },
        }
    }

    #[test]
    fn kuendigung_happy_path() {
        let state = WimGasKuendigungState::New;
        let output = WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44022, true)).unwrap();
        assert_eq!(output.events.len(), 2);
        assert!(matches!(
            output.events[0],
            WimGasKuendigungEvent::Initiated { .. }
        ));
        assert!(matches!(
            output.events[1],
            WimGasKuendigungEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn kuendigung_validation_failure_rejects() {
        let state = WimGasKuendigungState::New;
        let output =
            WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44022, false)).unwrap();
        assert!(matches!(
            output.events[1],
            WimGasKuendigungEvent::Rejected { .. }
        ));
    }

    #[test]
    fn invalid_pid_rejected() {
        let state = WimGasKuendigungState::New;
        let result = WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44039, true));
        assert!(
            result.is_err(),
            "PID 44039 belongs to WiM Gas Anmeldung, not Kündigung"
        );
    }
}
