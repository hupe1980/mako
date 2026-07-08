//! GPKE Sperrung / Entsperrung workflow — NB-role inbound (ORDERS PIDs 17115–17117).
//!
//! When `makod` operates as the **Netzbetreiber (NB)**, the grid operator receives
//! Sperrauftrag / Anfrage Sperrung / Entsperrauftrag from the Lieferant (LF) or
//! passes an Anfrage Sperrung to the MSB:
//!
//! | PID   | Process name (AWH)                        | Direction     |
//! |-------|-------------------------------------------|---------------|
//! | 17115 | Sperrauftrag                              | LF → NB       |
//! | 17116 | Anfrage Sperrung (NB asks MSB)            | NB → MSB      |
//! | 17117 | Entsperrauftrag                           | LF → NB       |
//! | 39000 | Stornierung Sperr-/Entsperrauftrag        | LF → NB       |
//!
//! The NB dispatches ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) back to the LF
//! via the outbox. When NB forwards an Anfrage Sperrung (17116) to the MSB, the MSB
//! responds with ORDRSP 19118 (Bestätigung) or 19119 (Ablehnung).
//!
//! For the **LF-side** workflow (LF initiates Sperrung and awaits NB's ORDRSP),
//! see [`crate::sperrung_lf`].
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE** — Geschäftsprozesse zur Kundenbelieferung mit Elektrizität
//! - **AWH Sperrprozesse Gas / GPKE Teil 2** — BNetzA ruling BK6-22-024
//! - **APERAK Frist**: 24 wall-clock hours for response
//! - **IFTSTA 21039** — Auftragsstatus dispatched by NB after execution (outbound)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the NB-role GPKE Sperrung workflow.
pub const WORKFLOW_NAME: &str = "gpke-sperrung";

/// GPKE Sperrung/Entsperrung ORDERS Prüfidentifikatoren received by NB from LF.
///
/// | PID   | Direction  | Description                              |
/// |-------|------------|------------------------------------------|
/// | 17115 | LF → NB    | Sperrauftrag                             |
/// | 17116 | NB → MSB   | Anfrage Sperrung (NB queries MSB)        |
/// | 17117 | LF → NB    | Entsperrauftrag                          |
pub const SPERRUNG_PIDS: &[u32] = &[17115, 17116, 17117];

/// ORDCHG Prüfidentifikatoren for Stornierung of a Sperr-/Entsperrauftrag.
///
/// - 39000: Stornierung Sperr-/Entsperrauftrag (LF → NB) — LF cancels a pending order.
/// - 39001: Weiterleitung der Stornierung (NB → MSB) — NB forwards LF's cancellation to MSB.
///
/// The NB responds with ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung) via outbox.
/// When MSB receives 39001, it processes the cancellation forwarded by NB.
pub const ORDCHG_STORNIERUNG_PIDS: &[u32] = &[39000, 39001];

