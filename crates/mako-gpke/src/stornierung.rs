//! GPKE Stornierung — cancellation of supplier-change requests (PIDs 55022–55024).
//!
//! A Lieferant (LFN) may request cancellation of a previously submitted
//! Anmeldung (55001), Abmeldung (55002), or Kündigung (55016) by sending a
//! UTILMD Strom message with PID 55022 (Anfrage nach Stornierung) to the NB.
//! The NB must respond within **24 wall-clock hours** with either:
//! - PID 55023 (Bestätigung Stornierung) — cancellation accepted; original process cancelled.
//! - PID 55024 (Ablehnung Stornierung) — cancellation rejected; original process continues.
//!
//! # Process flow
//!
//! ```text
//! LFN ──── 55022 Anfrage nach Stornierung ────►
//!     ◄─── 55023 Bestätigung                   ── NB (within 24h)
//!      or ◄─ 55024 Ablehnung                   ──
//! ```
//!
//! # BGM qualifier semantics
//!
//! The BGM 1001 qualifier in PID 55022 encodes the **type of the original message**
//! being cancelled, not the Stornierung action itself:
//! - `E01` — cancelling an Anmeldung Lieferbeginn (55001)
//! - `E02` — cancelling an Abmeldung Lieferende (55002)
//! - `E35` — cancelling a Kündigung Lieferbeginn (55016)
//!
//! The Stornierung action is indicated by `STS+7+E05` (Transaktionsgrund = Stornierung).
//!
//! # Regulatory basis
//!
//! - **BDEW UTILMD AHB Strom 2.1 / 2.2** — AHB rules for PIDs 55022–55024 (GPKE Teil 4)
//! - **BNetzA BK6-24-174** — Geschäftsprozesse Kundenlieferantenwechsel Strom (LFW24)
//! - **APERAK Frist: 24 Stunden** (wall-clock, same as all GPKE processes — BK6-22-024 §5)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Stable workflow name used as the `WorkflowId.name` in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "gpke-stornierung";

/// APERAK deadline label — 24-hour window per BK6-22-024 §5.
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```text
/// let due = received_at + time::Duration::hours(24);
/// let deadline = Deadline::new(process.stream_id().clone(), ..., STORNIERUNG_APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const STORNIERUNG_APERAK_WINDOW_LABEL: &str = "gpke-stornierung-aperak-24h";

/// PIDs handled by the GPKE Stornierung workflow (UTILMD Strom).
///
/// - 55022: Anfrage nach Stornierung (LFN → NB)
/// - 55023: Bestätigung Stornierung  (NB response — accepted)
/// - 55024: Ablehnung Stornierung    (NB response — rejected)
pub const STORNIERUNG_PIDS: &[u32] = &[55022, 55023, 55024];

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Stornierung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GpkeStornierungEvent {
    /// PID 55022 (Anfrage nach Stornierung) received and accepted for processing.
    StornierungReceived {
        /// Prüfidentifikator (must be 55022).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the message sender (LFN).
        sender: MarktpartnerCode,
        /// GLN of the NB receiving the request.
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from IDE+Z19 (identifies the original process being cancelled).
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
    /// NB dispatched a positive or negative APERAK response.
    ///
    /// Positive → process moves to `Completed`.
    /// Negative → process moves to `Rejected`.
    AperakDispatched {
        /// `true` = positive APERAK (PID 55023), `false` = negative (PID 55024).
        positive: bool,
        /// Rejection reason code or text (only meaningful when `positive = false`).
        reason: Option<String>,
    },
    /// Process terminated because the 24-hour APERAK deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for GpkeStornierungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::StornierungReceived { .. } => "GpkeStornierungReceived",
            Self::ValidationPassed { .. } => "GpkeStornierungValidationPassed",
            Self::ValidationFailed { .. } => "GpkeStornierungValidationFailed",
            Self::AperakDispatched { .. } => "GpkeStornierungAperakDispatched",
            Self::DeadlineExpired { .. } => "GpkeStornierungDeadlineExpired",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at `StornierungReceived` time and carried throughout.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GpkeStornierungData {
    /// Prüfidentifikator of the inbound 55022 message.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sender (LFN initiating the cancellation).
    pub sender: MarktpartnerCode,
    /// GLN of the NB receiving the request.
    pub receiver: MarktpartnerCode,
    /// Vorgangsnummer (IDE+Z19) identifying the original process to be cancelled.
    pub vorgang_id: MaLo,
    /// EDIFACT document date from DTM+137.
    pub document_date: String,
    /// EDIFACT message reference from the 55022 message.
    pub message_ref: Option<MessageRef>,
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GPKE Stornierung process stream.
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
pub enum GpkeStornierungState {
    /// No events yet.
    New,
    /// 55022 received; awaiting validation result.
    Initiated(GpkeStornierungData),
    /// AHB validation passed; NB must respond within 24 hours.
    ValidationPassed(GpkeStornierungData),
    /// Positive APERAK (55023) dispatched; cancellation accepted.
    AperakSent(GpkeStornierungData),
    /// Cancellation fully completed (positive APERAK sent, original process cancelled).
    Completed(GpkeStornierungData),
    /// Process rejected — validation failure, negative APERAK, or deadline expiry.
    Rejected {
        /// Human-readable reason for the rejection.
        reason: String,
    },
}

impl Default for GpkeStornierungState {
    fn default() -> Self {
        Self::New
    }
}

impl GpkeStornierungState {
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

/// Commands for the GPKE Stornierung workflow.
///
/// All domain values must be pre-extracted and validated at the transport boundary.
/// `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum GpkeStornierungCommand {
    /// Inbound UTILMD Strom PID 55022 accepted from the AS4 layer.
    ReceiveUtilmd {
        /// Must be 55022.
        pid: Pruefidentifikator,
        /// GLN of the sender (LFN requesting cancellation).
        sender: MarktpartnerCode,
        /// GLN of the receiver (NB).
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from IDE+Z19.
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
    /// NB dispatches a positive (55023) or negative (55024) APERAK response.
    ///
    /// Must be called within 24 hours of receiving the 55022 message.
    DispatchAperak {
        /// `true` for Bestätigung (55023), `false` for Ablehnung (55024).
        positive: bool,
        /// Rejection reason — required when `positive = false`.
        reason: Option<String>,
    },
    /// The 24-hour APERAK deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline (matches `STORNIERUNG_APERAK_WINDOW_LABEL`).
        label: Box<str>,
    },
}

