//! GPKE Anfrage Daten der individuellen Bestellung — UTILMD Strom PID 55555
//! (GPKE Teil 4, BK6-24-174).
//!
//! The Lieferant (LFN) sends this UTILMD message to the Netzbetreiber (NB)
//! to query data associated with a specific individual order / Vorgang.
//! The NB must respond within **24 wall-clock hours** (BK6-22-024 §5) with
//! either the requested data or a reasoned rejection.
//!
//! # Process overview
//!
//! ```text
//! LFN ──── UTILMD 55555 Anfrage Daten ────► NB
//!                                            ↓ (within 24 h per BK6-22-024)
//!                    ◄──── data response / rejection ────
//! ```
//!
//! # BGM and STS qualifier semantics
//!
//! PID 55555 always uses BGM qualifier `E03` (Änderungsmeldung).
//! The `STS` segment (DE 9015 Bearbeitungsstatus) refines the request:
//!
//! | STS 9015 | Meaning |
//! |----------|---------|
//! | `E07`    | Anfrage bezieht sich auf einen aktiven / bestätigten Vorgang |
//! | `E08`    | Anfrage bezieht sich auf einen noch nicht bestätigten Vorgang |
//!
//! The `IDE+Z19` object ID identifies the Vorgangsnummer whose data is
//! being requested.  The `RFF+Z13` provides a correlating reference.
//!
//! # Regulatory basis
//!
//! - **BDEW UTILMD AHB Strom S2.1 / S2.2** (profiles `fv20251001`, `fv20261001`)
//! - **BNetzA BK6-24-174** — GPKE Teil 4 (eff. 2025-06-06)
//! - **APERAK Frist: 24 Stunden** wall-clock (BK6-22-024 §5, same as all GPKE
//!   processes)

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// The single UTILMD PID handled by [`GpkeAnfrageBestellungWorkflow`].
///
/// 55555 = "Anfrage Daten der individuellen Bestellung" (LFN → NB, GPKE Teil 4).
pub const ANFRAGE_PID: u32 = 55555;

/// Stable workflow name used as `WorkflowId.name` in the process registry.
pub const WORKFLOW_NAME: &str = "gpke-anfrage-bestellung";

/// Deadline label for the 24-hour response window (BK6-22-024 §5).
///
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```text
/// let due = mako_engine::fristen::add_hours(received_at, 24);
/// let dl = Deadline::new(stream_id, …, ANFRAGE_WINDOW_LABEL, due);
/// deadline_store.register(&dl).await?;
/// ```
pub const ANFRAGE_WINDOW_LABEL: &str = "gpke-anfrage-bestellung-24h";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Anfrage-Daten-der-individuellen-Bestellung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AnfrageBestellungEvent {
    /// UTILMD PID 55555 received and accepted for processing.
    AnfrageErhalten {
        /// Must be 55555.
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the querying Lieferant (LFN).
        sender: MarktpartnerCode,
        /// GLN of the Netzbetreiber receiving the query.
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from `IDE+Z19` — identifies which order is being queried.
        vorgang_id: MaLo,
        /// `STS` DE 9015 qualifier from the message (`"E07"` or `"E08"`).
        bearbeitungsstatus: String,
        /// EDIFACT document date from `DTM+137` (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference (UNH 0062).
        message_ref: MessageRef,
    },
    /// AHB profile validation passed — no rule violations.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// AHB profile validation failed — the Anfrage is rejected.
    ValidationFailed {
        /// Human-readable list of validation error strings.
        errors: Vec<String>,
    },
    /// NB dispatched a response to the Anfrage (data provided or rejection).
    ResponseDispatched {
        /// `true` = data was provided; `false` = request rejected.
        data_provided: bool,
        /// Reason for rejection (only set when `data_provided = false`).
        reason: Option<String>,
    },
    /// The 24-hour deadline expired before the NB dispatched a response.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline.
        label: Box<str>,
    },
}

