//! GeLi Gas NB-side (GNB-role) Gas Sperrung / Entsperrung workflow.
//!
//! When `makod` operates as the **Gasnetzbetreiber (GNB)**, the GNB receives
//! Sperrauftrag / Anfrage Sperrung / Entsperrauftrag from the Lieferant (LF):
//!
//! | PID   | Process name (AWH)                                 | Direction     |
//! |-------|----------------------------------------------------|---------------|
//! | 17115 | Sperrauftrag                                       | LF → GNB      |
//! | 17116 | Anfrage Sperrung (GNB asks gMSB)                   | GNB → gMSB    |
//! | 17117 | Entsperrauftrag                                    | LF → GNB      |
//! | 39000 | Stornierung Sperr-/Entsperrauftrag                 | LF → GNB      |
//!
//! The GNB dispatches ORDRSP 19116 (Bestätigung) or 19117 (Ablehnung) back
//! to the LF via the outbox. When the GNB forwards an Anfrage Sperrung (17116)
//! to the gMSB, the gMSB responds with ORDRSP 19118 (Bestätigung) or 19119
//! (Ablehnung).
//!
//! For the **LF-side** workflow (LF initiates Gas-Sperrung and awaits GNB's
//! ORDRSP), see [`crate::sperrung_lf`].
//!
//! # Regulatory basis
//!
//! - **BK7-24-01-009** — GeLi Gas 3.0 (Gas Sperr-/Entsperrprozesse)
//! - **AWH Sperrprozesse Gas** — published under BK7-24-01-009
//! - **APERAK Frist**: **10 Werktage** (Saturday counts, Sunday and public
//!   holidays do not; German local time CET/CEST)
//! - Use `mako_engine::fristen::add_werktage(date, 10, BdewMaKo)` for the
//!   deadline computation

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Stable workflow name for the GNB-role Gas Sperrung workflow.
pub const WORKFLOW_NAME: &str = "geli-gas-sperrung-nb";

/// Gas Sperrung/Entsperrung ORDERS Prüfidentifikatoren received or relayed by GNB.
///
/// | PID   | Direction    | Description                               |
/// |-------|--------------|-------------------------------------------|
/// | 17115 | LF → GNB     | Gas-Sperrauftrag                          |
/// | 17116 | GNB → gMSB   | Anfrage Sperrung (GNB queries gMSB)       |
/// | 17117 | LF → GNB     | Gas-Entsperrauftrag                       |
///
/// 17116 is included so that a dual-role (GNB+gMSB) instance can correlate
/// the forwarded Anfrage Sperrung with this workflow.
pub const SPERRUNG_PIDS: &[u32] = &[17115, 17116, 17117];

/// ORDCHG Prüfidentifikatoren for Stornierung of a Gas-Sperr-/Entsperrauftrag.
///
/// - 39000: Stornierung Sperr-/Entsperrauftrag (LF → GNB) — LF cancels a pending order.
/// - 39001: Weiterleitung der Stornierung (GNB → gMSB) — GNB forwards LF's cancellation.
pub const ORDCHG_STORNIERUNG_PIDS: &[u32] = &[39000, 39001];

/// ORDRSP Prüfidentifikatoren received by GNB from gMSB after Anfrage Sperrung (17116).
///
/// - 19118: Bestätigung Anfrage Sperrung (gMSB → GNB) — gMSB confirms meter accessible.
/// - 19119: Ablehnung Anfrage Sperrung (gMSB → GNB) — gMSB cannot confirm access.
pub const MSB_ANTWORT_PIDS: &[u32] = &[19118, 19119];

