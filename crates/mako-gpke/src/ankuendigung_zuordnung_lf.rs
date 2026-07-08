//! GPKE Ankündigung Zuordnung LF — NB notifies the new supplier of assignment.
//!
//! Covers GPKE Teil 2 §2.2 post-assignment notification: after a Lieferantenwechsel
//! or Neuanlage has been processed, the Netzbetreiber (NB) sends a UTILMD 55607
//! "Ankündigung Zuordnung LF" to the new Lieferant (LFN). The LFN must respond
//! within 24 wall-clock hours (BK6-22-024 §4) with either Bestätigung (55608) or
//! Ablehnung (55609).
//!
//! This module implements the **receiving-party perspective** (Lieferant / LFN):
//! the system receives an inbound Ankündigung (55607) from the NB and responds with
//! Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2, FV2025-10-01)
//!
//! ## Inbound (NB → LF)
//!
//! | PID   | Process name (AHB)                               | Direction |
//! |-------|--------------------------------------------------|-----------|
//! | 55607 | Ankündigung Zuordnung LF (NB → LFN)              | NB → LF   |
//!
//! ## Outbound (LF → NB)
//!
//! | PID   | Process name (AHB)                               | Derived from    |
//! |-------|--------------------------------------------------|-----------------|
//! | 55608 | Bestätigung Zuordnung LF (LFN → NB)              | 55607 accepted  |
//! | 55609 | Ablehnung Zuordnung LF (LFN → NB)                | 55607 rejected  |
//!
//! # Regulatory basis
//!
//! - **BNetzA BK6-24-174 Anlage 1b** — GPKE Teil 2 §2.2 Ankündigung Zuordnung LF
//! - **UTILMD S2.1/S2.2** — EDI@Energy message format
//! - **APERAK 2.x** — Application error acknowledgement (**24h** wall-clock Frist,
//!   per BK6-22-024 §4)

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

/// Inbound PID for Ankündigung Zuordnung LF handled by
/// [`GpkeAnkuendigungZuordnungLfWorkflow`].
///
/// | PID   | Process (AHB name)                          | AHB profile                    |
/// |-------|---------------------------------------------|--------------------------------|
/// | 55607 | Ankündigung Zuordnung LF (NB → LFN)         | S2.1–S2.2 — import pending ⚠  |
///
/// **Action required:** Run `cargo xtask import-xml-ahb --message-type UTILMD --pid 55607`
/// to populate AHB rules. Until then, the adapter applies a vacuous-validation guard
/// that forces `validation_passed = false` for all inbound 55607 messages.
///
/// Response PIDs (55608/55609) are derived internally and never routed as inbound.
pub const ANKUENDIGUNG_ZUORDNUNG_PIDS: &[u32] = &[55607];

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "gpke-ankuendigung-zuordnung-lf";

