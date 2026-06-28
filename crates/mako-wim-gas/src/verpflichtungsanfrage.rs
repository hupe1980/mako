//! WiM Gas Verpflichtungsanfrage — obligation inquiry workflow (PIDs 44168–44170).
//!
#![allow(missing_docs)]
//! The grid operator (NB) sends a UTILMD G Verpflichtungsanfrage to the gas
//! metering-point operator (gMSB) to confirm or reject obligation to serve a
//! Marktlokation. The gMSB must respond with an APERAK within **10 Werktage**.
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
//! | 44168 | Verpflichtungsanfrage (NB → gMSB) | NB → gMSB |
//! | 44169 | Bestätigung Verpflichtungsanfrage | gMSB → NB |
//! | 44170 | Ablehnung Verpflichtungsanfrage | gMSB → NB |

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
pub const WORKFLOW_NAME: &str = "wim-gas-verpflichtungsanfrage";

/// Deadline label for the 10-Werktage APERAK window (BK7-24-01-009).
pub const APERAK_WINDOW_LABEL: &str = "wim-gas-verpflichtungsanfrage-aperak-10-werktage";

/// PIDs handled by this workflow (NB → gMSB Verpflichtungsanfrage).
pub const VERPFLICHTUNGSANFRAGE_PIDS: &[u32] = &[
    44168, // Verpflichtungsanfrage (NB → gMSB)
    44169, // Bestätigung
    44170, // Ablehnung
];

// ── Domain events ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimGasVerpflichtungsanfrageEvent {
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

impl EventPayload for WimGasVerpflichtungsanfrageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "WimGasVerpflichtungsanfrageInitiated",
            Self::ValidationPassed { .. } => "WimGasVerpflichtungsanfrageValidationPassed",
            Self::AperakDispatched { .. } => "WimGasVerpflichtungsanfrageAperakDispatched",
            Self::Completed => "WimGasVerpflichtungsanfrageCompleted",
            Self::Rejected { .. } => "WimGasVerpflichtungsanfrageRejected",
            Self::DeadlineExpired { .. } => "WimGasVerpflichtungsanfrageDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WimGasVerpflichtungsanfrageData {
    pub malo_id: MaLo,
    pub sender: MarktpartnerCode,
    pub receiver: MarktpartnerCode,
    pub document_date: String,
    pub pruefidentifikator: Pruefidentifikator,
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

/// State of a WiM Gas Verpflichtungsanfrage process.
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Completed
///                                    ↘ Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum WimGasVerpflichtungsanfrageState {
    New,
    Initiated(WimGasVerpflichtungsanfrageData),
    ValidationPassed(WimGasVerpflichtungsanfrageData),
    AperakSent(WimGasVerpflichtungsanfrageData),
    Completed(WimGasVerpflichtungsanfrageData),
    Rejected { reason: String },
}

impl Default for WimGasVerpflichtungsanfrageState {
    fn default() -> Self {
        Self::New
    }
}

