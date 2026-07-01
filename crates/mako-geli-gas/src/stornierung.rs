//! GeLi Gas / WiM Gas Stornierung — cancellation of supply-change and MSB-change
//! requests (PIDs 44022–44024).
//!
//! Per BDEW PID 3.3 xlsx: PIDs 44022–44024 are multi-domain — both **"GeLi Gas 2.0"**
//! (supply-change cancellation, LFN/LFA role) and **"WiM Gas"** (MSB-change cancellation,
//! MSB role). Per PID 4.0 xlsx: both **"GeLi Gas 2.0"** and **"AWH WiM Gas 2.0"**.
//! PID routing is currently performed by `WimGasModule` in `mako-wim-gas`.
//! Role-based routing for the GeLi Gas context (LFN/LFA roles) is a TODO.
//!
//! A market participant (LFN / LFA / GNB) may request cancellation of a previously
//! submitted Anmeldung, Abmeldung, or Kündigung by sending a UTILMD G message with
//! PID 44022 (Anfrage nach Stornierung) to the GNB. The GNB must respond within
//! **10 Werktage** with either:
//! - PID 44023 (Bestätigung Stornierung) — cancellation accepted; original process cancelled.
//! - PID 44024 (Ablehnung Stornierung) — cancellation rejected; original process continues.
//!
//! # Process flow
//!
//! ```text
//! LFN/LFA ──── 44022 Anfrage nach Stornierung ────►
//!         ◄─── 44023 Bestätigung                  ── GNB (within 10 Werktage)
//!          or ◄─ 44024 Ablehnung                  ──
//! ```
//!
//! # BGM qualifier semantics
//!
//! The BGM 1001 qualifier in PID 44022 encodes the **type of the original message**
//! being cancelled, not the Stornierung action itself:
//! - `E01` — cancelling an Anmeldung (Lieferbeginn Gas)
//! - `E02` — cancelling an Abmeldung (Lieferende Gas)
//! - `E35` — cancelling a Kündigung Lieferbeginn Gas
//!
//! The Stornierung action is indicated by `STS+7+E05` (Transaktionsgrund = Stornierung).
//!
//! # Regulatory basis
//!
//! - **BDEW UTILMD AHB Gas 1.1 / 1.2** — AHB rules for PIDs 44022–44024
//! - **BNetzA BK7-24-01-009** — Geschäftsprozesse Lieferantenwechsel Gas 3.0 (eff. 2025-09-24)
//! - **APERAK Frist: 10 Werktage** (same as all GeLi Gas processes)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name used as the `WorkflowId.name` in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "geli-gas-stornierung";

