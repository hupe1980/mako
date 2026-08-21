//! GPKE Anfrage zur Beendigung der Zuordnung — NB-initiated Abmeldeanfrage.
//!
//! GPKE Teil 2: the Netzbetreiber asks the current Lieferant (LFA) to end the
//! Zuordnung of a Marktlokation (e.g. after a Netzbetreiberwechsel or when the
//! NB's records require the assignment to end). The LFA confirms (55011) or
//! rejects (55012). This mirrors the GeLi Gas Abmeldungsanfrage (44010–44012).
//!
//! This module implements the **receiving-party perspective** (Lieferant / LFA):
//! the system receives the inbound Anfrage from the NB and responds with
//! Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! ## Inbound (NB → LFA)
//!
//! | PID   | Process name (AHB)                            | Direction |
//! |-------|-----------------------------------------------|-----------|
//! | 55010 | Anfrage zur Beendigung der Zuordnung (NB→LFA) | NB → LFA  |
//!
//! ## Outbound (LFA → NB)
//!
//! | PID   | Process name (AHB)                            | Derived from   |
//! |-------|-----------------------------------------------|----------------|
//! | 55011 | Bestätigung Beendigung der Zuordnung (LFA→NB) | 55010 accepted |
//! | 55012 | Ablehnung Beendigung der Zuordnung (LFA→NB)   | 55010 rejected |
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 2** — Beendigung der Zuordnung
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK 2.x** — **24h** wall-clock Frist (BK6-22-024 §4)

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-beendigung-zuordnung";

/// Inbound PIDs handled by [`GpkeBeendigungZuordnungWorkflow`].
///
/// | PID   | Process (AHB name)                            | AHB profile  |
/// |-------|-----------------------------------------------|--------------|
/// | 55010 | Anfrage zur Beendigung der Zuordnung (NB→LFA) | S2.1–S2.2 ✅ |
pub const BEENDIGUNG_ZUORDNUNG_PIDS: &[u32] = &[55010];

