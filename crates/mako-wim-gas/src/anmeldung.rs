//! WiM Gas Anmeldung / Ende / Vorläufige Abmeldung — MSB change workflow.
//!
//! Covers the processes by which:
//! - **Anmeldung** (MSBN → NB): new gas metering-point operator (gMSB) announces
//!   the start of service (PIDs 44039–44041).
//! - **Ende** (NB → MSBN): grid operator terminates the MSB relationship
//!   (PIDs 44042–44044).
//! - **Vorläufige Abmeldung / Ende** (NB → MSBA): preliminary de-registration
//!   and termination of the old MSB (PIDs 44051–44053).
//!
//! In all three sub-processes the NB must respond with an APERAK within
//! **10 Werktage** (business days) — the same Frist as GeLi Gas.
//!
//! # Regulatory basis
//!
//! - **BNetzA BK7-24-01-009** — GeLi Gas 3.0 / WiM Gas ruling,
//!   Beschluss 12.09.2025, abgeschlossen 24.09.2025
//! - **BDEW/VKU/GEODE/FNBGas AWH WiM Gas V2.0** (2025-08-04)
//! - **UTILMD AHB Gas 1.1 / 1.2** — message specification
//!
//! # APERAK Frist
//!
//! **10 Werktage** (BK7-24-01-009). Saturday **counts** as a Werktag;
//! Sunday and federal public holidays do not.
//!
//! ```rust,ignore
//! use mako_engine::fristen::{self, HolidayCalendar};
//! let due = fristen::add_werktage(received_date, 10, HolidayCalendar::BdewMaKo);
//! ```
//!
//! # Key differences from WiM Strom
//!
//! | Aspect | WiM Strom | WiM Gas |
//! |---|---|---|
//! | APERAK Frist | **5 Werktage** | **10 Werktage** |
//! | Ruling | BK6-24-174 | BK7-24-01-009 |
//! | EDIFACT | UTILMD S2.x | UTILMD G1.x |
//! | Location object | Messlokation (MeLo) | Marktlokation (MaLo) |
//!
//! # PID table
//!
//! | PID | Process | Direction |
//! |---|---|---|
//! | 44039 | Anmeldung MSB Gas (Anfrage MSBN) | MSBN → NB |
//! | 44040 | Bestätigung Anmeldung MSB Gas | NB → MSBN |
//! | 44041 | Ablehnung Anmeldung MSB Gas | NB → MSBN |
//! | 44042 | Ende MSB Gas (Anfrage NB) | NB → MSBN |
//! | 44043 | Bestätigung Ende MSB Gas | MSBN → NB |
//! | 44044 | Ablehnung Ende MSB Gas | MSBN → NB |
//! | 44051 | Vorläufige Abmeldung MSB Gas | NB → MSBA |
//! | 44052 | Bestätigung Vorl. Abmeldung | MSBA → NB |
//! | 44053 | Ablehnung Vorl. Abmeldung | MSBA → NB |

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
use std::collections::HashMap;

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "wim-gas-anmeldung";

/// Deadline label for the 10-Werktage APERAK response window (BK7-24-01-009).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 10, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const APERAK_WINDOW_LABEL: &str = "wim-gas-aperak-10-werktage";

// ── PIDs ─────────────────────────────────────────────────────────────────────

