//! WiM Preisanfrage / Angebot — REQOTE/QUOTES price-quote exchange for
//! Messstellenbetreiber device services.
//!
//! Covers the REQOTE/QUOTES process per BDEW WiM documentation:
//! the new MSB (nMSB) requests pricing for device services from the
//! existing MSB, and the existing MSB responds with an Angebot (quote).
//!
//! This module implements both sides of the exchange:
//! - **Anfragend (nMSB)**: sends REQOTE and receives QUOTES response.
//! - **Anbietend (aMSB/NB)**: receives REQOTE and sends QUOTES response.
//!
//! # Prüfidentifikatoren (REQOTE AHB 1.2 / QUOTES AHB 1.1a)
//!
//! ## Inbound REQOTE
//!
//! | PID   | Process name (AHB)                              | Direction   |
//! |-------|-------------------------------------------------|-------------|
//! | 35001 | Anforderung Angebot                             | nMSB → aMSB |
//! | 35002 | Anfrage                                         | nMSB → aMSB |
//! | 35003 | Anfrage von Werten für Rechnungsabwicklung       | nMSB → aMSB |
//! | 35004 | Anfrage einer Konfiguration                     | nMSB → aMSB |
//! | 35005 | Anfrage Angebot Änderung                        | nMSB → aMSB |
//!
//! ## Outbound QUOTES (response)
//!
//! | PID   | Process name (AHB)                              | Derived from |
//! |-------|-------------------------------------------------|--------------|
//! | 15001 | Angebot Geräteübernahme                         | 35001        |
//! | 15002 | Angebot                                         | 35002        |
//! | 15003 | Angebot zur Anfrage von Werten für ESA          | 35003        |
//! | 15004 | Angebot zur Anfrage einer Konfiguration         | 35004        |
//! | 15005 | Angebot zur Anfrage Änderung Technik            | 35005        |
//!
//! # Regulatory basis
//!
//! - **BDEW WiM** — Wechselprozesse im Messwesen Strom
//! - **REQOTE AHB 1.2** / **QUOTES AHB 1.1a** — EDI@Energy message format
//! - **APERAK 2.x** — **5 Werktage** Frist (BK6-24-174)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    types::{MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID sets ──────────────────────────────────────────────────────────────────

/// Inbound REQOTE Prüfidentifikatoren handled by [`WimPreisanfrageWorkflow`].
///
/// | PID   | Process (AHB)                                | AHB version |
/// |-------|----------------------------------------------|-------------|
/// | 35001 | Anforderung Angebot (nMSB → aMSB)            | 1.2 ✅      |
/// | 35002 | Anfrage                                      | 1.2 ✅      |
/// | 35003 | Anfrage von Werten für Rechnungsabwicklung    | 1.2 ✅      |
/// | 35004 | Anfrage einer Konfiguration                  | 1.2 ✅      |
/// | 35005 | Anfrage Angebot Änderung                     | 1.2 ✅      |
pub const REQOTE_PIDS: &[u32] = &[35001, 35002, 35003, 35004, 35005];

/// Inbound QUOTES Prüfidentifikatoren (responses received by the nMSB side).
///
/// | PID   | Process (AHB)                                | AHB version |
/// |-------|----------------------------------------------|-------------|
/// | 15001 | Angebot Geräteübernahme                      | 1.1a ✅     |
/// | 15002 | Angebot                                      | 1.1a ✅     |
/// | 15003 | Angebot zur Anfrage von Werten für ESA       | 1.1a ✅     |
/// | 15004 | Angebot zur Anfrage einer Konfiguration      | 1.1a ✅     |
/// | 15005 | Angebot zur Anfrage Änderung Technik         | 1.1a ✅     |
pub const QUOTES_PIDS: &[u32] = &[15001, 15002, 15003, 15004, 15005];

