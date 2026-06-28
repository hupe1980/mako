//! GPKE Neuanlage — new Marktlokation registration process.
//!
//! Covers the GPKE Neuanlage process per BNetzA BK6-24-174 Anlage 1b (GPKE
//! Teil 2 §2.2): registration of a **new** Marktlokation not previously in the
//! grid topology. This is distinct from Lieferbeginn (PID 55001) which assumes
//! the location already exists.
//!
//! This module implements the **receiving-party perspective** (Netzbetreiber / NB):
//! the system receives an inbound Anmeldung Neuanlage from the Lieferant/Einspeiser
//! and responds with Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! ## Inbound ANFRAGE — routed to `gpke-neuanlage`
//!
//! | PID   | Process name (AHB)                               | Direction |
//! |-------|--------------------------------------------------|-----------|
//! | 55600 | Anmeldung neue verb. MaLo (LF → NB)             | LF → NB   |
//! | 55601 | Anmeldung neue erz. MaLo (LF → NB)              | LF → NB   |
//!
//! ## Outbound ANTWORT — derived by this workflow, NOT routed as inbound
//!
//! | PID   | Process name (AHB)                               | Derived from  |
//! |-------|--------------------------------------------------|---------------|
//! | 55602 | Bestätigung Anmeldung neue verb. MaLo (NB → LF) | 55600 accepted|
//! | 55603 | Bestätigung Anmeldung neue erz. MaLo (NB → LF)  | 55601 accepted|
//! | 55604 | Ablehnung Anmeldung neue verb. MaLo (NB → LF)   | 55600 rejected|
//! | 55605 | Ablehnung Anmeldung neue erz. MaLo (NB → LF)    | 55601 rejected|
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 1b** — GPKE Teil 2 §2.2 Neuanlage
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK 2.x** — Application error acknowledgement (**24h** wall-clock Frist,
//!   same as Lieferbeginn/Lieferende per BK6-22-024)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Inbound ANFRAGE PIDs for Neuanlage handled by [`GpkeNeuanlageWorkflow`].
///
/// | PID   | Process (AHB name)                                  | AHB profile  |
/// |-------|-----------------------------------------------------|--------------|
/// | 55600 | Anmeldung neue verb. MaLo (LF → NB)                | S2.1–S2.2 ✅ |
/// | 55601 | Anmeldung neue erz. MaLo (LF → NB)                 | S2.1–S2.2 ✅ |
///
/// Response PIDs (55602–55605) are derived internally and never routed.
pub const NEUANLAGE_PIDS: &[u32] = &[55600, 55601];

/// Deadline label for the 24h APERAK response window (BK6-22-024).
///
/// Register immediately after `ValidationPassed`:
/// ```rust,ignore
/// let due = mako_engine::fristen::add_hours(received_at, 24);
/// let dl = Deadline::new(stream_id, ..., NEUANLAGE_APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&dl).await?;
/// ```
pub const NEUANLAGE_APERAK_WINDOW_LABEL: &str = "gpke-neuanlage-aperak-window";

// ── Response PID derivation ───────────────────────────────────────────────────

/// Derive the outbound UTILMD response PID for a Neuanlage Anfrage.
///
/// | Anfrage | accepted | Response PID |
/// |---------|----------|--------------|
/// | 55600   | true     | 55602        |
/// | 55600   | false    | 55604        |
/// | 55601   | true     | 55603        |
/// | 55601   | false    | 55605        |
fn neuanlage_response_pid(anfrage_pid: u32, accepted: bool) -> Option<Pruefidentifikator> {
    let code: u32 = match anfrage_pid {
        55600 => {
            if accepted {
                55602
            } else {
                55604
            }
        }
        55601 => {
            if accepted {
                55603
            } else {
                55605
            }
        }
        _ => return None,
    };
    Pruefidentifikator::new(code).ok()
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Neuanlage workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum NeuanlageEvent {
    /// UTILMD Neuanlage Anfrage received.
    AnmeldungErhalten {
        /// Marktlokation EIC code (to be assigned).
        location_id: MaLo,
        /// GLN of the Lieferant / Einspeiser.
        sender: MarktpartnerCode,
        /// GLN of the Netzbetreiber.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55600 or 55601).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation (no rule violations).
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound UTILMD response (55602–55605) was sent.
    AntwortGesendet {
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
        /// `true` = accepted (Bestätigung), `false` = rejected (Ablehnung).
        accepted: bool,
        /// Rejection reason (set only when `accepted = false`).
        reason: Option<String>,
    },
    /// Neuanlage completed — MaLo is active in the grid.
    Aktiviert,
    /// APERAK 29001 dispatched for technical processing failure.
    AperakFehlerDispatched {
        /// APERAK PID sent.
        aperak_pid: Pruefidentifikator,
        /// Error reason included in the APERAK.
        reason: String,
        /// Reference ID of the outbound APERAK message.
        outbound_ref: MessageRef,
    },
    /// Process closed due to validation failure, rejection, or deadline expiry.
    Rejected {
        /// Human-readable reason.
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

impl EventPayload for NeuanlageEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnmeldungErhalten { .. } => "NeuanlageAnmeldungErhalten",
            Self::ValidationPassed { .. } => "NeuanlageValidationPassed",
            Self::AntwortGesendet { .. } => "NeuanlageAntwortGesendet",
            Self::Aktiviert => "NeuanlageAktiviert",
            Self::AperakFehlerDispatched { .. } => "NeuanlageAperakFehlerDispatched",
            Self::Rejected { .. } => "NeuanlageRejected",
            Self::DeadlineExpired { .. } => "NeuanlageDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnmeldungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NeuanlageData {
    /// EIC/MaLo code of the new location.
    pub location_id: MaLo,
    /// GLN of the requesting Lieferant / Einspeiser.
    pub sender: MarktpartnerCode,
    /// GLN of the responsible Netzbetreiber.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Requested connection date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55600 or 55601).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a GPKE Neuanlage process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Aktiviert
///                                      ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum NeuanlageState {
    /// No events yet.
    New,
    /// Anfrage received; AHB validation pending.
    Eingegangen(NeuanlageData),
    /// Validation passed; response not yet sent.
    ValidationPassed(NeuanlageData),
    /// Response sent; awaiting MaLo activation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: NeuanlageData,
        /// Derived outbound response PID.
        response_pid: Option<Pruefidentifikator>,
    },
    /// MaLo is active in the grid.
    Aktiviert(NeuanlageData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl Default for NeuanlageState {
    fn default() -> Self {
        Self::New
    }
}

impl NeuanlageState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Aktiviert(_) => "Aktiviert",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&NeuanlageData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&NeuanlageData> {
        match self {
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Aktiviert(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Neuanlage workflow.
#[derive(Clone)]
pub enum NeuanlageCommand {
    /// Inbound UTILMD Neuanlage Anfrage received from the AS4 layer.
    ReceiveAnmeldung {
        /// BDEW Prüfidentifikator (55600 or 55601).
        pid: Pruefidentifikator,
        /// GLN of the sender (LF).
        sender: MarktpartnerCode,
        /// GLN of the receiver (NB).
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (YYYYMMDD).
        document_date: String,
        /// Requested connection date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings (for the `Rejected` event).
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response to the Lieferant.
    ///
    /// Derive the response PID from the Anfrage PID:
    /// - 55600 → 55602 (accepted) / 55604 (rejected)
    /// - 55601 → 55603 (accepted) / 55605 (rejected)
    ///
    /// BK6-24-174 / BK6-22-024: Response within **24 wall-clock hours**.
    SendAntwort {
        /// `true` = Bestätigung, `false` = Ablehnung.
        accepted: bool,
        /// Rejection reason (required when `accepted = false`).
        reason: Option<String>,
    },
    /// Mark the new Marktlokation as active in the grid.
    Aktivieren,
    /// Dispatch APERAK 29001 for technical processing failure.
    DispatchAperakFehler {
        /// Error reason in the APERAK.
        reason: String,
        /// Message reference of the outbound APERAK.
        outbound_ref: MessageRef,
    },
    /// A registered deadline fired; record expiry and close the process.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for NeuanlageCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Neuanlage workflow (PIDs 55600/55601).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeNeuanlageWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-neuanlage", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeNeuanlageWorkflow;

impl Workflow for GpkeNeuanlageWorkflow {
    type State = NeuanlageState;
    type Event = NeuanlageEvent;
    type Command = NeuanlageCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (NEUANLAGE_APERAK_WINDOW_LABEL, NeuanlageState::Eingegangen(_))
            | (NEUANLAGE_APERAK_WINDOW_LABEL, NeuanlageState::ValidationPassed(_)) => {
                Some(NeuanlageCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            NeuanlageEvent::AnmeldungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                ..
            } => NeuanlageState::Eingegangen(NeuanlageData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            NeuanlageEvent::ValidationPassed { .. } => match state {
                NeuanlageState::Eingegangen(data) => NeuanlageState::ValidationPassed(data),
                other => other,
            },
            NeuanlageEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        NeuanlageState::ValidationPassed(data) => NeuanlageState::AntwortGesendet {
                            response_pid: *response_pid,
                            data,
                        },
                        other => other,
                    }
                } else {
                    NeuanlageState::Rejected {
                        reason: "Neuanlage abgelehnt".to_owned(),
                    }
                }
            }
            NeuanlageEvent::Aktiviert => match state {
                NeuanlageState::AntwortGesendet { data, .. } => NeuanlageState::Aktiviert(data),
                other => other,
            },
            NeuanlageEvent::AperakFehlerDispatched { reason, .. } => NeuanlageState::Rejected {
                reason: format!("APERAK 29001: {reason}"),
            },
            NeuanlageEvent::Rejected { reason } => NeuanlageState::Rejected {
                reason: reason.clone(),
            },
            NeuanlageEvent::DeadlineExpired { label, .. } => match state {
                NeuanlageState::Aktiviert(_) | NeuanlageState::Rejected { .. } => state,
                _ => NeuanlageState::Rejected {
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
            NeuanlageCommand::ReceiveAnmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, NeuanlageState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !NEUANLAGE_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Neuanlage PID (55600 or 55601), got {pid}",
                    )));
                }
                let mut events = vec![NeuanlageEvent::AnmeldungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(NeuanlageEvent::ValidationPassed { message_ref });
                } else {
                    events.push(NeuanlageEvent::Rejected {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }

            NeuanlageCommand::SendAntwort { accepted, reason } => {
                let data = match state {
                    NeuanlageState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let response_pid =
                    neuanlage_response_pid(data.pruefidentifikator.as_u32(), accepted);
                Ok(vec![NeuanlageEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason,
                }]
                .into())
            }

            NeuanlageCommand::Aktivieren => {
                if !matches!(state, NeuanlageState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![NeuanlageEvent::Aktiviert].into())
            }

            NeuanlageCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    NeuanlageState::Eingegangen(_) | NeuanlageState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![NeuanlageEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            NeuanlageCommand::TimeoutExpired { deadline_id, label } => {
                // Idempotent: no-op in terminal states.
                match state {
                    NeuanlageState::Aktiviert(_) | NeuanlageState::Rejected { .. } => {
                        Ok(vec![].into())
                    }
                    _ => Ok(vec![NeuanlageEvent::DeadlineExpired { deadline_id, label }].into()),
                }
            }
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
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn anmeldung_cmd(pid_code: u32, ok: bool) -> NeuanlageCommand {
        NeuanlageCommand::ReceiveAnmeldung {
            pid: pid(pid_code),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("NEUA-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    #[test]
    fn neuanlage_55600_happy_path() {
        // Step 1: receive 55600 Anmeldung
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, true))
            .unwrap();
        assert_eq!(out.events.len(), 2); // AnmeldungErhalten + ValidationPassed
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::ValidationPassed(_)));

        // Step 2: send Bestätigung
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(accepted);
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(55602));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out
            .events
            .iter()
            .fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::AntwortGesendet { .. }));

        // Step 3: activate
        let out = GpkeNeuanlageWorkflow::handle(&state, NeuanlageCommand::Aktivieren).unwrap();
        let state = out
            .events
            .iter()
            .fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Aktiviert(_)));
    }

    #[test]
    fn neuanlage_55601_rejected() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55601, true))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::SendAntwort {
                accepted: false,
                reason: Some("Kapazitätsmangel".to_owned()),
            },
        )
        .unwrap();
        if let NeuanlageEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(!accepted);
            assert_eq!(response_pid.map(|p| p.as_u32()), Some(55605));
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = out
            .events
            .iter()
            .fold(state, GpkeNeuanlageWorkflow::apply);
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_validation_failure_rejects() {
        let out = GpkeNeuanlageWorkflow::handle(&NeuanlageState::New, anmeldung_cmd(55600, false))
            .unwrap();
        let state = out.events.iter().fold(NeuanlageState::New, |s, e| {
            GpkeNeuanlageWorkflow::apply(s, e)
        });
        assert!(matches!(state, NeuanlageState::Rejected { .. }));
    }

    #[test]
    fn neuanlage_wrong_pid_rejected() {
        let result = GpkeNeuanlageWorkflow::handle(
            &NeuanlageState::New,
            NeuanlageCommand::ReceiveAnmeldung {
                pid: pid(55001), // wrong PID
                sender: mcod("4012345000023"),
                receiver: mcod("9900357000004"),
                location_id: malo("51238696781"),
                document_date: "20251001".to_owned(),
                process_date: "20260101".to_owned(),
                message_ref: mref("NEUA-X"),
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_terminal_state_is_noop() {
        let state = NeuanlageState::Aktiviert(NeuanlageData {
            location_id: malo("51238696781"),
            sender: mcod("4012345000023"),
            receiver: mcod("9900357000004"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55600),
        });
        let dl_id = DeadlineId::new();
        let out = GpkeNeuanlageWorkflow::handle(
            &state,
            NeuanlageCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: NEUANLAGE_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}