/// Deadline label for the 24h APERAK Frist (BK6-22-024 §4).
pub const BEENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL: &str =
    "gpke-beendigung-zuordnung-aperak-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Beendigung-der-Zuordnung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum BeendigungZuordnungEvent {
    /// PID 55010 Anfrage zur Beendigung der Zuordnung received.
    AnfrageErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LFA.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Requested Zuordnungsende date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55010).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound response (55011 or 55012) dispatched to the NB.
    AntwortGesendet {
        /// Response PID actually dispatched.
        response_pid: Pruefidentifikator,
        /// `true` = Bestätigung, `false` = Ablehnung — read from the
        /// Antwortcode's published Cluster, never supplied separately.
        accepted: bool,
        /// The answer as sent, kept for the audit trail.
        antwort: crate::lf_antwort::LfAntwort,
    },
    /// Zuordnung ended per the NB Anfrage.
    Beendet,
    /// APERAK 29001 dispatched for technical failure.
    AperakFehlerDispatched {
        /// APERAK PID.
        aperak_pid: Pruefidentifikator,
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
    },
    /// Process rejected due to validation failure or deadline expiry.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl EventPayload for BeendigungZuordnungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageErhalten { .. } => "BeendigungZuordnungAnfrageErhalten",
            Self::ValidationPassed { .. } => "BeendigungZuordnungValidationPassed",
            Self::AntwortGesendet { .. } => "BeendigungZuordnungAntwortGesendet",
            Self::Beendet => "BeendigungZuordnungBeendet",
            Self::AperakFehlerDispatched { .. } => "BeendigungZuordnungAperakFehlerDispatched",
            Self::Rejected { .. } => "BeendigungZuordnungRejected",
            Self::DeadlineExpired { .. } => "BeendigungZuordnungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnfrageErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeendigungZuordnungData {
    /// EIC/MaLo code.
    pub location_id: MaLo,
    /// GLN of the NB who initiated the request.
    pub sender: MarktpartnerCode,
    /// GLN of the affected LFA.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Requested Zuordnungsende date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55010).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a GPKE Beendigung-der-Zuordnung process.
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Beendet
///                                       ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum BeendigungZuordnungState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received.
    Eingegangen(BeendigungZuordnungData),
    /// Validation passed; response not yet sent.
    ValidationPassed(BeendigungZuordnungData),
    /// Response dispatched; awaiting Zuordnungsende confirmation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: BeendigungZuordnungData,
        /// Response PID sent (55011 or 55012).
        response_pid: Pruefidentifikator,
    },
    /// Zuordnung ended.
    Beendet(BeendigungZuordnungData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl BeendigungZuordnungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Beendet(_) => "Beendet",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&BeendigungZuordnungData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&BeendigungZuordnungData> {
        match self {
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Beendet(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Beendigung-der-Zuordnung workflow.
#[derive(Clone)]
pub enum BeendigungZuordnungCommand {
    /// Inbound UTILMD PID 55010 Anfrage received from the AS4 layer.
    ReceiveAnfrage {
        /// BDEW Prüfidentifikator (55010).
        pid: Pruefidentifikator,
        /// GLN of the NB.
        sender: MarktpartnerCode,
        /// GLN of the LFA.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Requested Zuordnungsende date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response (55011 = Bestätigung, 55012 = Ablehnung).
    ///
    /// The LFA has 24 wall-clock hours (BK6-22-024 §4) to respond.
    SendAntwort {
        /// The resolved answer: Antwortcode, its EBD, and the Cluster that
        /// selects the response PID.
        antwort: crate::lf_antwort::LfAntwort,
    },
    /// Record that the Zuordnung has ended.
    BeendenBestaetigen,
    /// Dispatch APERAK 29001 for technical processing failure.
    DispatchAperakFehler {
        /// Error reason.
        reason: String,
        /// Outbound APERAK message reference.
        outbound_ref: MessageRef,
    },
    /// A registered deadline fired; close the process.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for BeendigungZuordnungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Anfrage-zur-Beendigung-der-Zuordnung workflow (55010 inbound, 55011/55012
/// outbound).
pub struct GpkeBeendigungZuordnungWorkflow;

impl Workflow for GpkeBeendigungZuordnungWorkflow {
    type State = BeendigungZuordnungState;
    type Event = BeendigungZuordnungEvent;
    type Command = BeendigungZuordnungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                BEENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL,
                BeendigungZuordnungState::Eingegangen(_)
                | BeendigungZuordnungState::ValidationPassed(_),
            ) => Some(BeendigungZuordnungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            BeendigungZuordnungEvent::AnfrageErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                ..
            } => BeendigungZuordnungState::Eingegangen(BeendigungZuordnungData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            BeendigungZuordnungEvent::ValidationPassed { .. } => match state {
                BeendigungZuordnungState::Eingegangen(data) => {
                    BeendigungZuordnungState::ValidationPassed(data)
                }
                other => other,
            },
            BeendigungZuordnungEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        BeendigungZuordnungState::ValidationPassed(data) => {
                            BeendigungZuordnungState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    BeendigungZuordnungState::Rejected {
                        reason: "Anfrage abgelehnt".to_owned(),
                    }
                }
            }
            BeendigungZuordnungEvent::Beendet => match state {
                BeendigungZuordnungState::AntwortGesendet { data, .. } => {
                    BeendigungZuordnungState::Beendet(data)
                }
                other => other,
            },
            BeendigungZuordnungEvent::AperakFehlerDispatched { reason, .. } => {
                BeendigungZuordnungState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            BeendigungZuordnungEvent::Rejected { reason } => BeendigungZuordnungState::Rejected {
                reason: reason.clone(),
            },
            BeendigungZuordnungEvent::DeadlineExpired { label, .. } => match state {
                BeendigungZuordnungState::Beendet(_)
                | BeendigungZuordnungState::Rejected { .. } => state,
                _ => BeendigungZuordnungState::Rejected {
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
            BeendigungZuordnungCommand::ReceiveAnfrage {
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
                if !matches!(state, BeendigungZuordnungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !BEENDIGUNG_ZUORDNUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Anfrage zur Beendigung der Zuordnung PID (55010), got {pid}",
                    )));
                }
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![BeendigungZuordnungEvent::AnfrageErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(BeendigungZuordnungEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — APERAK AHB 1.0 §2.4.
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
                    events.push(BeendigungZuordnungEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 — APERAK AHB 1.0 §2.1.1.
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

            BeendigungZuordnungCommand::SendAntwort { antwort } => {
                let data = match state {
                    BeendigungZuordnungState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                // The Cluster the Antwortcode sits in decides the PID. A caller
                // cannot pick one independently of the other — that is how an
                // Ablehnungscode could otherwise ride a Bestätigung.
                let accepted = antwort.zustimmung;
                let response_code: u32 = if accepted { 55011 } else { 55012 };
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;

                // The outbox entry *is* the answer. Without it the event log
                // recorded the process as answered while the counterparty saw
                // nothing but its Frist expire.
                let outbox = vec![
                    crate::lf_antwort::antwort_outbox(
                        response_code,
                        &antwort,
                        &data.location_id,
                        &data.sender,
                        &data.receiver,
                        &data.process_date,
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![BeendigungZuordnungEvent::AntwortGesendet {
                        response_pid,
                        accepted,
                        antwort,
                    }],
                    outbox,
                ))
            }

            BeendigungZuordnungCommand::BeendenBestaetigen => {
                if !matches!(state, BeendigungZuordnungState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![BeendigungZuordnungEvent::Beendet].into())
            }

            BeendigungZuordnungCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    BeendigungZuordnungState::Eingegangen(_)
                    | BeendigungZuordnungState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                Ok(vec![BeendigungZuordnungEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            BeendigungZuordnungCommand::TimeoutExpired { deadline_id, label } => match state {
                BeendigungZuordnungState::Beendet(_)
                | BeendigungZuordnungState::Rejected { .. } => Ok(vec![].into()),
                _ => Ok(
                    vec![BeendigungZuordnungEvent::DeadlineExpired { deadline_id, label }].into(),
                ),
            },
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::workflow::Workflow;

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

    fn anfrage_cmd(ok: bool) -> BeendigungZuordnungCommand {
        BeendigungZuordnungCommand::ReceiveAnfrage {
            pid: pid(55010),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("BEEND-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    fn apply_all(
        init: BeendigungZuordnungState,
        events: &[BeendigungZuordnungEvent],
    ) -> BeendigungZuordnungState {
        events
            .iter()
            .fold(init, GpkeBeendigungZuordnungWorkflow::apply)
    }

    #[test]
    fn happy_path_bestaetigung() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(true),
        )
        .unwrap();
        // AnfrageErhalten + ValidationPassed events; APERAK 312 outbox.
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].payload["document_code"], "312");
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        assert!(matches!(
            state,
            BeendigungZuordnungState::ValidationPassed(_)
        ));

        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::zustimmung("A36", "E_0624"),
            },
        )
        .unwrap();
        if let BeendigungZuordnungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55011);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::BeendenBestaetigen,
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Beendet(_)));
    }

    #[test]
    fn ablehnung_yields_55012() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(true),
        )
        .unwrap();
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &state,
            BeendigungZuordnungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::ablehnung("A35", "E_0624")
                    .with_bemerkung("Widerspruch"),
            },
        )
        .unwrap();
        if let BeendigungZuordnungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55012);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Rejected { .. }));
    }

    #[test]
    fn validation_failure_emits_aperak_313() {
        let out = GpkeBeendigungZuordnungWorkflow::handle(
            &BeendigungZuordnungState::New,
            anfrage_cmd(false),
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(BeendigungZuordnungState::New, &out.events);
        assert!(matches!(state, BeendigungZuordnungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let mut cmd = anfrage_cmd(true);
        if let BeendigungZuordnungCommand::ReceiveAnfrage { pid: p, .. } = &mut cmd {
            *p = pid(55001);
        }
        assert!(
            GpkeBeendigungZuordnungWorkflow::handle(&BeendigungZuordnungState::New, cmd).is_err()
        );
    }
}