/// Deadline label for the 5-Werktage response window (BK6-24-174).
pub const PREISANFRAGE_DEADLINE_LABEL: &str = "wim-preisanfrage-antwort";

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound QUOTES response PID for a given REQOTE anfrage PID.
///
/// | REQOTE | QUOTES response |
/// |--------|-----------------|
/// | 35001  | 15001           |
/// | 35002  | 15002           |
/// | 35003  | 15003           |
/// | 35004  | 15004           |
/// | 35005  | 15005           |
fn quotes_response_pid(reqote_pid: u32) -> Option<Pruefidentifikator> {
    let code: u32 = match reqote_pid {
        35001 => 15001,
        35002 => 15002,
        35003 => 15003,
        35004 => 15004,
        35005 => 15005,
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Preisanfrage workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum PreisanfrageEvent {
    /// REQOTE Anfrage received (aMSB perspective) or sent (nMSB perspective).
    AnfrageErhalten {
        /// GLN of the sender (nMSB).
        sender: MarktpartnerCode,
        /// GLN of the receiver (aMSB).
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (35001–35005).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound QUOTES response dispatched to the counterparty.
    AngebotGesendet {
        /// QUOTES response PID (15001–15005).
        response_pid: Option<Pruefidentifikator>,
        /// EDIFACT message reference of the QUOTES response.
        message_ref: MessageRef,
    },
    /// QUOTES response received (nMSB perspective).
    AngebotErhalten {
        /// QUOTES Prüfidentifikator (15001–15005).
        response_pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Process completed.
    Abgeschlossen,
    /// APERAK 29001 dispatched.
    AperakFehlerDispatched {
        /// APERAK PID.
        aperak_pid: Pruefidentifikator,
        /// Error reason.
        reason: String,
        /// Outbound message reference.
        outbound_ref: MessageRef,
    },
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for PreisanfrageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "PreisanfrageAnfrageErhalten",
            Self::ValidationPassed { .. } => "PreisanfrageValidationPassed",
            Self::AngebotGesendet { .. } => "PreisanfrageAngebotGesendet",
            Self::AngebotErhalten { .. } => "PreisanfrageAngebotErhalten",
            Self::Abgeschlossen => "PreisanfrageAbgeschlossen",
            Self::AperakFehlerDispatched { .. } => "PreisanfrageAperakFehlerDispatched",
            Self::Rejected { .. } => "PreisanfrageRejected",
            Self::DeadlineExpired { .. } => "PreisanfrageDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnfrageErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreisanfrageData {
    /// GLN of the requesting nMSB.
    pub sender: MarktpartnerCode,
    /// GLN of the receiving aMSB.
    pub receiver: MarktpartnerCode,
    /// BDEW Prüfidentifikator (35001–35005).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a WiM Preisanfrage process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AngebotGesendet → Abgeschlossen
///                                       ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum PreisanfrageState {
    /// No events yet.
    New,
    /// REQOTE received; awaiting response.
    Eingegangen(PreisanfrageData),
    /// Validation passed; response not yet sent.
    ValidationPassed(PreisanfrageData),
    /// QUOTES response sent (aMSB) or received (nMSB).
    AngebotAusgetauscht {
        /// Data from the anfrage.
        data: PreisanfrageData,
        /// QUOTES response PID.
        response_pid: Option<Pruefidentifikator>,
    },
    /// Process completed.
    Abgeschlossen(PreisanfrageData),
    /// Process rejected.
    Rejected {
        /// Reason.
        reason: String,
    },
}

impl Default for PreisanfrageState {
    fn default() -> Self {
        Self::New
    }
}

impl PreisanfrageState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AngebotAusgetauscht { .. } => "AngebotAusgetauscht",
            Self::Abgeschlossen(_) => "Abgeschlossen",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Preisanfrage workflow.
#[derive(Clone)]
pub enum PreisanfrageCommand {
    /// Inbound REQOTE received.
    ReceiveReqote {
        /// BDEW Prüfidentifikator (35001–35005).
        pid: Pruefidentifikator,
        /// GLN of the sender.
        sender: MarktpartnerCode,
        /// GLN of the receiver.
        receiver: MarktpartnerCode,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation passed.
        validation_passed: bool,
        /// Validation errors.
        validation_errors: Vec<String>,
    },
    /// Dispatch the outbound QUOTES response.
    SendAngebot {
        /// EDIFACT message reference of the QUOTES response.
        message_ref: MessageRef,
    },
    /// Inbound QUOTES received (nMSB side).
    ReceiveAngebot {
        /// QUOTES Prüfidentifikator (15001–15005).
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Mark the exchange as complete.
    Abschliessen,
    /// Dispatch APERAK 29001.
    DispatchAperakFehler {
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
    },
    /// Deadline expired.
    TimeoutExpired {
        /// Deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for PreisanfrageCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Preisanfrage workflow (REQOTE/QUOTES, PIDs 35001–35005 / 15001–15005).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<WimPreisanfrageWorkflow>(
///     tenant_id,
///     WorkflowId::new("wim-preisanfrage", "FV2025-10-01"),
/// );
/// ```
pub struct WimPreisanfrageWorkflow;

impl Workflow for WimPreisanfrageWorkflow {
    type State = PreisanfrageState;
    type Event = PreisanfrageEvent;
    type Command = PreisanfrageCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (PREISANFRAGE_DEADLINE_LABEL, PreisanfrageState::Eingegangen(_))
            | (PREISANFRAGE_DEADLINE_LABEL, PreisanfrageState::ValidationPassed(_)) => {
                Some(PreisanfrageCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            PreisanfrageEvent::AnfrageErhalten {
                sender,
                receiver,
                pruefidentifikator,
                ..
            } => PreisanfrageState::Eingegangen(PreisanfrageData {
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            PreisanfrageEvent::ValidationPassed { .. } => match state {
                PreisanfrageState::Eingegangen(data) => PreisanfrageState::ValidationPassed(data),
                other => other,
            },
            PreisanfrageEvent::AngebotGesendet { response_pid, .. } => match state {
                PreisanfrageState::ValidationPassed(data) => {
                    PreisanfrageState::AngebotAusgetauscht {
                        response_pid: *response_pid,
                        data,
                    }
                }
                other => other,
            },
            PreisanfrageEvent::AngebotErhalten { response_pid, .. } => match state {
                PreisanfrageState::ValidationPassed(data) => {
                    PreisanfrageState::AngebotAusgetauscht {
                        response_pid: Some(*response_pid),
                        data,
                    }
                }
                other => other,
            },
            PreisanfrageEvent::Abgeschlossen => match state {
                PreisanfrageState::AngebotAusgetauscht { data, .. } => {
                    PreisanfrageState::Abgeschlossen(data)
                }
                other => other,
            },
            PreisanfrageEvent::AperakFehlerDispatched { reason, .. } => {
                PreisanfrageState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            PreisanfrageEvent::Rejected { reason } => PreisanfrageState::Rejected {
                reason: reason.clone(),
            },
            PreisanfrageEvent::DeadlineExpired { label, .. } => match state {
                PreisanfrageState::Abgeschlossen(_) | PreisanfrageState::Rejected { .. } => state,
                _ => PreisanfrageState::Rejected {
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
            PreisanfrageCommand::ReceiveReqote {
                pid,
                sender,
                receiver,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, PreisanfrageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !REQOTE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected REQOTE PID (35001–35005), got {pid}",
                    )));
                }
                let mut events = vec![PreisanfrageEvent::AnfrageErhalten {
                    sender,
                    receiver,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(PreisanfrageEvent::ValidationPassed { message_ref });
                } else {
                    events.push(PreisanfrageEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            PreisanfrageCommand::SendAngebot { message_ref } => {
                let data = match state {
                    PreisanfrageState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let response_pid = quotes_response_pid(data.pruefidentifikator.as_u32());
                Ok(vec![PreisanfrageEvent::AngebotGesendet {
                    response_pid,
                    message_ref,
                }]
                .into())
            }

            PreisanfrageCommand::ReceiveAngebot { pid, message_ref } => {
                if !QUOTES_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected QUOTES PID (15001–15005), got {pid}",
                    )));
                }
                match state {
                    PreisanfrageState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                Ok(vec![PreisanfrageEvent::AngebotErhalten {
                    response_pid: pid,
                    message_ref,
                }]
                .into())
            }

            PreisanfrageCommand::Abschliessen => {
                if !matches!(state, PreisanfrageState::AngebotAusgetauscht { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AngebotAusgetauscht",
                        state.label(),
                    ));
                }
                Ok(vec![PreisanfrageEvent::Abgeschlossen].into())
            }

            PreisanfrageCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    PreisanfrageState::Eingegangen(_) | PreisanfrageState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![PreisanfrageEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            PreisanfrageCommand::TimeoutExpired { deadline_id, label } => match state {
                PreisanfrageState::Abgeschlossen(_) | PreisanfrageState::Rejected { .. } => {
                    Ok(vec![].into())
                }
                _ => Ok(vec![PreisanfrageEvent::DeadlineExpired { deadline_id, label }].into()),
            },
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{ids::DeadlineId, workflow::Workflow};

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn apply_all(init: PreisanfrageState, events: &[PreisanfrageEvent]) -> PreisanfrageState {
        events.iter().fold(init, WimPreisanfrageWorkflow::apply)
    }

    #[test]
    fn reqote_happy_path_35001() {
        let out = WimPreisanfrageWorkflow::handle(
            &PreisanfrageState::New,
            PreisanfrageCommand::ReceiveReqote {
                pid: pid(35001),
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                message_ref: mref("REQOTE-001"),
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 2);
        let state = apply_all(PreisanfrageState::New, &out.events);
        assert!(matches!(state, PreisanfrageState::ValidationPassed(_)));

        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::SendAngebot {
                message_ref: mref("QUOTES-001"),
            },
        )
        .unwrap();
        if let PreisanfrageEvent::AngebotGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(15001));
        } else {
            panic!("expected AngebotGesendet");
        }
    }

    #[test]
    fn reqote_wrong_pid_rejected() {
        let result = WimPreisanfrageWorkflow::handle(
            &PreisanfrageState::New,
            PreisanfrageCommand::ReceiveReqote {
                pid: pid(55001),
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                message_ref: mref("X"),
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_abgeschlossen_is_noop() {
        let data = PreisanfrageData {
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            pruefidentifikator: pid(35001),
        };
        let state = PreisanfrageState::Abgeschlossen(data);
        let dl_id = DeadlineId::new();
        let out = WimPreisanfrageWorkflow::handle(
            &state,
            PreisanfrageCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: PREISANFRAGE_DEADLINE_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}
