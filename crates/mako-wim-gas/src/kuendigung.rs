//! WiM Gas Kündigung MSB Gas — termination workflow (PIDs 44039–44041).
//!
//! The new gas metering-point operator (MSBN) or network operator (NB) sends a
//! UTILMD G Kündigung to terminate the MSB service relationship.
//! The receiver must respond with an APERAK within **10 Werktage** (BK7-24-01-009).
//!
//! # Regulatory basis
//!
//! - **BNetzA BK7-24-01-009** — GeLi Gas 3.0 / WiM Gas ruling
//! - **BDEW/VKU/GEODE/FNBGas AWH WiM Gas V2.0** (2025-08-04)
//! - **UTILMD AHB Gas 1.1** — PIDs 44039–44041 confirmed as WiM Gas (L15977–L15981)
//!
//! # PID table
//!
//! | PID | Process | Direction |
//! |---|---|---|
//! | 44039 | Kündigung MSB Gas (Anfrage) | MSBN → NB |
//! | 44040 | Bestätigung Kündigung MSB Gas | NB → MSBN |
//! | 44041 | Ablehnung Kündigung MSB Gas | NB → MSBN |

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

/// PIDs handled by this workflow (WiM Gas Kündigung MSB Gas).
///
/// Confirmed as WiM Gas PIDs in UTILMD AHB Gas 1.1 (L15977–L15981).
/// Note: PIDs 44022–44024 are GeLi Gas Stornierung — not WiM Gas.
pub const KUENDIGUNG_PIDS: &[u32] = &[
    44039, // Kündigung MSB Gas (Anfrage)
    44040, // Bestätigung Kündigung MSB Gas
    44041, // Ablehnung Kündigung MSB Gas
];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas Kündigung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimGasKuendigungEvent {
    /// Process initiated by a valid UTILMD G Kündigung message.
    Initiated {
        /// Marktlokation EIC code from IDE+Z19.
        malo_id: MaLo,
        /// GLN of the MSBN sender.
        sender: MarktpartnerCode,
        /// GLN of the NB receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT document date string from DTM+137.
        document_date: String,
        /// UNH message reference of the triggering ANFRAGE.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator of the triggering message.
        pruefidentifikator: Pruefidentifikator,
    },
    /// AHB validation confirmed conformant; APERAK timer starts.
    ValidationPassed {
        /// UNH reference of the validated message.
        message_ref: MessageRef,
    },
    /// NB ERP dispatched an APERAK response.
    AperakDispatched {
        /// `true` = Bestätigung, `false` = Ablehnung.
        positive: bool,
        /// Rejection reason when `positive` is `false`.
        reason: Option<String>,
    },
    /// Process completed after positive APERAK acknowledged.
    Completed,
    /// Process rejected due to validation failure or negative APERAK.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// APERAK deadline timer expired before the ERP responded.
    DeadlineExpired {
        /// Unique deadline identifier.
        deadline_id: DeadlineId,
        /// Deadline label (matches [`APERAK_WINDOW_LABEL`]).
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

/// Persistent domain data for an in-flight WiM Gas Kündigung process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WimGasKuendigungData {
    /// Marktlokation EIC code (object of the termination request).
    pub malo_id: MaLo,
    /// GLN of the initiating MSBN.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving NB.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date string (YYYYMMDDHHMMZZZ from DTM+137).
    pub document_date: String,
    /// BDEW Prüfidentifikator of the triggering ANFRAGE message.
    pub pruefidentifikator: Pruefidentifikator,
    /// UNH message reference of the triggering ANFRAGE.
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
    /// Initial state before any event is applied.
    New,
    /// ANFRAGE received; APERAK deadline timer should now be started.
    Initiated(WimGasKuendigungData),
    /// AHB validation passed; waiting for ERP to dispatch APERAK.
    ValidationPassed(WimGasKuendigungData),
    /// APERAK dispatched; waiting for ERP to mark completed.
    AperakSent(WimGasKuendigungData),
    /// Process ended normally after positive APERAK acknowledged.
    Completed(WimGasKuendigungData),
    /// Process ended abnormally (validation failure, deadline, or negative APERAK).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for WimGasKuendigungState {
    fn default() -> Self {
        Self::New
    }
}

impl WimGasKuendigungState {
    /// Current lifecycle status label, suitable for logging and serialisation.
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

/// Commands accepted by [`WimGasKuendigungWorkflow`].
#[derive(Clone)]
pub enum WimGasKuendigungCommand {
    /// Inbound UTILMD G Kündigung received from the MSBN.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator (one of [`KUENDIGUNG_PIDS`]).
        pid: Pruefidentifikator,
        /// GLN of the MSBN sender.
        sender: MarktpartnerCode,
        /// GLN of the NB receiver.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code from IDE+Z19.
        malo_id: MaLo,
        /// EDIFACT document date string from DTM+137.
        document_date: String,
        /// UNH reference of the triggering message.
        message_ref: MessageRef,
        /// `true` if AHB validation passed.
        validation_passed: bool,
        /// Human-readable AHB rule violations (empty when `validation_passed` is `true`).
        validation_errors: Vec<String>,
    },
    /// The NB ERP has decided to accept or reject the Kündigung.
    DispatchAperak {
        /// `true` = Bestätigung, `false` = Ablehnung.
        positive: bool,
        /// Optional rejection reason (populated when `positive` is `false`).
        reason: Option<String>,
    },
    /// Mark the process as fully completed after the APERAK was acknowledged.
    Complete,
    /// An APERAK deadline timer fired before the ERP responded.
    TimeoutExpired {
        /// Unique deadline identifier.
        deadline_id: DeadlineId,
        /// Deadline label (matches [`APERAK_WINDOW_LABEL`]).
        label: Box<str>,
    },
}

impl CommandPayload for WimGasKuendigungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas Kündigung MSB Gas workflow (PIDs 44039–44041).
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
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_gln = sender.clone();
                let receiver_gln = receiver.clone();

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
                    Ok(events.into())
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(WimGasKuendigungEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 (Verarbeitbarkeitsfehlermeldung) — mandatory
                    // per APERAK AHB 1.0 §2.1.1.
                    // APERAK Frist (Gas Folgeprozess): nächster Werktag 12 Uhr (APERAK AHB 1.0 §2.3.1).
                    // Note: 10 Werktage is the WiM Gas process window (BK7-24-01-009), NOT the APERAK sending deadline.
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_gln.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_gln.as_str(),
                                "pid":        29001_u32,
                                "error_code": "Z29",
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
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
                // Always enqueue an APERAK outbox entry so the ERP layer sees the
                // APERAK decision.  The renderer/sender applies the Gas silence rule:
                //   positive = true  → suppress_wire, no wire EDIFACT (APERAK AHB 1.0 §2.3)
                //   positive = false → BGM+313 rendered and sent
                let mut aperak_payload = serde_json::json!({
                    "sender":   data.receiver.as_str(),  // NB = sender of APERAK wire message
                    "pid":      data.pruefidentifikator.as_u32(),
                    "malo":     data.malo_id.as_str(),
                    "positive": positive,
                });
                if positive {
                    aperak_payload["suppress_wire"] = serde_json::Value::Bool(true);
                }
                if let Some(ref mr) = data.message_ref {
                    aperak_payload["orig_message_ref"] =
                        serde_json::Value::String(mr.as_str().to_owned());
                }
                if let Some(ref r) = reason {
                    aperak_payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry =
                    PendingOutbox::new("APERAK", data.sender.as_str(), aperak_payload).caused_by(0);
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

/// Read-model projection over all WiM Gas Kündigung streams.
///
/// Maintained by replaying [`WimGasKuendigungEvent`] envelopes in sequence.
#[derive(Debug, Default)]
pub struct WimGasKuendigungProjection {
    /// Keyed by stream ID string.
    pub records: HashMap<String, WimGasKuendigungRecord>,
    /// Highest event sequence number applied.
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
        let output = WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44039, true)).unwrap();
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
            WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44039, false)).unwrap();
        assert!(matches!(
            output.events[1],
            WimGasKuendigungEvent::Rejected { .. }
        ));
    }

    #[test]
    fn invalid_pid_rejected() {
        let state = WimGasKuendigungState::New;
        let result = WimGasKuendigungWorkflow::handle(&state, kuendigung_cmd(44022, true));
        assert!(
            result.is_err(),
            "PID 44022 is GeLi Gas Stornierung, not WiM Gas Kündigung"
        );
    }
}
