//! WiM Stornierung — cancellation of commissioning orders (ORDCHG PID 39002).
//!
//! Models the BDEW WiM Strom Teil 2 process by which a party cancels an existing
//! device commissioning order by sending an ORDCHG message.
//!
//! **PIDs 39000** ("Stornierung Sperr-/Entsperrauftrag", AWH Sperrprozesse Gas) and
//! **39001** ("Weiterleitung der Stornierung", AWH Sperrprozesse Gas) belong to
//! Gas Sperrprozesse and must NOT be registered here.
//!
//! # Process flow
//!
//! ```text
//! Requesting party → ORDCHG 39002 (Stornierung der Bestellung) ───────── NB/aMSB
//!                                                                             │
//! Requesting party ← ORDRSP 19013 (Bestätigung Stornierung) ←─── 5 Werktage
//!                  or ORDRSP 19014 (Ablehnung Stornierung)
//! ```
//!
//! Only ORDCHG 39002 is registered in the PID router.
//! ORDRSP 19013/19014 are dispatched as outbox entries.
//!
//! # Regulatory basis
//!
//! - **BDEW WiM AHB Strom** — Stornierung der Bestellung (Teil 2)
//! - **BNetzA BK6-24-174** — 5 Werktage Frist for ORDRSP response

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    projection::Projection,
    types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID constants ─────────────────────────────────────────────────────────────

/// Inbound ORDCHG PID — triggers a new `WimStornierungWorkflow` process.
///
/// **PID 39000** ("Stornierung Sperr-/Entsperrauftrag", AWH Sperrprozesse Gas)
/// belongs to Gas Sperrprozesse (`mako-geli-gas` / `mako-wim-gas`) per
/// `docs/pid-reference.md`. It must not be registered in `mako-wim`.
///
/// **PID 39001** ("Weiterleitung der Stornierung", AWH Sperrprozesse Gas)
/// also belongs to Gas Sperrprozesse. It must not be registered here.
pub const STORNIERUNG_PID: u32 = 39_002;

/// ORDRSP PID for a **positive** Stornierung response (cancellation accepted).
/// Per BDEW ORDRSP AHB / WiM Strom Teil 2.
pub const BESTAETIGUNG_PID: u32 = 19_013;

/// ORDRSP PID for a **negative** Stornierung response (cancellation rejected).
/// Per BDEW ORDRSP AHB / WiM Strom Teil 2.
pub const ABLEHNUNG_PID: u32 = 19_014;

/// Deadline label for the 5-Werktage ORDRSP response window (WiM BK6-18-032).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 5, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STORNIERUNG_DEADLINE_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STORNIERUNG_DEADLINE_LABEL: &str = "wim-stornierung-deadline";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StornierungEvent {
    /// ORDCHG 39002 (Stornierung der Bestellung) received.
    StornierungReceived {
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Document date from DTM segment.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Reference to the original ORDERS message being cancelled.
        cancelled_ref: Option<MessageRef>,
    },
    /// EDIFACT ORDCHG passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// ORDRSP 19013 dispatched — cancellation accepted.
    Bestaetigt {
        /// Message reference of the dispatched ORDRSP.
        response_ref: MessageRef,
    },
    /// ORDRSP 19014 dispatched — cancellation rejected.
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
        /// Message reference of the dispatched ORDRSP.
        response_ref: MessageRef,
    },
    /// Process rejected due to validation failure.
    ValidationFailed {
        /// Validation failure description.
        reason: String,
    },
    /// A registered deadline fired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for StornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StornierungReceived { .. } => "WimStornierungReceived",
            Self::ValidationPassed { .. } => "WimStornierungValidationPassed",
            Self::Bestaetigt { .. } => "WimStornierungBestaetigt",
            Self::Abgelehnt { .. } => "WimStornierungAbgelehnt",
            Self::ValidationFailed { .. } => "WimStornierungValidationFailed",
            Self::DeadlineExpired { .. } => "WimStornierungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the ORDCHG.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StornierungData {
    /// GLN of the cancellation requester.
    pub sender: MarktpartnerCode,
    /// GLN of the NB/aMSB receiving the cancellation.
    pub receiver: MarktpartnerCode,
    /// Messlokation EIC code.
    pub melo_id: MeLo,
    /// Document date from DTM segment.
    pub document_date: String,
    /// Reference to the original ORDERS message being cancelled.
    pub cancelled_ref: Option<MessageRef>,
}