impl CommandPayload for GpkeStornierungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Stornierung workflow (PIDs 55022–55024).
///
/// The NB (Netzbetreiber) is the process owner. The inbound message is PID 55022
/// (Anfrage nach Stornierung). The NB responds with PID 55023 (Bestätigung) or
/// PID 55024 (Ablehnung) within **24 wall-clock hours** per BK6-22-024 §5.
pub struct GpkeStornierungWorkflow;

impl Workflow for GpkeStornierungWorkflow {
    type State = GpkeStornierungState;
    type Event = GpkeStornierungEvent;
    type Command = GpkeStornierungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                STORNIERUNG_APERAK_WINDOW_LABEL,
                GpkeStornierungState::Initiated(_)
                | GpkeStornierungState::ValidationPassed(_)
                | GpkeStornierungState::AperakSent(_),
            ) => Some(GpkeStornierungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GpkeStornierungEvent::StornierungReceived {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                document_date,
                message_ref,
            } => GpkeStornierungState::Initiated(GpkeStornierungData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                document_date: document_date.clone(),
                message_ref: Some(message_ref.clone()),
            }),

            GpkeStornierungEvent::ValidationPassed { .. } => {
                if let GpkeStornierungState::Initiated(data) = state {
                    GpkeStornierungState::ValidationPassed(data)
                } else {
                    state
                }
            }

            GpkeStornierungEvent::ValidationFailed { errors } => GpkeStornierungState::Rejected {
                reason: errors.join("; "),
            },

            GpkeStornierungEvent::AperakDispatched { positive, reason } => match state {
                GpkeStornierungState::ValidationPassed(data) => {
                    if *positive {
                        GpkeStornierungState::Completed(data)
                    } else {
                        GpkeStornierungState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "negative APERAK".to_owned()),
                        }
                    }
                }
                _ => state,
            },

            GpkeStornierungEvent::DeadlineExpired { label, .. } => match state {
                GpkeStornierungState::Completed(_) | GpkeStornierungState::Rejected { .. } => state,
                _ => GpkeStornierungState::Rejected {
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
            GpkeStornierungCommand::ReceiveUtilmd {
                pid,
                sender,
                receiver,
                vorgang_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GpkeStornierungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported GPKE Stornierung PID {pid} (expected one of: {STORNIERUNG_PIDS:?})",
                    )));
                }
                let mut events = vec![GpkeStornierungEvent::StornierungReceived {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    vorgang_id,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(GpkeStornierungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GpkeStornierungEvent::ValidationFailed {
                        errors: validation_errors,
                    });
                }
                Ok(WorkflowOutput::events(events))
            }

            GpkeStornierungCommand::DispatchAperak { positive, reason } => {
                let data = match state {
                    GpkeStornierungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.status_str(),
                        ));
                    }
                };
                // APERAK AHB 1.0 §2.4: Strom UTILMD always requires APERAK (BGM+312 or BGM+313).
                // Strom UTILMD (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
                let mut aperak_payload = serde_json::json!({
                    "sender":        data.receiver.as_str(),
                    "receiver":      data.sender.as_str(),
                    "pid":           29001_u32,
                    "document_code": if positive { "312" } else { "313" },
                });
                if !positive {
                    aperak_payload["error_code"] =
                        serde_json::Value::String(mako_engine::erc::codes::Z29.to_owned());
                }
                if let Some(ref r) = reason {
                    aperak_payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox = vec![
                    PendingOutbox::new("APERAK", data.sender.as_str(), aperak_payload).caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![GpkeStornierungEvent::AperakDispatched { positive, reason }],
                    outbox,
                ))
            }

            GpkeStornierungCommand::TimeoutExpired { deadline_id, label } => match state {
                GpkeStornierungState::Completed(_) | GpkeStornierungState::Rejected { .. } => {
                    Ok(WorkflowOutput::events(vec![]))
                }
                _ => Ok(WorkflowOutput::events(vec![
                    GpkeStornierungEvent::DeadlineExpired { deadline_id, label },
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

    fn stornierung_cmd(p: u32, validation_passed: bool) -> GpkeStornierungCommand {
        GpkeStornierungCommand::ReceiveUtilmd {
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
    fn receive_valid_55022_transitions_to_validation_passed() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationPassed"
        );
        assert!(matches!(
            output.events[0],
            GpkeStornierungEvent::StornierungReceived { .. }
        ));
        assert!(matches!(
            output.events[1],
            GpkeStornierungEvent::ValidationPassed { .. }
        ));

        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::ValidationPassed(_)));
    }

    #[test]
    fn receive_invalid_55022_transitions_to_rejected() {
        let state = GpkeStornierungState::New;
        let output =
            GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, false)).unwrap();
        assert_eq!(
            output.events.len(),
            2,
            "StornierungReceived + ValidationFailed"
        );

        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::Rejected { .. }));
    }

    #[test]
    fn dispatch_positive_aperak_completes_process() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::ValidationPassed(_)));

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::DispatchAperak {
                positive: true,
                reason: None,
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(
            matches!(state, GpkeStornierungState::Completed(_)),
            "positive APERAK must complete stornierung; got: {state:?}"
        );
    }

    #[test]
    fn dispatch_negative_aperak_rejects_process() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::DispatchAperak {
                positive: false,
                reason: Some("Vorgang nicht stornierbar".to_owned()),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::Rejected { .. }));
    }

    #[test]
    fn deadline_expired_rejects_initiated_process() {
        let state = GpkeStornierungState::New;
        let output = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55022, true)).unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);

        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: STORNIERUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = output
            .events
            .iter()
            .fold(state, GpkeStornierungWorkflow::apply);
        assert!(matches!(state, GpkeStornierungState::Rejected { .. }));
    }

    #[test]
    fn reject_unsupported_pid() {
        let state = GpkeStornierungState::New;
        let result = GpkeStornierungWorkflow::handle(&state, stornierung_cmd(55021, true));
        assert!(result.is_err(), "PID 55021 must be rejected");
    }

    #[test]
    fn deadline_is_noop_for_completed_process() {
        let data = GpkeStornierungData {
            pruefidentifikator: pid(55022),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9907317000007"),
            vorgang_id: MaLo::new("STORNO0000A"),
            document_date: "20251001000000+00".to_owned(),
            message_ref: None,
        };
        let state = GpkeStornierungState::Completed(data);
        let output = GpkeStornierungWorkflow::handle(
            &state,
            GpkeStornierungCommand::TimeoutExpired {
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
