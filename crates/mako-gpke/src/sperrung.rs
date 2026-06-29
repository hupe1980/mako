//! GPKE Sperrung / Entsperrung workflow (ORDERS PIDs 17115–17117).
//!
//! Covers the GPKE disconnection and reconnection process for electricity supply,
//! using the ORDERS format per "AWH Sperrprozesse" (same PID space as Gas Sperrung):
//!
//! | PID   | Process name (AWH)              | Direction  |
//! |-------|-------------------------------|------------|
//! | 17115 | Sperrauftrag                    | NB → LFN   |
//! | 17116 | Anfrage Sperrung                | NB → LFN   |
//! | 17117 | Entsperrauftrag                 | NB → LFN   |
//!
//! ORDRSP responses (19116 Bestätigung / 19117 Ablehnung Sperr-/Entsperrauftrag)
//! are dispatched by the workflow after validation.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **BK6-22-024** — BNetzA ruling; APERAK within **24 wall-clock hours**

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// GPKE Sperrung/Entsperrung ORDERS Prüfidentifikatoren handled by [`GpkeSperrungWorkflow`].
///
/// - 17115: Sperrauftrag (NB → LFN)
/// - 17116: Anfrage Sperrung (NB → LFN)
/// - 17117: Entsperrauftrag (NB → LFN)
pub const SPERRUNG_PIDS: &[u32] = &[17115, 17116, 17117];

/// Deadline label for the 24-wall-clock-hour execution confirmation window.
///
/// BK6-22-024: NB must dispatch the APERAK within **24 wall-clock hours**.
/// Register a `Deadline` with this label immediately after `ValidationPassed`:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::add_hours(received_at, 24);
/// let deadline = Deadline::new(process.stream_id().clone(), ..., SPERRUNG_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const SPERRUNG_WINDOW_LABEL: &str = "gpke-sperrung-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Sperrung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SperrungEvent {
    /// ORDERS Sperrung/Entsperrauftrag (17115/17116/17117) received from NB.
    AnweisungErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (17115, 17116, or 17117).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Sperrung/Entsperrung was executed and the outcome confirmed.
    AusfuehrungBestaetigt {
        /// `true` = Sperrung executed; `false` = could not be carried out.
        durchgefuehrt: bool,
        /// Optional reason for non-execution.
        reason: Option<String>,
    },
    /// Process was rejected (validation failure or deadline expiry).
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

impl EventPayload for SperrungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnweisungErhalten { .. } => "SperrungAnweisungErhalten",
            Self::ValidationPassed { .. } => "SperrungValidationPassed",
            Self::AusfuehrungBestaetigt { .. } => "SperrungAusfuehrungBestaetigt",
            Self::Rejected { .. } => "SperrungRejected",
            Self::DeadlineExpired { .. } => "SperrungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data set when the Anweisung Sperrung is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SperrungData {
    /// EIC/MaLo supply location code.
    pub location_id: MaLo,
    /// GLN of the issuing NB.
    pub sender: MarktpartnerCode,
    /// EDIFACT document date from the UTILMD.
    pub document_date: String,
    /// BDEW Prüfidentifikator (17115, 17116, or 17117).
    pub pruefidentifikator: Pruefidentifikator,
}

/// Current state of a GPKE Sperrung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AnweisungErhalten → ValidationPassed → Ausgefuehrt
///                                           ↘ Rejected (deadline or execution failed)
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum SperrungState {
    /// No events yet.
    New,
    /// ORDERS Sperrauftrag/Entsperrauftrag (17115/17116/17117) received; awaiting validation.
    AnweisungErhalten(SperrungData),
    /// Validation passed; awaiting execution confirmation.
    ValidationPassed(SperrungData),
    /// Sperrung/Entsperrung executed (terminal success).
    Ausgefuehrt(SperrungData),
    /// Process rejected (terminal failure).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for SperrungState {
    fn default() -> Self {
        Self::New
    }
}