/// Deadline label for the 24h APERAK response window (BK6-22-024 §4).
///
/// Register immediately after `ValidationPassed`:
/// ```rust,ignore
/// let due = mako_engine::fristen::add_hours(received_at, 24);
/// let dl = Deadline::new(stream_id, ..., ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL, due);
/// deadline_store.register(&dl).await?;
/// ```
pub const ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL: &str =
    "gpke-ankuendigung-zuordnung-lf-aperak-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Ankündigung Zuordnung LF workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AnkuendigungZuordnungLfEvent {
    /// PID 55607 Ankündigung Zuordnung LF received from the NB.
    AnkuendigungErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LF.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Announced assignment date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55607).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound response (55608 or 55609) dispatched to the NB.
    AntwortGesendet {
        /// Response PID: 55608 (Bestätigung) or 55609 (Ablehnung).
        response_pid: Pruefidentifikator,
        /// `true` = accepted (Bestätigung), `false` = rejected (Ablehnung).
        accepted: bool,
        /// Rejection reason (when `accepted = false`).
        reason: Option<String>,
    },
    /// Supply location assignment acknowledged — process complete.
    Zugeordnet,
    /// APERAK 29001 dispatched for technical processing failure.
    AperakFehlerDispatched {
        /// APERAK PID sent.
        aperak_pid: Pruefidentifikator,
        /// Error reason included in the APERAK.
        reason: String,
        /// Reference ID of the outbound APERAK message.
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

impl EventPayload for AnkuendigungZuordnungLfEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnkuendigungErhalten { .. } => "AnkuendigungZuordnungLfAnkuendigungErhalten",
            Self::ValidationPassed { .. } => "AnkuendigungZuordnungLfValidationPassed",
            Self::AntwortGesendet { .. } => "AnkuendigungZuordnungLfAntwortGesendet",
            Self::Zugeordnet => "AnkuendigungZuordnungLfZugeordnet",
            Self::AperakFehlerDispatched { .. } => "AnkuendigungZuordnungLfAperakFehlerDispatched",
            Self::Rejected { .. } => "AnkuendigungZuordnungLfRejected",
            Self::DeadlineExpired { .. } => "AnkuendigungZuordnungLfDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnkuendigungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnkuendigungZuordnungLfData {
    /// EIC/MaLo code.
    pub location_id: MaLo,
    /// GLN of the NB who sent the assignment announcement.
    pub sender: MarktpartnerCode,
    /// GLN of the new LF (LFN).
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Announced assignment date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55607).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a GPKE Ankündigung Zuordnung LF process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Zugeordnet
///                                       ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AnkuendigungZuordnungLfState {
    /// No events yet.
    New,
    /// Ankündigung received; AHB validation pending.
    Eingegangen(AnkuendigungZuordnungLfData),
    /// Validation passed; response not yet sent.
    ValidationPassed(AnkuendigungZuordnungLfData),
    /// Response dispatched; awaiting assignment confirmation.
    AntwortGesendet {
        /// Data from the Ankündigung.
        data: AnkuendigungZuordnungLfData,
        /// Response PID sent (55608 or 55609).
        response_pid: Pruefidentifikator,
    },
    /// Assignment acknowledged — process complete.
    Zugeordnet(AnkuendigungZuordnungLfData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl Default for AnkuendigungZuordnungLfState {
    fn default() -> Self {
        Self::New
    }
}

impl AnkuendigungZuordnungLfState {
    /// Stable string label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::AntwortGesendet { .. } => "AntwortGesendet",
            Self::Zugeordnet(_) => "Zugeordnet",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Return `Some(&AnkuendigungZuordnungLfData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&AnkuendigungZuordnungLfData> {
        match self {
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Zugeordnet(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Ankündigung Zuordnung LF workflow.
#[derive(Clone)]
pub enum AnkuendigungZuordnungLfCommand {
    /// Inbound UTILMD PID 55607 Ankündigung received from the NB.
    ReceiveAnkuendigung {
        /// BDEW Prüfidentifikator (55607).
        pid: Pruefidentifikator,
        /// GLN of the NB.
        sender: MarktpartnerCode,
        /// GLN of the LFN.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Announced assignment date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response (55608 = Bestätigung, 55609 = Ablehnung).
    ///
    /// The LFN has 24 wall-clock hours (BK6-22-024 §4) to respond.
    SendAntwort {
        /// `true` = Bestätigung (55608), `false` = Ablehnung (55609).
        accepted: bool,
        /// Rejection reason (required when `accepted = false`).
        reason: Option<String>,
    },
    /// Confirm that the assignment has been acknowledged and the process is complete.
    ZuordnungBestaetigen,
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

impl CommandPayload for AnkuendigungZuordnungLfCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Ankündigung Zuordnung LF workflow (PID 55607 inbound, 55608/55609 outbound).
///
/// The NB sends a UTILMD 55607 "Ankündigung Zuordnung LF" to the new Lieferant
/// (LFN) after a Lieferantenwechsel or Neuanlage has been processed. The LFN
/// must respond within 24 wall-clock hours (BK6-22-024 §4).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeAnkuendigungZuordnungLfWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-ankuendigung-zuordnung-lf", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeAnkuendigungZuordnungLfWorkflow;

impl Workflow for GpkeAnkuendigungZuordnungLfWorkflow {
    type State = AnkuendigungZuordnungLfState;
    type Event = AnkuendigungZuordnungLfEvent;
    type Command = AnkuendigungZuordnungLfCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL,
                AnkuendigungZuordnungLfState::Eingegangen(_),
            )
            | (
                ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL,
                AnkuendigungZuordnungLfState::ValidationPassed(_),
            ) => Some(AnkuendigungZuordnungLfCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AnkuendigungZuordnungLfEvent::AnkuendigungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                ..
            } => AnkuendigungZuordnungLfState::Eingegangen(AnkuendigungZuordnungLfData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            AnkuendigungZuordnungLfEvent::ValidationPassed { .. } => match state {
                AnkuendigungZuordnungLfState::Eingegangen(data) => {
                    AnkuendigungZuordnungLfState::ValidationPassed(data)
                }
                other => other,
            },
            AnkuendigungZuordnungLfEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        AnkuendigungZuordnungLfState::ValidationPassed(data) => {
                            AnkuendigungZuordnungLfState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    AnkuendigungZuordnungLfState::Rejected {
                        reason: "Zuordnung abgelehnt".to_owned(),
                    }
                }
            }
            AnkuendigungZuordnungLfEvent::Zugeordnet => match state {
                AnkuendigungZuordnungLfState::AntwortGesendet { data, .. } => {
                    AnkuendigungZuordnungLfState::Zugeordnet(data)
                }
                other => other,
            },
            AnkuendigungZuordnungLfEvent::AperakFehlerDispatched { reason, .. } => {
                AnkuendigungZuordnungLfState::Rejected {
                    reason: format!("APERAK 29001: {reason}"),
                }
            }
            AnkuendigungZuordnungLfEvent::Rejected { reason } => {
                AnkuendigungZuordnungLfState::Rejected {
                    reason: reason.clone(),
                }
            }
            AnkuendigungZuordnungLfEvent::DeadlineExpired { label, .. } => match state {
                AnkuendigungZuordnungLfState::Zugeordnet(_)
                | AnkuendigungZuordnungLfState::Rejected { .. } => state,
                _ => AnkuendigungZuordnungLfState::Rejected {
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
            AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
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
                if !matches!(state, AnkuendigungZuordnungLfState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ANKUENDIGUNG_ZUORDNUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Ankündigung Zuordnung LF PID (55607), got {pid}",
                    )));
                }
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![AnkuendigungZuordnungLfEvent::AnkuendigungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(AnkuendigungZuordnungLfEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — mandatory per APERAK AHB 1.0 §2.4.
                    // Strom UTILMD (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
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
                    events.push(AnkuendigungZuordnungLfEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 — mandatory per APERAK AHB 1.0 §2.1.1.
                    // Strom UTILMD (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
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

            AnkuendigungZuordnungLfCommand::SendAntwort { accepted, reason } => {
                match state {
                    AnkuendigungZuordnungLfState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let response_code: u32 = if accepted { 55608 } else { 55609 };
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![AnkuendigungZuordnungLfEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason,
                }]
                .into())
            }

            AnkuendigungZuordnungLfCommand::ZuordnungBestaetigen => {
                if !matches!(state, AnkuendigungZuordnungLfState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![AnkuendigungZuordnungLfEvent::Zugeordnet].into())
            }

            AnkuendigungZuordnungLfCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    AnkuendigungZuordnungLfState::Eingegangen(_)
                    | AnkuendigungZuordnungLfState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![AnkuendigungZuordnungLfEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            AnkuendigungZuordnungLfCommand::TimeoutExpired { deadline_id, label } => match state {
                AnkuendigungZuordnungLfState::Zugeordnet(_)
                | AnkuendigungZuordnungLfState::Rejected { .. } => Ok(vec![].into()),
                _ => Ok(
                    vec![AnkuendigungZuordnungLfEvent::DeadlineExpired { deadline_id, label }]
                        .into(),
                ),
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
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }

    fn ankuendigung_cmd(ok: bool) -> AnkuendigungZuordnungLfCommand {
        AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
            pid: pid(55607),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("ZUORD-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    fn apply_all(
        init: AnkuendigungZuordnungLfState,
        events: &[AnkuendigungZuordnungLfEvent],
    ) -> AnkuendigungZuordnungLfState {
        events
            .iter()
            .fold(init, GpkeAnkuendigungZuordnungLfWorkflow::apply)
    }

    #[test]
    fn happy_path_bestaetigung() {
        // ── Step 1: receive 55607 Ankündigung ────────────────────────────────
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            ankuendigung_cmd(true),
        )
        .unwrap();
        assert_eq!(out.events.len(), 2); // AnkuendigungErhalten + ValidationPassed
        let state = apply_all(AnkuendigungZuordnungLfState::New, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::ValidationPassed(_)),
            "must be ValidationPassed after receiving 55607"
        );

        // ── Step 2: LFN accepts (→ 55608 Bestätigung) ───────────────────────
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &state,
            AnkuendigungZuordnungLfCommand::SendAntwort {
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        if let AnkuendigungZuordnungLfEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(accepted);
            assert_eq!(response_pid.as_u32(), 55608);
        } else {
            panic!("expected AntwortGesendet event");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            AnkuendigungZuordnungLfState::AntwortGesendet { .. }
        ));

        // ── Step 3: confirm assignment complete ──────────────────────────────
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &state,
            AnkuendigungZuordnungLfCommand::ZuordnungBestaetigen,
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::Zugeordnet(_)),
            "must be Zugeordnet after ZuordnungBestaetigen"
        );
        if let AnkuendigungZuordnungLfState::Zugeordnet(data) = state {
            assert_eq!(data.location_id.as_str(), "51238696781");
            assert_eq!(data.pruefidentifikator.as_u32(), 55607);
        }
    }

    #[test]
    fn ablehnung_path() {
        // ── Receive 55607 ────────────────────────────────────────────────────
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            ankuendigung_cmd(true),
        )
        .unwrap();
        let state = apply_all(AnkuendigungZuordnungLfState::New, &out.events);

        // ── LFN rejects (→ 55609 Ablehnung) ─────────────────────────────────
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &state,
            AnkuendigungZuordnungLfCommand::SendAntwort {
                accepted: false,
                reason: Some("Unbekannte Marktlokation".to_owned()),
            },
        )
        .unwrap();
        assert_eq!(out.events.len(), 1);
        if let AnkuendigungZuordnungLfEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(!accepted);
            assert_eq!(response_pid.as_u32(), 55609);
        } else {
            panic!("expected AntwortGesendet event");
        }
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::Rejected { .. }),
            "must be Rejected after SendAntwort(accepted=false)"
        );
    }

    #[test]
    fn validation_failure_rejects_immediately() {
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            ankuendigung_cmd(false),
        )
        .unwrap();
        assert_eq!(out.events.len(), 2); // AnkuendigungErhalten + Rejected
        let state = apply_all(AnkuendigungZuordnungLfState::New, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::Rejected { .. }),
            "must be Rejected on validation failure"
        );
    }

    #[test]
    fn wrong_pid_rejected() {
        let bad_cmd = AnkuendigungZuordnungLfCommand::ReceiveAnkuendigung {
            pid: pid(55007), // NB-Lieferende, not Zuordnung LF
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("WRONG-001"),
            validation_passed: true,
            validation_errors: vec![],
        };
        let result = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            bad_cmd,
        );
        assert!(result.is_err(), "wrong PID must produce an error");
    }

    #[test]
    fn duplicate_receive_rejected() {
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            ankuendigung_cmd(true),
        )
        .unwrap();
        let state = apply_all(AnkuendigungZuordnungLfState::New, &out.events);

        // Second ReceiveAnkuendigung in non-New state must fail.
        let result = GpkeAnkuendigungZuordnungLfWorkflow::handle(&state, ankuendigung_cmd(true));
        assert!(result.is_err(), "duplicate ReceiveAnkuendigung must fail");
    }

    #[test]
    fn send_antwort_rejected_in_new_state() {
        let result = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            AnkuendigungZuordnungLfCommand::SendAntwort {
                accepted: true,
                reason: None,
            },
        );
        assert!(
            result.is_err(),
            "SendAntwort in New state must produce an error"
        );
    }

    #[test]
    fn zuordnung_bestaetigen_rejected_in_wrong_state() {
        let result = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            AnkuendigungZuordnungLfCommand::ZuordnungBestaetigen,
        );
        assert!(
            result.is_err(),
            "ZuordnungBestaetigen in New state must produce an error"
        );
    }

    #[test]
    fn deadline_in_eingegangen_closes_process() {
        // Construct Eingegangen state directly (bypass handle to skip validation).
        let eingegangen = AnkuendigungZuordnungLfState::Eingegangen(AnkuendigungZuordnungLfData {
            location_id: malo("51238696781"),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55607),
        });
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &eingegangen,
            AnkuendigungZuordnungLfCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(eingegangen, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::Rejected { .. }),
            "deadline in Eingegangen must close process as Rejected"
        );
    }

    #[test]
    fn deadline_in_terminal_state_is_noop() {
        let terminal = AnkuendigungZuordnungLfState::Rejected {
            reason: "already closed".to_owned(),
        };
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &terminal,
            AnkuendigungZuordnungLfCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            out.events.is_empty(),
            "deadline in terminal state must emit no events"
        );
    }

    #[test]
    fn deadline_in_validation_passed_closes_process() {
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &AnkuendigungZuordnungLfState::New,
            ankuendigung_cmd(true),
        )
        .unwrap();
        let state = apply_all(AnkuendigungZuordnungLfState::New, &out.events);
        assert!(matches!(
            state,
            AnkuendigungZuordnungLfState::ValidationPassed(_)
        ));

        // Deadline fires before LFN responds.
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &state,
            AnkuendigungZuordnungLfCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(
            matches!(state, AnkuendigungZuordnungLfState::Rejected { .. }),
            "deadline in ValidationPassed must close process as Rejected"
        );
    }

    #[test]
    fn deadline_in_terminal_zugeordnet_state_is_noop() {
        let terminal = AnkuendigungZuordnungLfState::Zugeordnet(AnkuendigungZuordnungLfData {
            location_id: malo("51238696781"),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55607),
        });
        let out = GpkeAnkuendigungZuordnungLfWorkflow::handle(
            &terminal,
            AnkuendigungZuordnungLfCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANKUENDIGUNG_ZUORDNUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(
            out.events.is_empty(),
            "deadline in terminal Zugeordnet state must emit no events"
        );
    }
}