/// Deadline label for the 10-Werktage GNB execution window.
///
/// BK7-24-01-009 (AWH Sperrprozesse Gas): GNB must execute and confirm within
/// **10 Werktage** of receiving the Sperrauftrag.
///
/// ```rust,ignore
/// let due = mako_engine::fristen::add_werktage(received_date, 10, BdewMaKo);
/// let deadline = Deadline::new(process.stream_id().clone(), ..., ANTWORT_WINDOW_LABEL, due);
/// deadline_store.register(&deadline).await?;
/// ```
pub const ANTWORT_WINDOW_LABEL: &str = "geli-gas-sperrung-nb-10wt";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GeLi Gas GNB-side Gas Sperrung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GasSperrungNbEvent {
    /// ORDERS Gas-Sperrauftrag/Entsperrauftrag (17115/17116/17117) received from LF.
    AnweisungErhalten {
        /// Marktlokation EIC code (gas supply point).
        location_id: MaLo,
        /// GLN of the sending LF.
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
    /// Gas-Sperrung/Entsperrung was executed and the outcome confirmed.
    AusfuehrungBestaetigt {
        /// `true` = executed successfully; `false` = could not be carried out.
        durchgefuehrt: bool,
        /// Optional reason for non-execution.
        reason: Option<String>,
    },
    /// ORDCHG 39000 (Stornierung) received from LF — LF cancels a pending Sperrauftrag.
    StornierungErhalten {
        /// Prüfidentifikator (always 39000).
        pruefidentifikator: Pruefidentifikator,
        /// GLN of the LF sending the Stornierung.
        sender: MarktpartnerCode,
        /// EDIFACT message reference of the ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19118/19119 received from gMSB after GNB forwarded Anfrage Sperrung (17116).
    ///
    /// - `is_confirmed = true` (19118): gMSB confirms meter access.
    /// - `is_confirmed = false` (19119): gMSB denies access.
    MsbAntwortErhalten {
        /// 19118 = Bestätigung, 19119 = Ablehnung.
        pruefidentifikator: Pruefidentifikator,
        /// `true` = 19118 (confirmed); `false` = 19119 (rejected).
        is_confirmed: bool,
        /// EDIFACT message reference of the ORDRSP.
        message_ref: MessageRef,
    },
    /// Process rejected (validation failure or deadline expiry).
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

impl EventPayload for GasSperrungNbEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnweisungErhalten { .. } => "GasSperrungNbAnweisungErhalten",
            Self::ValidationPassed { .. } => "GasSperrungNbValidationPassed",
            Self::AusfuehrungBestaetigt { .. } => "GasSperrungNbAusfuehrungBestaetigt",
            Self::StornierungErhalten { .. } => "GasSperrungNbStornierungErhalten",
            Self::MsbAntwortErhalten { .. } => "GasSperrungNbMsbAntwortErhalten",
            Self::Rejected { .. } => "GasSperrungNbRejected",
            Self::DeadlineExpired { .. } => "GasSperrungNbDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the GNB receives an Anweisung Sperrung.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GasSperrungNbData {
    /// EIC/MaLo of the gas supply location.
    pub location_id: MaLo,
    /// GLN of the issuing LF.
    pub sender: MarktpartnerCode,
    /// EDIFACT document date from the ORDERS.
    pub document_date: String,
    /// BDEW Prüfidentifikator (17115, 17116, or 17117).
    pub pruefidentifikator: Pruefidentifikator,
}

/// Current state of a GeLi Gas GNB-side Sperrung process stream.
///
/// # Lifecycle
///
/// ```text
/// New → AnweisungErhalten → ValidationPassed → Ausgefuehrt      [terminal]
///                                            ↘ Rejected          [terminal]
///     ↘ Rejected (validation failed)                             [terminal]
///     ↘ (either state) → Storniert (LF cancels before exec)     [terminal]
///     ↘ (active state) → DeadlineExpired (10-WT window elapsed)  [terminal]
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum GasSperrungNbState {
    /// No events yet.
    New,
    /// ORDERS Sperrauftrag/Entsperrauftrag received; awaiting validation result.
    AnweisungErhalten(GasSperrungNbData),
    /// Validation passed; awaiting execution confirmation.
    ValidationPassed(GasSperrungNbData),
    /// Gas-Sperrung/Entsperrung executed (terminal success).
    Ausgefuehrt(GasSperrungNbData),
    /// LF cancelled the Sperrauftrag before execution (terminal).
    Storniert(GasSperrungNbData),
    /// Process rejected (terminal failure).
    Rejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl Default for GasSperrungNbState {
    fn default() -> Self {
        Self::New
    }
}

impl GasSperrungNbState {
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

    /// Returns `Some(&GasSperrungNbData)` if the process has been initiated.
    #[must_use]
    pub fn sperrung_data(&self) -> Option<&GasSperrungNbData> {
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

/// Commands for the GeLi Gas GNB-side Gas Sperrung workflow.
#[derive(Clone)]
pub enum GasSperrungNbCommand {
    /// Inbound ORDERS 17115/17116/17117 received from LF.
    ReceiveSperrung {
        /// Prüfidentifikator of the inbound ORDERS (17115, 17116, or 17117).
        pid: Pruefidentifikator,
        /// GLN of the LF sending the Sperrungsanweisung.
        sender: MarktpartnerCode,
        /// EIC/MaLo of the gas supply location to be locked/unlocked.
        location_id: MaLo,
        /// Document date from the ORDERS.
        document_date: String,
        /// Message reference of the inbound ORDERS.
        message_ref: MessageRef,
        /// `true` if `msg.validate()` returned no errors.
        validation_passed: bool,
        /// Human-readable validation issue strings for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Confirm that the Gas-Sperrung/Entsperrung was (or could not be) executed.
    BestaetigueSperrung {
        /// `true` if the Gas-Sperrung/Entsperrung was executed successfully.
        durchgefuehrt: bool,
        /// Optional reason when `durchgefuehrt = false`.
        reason: Option<String>,
    },
    /// Inbound ORDCHG 39000 (Stornierung) received from LF.
    ReceiveStornierung {
        /// Prüfidentifikator (always 39000).
        pid: Pruefidentifikator,
        /// GLN of the LF sending the Stornierung.
        sender: MarktpartnerCode,
        /// Message reference of the inbound ORDCHG.
        message_ref: MessageRef,
    },
    /// Inbound ORDRSP 19118/19119 received from gMSB after GNB forwarded Anfrage Sperrung.
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

impl CommandPayload for GasSperrungNbCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GeLi Gas GNB-side Gas Sperrung / Entsperrung workflow (ORDERS PIDs 17115–17117).
///
/// Regulatory basis: BK7-24-01-009 (AWH Sperrprozesse Gas).
/// APERAK Frist: **10 Werktage** (BK7-24-01-009).
pub struct GeliGasSperrungNbWorkflow;

impl Workflow for GeliGasSperrungNbWorkflow {
    type State = GasSperrungNbState;
    type Event = GasSperrungNbEvent;
    type Command = GasSperrungNbCommand;

    /// Deadline compensation for the Gas Sperrung 10-Werktage window.
    ///
    /// | Label | State guard | Command emitted | BNetzA rule |
    /// |---|---|---|---|
    /// | `"geli-gas-sperrung-nb-10wt"` | `AnweisungErhalten` or `ValidationPassed` | `TimeoutExpired` | BK7-24-01-009 — 10 Werktage |
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ANTWORT_WINDOW_LABEL,
                GasSperrungNbState::AnweisungErhalten(_) | GasSperrungNbState::ValidationPassed(_),
            ) => Some(GasSperrungNbCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            GasSperrungNbEvent::AnweisungErhalten {
                location_id,
                sender,
                document_date,
                pruefidentifikator,
                ..
            } => GasSperrungNbState::AnweisungErhalten(GasSperrungNbData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                document_date: document_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            GasSperrungNbEvent::ValidationPassed { .. } => match state {
                GasSperrungNbState::AnweisungErhalten(data) => {
                    GasSperrungNbState::ValidationPassed(data)
                }
                other => other,
            },
            GasSperrungNbEvent::AusfuehrungBestaetigt {
                durchgefuehrt,
                reason,
            } => {
                if *durchgefuehrt {
                    match state {
                        GasSperrungNbState::ValidationPassed(data) => {
                            GasSperrungNbState::Ausgefuehrt(data)
                        }
                        other => other,
                    }
                } else {
                    let msg = reason
                        .as_deref()
                        .unwrap_or("Gas-Sperrung konnte nicht durchgeführt werden");
                    GasSperrungNbState::Rejected {
                        reason: msg.to_owned(),
                    }
                }
            }
            GasSperrungNbEvent::Rejected { reason } => GasSperrungNbState::Rejected {
                reason: reason.clone(),
            },
            GasSperrungNbEvent::StornierungErhalten { .. } => match state {
                GasSperrungNbState::AnweisungErhalten(data)
                | GasSperrungNbState::ValidationPassed(data) => GasSperrungNbState::Storniert(data),
                // Already terminal: ignore (idempotent).
                other => other,
            },
            GasSperrungNbEvent::MsbAntwortErhalten { .. } => {
                // Informational: the adapter reads this event to decide whether to
                // proceed with execution. No state transition required here.
                state
            }
            GasSperrungNbEvent::DeadlineExpired { label, .. } => match state {
                GasSperrungNbState::Ausgefuehrt(_)
                | GasSperrungNbState::Storniert(_)
                | GasSperrungNbState::Rejected { .. } => state,
                _ => GasSperrungNbState::Rejected {
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
            GasSperrungNbCommand::ReceiveSperrung {
                pid,
                sender,
                location_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, GasSperrungNbState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !SPERRUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas-Sperrung PID (17115, 17116, or 17117), got {pid}",
                    )));
                }
                let mut events = vec![GasSperrungNbEvent::AnweisungErhalten {
                    location_id,
                    sender,
                    document_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(GasSperrungNbEvent::ValidationPassed { message_ref });
                } else {
                    events.push(GasSperrungNbEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            GasSperrungNbCommand::BestaetigueSperrung {
                durchgefuehrt,
                reason,
            } => {
                if !matches!(state, GasSperrungNbState::ValidationPassed(_)) {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.label(),
                    ));
                }
                Ok(vec![GasSperrungNbEvent::AusfuehrungBestaetigt {
                    durchgefuehrt,
                    reason,
                }]
                .into())
            }

            GasSperrungNbCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GasSperrungNbEvent::DeadlineExpired { deadline_id, label }].into())
            }

            GasSperrungNbCommand::ReceiveStornierung {
                pid,
                sender,
                message_ref,
            } => {
                if !ORDCHG_STORNIERUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Gas-Stornierung PID (39000 or 39001), got {pid}",
                    )));
                }
                if state.is_terminal() {
                    return Err(WorkflowError::rejected(format!(
                        "Gas-Stornierung rejected: process is already terminal ({})",
                        state.label()
                    )));
                }
                Ok(vec![GasSperrungNbEvent::StornierungErhalten {
                    pruefidentifikator: pid,
                    sender,
                    message_ref,
                }]
                .into())
            }

            GasSperrungNbCommand::ReceiveMsbAntwort {
                pid,
                is_confirmed,
                message_ref,
            } => {
                if !MSB_ANTWORT_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected an gMSB-Antwort PID (19118 or 19119), got {pid}",
                    )));
                }
                Ok(vec![GasSperrungNbEvent::MsbAntwortErhalten {
                    pruefidentifikator: pid,
                    is_confirmed,
                    message_ref,
                }]
                .into())
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::workflow::Workflow;

    fn pid(n: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(n).unwrap()
    }

    fn make_ref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }

    fn gln(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    #[test]
    fn receive_sperrauftrag_validation_passed() {
        let state = GasSperrungNbState::New;
        let cmd = GasSperrungNbCommand::ReceiveSperrung {
            pid: pid(17115),
            sender: gln("9900000000001"),
            location_id: malo("DE0000000000000000000000000000001"),
            document_date: "20260101".into(),
            message_ref: make_ref("REF001"),
            validation_passed: true,
            validation_errors: vec![],
        };
        let out = GeliGasSperrungNbWorkflow::handle(&state, cmd).unwrap();
        let events = out.events;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            GasSperrungNbEvent::AnweisungErhalten { .. }
        ));
        assert!(matches!(
            events[1],
            GasSperrungNbEvent::ValidationPassed { .. }
        ));
    }

    #[test]
    fn receive_entsperrauftrag_validation_failed() {
        let state = GasSperrungNbState::New;
        let cmd = GasSperrungNbCommand::ReceiveSperrung {
            pid: pid(17117),
            sender: gln("9900000000001"),
            location_id: malo("DE0000000000000000000000000000001"),
            document_date: "20260101".into(),
            message_ref: make_ref("REF002"),
            validation_passed: false,
            validation_errors: vec!["AHB rule 5 violated".into()],
        };
        let out = GeliGasSperrungNbWorkflow::handle(&state, cmd).unwrap();
        let events = out.events;
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            GasSperrungNbEvent::AnweisungErhalten { .. }
        ));
        assert!(matches!(events[1], GasSperrungNbEvent::Rejected { .. }));
    }

    #[test]
    fn stornierung_accepted_when_pending() {
        let data = GasSperrungNbData {
            location_id: malo("DE0000000000000000000000000000001"),
            sender: gln("9900000000001"),
            document_date: "20260101".into(),
            pruefidentifikator: pid(17115),
        };
        let state = GasSperrungNbState::ValidationPassed(data);
        let cmd = GasSperrungNbCommand::ReceiveStornierung {
            pid: pid(39000),
            sender: gln("9900000000001"),
            message_ref: make_ref("STORNO001"),
        };
        let out = GeliGasSperrungNbWorkflow::handle(&state, cmd).unwrap();
        assert!(matches!(
            out.events[0],
            GasSperrungNbEvent::StornierungErhalten { .. }
        ));
    }

    #[test]
    fn timeout_on_active_state_emits_deadline_expired() {
        let data = GasSperrungNbData {
            location_id: malo("DE0000000000000000000000000000001"),
            sender: gln("9900000000001"),
            document_date: "20260101".into(),
            pruefidentifikator: pid(17115),
        };
        let state = GasSperrungNbState::ValidationPassed(data);
        let cmd = GasSperrungNbCommand::TimeoutExpired {
            deadline_id: DeadlineId::new(),
            label: ANTWORT_WINDOW_LABEL.into(),
        };
        let out = GeliGasSperrungNbWorkflow::handle(&state, cmd).unwrap();
        assert!(matches!(
            out.events[0],
            GasSperrungNbEvent::DeadlineExpired { .. }
        ));
    }

    #[test]
    fn apply_state_machine_transitions() {
        let data = GasSperrungNbData {
            location_id: malo("DE0000000000000000000000000000001"),
            sender: gln("9900000000001"),
            document_date: "20260101".into(),
            pruefidentifikator: pid(17115),
        };
        // AnweisungErhalten → ValidationPassed
        let s1 = GasSperrungNbState::AnweisungErhalten(data.clone());
        let s2 = GeliGasSperrungNbWorkflow::apply(
            s1,
            &GasSperrungNbEvent::ValidationPassed {
                message_ref: make_ref("REF001"),
            },
        );
        assert!(matches!(s2, GasSperrungNbState::ValidationPassed(_)));

        // ValidationPassed → Ausgefuehrt
        let s3 = GeliGasSperrungNbWorkflow::apply(
            s2,
            &GasSperrungNbEvent::AusfuehrungBestaetigt {
                durchgefuehrt: true,
                reason: None,
            },
        );
        assert!(matches!(s3, GasSperrungNbState::Ausgefuehrt(_)));
    }
}
