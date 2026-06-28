//! GeLi Gas Anweisung Sperrung Gas (PID 44555) — disconnection/reconnection
//! order workflow for gas supply.
//!
//! Covers the process from the **receiving-party perspective** (Lieferant /
//! LFN): the system receives an inbound Anweisung Sperrung Gas (44555) from
//! the Gasnetzbetreiber (GNB) and acknowledges execution.
//!
//! # Prüfidentifikator
//!
//! | PID   | Process name (GeLi Gas AHB)                | Direction   |
//! |-------|--------------------------------------------|-------------|
//! | 44555 | Anweisung Sperrung Gas (GNB → LFN / GNB)   | GNB → LFN   |
//!
//! # Regulatory basis
//!
//! - **BDEW GeLi Gas** — Geschäftsprozesse Lieferantenwechsel Gas
//! - **BK7** — BNetzA rulings for gas market; APERAK within **10 Werktage**
//! - **UTILMD G S2.2** — EDI@Energy message format (fv20251001_gas+)
//!
//! # Key differences from GPKE Sperrung (PID 55555)
//!
//! | Aspect | GPKE 55555 (Strom) | GeLi Gas 44555 (Gas) |
//! |---|---|---|
//! | Sender | NB (Electricity) | GNB (Gas Netzbetreiber) |
//! | APERAK Frist | 24 wall-clock hours | **10 Werktage** |
//! | Frist helper | `fristen::add_hours(24)` | **`fristen::add_werktage(10, BdewMaKo)`** |
//! | Profile | `fv20251001` UTILMD S2.x | **`fv20251001_gas` UTILMD G** |

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// GeLi Gas Sperrung Prüfidentifikator handled by [`GeliGasSperrungWorkflow`].
pub const SPERRUNG_PIDS: &[u32] = &[44555];

/// Workflow name key used in [`mako_engine::pid_router::PidRouter`].
pub const WORKFLOW_NAME: &str = "geli-gas-sperrung";

/// Deadline label for the 10-Werktage execution confirmation window.
///
/// Register a deadline with this label immediately after
/// `GasSperrungEvent::ValidationPassed` is processed:
///
/// ```rust,ignore
/// let due = mako_engine::fristen::add_werktage(
///     received_at.date(),
///     10,
///     mako_engine::fristen::HolidayCalendar::BdewMaKo,
/// );
/// let deadline = Deadline::new(
///     process.stream_id().clone(), ..., SPERRUNG_WINDOW_LABEL, due,
/// );
/// deadline_store.register(&deadline).await?;
/// ```
pub const SPERRUNG_WINDOW_LABEL: &str = "gas-sperrung-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas Sperrung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasSperrungEvent {
    /// Anweisung Sperrung Gas (44555) received from GNB.
    AnweisungErhalten {
        /// Gas supply location (Marktlokation).
        malo_id: MaLo,
        /// GLN of the sending GNB.
        gnb: MarktpartnerCode,
        /// GLN of the receiving LFN.
        lieferant: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (44555).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no AHB issues).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Sperrung/Entsperrung was executed; outcome confirmed.
    AusfuehrungBestaetigt {
        /// `true` = disconnection/reconnection executed successfully.
        /// `false` = could not be carried out.
        durchgefuehrt: bool,
        /// Optional reason for non-execution.
        reason: Option<String>,
    },
    /// Process rejected (validation failure or regulatory deadline expiry).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline expired before execution was confirmed.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GasSperrungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnweisungErhalten { .. } => "GasSperrungAnweisungErhalten",
            Self::ValidationPassed { .. } => "GasSperrungValidationPassed",
            Self::AusfuehrungBestaetigt { .. } => "GasSperrungAusfuehrungBestaetigt",
            Self::Rejected { .. } => "GasSperrungRejected",
            Self::DeadlineExpired { .. } => "GasSperrungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the Anweisung Sperrung Gas is received.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasSperrungData {
    /// Gas supply location (Marktlokation).
    pub malo_id: MaLo,
    /// GLN of the issuing GNB (Gasnetzbetreiber).
    pub gnb: MarktpartnerCode,
    /// GLN of the LFN (receiving Lieferant).
    pub lieferant: MarktpartnerCode,
    /// EDIFACT document date from the UTILMD G.
    pub document_date: String,
    /// BDEW Prüfidentifikator (always 44555).
    pub pruefidentifikator: Pruefidentifikator,
}