/// APERAK deadline label — 10-Werktage window per BK7-24-01-009.
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```text
/// let due = mako_engine::fristen::deadline_at_werktage(
///     received_at, 10, HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STORNIERUNG_APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STORNIERUNG_APERAK_WINDOW_LABEL: &str = "geli-gas-stornierung-aperak-10-werktage";

/// PIDs handled by the GeLi Gas Stornierung workflow (UTILMD G).
///
/// - 44022: Anfrage nach Stornierung (initiator → GNB)
/// - 44023: Bestätigung Stornierung  (GNB response — accepted)
/// - 44024: Ablehnung Stornierung    (GNB response — rejected)
pub const STORNIERUNG_PIDS: &[u32] = &[44022, 44023, 44024];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeliGasStornierungEvent {
    /// PID 44022 (Anfrage nach Stornierung) received and accepted for processing.
    StornierungReceived {
        /// Prüfidentifikator (must be 44022).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the message sender (LFN / LFA).
        sender: MarktpartnerCode,
        /// GLN of the GNB receiving the request.
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from IDE+24 (identifies the original process being cancelled).
        vorgang_id: MaLo,
        /// EDIFACT document date string (YYYYMMDDHHMMZZZ from DTM+137+303).
        document_date: String,
        /// EDIFACT message reference (UNH 0062).
        message_ref: MessageRef,
    },
    /// AHB profile validation passed — no rule violations.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// AHB profile validation failed — message rejected without an APERAK.
    ValidationFailed {
        /// Human-readable list of validation issue strings.
        errors: Vec<String>,
    },
    /// GNB dispatched a positive or negative APERAK response.
    ///
    /// Positive → process moves to `Completed`.
    /// Negative → process moves to `Rejected`.
    AperakDispatched {
        /// `true` = positive APERAK (PID 44023), `false` = negative (PID 44024).
        positive: bool,
        /// Rejection reason code or text (only meaningful when `positive = false`).
        reason: Option<String>,
    },
    /// Process terminated because the 10-Werktage APERAK deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for GeliGasStornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StornierungReceived { .. } => "GeliGasStornierungReceived",
            Self::ValidationPassed { .. } => "GeliGasStornierungValidationPassed",
            Self::ValidationFailed { .. } => "GeliGasStornierungValidationFailed",
            Self::AperakDispatched { .. } => "GeliGasStornierungAperakDispatched",
            Self::DeadlineExpired { .. } => "GeliGasStornierungDeadlineExpired",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at `StornierungReceived` time and carried throughout.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeliGasStornierungData {
    /// Prüfidentifikator of the inbound 44022 message.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sender (LFN / LFA initiating the cancellation).
    pub sender: MarktpartnerCode,
    /// GLN of the GNB receiving the request.
    pub receiver: MarktpartnerCode,
    /// Vorgangsnummer (IDE+24) identifying the original process to be cancelled.
    pub vorgang_id: MaLo,
    /// EDIFACT document date from DTM+137.
    pub document_date: String,
    /// EDIFACT message reference from the 44022 message.
    pub message_ref: Option<MessageRef>,
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GeLi Gas Stornierung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → AperakSent → Completed
///                                    ↘ Rejected
///     ↘ ValidationFailed → Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GeliGasStornierungState {
    /// No events yet.
    New,
    /// 44022 received; awaiting validation result.
    Initiated(GeliGasStornierungData),
    /// AHB validation passed; GNB must respond within 10 Werktage.
    ValidationPassed(GeliGasStornierungData),
    /// Positive APERAK (44023) dispatched; cancellation accepted.
    AperakSent(GeliGasStornierungData),
    /// Cancellation fully completed (positive APERAK sent, original process cancelled).
    Completed(GeliGasStornierungData),
    /// Process rejected — validation failure, negative APERAK, or deadline expiry.
    Rejected {
        /// Human-readable reason for the rejection.
        reason: String,
    },
}

impl Default for GeliGasStornierungState {
    fn default() -> Self {
        Self::New
    }
}

impl GeliGasStornierungState {
    /// Stable status string for metrics and projections.
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

/// Commands for the GeLi Gas Stornierung workflow.
///
/// All domain values must be pre-extracted and validated at the transport boundary.
/// `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum GeliGasStornierungCommand {
    /// Inbound UTILMD G PID 44022 accepted from the AS4 layer.
    ReceiveUtilmd {
        /// Must be 44022.
        pid: Pruefidentifikator,
        /// GLN of the sender (LFN / LFA).
        sender: MarktpartnerCode,
        /// GLN of the receiver (GNB).
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from IDE+24.
        vorgang_id: MaLo,
        /// EDIFACT document date from DTM+137.
        document_date: String,
        /// EDIFACT message reference from UNH.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed with no errors.
        validation_passed: bool,
        /// Human-readable validation error strings (empty when `validation_passed = true`).
        validation_errors: Vec<String>,
    },
    /// GNB dispatches a positive (44023) or negative (44024) APERAK response.
    ///
    /// Must be called within 10 Werktage of receiving the 44022 message.
    DispatchAperak {
        /// `true` for Bestätigung (44023), `false` for Ablehnung (44024).
        positive: bool,
        /// Rejection reason — required when `positive = false`.
        reason: Option<String>,
    },
    /// The 10-Werktage APERAK deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline (matches `STORNIERUNG_APERAK_WINDOW_LABEL`).
        label: Box<str>,
    },
}

impl CommandPayload for GeliGasStornierungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas Stornierung workflow (PIDs 44022–44024).
///
/// The GNB (Gasnetzbetreiber) is the process owner. The inbound message is PID 44022
/// (Anfrage nach Stornierung). The GNB responds with PID 44023 (Bestätigung) or
/// PID 44024 (Ablehnung) within 10 Werktage.
pub struct GeliGasStornierungWorkflow;