impl EventPayload for AnfrageBestellungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "AnfrageBestellungErhalten",
            Self::ValidationPassed { .. } => "AnfrageBestellungValidationPassed",
            Self::ValidationFailed { .. } => "AnfrageBestellungValidationFailed",
            Self::ResponseDispatched { .. } => "AnfrageBestellungResponseDispatched",
            Self::DeadlineExpired { .. } => "AnfrageBestellungDeadlineExpired",
        }
    }
}

// ── Domain data ───────────────────────────────────────────────────────────────

/// Business data recorded at `AnfrageErhalten` time and carried forward.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnfrageData {
    /// Prüfidentifikator — always 55555 at construction time.
    pub pruefidentifikator: Pruefidentifikator,
    /// GLN of the sender (LFN).
    pub sender: MarktpartnerCode,
    /// GLN of the receiver (NB).
    pub receiver: MarktpartnerCode,
    /// Vorgangsnummer from `IDE+Z19` identifying the queried order.
    pub vorgang_id: MaLo,
    /// STS DE 9015 Bearbeitungsstatus qualifier (`"E07"` or `"E08"`).
    pub bearbeitungsstatus: String,
    /// EDIFACT document date from `DTM+137`.
    pub document_date: String,
    /// EDIFACT message reference from UNH 0062.
    pub message_ref: Option<MessageRef>,
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Current state of a GPKE "Anfrage Daten der individuellen Bestellung" stream.
///
/// # Lifecycle
///
/// ```text
/// New → Initiated → ValidationPassed → ResponseDispatched (Completed)
///     ↘ ValidationFailed → Rejected
///                        ↘ DeadlineExpired → Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AnfrageBestellungState {
    /// No events yet. Stream exists but process has not started.
    New,
    /// PID 55555 received; AHB validation result not yet recorded.
    Initiated(AnfrageData),
    /// Validation passed; NB must respond within 24 hours.
    ValidationPassed(AnfrageData),
    /// NB dispatched a data response. Process complete (accepted or rejected by NB).
    ResponseDispatched(AnfrageData),
    /// Process rejected — validation failure or deadline expiry.
    Rejected {
        /// Human-readable reason for diagnostics and audit.
        reason: String,
    },
}

impl Default for AnfrageBestellungState {
    fn default() -> Self {
        Self::New
    }
}

impl AnfrageBestellungState {
    /// Stable status string for metrics and projections.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Initiated(_) => "Initiated",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::ResponseDispatched(_) => "ResponseDispatched",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Anfrage-Daten workflow.
///
/// All domain values must be pre-extracted and validated at the transport
/// boundary (AS4 adapter). `Workflow::handle()` is pure — no I/O.
#[derive(Clone)]
pub enum AnfrageBestellungCommand {
    /// Inbound UTILMD PID 55555 received from the AS4 layer.
    ///
    /// The adapter constructs this command after extracting domain fields
    /// from the raw EDIFACT message and invoking AHB profile validation.
    ReceiveAnfrage {
        /// Must be 55555.
        pid: Pruefidentifikator,
        /// GLN of the sender (LFN initiating the query).
        sender: MarktpartnerCode,
        /// GLN of the receiver (NB).
        receiver: MarktpartnerCode,
        /// Vorgangsnummer from `IDE+Z19` — identifies the queried order.
        vorgang_id: MaLo,
        /// STS DE 9015 Bearbeitungsstatus qualifier from the message.
        bearbeitungsstatus: String,
        /// EDIFACT document date from `DTM+137` (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference from UNH 0062.
        message_ref: MessageRef,
        /// `true` if AHB profile validation passed with no errors.
        validation_passed: bool,
        /// Human-readable validation error strings (empty when `validation_passed = true`).
        validation_errors: Vec<String>,
    },
    /// NB dispatches a response — provides the requested data or rejects the query.
    ///
    /// Must be called within **24 wall-clock hours** of receiving PID 55555
    /// (BK6-22-024 §5). The ERP determines whether data is provided or the
    /// request is rejected.
    DispatchResponse {
        /// `true` = NB provides the requested data; `false` = NB rejects.
        data_provided: bool,
        /// Rejection reason — required when `data_provided = false`.
        reason: Option<String>,
    },
    /// The 24-hour deadline fired before the NB dispatched a response.
    ///
    /// The scheduler constructs this command from `Deadline::label()` matching
    /// [`ANFRAGE_WINDOW_LABEL`]. The workflow records `DeadlineExpired` and
    /// transitions to `Rejected`.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label of the expired deadline (must match [`ANFRAGE_WINDOW_LABEL`]).
        label: Box<str>,
    },
}