/// Current state of a GeLi Gas Sperrung process stream.
///
/// # Lifecycle
///
/// ```text
/// New
///  └─► AnweisungErhalten
///       └─► ValidationPassed ─────────────────────► Ausgefuehrt (terminal)
///       │                     BestaetigueSperrung
///       │
///       └─► Rejected (failed validation)
///  (any non-terminal) ──► Rejected (deadline expiry)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GasSperrungState {
    /// No events yet.
    New,
    /// UTILMD G 44555 received; awaiting validation.
    AnweisungErhalten(GasSperrungData),
    /// Validation passed; awaiting execution confirmation (10-Werktage window).
    ValidationPassed(GasSperrungData),
    /// Sperrung/Entsperrung confirmed (terminal success).
    Ausgefuehrt(GasSperrungData),
    /// Process rejected (terminal failure).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for GasSperrungState {
    fn default() -> Self {
        Self::New
    }
}

impl GasSperrungState {
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

    /// Return `Some(&GasSperrungData)` if the process has been initiated.
    #[must_use]
    pub fn sperrung_data(&self) -> Option<&GasSperrungData> {
        match self {
            Self::AnweisungErhalten(d) | Self::ValidationPassed(d) | Self::Ausgefuehrt(d) => {
                Some(d)
            }
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GeLi Gas Sperrung workflow.
///
/// **All domain values must be pre-extracted by the transport layer** before
/// constructing a command. `Workflow::handle()` is pure — no I/O, no parsing.
#[derive(Clone)]
pub enum GasSperrungCommand {
    /// Inbound UTILMD G 44555 received from GNB. Domain fields extracted and
    /// validation performed by the caller before constructing this command.
    ReceiveSperrung {
        /// BDEW Prüfidentifikator (44555).
        pid: Pruefidentifikator,
        /// GLN of the issuing GNB (sender in the UTILMD G envelope).
        gnb: MarktpartnerCode,
        /// GLN of the LFN (receiver in the UTILMD G envelope).
        lieferant: MarktpartnerCode,
        /// Marktlokation EIC code.
        malo_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned a report with no AHB errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Confirm that the Sperrung/Entsperrung was (or could not be) executed.
    ///
    /// Must be called within the **10-Werktage** regulatory window after
    /// receiving the Anweisung. Use
    /// `fristen::add_werktage(10, HolidayCalendar::BdewMaKo)` to compute the
    /// deadline.
    ///
    /// Set `durchgefuehrt = true` when disconnection/reconnection succeeded.
    /// Set `durchgefuehrt = false` and populate `reason` when it failed
    /// (e.g. meter access blocked, safety interlock).
    BestaetigueSperrung {
        /// `true` if executed successfully, `false` if could not be carried out.
        durchgefuehrt: bool,
        /// Reason for non-execution (only set when `durchgefuehrt = false`).
        reason: Option<String>,
    },
    /// A registered deadline fired and was dispatched by the scheduler.
    ///
    /// Transitions any non-terminal state to `Rejected`. Idempotent on
    /// `Ausgefuehrt` and `Rejected`.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GasSperrungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas Anweisung Sperrung Gas (PID 44555) workflow.
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GeliGasSperrungWorkflow>(
///     tenant_id,
///     WorkflowId::new("geli-gas-sperrung", "FV2025-10-01"),
/// );
/// ```
pub struct GeliGasSperrungWorkflow;

impl Workflow for GeliGasSperrungWorkflow {
    type State = GasSperrungState;
    type Event = GasSperrungEvent;
    type Command = GasSperrungCommand;

    /// Deadline compensation for GeLi Gas Sperrung regulatory timeouts.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"gas-sperrung-window"` | non-terminal | `TimeoutExpired` | BK7 GeLi Gas — 10-Werktage Frist |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                SPERRUNG_WINDOW_LABEL,
                GasSperrungState::AnweisungErhalten(_) | GasSperrungState::ValidationPassed(_),
            ) => Some(GasSperrungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasSperrungEvent::AnweisungErhalten {
                malo_id,
                gnb,
                lieferant,
                document_date,
                pruefidentifikator,
                ..
            } => GasSperrungState::AnweisungErhalten(GasSperrungData {
                malo_id: malo_id.clone(),
                gnb: gnb.clone(),
                lieferant: lieferant.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),

            GasSperrungEvent::ValidationPassed { .. } => match state {
                GasSperrungState::AnweisungErhalten(data) => {
                    GasSperrungState::ValidationPassed(data)
                }
                other => other,
            },

            GasSperrungEvent::AusfuehrungBestaetigt {
                durchgefuehrt,
                reason,
            } => {
                if *durchgefuehrt {
                    match state {
                        GasSperrungState::ValidationPassed(data) => {
                            GasSperrungState::Ausgefuehrt(data)
                        }
                        other => other,
                    }
                } else {
                    let msg = reason
                        .as_deref()
                        .unwrap_or("Sperrung konnte nicht durchgeführt werden");
                    GasSperrungState::Rejected {
                        reason: msg.to_owned(),
                    }
                }
            }

            GasSperrungEvent::Rejected { reason } => GasSperrungState::Rejected {
                reason: reason.clone(),
            },

            GasSperrungEvent::DeadlineExpired { label, .. } => match state {
                GasSperrungState::Ausgefuehrt(_) | GasSperrungState::Rejected { .. } => state,
                _ => GasSperrungState::Rejected {
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
            GasSperrungCommand::ReceiveSperrung {
                pid,
                gnb,
                lieferant,
                malo_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GasSperrungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !SPERRUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected PID 44555 (Anweisung Sperrung Gas), got {pid}",
                    )));
                }
                let mut events = vec![GasSperrungEvent::AnweisungErhalten {
                    malo_id,
                    gnb,
                    lieferant,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GasSperrungEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GasSperrungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GasSperrungCommand::BestaetigueSperrung {
                durchgefuehrt,
                reason,
            } => {
                let data = match state {
                    GasSperrungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };

                // Build ERP outbox payload so the downstream system is notified
                // of the execution outcome.
                let mut payload = serde_json::json!({
                    "pid":            data.pruefidentifikator.as_u32(),
                    "malo":           data.malo_id.as_str(),
                    "gnb":            data.gnb.as_str(),
                    "lieferant":      data.lieferant.as_str(),
                    "durchgefuehrt":  durchgefuehrt,
                });
                if let Some(ref r) = reason {
                    payload["reason"] = serde_json::Value::String(r.clone());
                }
                let outbox_entry =
                    PendingOutbox::new("GasSperrungBestaetigung", data.lieferant.as_str(), payload);

                Ok(WorkflowOutput::with_outbox(
                    vec![GasSperrungEvent::AusfuehrungBestaetigt {
                        durchgefuehrt,
                        reason,
                    }],
                    vec![outbox_entry],
                ))
            }

            GasSperrungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    GasSperrungState::Ausgefuehrt(_) | GasSperrungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }

                // Compensation: notify ERP that the 10-Werktage window expired
                // without execution confirmation. Persisted atomically with the
                // DeadlineExpired event via WriteBatch (dual-write contract).
                let outbox: Vec<PendingOutbox> = if let Some(data) = state.sperrung_data() {
                    vec![PendingOutbox::new(
                        "GasSperrungTimeout",
                        data.lieferant.as_str(),
                        serde_json::json!({
                            "pid":       data.pruefidentifikator.as_u32(),
                            "malo":      data.malo_id.as_str(),
                            "gnb":       data.gnb.as_str(),
                            "lieferant": data.lieferant.as_str(),
                            "label":     label.as_ref(),
                        }),
                    )]
                } else {
                    vec![]
                };

                Ok(WorkflowOutput::with_outbox(
                    vec![GasSperrungEvent::DeadlineExpired { deadline_id, label }],
                    outbox,
                ))
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::types::Pruefidentifikator;
    use mako_engine::workflow::Workflow;

    fn gnb() -> MarktpartnerCode {
        MarktpartnerCode::new("4012345000009")
    }

    fn lieferant() -> MarktpartnerCode {
        MarktpartnerCode::new("9876543210001")
    }

    fn malo() -> MaLo {
        MaLo::new("DE0000000000000000000000000044555")
    }

    fn apply_all(mut state: GasSperrungState, events: &[GasSperrungEvent]) -> GasSperrungState {
        for e in events {
            state = GeliGasSperrungWorkflow::apply(state, e);
        }
        state
    }

    fn receive_cmd(valid: bool) -> GasSperrungCommand {
        GasSperrungCommand::ReceiveSperrung {
            pid: Pruefidentifikator::new(44555).unwrap(),
            gnb: gnb(),
            lieferant: lieferant(),
            malo_id: malo(),
            document_date: "20251001".to_owned(),
            message_ref: MessageRef::new("MSG-001"),
            validation_passed: valid,
            validation_errors: if valid {
                vec![]
            } else {
                vec!["BGM missing".to_owned()]
            },
        }
    }

    #[test]
    fn happy_path_receive_and_confirm() {
        let state = GasSperrungState::default();
        let out = GeliGasSperrungWorkflow::handle(&state, receive_cmd(true)).unwrap();
        assert_eq!(out.events.len(), 2, "AnweisungErhalten + ValidationPassed");
        assert!(out.outbox.is_empty());

        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSperrungState::ValidationPassed(_)));

        let out = GeliGasSperrungWorkflow::handle(
            &state,
            GasSperrungCommand::BestaetigueSperrung {
                durchgefuehrt: true,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1, "ERP notification outbox entry");

        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSperrungState::Ausgefuehrt(_)));
    }

    #[test]
    fn validation_failure_rejects() {
        let state = GasSperrungState::default();
        let out = GeliGasSperrungWorkflow::handle(&state, receive_cmd(false)).unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSperrungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let state = GasSperrungState::default();
        let err = GeliGasSperrungWorkflow::handle(
            &state,
            GasSperrungCommand::ReceiveSperrung {
                pid: Pruefidentifikator::new(44001).unwrap(), // supplier-change PID
                gnb: gnb(),
                lieferant: lieferant(),
                malo_id: malo(),
                document_date: "20251001".to_owned(),
                message_ref: MessageRef::new("MSG-002"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("44555"));
    }

    #[test]
    fn deadline_expiry_in_validation_passed() {
        let state = GasSperrungState::default();
        let out = GeliGasSperrungWorkflow::handle(&state, receive_cmd(true)).unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSperrungState::ValidationPassed(_)));

        let deadline_id = mako_engine::ids::DeadlineId::new();
        let out = GeliGasSperrungWorkflow::handle(
            &state,
            GasSperrungCommand::TimeoutExpired {
                deadline_id,
                label: SPERRUNG_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        assert_eq!(out.outbox.len(), 1, "ERP timeout notification");

        let state = apply_all(state, &out.events);
        assert!(matches!(state, GasSperrungState::Rejected { .. }));
    }

    #[test]
    fn deadline_idempotent_on_terminal_states() {
        let data = GasSperrungData {
            malo_id: malo(),
            gnb: gnb(),
            lieferant: lieferant(),
            document_date: "20251001".to_owned(),
            pruefidentifikator: Pruefidentifikator::new(44555).unwrap(),
        };

        // Ausgefuehrt is terminal — deadline must be a no-op
        let state = GasSperrungState::Ausgefuehrt(data.clone());
        let out = GeliGasSperrungWorkflow::handle(
            &state,
            GasSperrungCommand::TimeoutExpired {
                deadline_id: mako_engine::ids::DeadlineId::new(),
                label: SPERRUNG_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty(), "no events on terminal Ausgefuehrt");

        // Rejected is terminal — deadline must be a no-op
        let state = GasSperrungState::Rejected {
            reason: "prior failure".to_owned(),
        };
        let out = GeliGasSperrungWorkflow::handle(
            &state,
            GasSperrungCommand::TimeoutExpired {
                deadline_id: mako_engine::ids::DeadlineId::new(),
                label: SPERRUNG_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty(), "no events on terminal Rejected");
    }
}