/// PIDs handled by this workflow (UTILMD G — WiM Gas Anmeldung/Ende/Vorl. Abmeldung).
///
/// Note: AHB profiles for WiM Gas PIDs (44022–44053, 44168–44170) are not yet
/// in the `fv*_gas` profile set. Until `cargo xtask import-xml-ahb` imports them,
/// `msg.validate()` returns a vacuous pass for these PIDs. The adapters apply the
/// `pid_has_ahb_rules()` guard (same as ex-MPES PIDs 56001–56004) to prevent
/// false-positive validation. This guard self-corrects once profiles are imported.
pub const ANMELDUNG_PIDS: &[u32] = &[
    44039, 44040, 44041, // Anmeldung MSB Gas (MSBN ↔ NB)
    44042, 44043, 44044, // Ende MSB Gas (NB ↔ MSBN)
    44051, 44052, 44053, // Vorläufige Abmeldung / Ende MSB Gas (NB ↔ MSBA)
];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Gas Anmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WimGasAnmeldungEvent {
    /// Process initiated by a valid UTILMD G Anmeldung / Ende / Vorl. Abmeldung message.
    Initiated {
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// A positive or negative APERAK was dispatched within 10 Werktage.
    AperakDispatched {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// MSB change became active.
    Activated,
    /// Process was rejected and closed.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before the process completed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for WimGasAnmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Initiated { .. } => "WimGasAnmeldungInitiated",
            Self::ValidationPassed { .. } => "WimGasAnmeldungValidationPassed",
            Self::AperakDispatched { .. } => "WimGasAnmeldungAperakDispatched",
            Self::Activated => "WimGasAnmeldungActivated",
            Self::Rejected { .. } => "WimGasAnmeldungRejected",
            Self::DeadlineExpired { .. } => "WimGasAnmeldungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set at `Initiated` time and carried through every later state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WimGasAnmeldungData {
    /// EIC/MaLo code for the gas supply location.
    pub malo_id: MaLo,
    /// Market partner code (GLN) of the message sender.
    pub sender: MarktpartnerCode,
    /// Market partner code (GLN) of the message receiver.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date string.
    pub document_date: String,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: Pruefidentifikator,
    /// Original UTILMD G message reference, preserved for APERAK construction.
    #[serde(default)]
    pub message_ref: Option<MessageRef>,
}

/// Current state of a WiM Gas Anmeldung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Active
///                                    ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum WimGasAnmeldungState {
    /// No events yet.
    New,
    /// UTILMD G received and `Initiated` event applied.
    Initiated(WimGasAnmeldungData),
    /// EDIFACT validation passed; APERAK not yet dispatched.
    ValidationPassed(WimGasAnmeldungData),
    /// Positive APERAK dispatched; awaiting activation.
    AperakSent(WimGasAnmeldungData),
    /// MSB change is active.
    Active(WimGasAnmeldungData),
    /// Process rejected.
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for WimGasAnmeldungState {
    fn default() -> Self {
        Self::New
    }
}

impl WimGasAnmeldungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AperakSent(_) => "AperakSent",
            Self::Active(_) => "Active",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Gas Anmeldung workflow.
#[derive(Clone)]
pub enum WimGasAnmeldungCommand {
    /// Inbound UTILMD G accepted from the AS4 layer.
    ReceiveUtilmd {
        /// BDEW Prüfidentifikator.
        pid: Pruefidentifikator,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings.
        validation_errors: Vec<String>,
    },
    /// Dispatch a positive or negative APERAK (within 10 Werktage, BK7-24-01-009).
    DispatchAperak {
        /// `true` for positive APERAK, `false` for negative.
        positive: bool,
        /// Rejection reason (only set when `positive = false`).
        reason: Option<String>,
    },
    /// Mark the MSB change as active after all checks pass.
    Activate,
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for WimGasAnmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Gas Anmeldung / Ende / Vorläufige Abmeldung workflow.
///
/// Handles UTILMD G messages for the MSB change processes in the German gas
/// market. The receiving party must respond with an APERAK within **10 Werktage**
/// (BNetzA BK7-24-01-009).
pub struct WimGasAnmeldungWorkflow;

impl Workflow for WimGasAnmeldungWorkflow {
    type State = WimGasAnmeldungState;
    type Event = WimGasAnmeldungEvent;
    type Command = WimGasAnmeldungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                APERAK_WINDOW_LABEL,
                WimGasAnmeldungState::Initiated(_)
                | WimGasAnmeldungState::ValidationPassed(_)
                | WimGasAnmeldungState::AperakSent(_),
            ) => Some(WimGasAnmeldungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WimGasAnmeldungEvent::Initiated {
                malo_id,
                sender,
                receiver,
                document_date,
                message_ref,
                pruefidentifikator,
            } => WimGasAnmeldungState::Initiated(WimGasAnmeldungData {
                malo_id: malo_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                message_ref: Some(message_ref.clone()),
            }),
            WimGasAnmeldungEvent::ValidationPassed { .. } => {
                if let WimGasAnmeldungState::Initiated(data) = state {
                    WimGasAnmeldungState::ValidationPassed(data)
                } else {
                    state
                }
            }
            WimGasAnmeldungEvent::AperakDispatched { positive, .. } => match state {
                WimGasAnmeldungState::ValidationPassed(data) => {
                    if *positive {
                        WimGasAnmeldungState::AperakSent(data)
                    } else {
                        WimGasAnmeldungState::Rejected {
                            reason: "negative APERAK".to_owned(),
                        }
                    }
                }
                _ => state,
            },
            WimGasAnmeldungEvent::Activated => {
                if let WimGasAnmeldungState::AperakSent(data) = state {
                    WimGasAnmeldungState::Active(data)
                } else {
                    state
                }
            }
            WimGasAnmeldungEvent::Rejected { reason } => WimGasAnmeldungState::Rejected {
                reason: reason.clone(),
            },
            WimGasAnmeldungEvent::DeadlineExpired { label, .. } => match state {
                WimGasAnmeldungState::Active(_) | WimGasAnmeldungState::Rejected { .. } => state,
                _ => WimGasAnmeldungState::Rejected {
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
            WimGasAnmeldungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                malo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WimGasAnmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !ANMELDUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported WiM Gas Anmeldung PID {pid} (expected one of: {ANMELDUNG_PIDS:?})",
                    )));
                }
                let mut events = vec![WimGasAnmeldungEvent::Initiated {
                    malo_id,
                    sender,
                    receiver,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(WimGasAnmeldungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(WimGasAnmeldungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            WimGasAnmeldungCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    WimGasAnmeldungState::ValidationPassed(d) => d,
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
                    vec![WimGasAnmeldungEvent::AperakDispatched { positive, reason }],
                    vec![outbox_entry],
                ))
            }

            WimGasAnmeldungCommand::Activate => {
                if !matches!(state, WimGasAnmeldungState::AperakSent(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AperakSent",
                        state.status_str(),
                    ));
                }
                Ok(vec![WimGasAnmeldungEvent::Activated].into())
            }

            WimGasAnmeldungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    WimGasAnmeldungState::Active(_) | WimGasAnmeldungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }

                let mut outbox: Vec<PendingOutbox> = vec![];
                let data_opt = match &state {
                    WimGasAnmeldungState::Initiated(d)
                    | WimGasAnmeldungState::ValidationPassed(d)
                    | WimGasAnmeldungState::AperakSent(d) => Some(d),
                    _ => None,
                };
                if let Some(data) = data_opt {
                    outbox.push(PendingOutbox::new(
                        "AperakTimeout",
                        data.sender.as_str(),
                        serde_json::json!({
                            "pid":          data.pruefidentifikator.as_u32(),
                            "malo":         data.malo_id.as_str(),
                            "sender":       data.sender.as_str(),
                            "receiver":     data.receiver.as_str(),
                            "deadline_label": label.as_ref(),
                            "deadline_id":  deadline_id,
                        }),
                    ));
                }

                let event = WimGasAnmeldungEvent::DeadlineExpired { deadline_id, label };
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

/// Read-model record for a single WiM Gas Anmeldung process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum WimGasAnmeldungRecord {
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
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// BDEW Prüfidentifikator.
        pruefidentifikator: Pruefidentifikator,
        /// Total events applied.
        event_count: usize,
    },
}