impl WimGasVerpflichtungsanfrageState {
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
pub enum WimGasVerpflichtungsanfrageCommand {
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

impl CommandPayload for WimGasVerpflichtungsanfrageCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas Verpflichtungsanfrage workflow (PIDs 44168–44170).
pub struct WimGasVerpflichtungsanfrageWorkflow;

impl Workflow for WimGasVerpflichtungsanfrageWorkflow {
    type State = WimGasVerpflichtungsanfrageState;
    type Event = WimGasVerpflichtungsanfrageEvent;
    type Command = WimGasVerpflichtungsanfrageCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                APERAK_WINDOW_LABEL,
                WimGasVerpflichtungsanfrageState::Initiated(_)
                | WimGasVerpflichtungsanfrageState::ValidationPassed(_)
                | WimGasVerpflichtungsanfrageState::AperakSent(_),
            ) => Some(WimGasVerpflichtungsanfrageCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimGasVerpflichtungsanfrageEvent::Initiated {
                malo_id,
                sender,
                receiver,
                document_date,
                message_ref,
                pruefidentifikator,
            } => WimGasVerpflichtungsanfrageState::Initiated(WimGasVerpflichtungsanfrageData {
                malo_id: malo_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),
            WimGasVerpflichtungsanfrageEvent::ValidationPassed { .. } => {
                if let WimGasVerpflichtungsanfrageState::Initiated(data) = state {
                    WimGasVerpflichtungsanfrageState::ValidationPassed(data)
                } else {
                    state
                }
            }
            WimGasVerpflichtungsanfrageEvent::AperakDispatched { positive, .. } => match state {
                WimGasVerpflichtungsanfrageState::ValidationPassed(data) => {
                    if *positive {
                        WimGasVerpflichtungsanfrageState::AperakSent(data)
                    } else {
                        WimGasVerpflichtungsanfrageState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            WimGasVerpflichtungsanfrageEvent::Completed => {
                if let WimGasVerpflichtungsanfrageState::AperakSent(data) = state {
                    WimGasVerpflichtungsanfrageState::Completed(data)
                } else {
                    state
                }
            }
            WimGasVerpflichtungsanfrageEvent::Rejected { reason } => {
                WimGasVerpflichtungsanfrageState::Rejected {
                    reason: reason.clone(),
                }
            }
            WimGasVerpflichtungsanfrageEvent::DeadlineExpired { label, .. } => match state {
                WimGasVerpflichtungsanfrageState::Completed(_)
                | WimGasVerpflichtungsanfrageState::Rejected { .. } => state,
                _ => WimGasVerpflichtungsanfrageState::Rejected {
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
            WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WimGasVerpflichtungsanfrageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !VERPFLICHTUNGSANFRAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported WiM Gas Verpflichtungsanfrage PID {pid} (expected one of: {VERPFLICHTUNGSANFRAGE_PIDS:?})",
                    )));
                }
                let mut events = vec![WimGasVerpflichtungsanfrageEvent::Initiated {
                    malo_id,
                    sender,
                    receiver,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(WimGasVerpflichtungsanfrageEvent::ValidationPassed { message_ref });
                } else {
                    events.push(WimGasVerpflichtungsanfrageEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            WimGasVerpflichtungsanfrageCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    WimGasVerpflichtungsanfrageState::ValidationPassed(d) => d,
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
                    vec![WimGasVerpflichtungsanfrageEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            WimGasVerpflichtungsanfrageCommand::Complete => {
                if !matches!(state, WimGasVerpflichtungsanfrageState::AperakSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AperakSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![WimGasVerpflichtungsanfrageEvent::Completed].into())
            }

            WimGasVerpflichtungsanfrageCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    WimGasVerpflichtungsanfrageState::Completed(_)
                        | WimGasVerpflichtungsanfrageState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                let mut outbox: Vec<PendingOutbox> = vec![];
                let data_opt = match &state {
                    WimGasVerpflichtungsanfrageState::Initiated(d)
                    | WimGasVerpflichtungsanfrageState::ValidationPassed(d)
                    | WimGasVerpflichtungsanfrageState::AperakSent(d) => Some(d),
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
                let event =
                    WimGasVerpflichtungsanfrageEvent::DeadlineExpired { deadline_id, label };
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

/// Read-model record for a single WiM Gas Verpflichtungsanfrage process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum WimGasVerpflichtungsanfrageRecord {
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

impl WimGasVerpflichtungsanfrageRecord {
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
    pub fn active_data(&self) -> Option<WimGasVerpflichtungsanfrageRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                malo_id,
                pruefidentifikator,
                ..
            } => Some(WimGasVerpflichtungsanfrageRecordData {
                malo_id,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`WimGasVerpflichtungsanfrageRecord`].
#[derive(Debug, Clone, Copy)]
pub struct WimGasVerpflichtungsanfrageRecordData<'a> {
    /// Marktlokation EIC code.
    pub malo_id: &'a MaLo,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for WimGasVerpflichtungsanfrageRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

#[derive(Debug, Default)]
pub struct WimGasVerpflichtungsanfrageProjection {
    pub records: HashMap<String, WimGasVerpflichtungsanfrageRecord>,
    pub last_seq: u64,
}

impl Projection for WimGasVerpflichtungsanfrageProjection {
    fn name(&self) -> &'static str {
        "WimGasVerpflichtungsanfrageProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<WimGasVerpflichtungsanfrageEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            WimGasVerpflichtungsanfrageRecord::New { event_count }
            | WimGasVerpflichtungsanfrageRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            WimGasVerpflichtungsanfrageEvent::Initiated {
                malo_id,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = WimGasVerpflichtungsanfrageRecord::Active {
                    status: "Initiated",
                    malo_id,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            WimGasVerpflichtungsanfrageEvent::ValidationPassed { .. } => {
                if let WimGasVerpflichtungsanfrageRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            WimGasVerpflichtungsanfrageEvent::AperakDispatched { positive, .. } => {
                if let WimGasVerpflichtungsanfrageRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            WimGasVerpflichtungsanfrageEvent::Completed => {
                if let WimGasVerpflichtungsanfrageRecord::Active { status, .. } = record {
                    *status = "Completed";
                }
            }
            WimGasVerpflichtungsanfrageEvent::Rejected { .. } => {
                if let WimGasVerpflichtungsanfrageRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            WimGasVerpflichtungsanfrageEvent::DeadlineExpired { .. } => {
                if let WimGasVerpflichtungsanfrageRecord::Active { status, .. } = record {
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

    fn verpflichtung_cmd(pid: u32, validation_passed: bool) -> WimGasVerpflichtungsanfrageCommand {
        WimGasVerpflichtungsanfrageCommand::ReceiveUtilmd {
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
    fn verpflichtungsanfrage_happy_path() {
        let state = WimGasVerpflichtungsanfrageState::New;
        let output =
            WimGasVerpflichtungsanfrageWorkflow::handle(&state, verpflichtung_cmd(44168, true))
                .unwrap();
        assert_eq!(output.events.len(), 2);
        assert!(matches!(
            output.events[0],
            WimGasVerpflichtungsanfrageEvent::Initiated { .. }
        ));
        assert!(matches!(
            output.events[1],
            WimGasVerpflichtungsanfrageEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn validation_failure_rejects() {
        let state = WimGasVerpflichtungsanfrageState::New;
        let output =
            WimGasVerpflichtungsanfrageWorkflow::handle(&state, verpflichtung_cmd(44168, false))
                .unwrap();
        assert!(matches!(
            output.events[1],
            WimGasVerpflichtungsanfrageEvent::Rejected { .. }
        ));
    }

    #[test]
    fn invalid_pid_rejected() {
        let state = WimGasVerpflichtungsanfrageState::New;
        let result =
            WimGasVerpflichtungsanfrageWorkflow::handle(&state, verpflichtung_cmd(44022, true));
        assert!(
            result.is_err(),
            "PID 44022 belongs to WiM Gas Kündigung, not Verpflichtungsanfrage"
        );
    }
}