impl CommandPayload for AnfrageBestellungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Anfrage Daten der individuellen Bestellung workflow (PID 55555).
///
/// The NB (Netzbetreiber) is the process owner.  The inbound message is
/// PID 55555 (LFN → NB data query).  The NB must provide the data or reject
/// the query within **24 wall-clock hours** per BK6-22-024 §5.
///
/// Governed by **BK6-24-174** (GPKE Teil 4, eff. 2025-06-06).
pub struct GpkeAnfrageBestellungWorkflow;

impl Workflow for GpkeAnfrageBestellungWorkflow {
    type State = AnfrageBestellungState;
    type Event = AnfrageBestellungEvent;
    type Command = AnfrageBestellungCommand;

    /// Deadline compensation: fire `TimeoutExpired` when the 24-hour window
    /// lapses without the NB having dispatched a response.
    ///
    /// Only non-terminal states (`Initiated`, `ValidationPassed`) will trigger
    /// a `TimeoutExpired` command; terminal states (`ResponseDispatched`,
    /// `Rejected`) absorb the late-firing deadline as a no-op.
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ANFRAGE_WINDOW_LABEL,
                AnfrageBestellungState::Initiated(_) | AnfrageBestellungState::ValidationPassed(_),
            ) => Some(AnfrageBestellungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AnfrageBestellungEvent::AnfrageErhalten {
                pruefidentifikator,
                sender,
                receiver,
                vorgang_id,
                bearbeitungsstatus,
                document_date,
                message_ref,
            } => AnfrageBestellungState::Initiated(AnfrageData {
                pruefidentifikator: *pruefidentifikator,
                sender: sender.clone(),
                receiver: receiver.clone(),
                vorgang_id: vorgang_id.clone(),
                bearbeitungsstatus: bearbeitungsstatus.clone(),
                document_date: document_date.clone(),
                message_ref: Some(message_ref.clone()),
            }),

            AnfrageBestellungEvent::ValidationPassed { .. } => match state {
                AnfrageBestellungState::Initiated(data) => {
                    AnfrageBestellungState::ValidationPassed(data)
                }
                other => other,
            },

            AnfrageBestellungEvent::ValidationFailed { errors } => {
                AnfrageBestellungState::Rejected {
                    reason: errors.join("; "),
                }
            }

            AnfrageBestellungEvent::ResponseDispatched {
                data_provided,
                reason,
            } => match state {
                AnfrageBestellungState::ValidationPassed(data) => {
                    if *data_provided {
                        AnfrageBestellungState::ResponseDispatched(data)
                    } else {
                        AnfrageBestellungState::Rejected {
                            reason: reason
                                .clone()
                                .unwrap_or_else(|| "Anfrage abgelehnt".to_owned()),
                        }
                    }
                }
                other => other,
            },

            AnfrageBestellungEvent::DeadlineExpired { label, .. } => match state {
                AnfrageBestellungState::ResponseDispatched(_)
                | AnfrageBestellungState::Rejected { .. } => state,
                _ => AnfrageBestellungState::Rejected {
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
            AnfrageBestellungCommand::ReceiveAnfrage {
                pid,
                sender,
                receiver,
                vorgang_id,
                bearbeitungsstatus,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, AnfrageBestellungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if pid.as_u32() != ANFRAGE_PID {
                    return Err(WorkflowError::rejected(format!(
                        "unsupported PID {pid} for GpkeAnfrageBestellungWorkflow \
                         (expected {ANFRAGE_PID})",
                    )));
                }
                let mut events = vec![AnfrageBestellungEvent::AnfrageErhalten {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    vorgang_id,
                    bearbeitungsstatus,
                    document_date,
                    message_ref: message_ref.clone(),
                }];
                if validation_passed {
                    events.push(AnfrageBestellungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(AnfrageBestellungEvent::ValidationFailed {
                        errors: validation_errors,
                    });
                }
                Ok(WorkflowOutput::events(events))
            }

            AnfrageBestellungCommand::DispatchResponse {
                data_provided,
                reason,
            } => {
                if !matches!(state, AnfrageBestellungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                }
                Ok(WorkflowOutput::events(vec![
                    AnfrageBestellungEvent::ResponseDispatched {
                        data_provided,
                        reason,
                    },
                ]))
            }

            AnfrageBestellungCommand::TimeoutExpired { deadline_id, label } => match state {
                AnfrageBestellungState::ResponseDispatched(_)
                | AnfrageBestellungState::Rejected { .. } => Ok(WorkflowOutput::events(vec![])),
                _ => Ok(WorkflowOutput::events(vec![
                    AnfrageBestellungEvent::DeadlineExpired { deadline_id, label },
                ])),
            },
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{
        ids::DeadlineId,
        types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
        workflow::{Workflow, WorkflowOutput},
    };

    use super::*;

    fn pid() -> Pruefidentifikator {
        Pruefidentifikator::new(55555).unwrap()
    }
    fn sender() -> MarktpartnerCode {
        MarktpartnerCode::new("4012345000023")
    }
    fn receiver() -> MarktpartnerCode {
        MarktpartnerCode::new("9900357000004")
    }
    fn vorgang_id() -> MaLo {
        MaLo::new("DE0000000000000000000000000012345")
    }
    fn msg_ref() -> MessageRef {
        MessageRef::new("MSG-ANFR-001")
    }

    fn receive_valid() -> AnfrageBestellungCommand {
        AnfrageBestellungCommand::ReceiveAnfrage {
            pid: pid(),
            sender: sender(),
            receiver: receiver(),
            vorgang_id: vorgang_id(),
            bearbeitungsstatus: "E07".to_owned(),
            document_date: "20250115".to_owned(),
            message_ref: msg_ref(),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn receive_invalid() -> AnfrageBestellungCommand {
        AnfrageBestellungCommand::ReceiveAnfrage {
            pid: pid(),
            sender: sender(),
            receiver: receiver(),
            vorgang_id: vorgang_id(),
            bearbeitungsstatus: "E08".to_owned(),
            document_date: "20250115".to_owned(),
            message_ref: msg_ref(),
            validation_passed: false,
            validation_errors: vec!["segment BGM missing qualifier E03".to_owned()],
        }
    }

    // ── ReceiveAnfrage ────────────────────────────────────────────────────

    #[test]
    fn receive_valid_transitions_to_validation_passed() {
        let state = AnfrageBestellungState::New;
        let result = GpkeAnfrageBestellungWorkflow::handle(&state, receive_valid()).unwrap();
        let WorkflowOutput { events, .. } = result;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            AnfrageBestellungEvent::AnfrageErhalten { .. }
        ));
        assert!(matches!(
            events[1],
            AnfrageBestellungEvent::ValidationPassed { .. }
        ));
        // Apply and verify final state.
        let final_state = events
            .iter()
            .fold(state, GpkeAnfrageBestellungWorkflow::apply);
        assert!(matches!(
            final_state,
            AnfrageBestellungState::ValidationPassed(_)
        ));
    }

    #[test]
    fn receive_invalid_transitions_to_rejected() {
        let state = AnfrageBestellungState::New;
        let result = GpkeAnfrageBestellungWorkflow::handle(&state, receive_invalid()).unwrap();
        let WorkflowOutput { events, .. } = result;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            AnfrageBestellungEvent::AnfrageErhalten { .. }
        ));
        assert!(matches!(
            events[1],
            AnfrageBestellungEvent::ValidationFailed { .. }
        ));
        let final_state = events
            .iter()
            .fold(state, GpkeAnfrageBestellungWorkflow::apply);
        assert!(matches!(
            final_state,
            AnfrageBestellungState::Rejected { .. }
        ));
    }

    #[test]
    fn receive_wrong_pid_is_rejected() {
        let bad_pid = Pruefidentifikator::new(55001).unwrap();
        let cmd = AnfrageBestellungCommand::ReceiveAnfrage {
            pid: bad_pid,
            sender: sender(),
            receiver: receiver(),
            vorgang_id: vorgang_id(),
            bearbeitungsstatus: "E07".to_owned(),
            document_date: "20250115".to_owned(),
            message_ref: msg_ref(),
            validation_passed: true,
            validation_errors: vec![],
        };
        let err =
            GpkeAnfrageBestellungWorkflow::handle(&AnfrageBestellungState::New, cmd).unwrap_err();
        assert!(err.to_string().contains("55001"));
    }

    #[test]
    fn receive_in_non_new_state_is_invalid_state_error() {
        // Put state into Initiated first.
        let init_state = AnfrageBestellungState::New;
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&init_state, receive_valid()).unwrap();
        let initiated = events
            .iter()
            .fold(init_state, GpkeAnfrageBestellungWorkflow::apply);
        // Trying to receive again must fail with invalid_state.
        let err = GpkeAnfrageBestellungWorkflow::handle(&initiated, receive_valid()).unwrap_err();
        assert!(err.to_string().contains("New"));
    }

    // ── DispatchResponse ──────────────────────────────────────────────────

    fn make_validation_passed_state() -> AnfrageBestellungState {
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&AnfrageBestellungState::New, receive_valid())
                .unwrap();
        events.iter().fold(
            AnfrageBestellungState::New,
            GpkeAnfrageBestellungWorkflow::apply,
        )
    }

    #[test]
    fn dispatch_data_provided_transitions_to_response_dispatched() {
        let state = make_validation_passed_state();
        let cmd = AnfrageBestellungCommand::DispatchResponse {
            data_provided: true,
            reason: None,
        };
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AnfrageBestellungEvent::ResponseDispatched {
                data_provided: true,
                ..
            }
        ));
        let final_state = events
            .iter()
            .fold(state, GpkeAnfrageBestellungWorkflow::apply);
        assert!(matches!(
            final_state,
            AnfrageBestellungState::ResponseDispatched(_)
        ));
    }

    #[test]
    fn dispatch_rejection_transitions_to_rejected() {
        let state = make_validation_passed_state();
        let cmd = AnfrageBestellungCommand::DispatchResponse {
            data_provided: false,
            reason: Some("Vorgang nicht gefunden".to_owned()),
        };
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AnfrageBestellungEvent::ResponseDispatched {
                data_provided: false,
                ..
            }
        ));
        let final_state = events
            .iter()
            .fold(state, GpkeAnfrageBestellungWorkflow::apply);
        assert!(matches!(
            final_state,
            AnfrageBestellungState::Rejected { .. }
        ));
    }

    #[test]
    fn dispatch_response_from_wrong_state_returns_error() {
        // Must be in ValidationPassed — Initiated is not enough.
        let initiated = {
            let WorkflowOutput { events, .. } = GpkeAnfrageBestellungWorkflow::handle(
                &AnfrageBestellungState::New,
                receive_invalid(),
            )
            .unwrap();
            events.iter().fold(
                AnfrageBestellungState::New,
                GpkeAnfrageBestellungWorkflow::apply,
            )
        };
        // State is Rejected (validation failed) — DispatchResponse must fail.
        let err = GpkeAnfrageBestellungWorkflow::handle(
            &initiated,
            AnfrageBestellungCommand::DispatchResponse {
                data_provided: true,
                reason: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("ValidationPassed"));
    }

    // ── TimeoutExpired ────────────────────────────────────────────────────

    #[test]
    fn timeout_in_validation_passed_transitions_to_rejected() {
        let state = make_validation_passed_state();
        let cmd = AnfrageBestellungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANFRAGE_WINDOW_LABEL.into(),
        };
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&state, cmd).unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(
            events[0],
            AnfrageBestellungEvent::DeadlineExpired { .. }
        ));
        let final_state = events
            .iter()
            .fold(state, GpkeAnfrageBestellungWorkflow::apply);
        assert!(matches!(
            final_state,
            AnfrageBestellungState::Rejected { reason } if reason.contains("deadline")
        ));
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        // ResponseDispatched is terminal — TimeoutExpired must produce no events.
        let dispatched = {
            let state = make_validation_passed_state();
            let WorkflowOutput { events, .. } = GpkeAnfrageBestellungWorkflow::handle(
                &state,
                AnfrageBestellungCommand::DispatchResponse {
                    data_provided: true,
                    reason: None,
                },
            )
            .unwrap();
            events
                .iter()
                .fold(state, GpkeAnfrageBestellungWorkflow::apply)
        };
        let cmd = AnfrageBestellungCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANFRAGE_WINDOW_LABEL.into(),
        };
        let WorkflowOutput { events, .. } =
            GpkeAnfrageBestellungWorkflow::handle(&dispatched, cmd).unwrap();
        assert!(events.is_empty(), "no events emitted in terminal state");
    }

    // ── on_deadline ───────────────────────────────────────────────────────

    #[test]
    fn on_deadline_fires_for_initiated_and_validation_passed() {
        use mako_engine::deadline::Deadline;
        use mako_engine::ids::{ProcessId, StreamId, TenantId};
        use mako_engine::version::WorkflowId;
        use time::OffsetDateTime;

        let stream_id = StreamId::new("test-stream-anfrage-001");
        let due = OffsetDateTime::now_utc();
        let dl = Deadline::new(
            stream_id,
            ProcessId::new(),
            TenantId::from_party_id("9900357000004"),
            WorkflowId::new(WORKFLOW_NAME, "FV2025-10-01"),
            ANFRAGE_WINDOW_LABEL,
            due,
        );

        // Initiated state → should produce TimeoutExpired command.
        let initiated = {
            // receive_invalid() would go to Rejected — instead build Initiated
            // directly by applying only the AnfrageErhalten event (no ValidationPassed/Failed).
            let e0 = AnfrageBestellungEvent::AnfrageErhalten {
                pruefidentifikator: pid(),
                sender: sender(),
                receiver: receiver(),
                vorgang_id: vorgang_id(),
                bearbeitungsstatus: "E07".to_owned(),
                document_date: "20250115".to_owned(),
                message_ref: msg_ref(),
            };
            GpkeAnfrageBestellungWorkflow::apply(AnfrageBestellungState::New, &e0)
        };
        assert!(
            GpkeAnfrageBestellungWorkflow::on_deadline(&dl, &initiated).is_some(),
            "Initiated state must trigger TimeoutExpired"
        );

        // ValidationPassed → should also fire.
        let vp = make_validation_passed_state();
        assert!(
            GpkeAnfrageBestellungWorkflow::on_deadline(&dl, &vp).is_some(),
            "ValidationPassed state must trigger TimeoutExpired"
        );

        // ResponseDispatched (terminal) → must not fire.
        let WorkflowOutput { events, .. } = GpkeAnfrageBestellungWorkflow::handle(
            &vp,
            AnfrageBestellungCommand::DispatchResponse {
                data_provided: true,
                reason: None,
            },
        )
        .unwrap();
        let dispatched = events.iter().fold(
            make_validation_passed_state(),
            GpkeAnfrageBestellungWorkflow::apply,
        );
        assert!(
            GpkeAnfrageBestellungWorkflow::on_deadline(&dl, &dispatched).is_none(),
            "terminal state must not fire deadline"
        );
    }
}