/// State of a single WiM Stornierung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → StornierungReceived → ValidationPassed → Bestaetigt (ORDRSP 39001)
///                           ↘ ValidationFailed → (terminal)
///       ValidationPassed → Abgelehnt (ORDRSP 39002)
///       ValidationPassed → Abgelehnt (deadline expired)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum StornierungState {
    /// No events yet.
    #[default]
    New,
    /// ORDCHG received; awaiting validation.
    StornierungReceived(StornierungData),
    /// Validation passed; awaiting ORDRSP dispatch decision.
    ValidationPassed(StornierungData),
    /// Cancellation confirmed (ORDRSP 19013 dispatched).
    Bestaetigt(StornierungData),
    /// Cancellation rejected (ORDRSP 19014 dispatched or validation failed).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl StornierungState {
    /// Returns `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Bestaetigt(_) | Self::Abgelehnt { .. })
    }

    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::StornierungReceived(_) => "StornierungReceived",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Bestaetigt(_) => "Bestaetigt",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Stornierung workflow.
#[derive(Clone)]
pub enum StornierungCommand {
    /// Inbound ORDCHG 39002 — Stornierung der Bestellung (WiM Strom Teil 2).
    ///
    /// Domain fields extracted and EDIFACT validation performed at the
    /// transport boundary **before** constructing this command.
    ReceiveOrdchg {
        /// ORDCHG PID (39000).
        pid: Pruefidentifikator,
        /// GLN of the message sender.
        sender: MarktpartnerCode,
        /// GLN of the message receiver.
        receiver: MarktpartnerCode,
        /// Messlokation EIC code.
        melo_id: MeLo,
        /// Document date from DTM segment.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Reference to the original ORDERS being cancelled (from RFF+Z13).
        cancelled_ref: Option<MessageRef>,
        /// `true` if EDIFACT profile validation succeeded.
        validation_passed: bool,
        /// Validation error messages when `validation_passed = false`.
        validation_errors: Vec<String>,
    },
    /// Phase 2: Dispatch ORDRSP 19013 — cancellation accepted.
    ///
    /// **BNetzA BK6-18-032**: ORDRSP must be sent within **5 Werktage**.
    Accept {
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
    },
    /// Phase 2: Dispatch ORDRSP 19014 — cancellation rejected.
    Reject {
        /// Human-readable rejection reason.
        reason: String,
        /// Message reference assigned to the outbound ORDRSP.
        response_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for StornierungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Stornierung workflow (PID 39000).
///
/// Implements the BDEW WiM cancellation process from the **NB/aMSB
/// perspective** — the party that receives the ORDCHG and must issue an
/// ORDRSP within 5 Werktage.
pub struct WimStornierungWorkflow;

impl Workflow for WimStornierungWorkflow {
    type State = StornierungState;
    type Event = StornierungEvent;
    type Command = StornierungCommand;

    /// Deadline compensation for the WiM Stornierung 5-Werktage ORDRSP window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"wim-stornierung-deadline"` | `StornierungReceived` or `ValidationPassed` | `TimeoutExpired` | BK6-18-032 — 5 Werktage Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                STORNIERUNG_DEADLINE_LABEL,
                StornierungState::StornierungReceived(_) | StornierungState::ValidationPassed(_),
            ) => Some(StornierungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StornierungEvent::StornierungReceived {
                sender,
                receiver,
                melo_id,
                document_date,
                cancelled_ref,
                ..
            } => StornierungState::StornierungReceived(StornierungData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                melo_id: melo_id.clone(),
                document_date: document_date.clone(),
                cancelled_ref: cancelled_ref.clone(),
            }),
            StornierungEvent::ValidationPassed { .. } => {
                if let StornierungState::StornierungReceived(data) = state {
                    StornierungState::ValidationPassed(data)
                } else {
                    state
                }
            }
            StornierungEvent::Bestaetigt { .. } => {
                if let StornierungState::ValidationPassed(data) = state {
                    StornierungState::Bestaetigt(data)
                } else {
                    state
                }
            }
            StornierungEvent::Abgelehnt { reason, .. } => StornierungState::Abgelehnt {
                reason: reason.clone(),
            },
            StornierungEvent::ValidationFailed { reason } => StornierungState::Abgelehnt {
                reason: reason.clone(),
            },
            StornierungEvent::DeadlineExpired { label, .. } => match state {
                s if s.is_terminal() => s,
                _ => StornierungState::Abgelehnt {
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
            StornierungCommand::ReceiveOrdchg {
                pid,
                sender,
                receiver,
                melo_id,
                document_date,
                message_ref,
                cancelled_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, StornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if pid.as_u32() != STORNIERUNG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not the WiM Stornierung PID (expected {STORNIERUNG_PID})",
                        pid.as_u32()
                    )));
                }
                let mut events = vec![StornierungEvent::StornierungReceived {
                    sender,
                    receiver,
                    melo_id,
                    document_date,
                    message_ref: message_ref.clone(),
                    cancelled_ref,
                }];
                if validation_passed {
                    events.push(StornierungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(StornierungEvent::ValidationFailed {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            StornierungCommand::Accept { response_ref } => {
                if !matches!(state, StornierungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(vec![StornierungEvent::Bestaetigt { response_ref }].into())
            }

            StornierungCommand::Reject {
                reason,
                response_ref,
            } => {
                if !matches!(state, StornierungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(vec![StornierungEvent::Abgelehnt {
                    reason,
                    response_ref,
                }]
                .into())
            }

            StornierungCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![StornierungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Stornierung process stream.
///
/// Uses a type-state design so field access never requires `Option::unwrap`:
/// the `Active` variant carries all domain fields that are structurally
/// guaranteed to exist once the process moves past `New`.
#[derive(Debug)]
pub enum StornierungRecord {
    /// No `StornierungReceived` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `StornierungReceived` event applied; process fields now available.
    Active {
        /// Current lifecycle stage (e.g. "StornierungReceived", "Bestaetigt", "Abgelehnt").
        status: &'static str,
        /// Messlokation EIC code from the ORDCHG.
        melo_id: MeLo,
        /// GLN of the cancellation requester.
        sender: MarktpartnerCode,
        /// Total events applied.
        event_count: usize,
    },
}

impl StornierungRecord {
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

    /// Domain data for this record if it has been received, or `None` if `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<StornierungRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id, sender, ..
            } => Some(StornierungRecordData { melo_id, sender }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`StornierungRecord`].
#[derive(Debug, Clone, Copy)]
pub struct StornierungRecordData<'a> {
    /// Messlokation EIC code from the ORDCHG.
    pub melo_id: &'a MeLo,
    /// GLN of the cancellation requester.
    pub sender: &'a MarktpartnerCode,
}

impl Default for StornierungRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Stornierung streams.
#[derive(Debug, Default)]
pub struct StornierungProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, StornierungRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for StornierungProjection {
    fn name(&self) -> &'static str {
        "StornierungProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<StornierungEvent>() else {
            return;
        };

        // Increment event count on every decoded event.
        match record {
            StornierungRecord::New { event_count }
            | StornierungRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            StornierungEvent::StornierungReceived {
                sender, melo_id, ..
            } => {
                let count = record.event_count();
                *record = StornierungRecord::Active {
                    status: "StornierungReceived",
                    melo_id,
                    sender,
                    event_count: count,
                };
            }
            StornierungEvent::ValidationPassed { .. } => {
                if let StornierungRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            StornierungEvent::Bestaetigt { .. } => {
                if let StornierungRecord::Active { status, .. } = record {
                    *status = "Bestaetigt";
                }
            }
            StornierungEvent::Abgelehnt { .. }
            | StornierungEvent::ValidationFailed { .. }
            | StornierungEvent::DeadlineExpired { .. } => {
                if let StornierungRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn stornierung_cmd(valid: bool) -> StornierungCommand {
        StornierungCommand::ReceiveOrdchg {
            pid: Pruefidentifikator::new(39_002).unwrap(),
            sender: MarktpartnerCode::new("9900123456789"),
            receiver: MarktpartnerCode::new("4012345000023"),
            melo_id: MeLo::new("DE00056789012"),
            document_date: "20260101".to_owned(),
            message_ref: MessageRef::new("MSG-ORDCHG-001"),
            cancelled_ref: Some(MessageRef::new("MSG-ORDERS-001")),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["missing segment".to_owned()]
            },
        }
    }

    #[test]
    fn happy_path_stornierung_accepted() {
        let state = StornierungState::default();
        let events = WimStornierungWorkflow::handle(&state, stornierung_cmd(true)).unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        assert!(matches!(state, StornierungState::ValidationPassed(_)));

        let events = WimStornierungWorkflow::handle(
            &state,
            StornierungCommand::Accept {
                response_ref: MessageRef::new("MSG-ORDRSP-39001"),
            },
        )
        .unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        assert!(matches!(state, StornierungState::Bestaetigt(_)));
    }

    #[test]
    fn stornierung_rejected_by_nb() {
        let state = StornierungState::default();
        let events = WimStornierungWorkflow::handle(&state, stornierung_cmd(true)).unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);

        let events = WimStornierungWorkflow::handle(
            &state,
            StornierungCommand::Reject {
                reason: "Auftrag bereits ausgeführt".to_owned(),
                response_ref: MessageRef::new("MSG-ORDRSP-39002"),
            },
        )
        .unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        assert!(matches!(state, StornierungState::Abgelehnt { .. }));
    }

    #[test]
    fn validation_failure_rejects() {
        let state = StornierungState::default();
        let events = WimStornierungWorkflow::handle(&state, stornierung_cmd(false)).unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        assert!(matches!(state, StornierungState::Abgelehnt { .. }));
    }

    #[test]
    fn wrong_pid_is_rejected() {
        let state = StornierungState::default();
        let result = WimStornierungWorkflow::handle(
            &state,
            StornierungCommand::ReceiveOrdchg {
                pid: Pruefidentifikator::new(39_000).unwrap(), // Gas Sperrung PID — must be rejected
                sender: MarktpartnerCode::new("9900123456789"),
                receiver: MarktpartnerCode::new("4012345000023"),
                melo_id: MeLo::new("DE00056789012"),
                document_date: "20260101".to_owned(),
                message_ref: MessageRef::new("MSG-001"),
                cancelled_ref: None,
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(
            result.is_err(),
            "PID 39000 (Gas Sperrung) must be rejected by WiM Stornierung"
        );
    }

    #[test]
    fn deadline_on_active_rejects() {
        let state = StornierungState::default();
        let events = WimStornierungWorkflow::handle(&state, stornierung_cmd(true)).unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        let events = WimStornierungWorkflow::handle(
            &state,
            StornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "wim-stornierung-deadline".into(),
            },
        )
        .unwrap();
        let state = events.iter().fold(state, WimStornierungWorkflow::apply);
        assert!(matches!(state, StornierungState::Abgelehnt { .. }));
    }

    #[test]
    fn deadline_on_terminal_is_noop() {
        let terminal = StornierungState::Bestaetigt(StornierungData {
            sender: MarktpartnerCode::new("X"),
            receiver: MarktpartnerCode::new("Y"),
            melo_id: MeLo::new("Z"),
            document_date: String::new(),
            cancelled_ref: None,
        });
        let events = WimStornierungWorkflow::handle(
            &terminal,
            StornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: "late".into(),
            },
        )
        .unwrap();
        assert!(events.is_empty());
    }
}