/// ORDRSP Prüfidentifikatoren received by NB from MSB after an Anfrage Sperrung (17116).
///
/// - 19118: Bestätigung Anfrage Sperrung (MSB → NB) — MSB confirms meter is accessible.
/// - 19119: Ablehnung Anfrage Sperrung (MSB → NB) — MSB cannot confirm meter access.
pub const MSB_ANTWORT_PIDS: &[u32] = &[19118, 19119];

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
    /// ORDCHG 39000 (Stornierung Sperr-/Entsperrauftrag) received from LF.
    ///
    /// Emitted when the LF cancels a pending Sperrauftrag before execution.
    /// The NB must respond with ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung).
    StornierungErhalten {
        /// BDEW Prüfidentifikator of the ORDCHG (always 39000).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the LF sending the Stornierung.
        sender: MarktpartnerCode,
        /// EDIFACT message reference of the ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19118/19119 received from MSB after NB forwarded Anfrage Sperrung (17116).
    ///
    /// - `is_confirmed = true` (19118): MSB confirms meter access → NB can proceed.
    /// - `is_confirmed = false` (19119): MSB denies meter access → NB must reject.
    MsbAntwortErhalten {
        /// 19118 = Bestätigung, 19119 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` = 19118 Bestätigung; `false` = 19119 Ablehnung.
        is_confirmed: bool,
        /// EDIFACT message reference of the ORDRSP.
        message_ref: MessageRef,
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
            Self::StornierungErhalten { .. } => "SperrungStornierungErhalten",
            Self::MsbAntwortErhalten { .. } => "SperrungMsbAntwortErhalten",
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
    /// LF cancelled the Sperrauftrag before execution (terminal — LF storno accepted).
    Storniert(SperrungData),
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
            Self::Storniert(_) => "Storniert",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Returns `true` for terminal states (no further transitions possible).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Ausgefuehrt(_) | Self::Storniert(_) | Self::Rejected { .. }
        )
    }

    /// Return `Some(&SperrungData)` if the process has been initiated.
    #[must_use]
    pub fn sperrung_data(&self) -> Option<&SperrungData> {
        match self {
            Self::AnweisungErhalten(d)
            | Self::ValidationPassed(d)
            | Self::Ausgefuehrt(d)
            | Self::Storniert(d) => Some(d),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Sperrung workflow (NB-role perspective).
#[derive(Clone)]
pub enum SperrungCommand {
    /// Inbound ORDERS 17115/17116/17117 received from LF. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveSperrung {
        /// Prüfidentifikator of the inbound ORDERS (17115, 17116, or 17117).
        pid: Pruefidentifikator,
        /// GLN of the LF sending the Sperrungsanweisung.
        sender: MarktpartnerCode,
        /// GLN of the receiving NB (our own party code).
        receiver: MarktpartnerCode,
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
    /// Inbound ORDCHG 39000 (Stornierung Sperr-/Entsperrauftrag) received from LF.
    ///
    /// The LF is cancelling the pending Sperrauftrag. The NB must respond
    /// with ORDRSP 19128 (Bestätigung) or 19129 (Ablehnung) via outbox.
    ReceiveStornierung {
        /// Prüfidentifikator (always 39000).
        pid: Pruefidentifikator,
        /// GLN of the LF sending the Stornierung.
        sender: MarktpartnerCode,
        /// Message reference of the inbound ORDCHG.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19118/19119 received from MSB (after NB forwarded Anfrage Sperrung 17116).
    ReceiveMsbAntwort {
        /// 19118 = Bestätigung, 19119 = Ablehnung.
        pid: Pruefidentifikator,
        /// `true` for 19118 (Bestätigung), `false` for 19119 (Ablehnung).
        is_confirmed: bool,
        /// Message reference of the inbound ORDRSP.
        message_ref: MessageRef,
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
            SperrungEvent::StornierungErhalten { .. } => match state {
                SperrungState::AnweisungErhalten(data) | SperrungState::ValidationPassed(data) => {
                    SperrungState::Storniert(data)
                }
                // Already terminal: ignore (idempotent).
                other => other,
            },
            SperrungEvent::MsbAntwortErhalten { .. } => {
                // Informational: no state transition required; the adapter
                // reads the event to decide whether to proceed with execution.
                state
            }
            SperrungEvent::DeadlineExpired { label, .. } => match state {
                SperrungState::Ausgefuehrt(_)
                | SperrungState::Storniert(_)
                | SperrungState::Rejected { .. } => state,
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
                receiver,
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
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![SperrungEvent::AnweisungErhalten {
                    location_id,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(SperrungEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — mandatory per APERAK AHB 1.0 §2.4.
                    // Strom ORDERS (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":        receiver_gln.as_str(),
                                "receiver":      sender_mp_id.as_str(),
                                "pid":           29001_u32,
                                "document_code": "312",
                            }),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(SperrungEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 — mandatory per APERAK AHB 1.0 §2.1.1.
                    // Strom ORDERS (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
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
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungEvent::DeadlineExpired { deadline_id, label }].into())
            }

            SperrungCommand::ReceiveStornierung {
                pid,
                sender,
                message_ref,
            } => {
                if !ORDCHG_STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Stornierung PID (39000 or 39001), got {pid}",
                    )));
                }
                // Stornierung is only valid when a Sperrauftrag is pending.
                // If already executed or rejected, the NB must send ORDRSP 19129 (Ablehnung).
                if matches!(
                    state,
                    SperrungState::Ausgefuehrt(_)
                        | SperrungState::Storniert(_)
                        | SperrungState::Rejected { .. }
                ) {
                    return Err(WorkflowError::rejected(format!(
                        "Stornierung rejected: process is already terminal ({})",
                        state.label()
                    )));
                }
                Ok(vec![SperrungEvent::StornierungErhalten {
                    pruefidentifikator: pid,
                    sender,
                    message_ref,
                }]
                .into())
            }

            SperrungCommand::ReceiveMsbAntwort {
                pid,
                is_confirmed,
                message_ref,
            } => {
                if !MSB_ANTWORT_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an MSB-Antwort PID (19118 or 19119), got {pid}",
                    )));
                }
                Ok(vec![SperrungEvent::MsbAntwortErhalten {
                    pruefidentifikator: pid,
                    is_confirmed,
                    message_ref,
                }]
                .into())
            }
        }
    }
}