impl Workflow for GeliGasStornierungWorkflow {
    type State = GeliGasStornierungState;
    type Event = GeliGasStornierungEvent;
    type Command = GeliGasStornierungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                STORNIERUNG_APERAK_WINDOW_LABEL,
                GeliGasStornierungState::Initiated(_)
                | GeliGasStornierungState::ValidationPassed(_)
                | GeliGasStornierungState::AperakSent(_),
            ) => Some(GeliGasStornierungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GeliGasStornierungEvent::StornierungReceived {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                document_date,
                message_ref,
            } => GeliGasStornierungState::Initiated(GeliGasStornierungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                document_date: document_date.clone(),
                message_ref: Some(message_ref.clone()),
            }),

            GeliGasStornierungEvent::ValidationPassed { .. } => {
                if let GeliGasStornierungState::Initiated(data) = state {
                    GeliGasStornierungState::ValidationPassed(data)
                } else {
                    state
                }
            }

            GeliGasStornierungEvent::ValidationFailed { errors } => {
                GeliGasStornierungState::Rejected {
                    reason: errors.join("; "),
                }
            }

            GeliGasStornierungEvent::AperakDispatched { positive, reason } => match state {
                GeliGasStornierungState::ValidationPassed(data) => {
                    if *positive {
                        GeliGasStornierungState::Completed(data)
                    } else {
                        GeliGasStornierungState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "negative APERAK".to_owned()),
                        }
                    }
                }
                _ => state,
            },

            GeliGasStornierungEvent::DeadlineExpired { label, .. } => match state {
                GeliGasStornierungState::Completed(_)
                | GeliGasStornierungState::Rejected { .. } => state,
                _ => GeliGasStornierungState::Rejected {
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
            GeliGasStornierungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                vorgang_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GeliGasStornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported GeLi Gas Stornierung PID {pid} (expected one of: {STORNIERUNG_PIDS:?})",
                    )));
                }
                let mut events = vec![GeliGasStornierungEvent::StornierungReceived {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    vorgang_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GeliGasStornierungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GeliGasStornierungEvent::ValidationFailed {
                        errors: validation_errors,
                    });
                }
                Ok(WorkflowOutput::events(events))
            }

            GeliGasStornierungCommand::DispatchAperak { positive, reason } => {
                if !matches!(state, GeliGasStornierungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    GeliGasStornierungEvent::AperakDispatched { positive, reason },
                ]))
            }

            GeliGasStornierungCommand::TimeoutExpired { deadline_id, label } => match state {
                GeliGasStornierungState::Completed(_)
                | GeliGasStornierungState::Rejected { .. } => Ok(WorkflowOutput::events(vec![])),
                _ => Ok(WorkflowOutput::events(vec![
                    GeliGasStornierungEvent::DeadlineExpired { deadline_id, label },
                ])),
            },
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::ids::DeadlineId;

    use super::*;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).expect("valid PID")
    }

    fn stornierung_cmd(p: u32, validation_passed: bool) -> GeliGasStornierungCommand {
        GeliGasStornierungCommand::ReceiveUtilmd {
            pid: pid(p),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_date: "20251001000000+00".to_owned(),
            message_ref: MessageRef::new("00001"),
            validation_passed,
            validation_errors: if validation_passed {
                vec![]
            } else {
                vec!["missing DTM".to_owned()]
            },
        }
    }

    #[test]
    fn receive_valid_44022_transitions_to_validation_passed() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationPassed"
        );
        assert!(matches!(
            output.events[0],
            GeliGasStornierungEvent::StornierungReceived { .. }
        ));
        assert!(matches!(
            output.events[1],
            GeliGasStornierungEvent::ValidationPassed { .. }
        ));

        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GeliGasStornierungState::ValidationPassed(_)
        ));
    }

    #[test]
    fn receive_invalid_44022_transitions_to_rejected() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, false)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationFailed"
        );

        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(state, GeliGasStornierungState::Rejected { .. }));
    }

    #[test]
    fn dispatch_positive_aperak_completes_process() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(
            state,
            GeliGasStornierungState::ValidationPassed(_)
        ));

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(
            matches!(state, GeliGasStornierungState::Completed(_)),
            "positive APERAK must complete stornierung; got: {state:?}"
        );
    }

    #[test]
    fn dispatch_negative_aperak_rejects_process() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::DispatchAperak {
                positive: false,
                reason: Some("Vorgang nicht stornierbar".to_owned()),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(state, GeliGasStornierungState::Rejected { .. }));
    }

    #[test]
    fn deadline_expired_rejects_initiated_process() {
        let state = GeliGasStornierungState::New;
        let output =
            GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);

        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GeliGasStornierungWorkflow::apply);
        assert!(matches!(state, GeliGasStornierungState::Rejected { .. }));
    }

    #[test]
    fn reject_unsupported_pid() {
        let state = GeliGasStornierungState::New;
        let result = GeliGasStornierungWorkflow::handle(&state, stornierung_cmd(44022 - 1, true));
        assert!(result.is_err(), "PID 44021 must be rejected");
    }

    #[test]
    fn deadline_is_noop_for_completed_process() {
        let data = GeliGasStornierungData {
            pruefidentifikator: pid(44022),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_date: "20251001000000+00".to_owned(),
            message_ref: None,
        };
        let state = GeliGasStornierungState::Completed(data);
        let output = GeliGasStornierungWorkflow::handle(
            &state,
            GeliGasStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            output.events.is_empty(),
            "deadline must be no-op for Completed state"
        );
    }
}