impl WimGasAnmeldungRecord {
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
    pub fn active_data(&self) -> Option<WimGasAnmeldungRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => Some(WimGasAnmeldungRecordData {
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`WimGasAnmeldungRecord`].
#[derive(Debug, Clone, Copy)]
pub struct WimGasAnmeldungRecordData<'a> {
    /// Marktlokation EIC code.
    pub malo_id: &'a MaLo,
    /// GLN of the sender.
    pub sender: &'a MarktpartnerCode,
    /// GLN of the receiver.
    pub receiver: &'a MarktpartnerCode,
    /// BDEW Prüfidentifikator.
    pub pruefidentifikator: &'a Pruefidentifikator,
}

impl Default for WimGasAnmeldungRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model for WiM Gas Anmeldung streams.
#[derive(Debug, Default)]
pub struct WimGasAnmeldungProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, WimGasAnmeldungRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for WimGasAnmeldungProjection {
    fn name(&self) -> &'static str {
        "WimGasAnmeldungProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<WimGasAnmeldungEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            WimGasAnmeldungRecord::New { event_count }
            | WimGasAnmeldungRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            WimGasAnmeldungEvent::Initiated {
                malo_id,
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => {
                let count = record.event_count();
                *record = WimGasAnmeldungRecord::Active {
                    status: "Initiated",
                    malo_id,
                    sender,
                    receiver,
                    pruefidentifikator,
                    event_count: count,
                };
            }
            WimGasAnmeldungEvent::ValidationPassed { .. } => {
                if let WimGasAnmeldungRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            WimGasAnmeldungEvent::AperakDispatched { positive, .. } => {
                if let WimGasAnmeldungRecord::Active { status, .. } = record {
                    *status = if positive { "AperakSent" } else { "Rejected" };
                }
            }
            WimGasAnmeldungEvent::Activated => {
                if let WimGasAnmeldungRecord::Active { status, .. } = record {
                    *status = "Active";
                }
            }
            WimGasAnmeldungEvent::Rejected { .. } => {
                if let WimGasAnmeldungRecord::Active { status, .. } = record {
                    *status = "Rejected";
                }
            }
            WimGasAnmeldungEvent::DeadlineExpired { .. } => {
                if let WimGasAnmeldungRecord::Active { status, .. } = record {
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

    const MSBN_GLN: &str = "4012345000023";
    const NB_GLN: &str = "9900357000004";
    const MALO: &str = "51238696781";

    fn anmeldung_cmd(pid: u32, validation_passed: bool) -> WimGasAnmeldungCommand {
        WimGasAnmeldungCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new(MSBN_GLN),
            receiver: MarktpartnerCode::new(NB_GLN),
            malo_id: MaLo::new(MALO),
            document_date: "20251001".to_owned(),
            message_ref: MessageRef::new("00001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["rule violation".to_owned()]
            },
        }
    }

    #[test]
    fn anmeldung_happy_path_pid_44039() {
        let state = WimGasAnmeldungState::New;
        let output = WimGasAnmeldungWorkflow::handle(&state, anmeldung_cmd(44039, true)).unwrap();
        assert_eq!(output.events.len(), 2, "Initiated + ValidationPassed");
        assert!(matches!(
            output.events[0],
            WimGasAnmeldungEvent::Initiated { .. }
        ));
        assert!(matches!(
            output.events[1],
            WimGasAnmeldungEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn anmeldung_validation_failure_rejects() {
        let state = WimGasAnmeldungState::New;
        let output = WimGasAnmeldungWorkflow::handle(&state, anmeldung_cmd(44039, false)).unwrap();
        assert_eq!(output.events.len(), 2, "Initiated + Rejected");
        assert!(matches!(
            output.events[1],
            WimGasAnmeldungEvent::Rejected { .. }
        ));
    }

    #[test]
    fn ende_pid_44042_accepted() {
        let state = WimGasAnmeldungState::New;
        let output = WimGasAnmeldungWorkflow::handle(&state, anmeldung_cmd(44042, true)).unwrap();
        assert_eq!(output.events.len(), 2);
        assert!(matches!(
            output.events[1],
            WimGasAnmeldungEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn vorlaeutige_abmeldung_pid_44051_accepted() {
        let state = WimGasAnmeldungState::New;
        let output = WimGasAnmeldungWorkflow::handle(&state, anmeldung_cmd(44051, true)).unwrap();
        assert_eq!(output.events.len(), 2);
        assert!(matches!(
            output.events[1],
            WimGasAnmeldungEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn dispatch_aperak_positive() {
        let data = WimGasAnmeldungData {
            malo_id: MaLo::new(MALO),
            sender: MarktpartnerCode::new(MSBN_GLN),
            receiver: MarktpartnerCode::new(NB_GLN),
            document_date: "20251001".to_owned(),
            pruefidentifikator: Pruefidentifikator::new(44039).unwrap(),
            message_ref: Some(MessageRef::new("00001")),
        };
        let state = WimGasAnmeldungState::ValidationPassed(data);
        let output = WimGasAnmeldungWorkflow::handle(
            &state,
            WimGasAnmeldungCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(output.events.len(), 1);
        assert!(matches!(
            output.events[0],
            WimGasAnmeldungEvent::AperakDispatched { positive: true, .. }
        ));
        assert_eq!(output.outbox.len(), 1);
    }

    #[test]
    fn deadline_in_initiated_state_rejects() {
        let data = WimGasAnmeldungData {
            malo_id: MaLo::new(MALO),
            sender: MarktpartnerCode::new(MSBN_GLN),
            receiver: MarktpartnerCode::new(NB_GLN),
            document_date: "20251001".to_owned(),
            pruefidentifikator: Pruefidentifikator::new(44039).unwrap(),
            message_ref: None,
        };
        let state = WimGasAnmeldungState::Initiated(data);
        let output = WimGasAnmeldungWorkflow::handle(
            &state,
            WimGasAnmeldungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert_eq!(output.events.len(), 1);
        assert!(matches!(
            output.events[0],
            WimGasAnmeldungEvent::DeadlineExpired { .. }
        ));
        assert_eq!(output.outbox.len(), 1, "AperakTimeout outbox entry");
    }

    #[test]
    fn deadline_in_active_state_is_noop() {
        let data = WimGasAnmeldungData {
            malo_id: MaLo::new(MALO),
            sender: MarktpartnerCode::new(MSBN_GLN),
            receiver: MarktpartnerCode::new(NB_GLN),
            document_date: "20251001".to_owned(),
            pruefidentifikator: Pruefidentifikator::new(44039).unwrap(),
            message_ref: None,
        };
        let state = WimGasAnmeldungState::Active(data);
        let output = WimGasAnmeldungWorkflow::handle(
            &state,
            WimGasAnmeldungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(output.events.is_empty(), "no-op in Active state");
    }

    #[test]
    fn invalid_pid_rejected() {
        let state = WimGasAnmeldungState::New;
        let result = WimGasAnmeldungWorkflow::handle(&state, anmeldung_cmd(44001, true));
        assert!(
            result.is_err(),
            "PID 44001 belongs to GeLi Gas, not WiM Gas Anmeldung"
        );
    }
}