impl SperrungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnweisungErhalten(_) => "AnweisungErhalten",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Ausgefuehrt(_) => "Ausgefuehrt",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&SperrungData)` if the process has been initiated.
    #[must_use]
    pub fn sperrung_data(&self) -> Option<&SperrungData> {
        match self {
            Self::AnweisungErhalten(d) | Self::ValidationPassed(d) | Self::Ausgefuehrt(d) => {
                Some(d)
            }
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Sperrung workflow.
#[derive(Clone)]
pub enum SperrungCommand {
    /// Inbound ORDERS 17115/17116/17117 received from NB. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveSperrung {
        /// Prüfidentifikator of the inbound ORDERS (17115, 17116, or 17117).
        pid: Pruefidentifikator,
        /// GLN of the NB sending the Sperrungsanweisung.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the supply location to be locked/unlocked.
        location_id: MaLo,
        /// Document date from the UTILMD.
        document_date: String,
        /// Message reference of the inbound UTILMD.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned a report with no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Confirm that the Sperrung/Entsperrung was (or could not be) executed.
    ///
    /// Set `durchgefuehrt = true` when disconnection/reconnection succeeded.
    /// Set `durchgefuehrt = false` and populate `reason` when it could not be
    /// carried out (e.g. meter access denied, safety block).
    BestaetigueSperrung {
        /// `true` if the Sperrung/Entsperrung was executed successfully.
        durchgefuehrt: bool,
        /// Optional reason when `durchgefuehrt = false`.
        reason: Option<String>,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for SperrungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Sperrung / Entsperrung workflow (ORDERS PIDs 17115–17117).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeSperrungWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-sperrung", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeSperrungWorkflow;

impl Workflow for GpkeSperrungWorkflow {
    type State = SperrungState;
    type Event = SperrungEvent;
    type Command = SperrungCommand;

    /// Deadline compensation for the GPKE Sperrung 24h window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"gpke-sperrung-window"` | `AnweisungErhalten` or `ValidationPassed` | `TimeoutExpired` | BK6-22-024 — 24h wall-clock Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                SPERRUNG_WINDOW_LABEL,
                SperrungState::AnweisungErhalten(_) | SperrungState::ValidationPassed(_),
            ) => Some(SperrungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SperrungEvent::AnweisungErhalten {
                location_id,
                sender,
                document_date,
                pruefidentifikator,
                ..
            } => SperrungState::AnweisungErhalten(SperrungData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            SperrungEvent::ValidationPassed { .. } => match state {
                SperrungState::AnweisungErhalten(data) => SperrungState::ValidationPassed(data),
                other => other,
            },
            SperrungEvent::AusfuehrungBestaetigt {
                durchgefuehrt,
                reason,
            } => {
                if *durchgefuehrt {
                    match state {
                        SperrungState::ValidationPassed(data) => SperrungState::Ausgefuehrt(data),
                        other => other,
                    }
                } else {
                    let msg = reason
                        .as_deref()
                        .unwrap_or("Sperrung konnte nicht durchgeführt werden");
                    SperrungState::Rejected {
                        reason: msg.to_owned(),
                    }
                }
            }
            SperrungEvent::Rejected { reason } => SperrungState::Rejected {
                reason: reason.clone(),
            },
            SperrungEvent::DeadlineExpired { label, .. } => match state {
                SperrungState::Ausgefuehrt(_) | SperrungState::Rejected { .. } => state,
                _ => SperrungState::Rejected {
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
            SperrungCommand::ReceiveSperrung {
                pid,
                sender,
                location_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, SperrungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !SPERRUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Sperrung PID (17115, 17116, or 17117), got {pid}",
                    )));
                }
                let mut events = vec![SperrungEvent::AnweisungErhalten {
                    location_id,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(SperrungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(SperrungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            SperrungCommand::BestaetigueSperrung {
                durchgefuehrt,
                reason,
            } => {
                if !matches!(state, SperrungState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![SperrungEvent::AusfuehrungBestaetigt {
                    durchgefuehrt,
                    reason,
                }]
                .into())
            }

            SperrungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    SperrungState::Ausgefuehrt(_) | SperrungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}
