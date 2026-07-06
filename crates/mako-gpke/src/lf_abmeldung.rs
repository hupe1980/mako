//! GPKE NB-initiated Lieferende — Kündigung by the Netzbetreiber.
//!
//! Covers GPKE Teil 2 §2.5: the Netzbetreiber proactively terminates a supply
//! relationship (e.g., for non-payment under §41 EnWG or after judicial order).
//! This is distinct from Lieferende (PID 55002) where the **Lieferant** initiates.
//!
//! This module implements the **receiving-party perspective** (Lieferant / LFN):
//! the system receives an inbound Ankündigung from the NB and responds with
//! Bestätigung or Ablehnung.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2, FV2025-10-01)
//!
//! ## Inbound (NB → LF)
//!
//! | PID   | Process name (AHB)                                   | Direction |
//! |-------|------------------------------------------------------|-----------|
//! | 55007 | Ankündigung NB-seitiges Lieferende (NB → LFN)        | NB → LF   |
//!
//! ## Outbound (LF → NB)
//!
//! | PID   | Process name (AHB)                                   | Derived from |
//! |-------|------------------------------------------------------|--------------|
//! | 55008 | Bestätigung NB-seitiges Lieferende (LFN → NB)        | 55007 accepted |
//! | 55009 | Ablehnung NB-seitiges Lieferende (LFN → NB)          | 55007 rejected |
//!
//! # Note on LFW24 context
//!
//! PIDs 55007–55009 are **present in UTILMD AHB Strom 2.1 (FV2025-10-01)** and
//! in the BDEW Anwendungsübersicht 3.3. They were NOT removed by BK6-22-024
//! (LFW24); only the LF-initiated Lieferbeginn/Lieferende (55001/55002) was
//! redesigned for 24h processing. The NB-initiated Lieferende (55007–55009) is a
//! separate process under GPKE Teil 2 §2.5.
//!
//! # Regulatory basis
//!
//! - **BDEW GPKE Teil 2 §2.5** — NB-seitiges Lieferende
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
pub const WORKFLOW_NAME: &str = "gpke-lf-abmeldung";

/// Inbound PIDs for NB-initiated Lieferende handled by [`GpkeLfAbmeldungWorkflow`].
///
/// | PID   | Process (AHB name)                          | AHB profile  |
/// |-------|---------------------------------------------|--------------|
/// | 55007 | Ankündigung NB-seitiges Lieferende (NB→LFN) | S2.1–S2.2 ✅ |
pub const LF_ABMELDUNG_PIDS: &[u32] = &[55007];

/// Deadline label for the 24h APERAK Frist (BK6-22-024 §4).
pub const LF_ABMELDUNG_APERAK_WINDOW_LABEL: &str = "gpke-lf-abmeldung-aperak-window";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE LF Abmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum LfAbmeldungEvent {
    /// PID 55007 Ankündigung NB-seitiges Lieferende received.
    AnkuendigungErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// GLN of the sending NB.
        sender: MarktpartnerCode,
        /// GLN of the receiving LF.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Announced supply end date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55007).
        pruefidentifikator: Pruefidentifikator,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound response (55008 or 55009) dispatched to the NB.
    AntwortGesendet {
        /// Response PID: 55008 (accepted) or 55009 (rejected).
        response_pid: Pruefidentifikator,
        /// `true` = accepted (Bestätigung), `false` = rejected (Ablehnung).
        accepted: bool,
        /// Rejection reason (when `accepted = false`).
        reason: Option<String>,
    },
    /// Supply relationship ended per NB notice.
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

impl EventPayload for LfAbmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnkuendigungErhalten { .. } => "LfAbmeldungAnkuendigungErhalten",
            Self::ValidationPassed { .. } => "LfAbmeldungValidationPassed",
            Self::AntwortGesendet { .. } => "LfAbmeldungAntwortGesendet",
            Self::Beendet => "LfAbmeldungBeendet",
            Self::AperakFehlerDispatched { .. } => "LfAbmeldungAperakFehlerDispatched",
            Self::Rejected { .. } => "LfAbmeldungRejected",
            Self::DeadlineExpired { .. } => "LfAbmeldungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `AnkuendigungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfAbmeldungData {
    /// EIC/MaLo code.
    pub location_id: MaLo,
    /// GLN of the NB who initiated the termination.
    pub sender: MarktpartnerCode,
    /// GLN of the affected LF.
    pub receiver: MarktpartnerCode,
    /// EDIFACT document date (`YYYYMMDD`).
    pub document_date: String,
    /// Announced supply end date (`YYYYMMDD`).
    pub process_date: String,
    /// BDEW Prüfidentifikator (55007).
    pub pruefidentifikator: Pruefidentifikator,
}

/// State of a GPKE LF Abmeldung process.
///
/// # Lifecycle
///
/// ```text
/// New → Eingegangen → ValidationPassed → AntwortGesendet → Beendet
///                                       ↘ Rejected
///     ↘ Rejected (failed validation)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum LfAbmeldungState {
    /// No events yet.
    New,
    /// Ankündigung received.
    Eingegangen(LfAbmeldungData),
    /// Validation passed; response not yet sent.
    ValidationPassed(LfAbmeldungData),
    /// Response dispatched; awaiting supply-end confirmation.
    AntwortGesendet {
        /// Data from the Ankündigung.
        data: LfAbmeldungData,
        /// Response PID sent (55008 or 55009).
        response_pid: Pruefidentifikator,
    },
    /// Supply relationship ended.
    Beendet(LfAbmeldungData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl Default for LfAbmeldungState {
    fn default() -> Self {
        Self::New
    }
}

impl LfAbmeldungState {
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

    /// Return `Some(&LfAbmeldungData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&LfAbmeldungData> {
        match self {
            Self::Eingegangen(d) | Self::ValidationPassed(d) | Self::Beendet(d) => Some(d),
            Self::AntwortGesendet { data, .. } => Some(data),
            Self::New | Self::Rejected { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE LF Abmeldung workflow.
#[derive(Clone)]
pub enum LfAbmeldungCommand {
    /// Inbound UTILMD PID 55007 Ankündigung received from the AS4 layer.
    ReceiveAnkuendigung {
        /// BDEW Prüfidentifikator (55007).
        pid: Pruefidentifikator,
        /// GLN of the NB.
        sender: MarktpartnerCode,
        /// GLN of the LF.
        receiver: MarktpartnerCode,
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Announced supply end date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response (55008 = Bestätigung, 55009 = Ablehnung).
    ///
    /// The LF has 24 wall-clock hours (BK6-22-024 §4) to respond.
    SendAntwort {
        /// `true` = Bestätigung (55008), `false` = Ablehnung (55009).
        accepted: bool,
        /// Rejection reason (required when `accepted = false`).
        reason: Option<String>,
    },
    /// Record that the supply relationship has ended.
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

impl CommandPayload for LfAbmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE NB-initiated Lieferende workflow (PID 55007 inbound, 55008/55009 outbound).
///
/// Spawn via [`mako_engine::process::Process`]:
/// ```rust,ignore
/// let process = ctx.spawn::<GpkeLfAbmeldungWorkflow>(
///     tenant_id,
///     WorkflowId::new("gpke-lf-abmeldung", "FV2025-10-01"),
/// );
/// ```
pub struct GpkeLfAbmeldungWorkflow;

impl Workflow for GpkeLfAbmeldungWorkflow {
    type State = LfAbmeldungState;
    type Event = LfAbmeldungEvent;
    type Command = LfAbmeldungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (LF_ABMELDUNG_APERAK_WINDOW_LABEL, LfAbmeldungState::Eingegangen(_))
            | (LF_ABMELDUNG_APERAK_WINDOW_LABEL, LfAbmeldungState::ValidationPassed(_)) => {
                Some(LfAbmeldungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            LfAbmeldungEvent::AnkuendigungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                ..
            } => LfAbmeldungState::Eingegangen(LfAbmeldungData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
            }),
            LfAbmeldungEvent::ValidationPassed { .. } => match state {
                LfAbmeldungState::Eingegangen(data) => LfAbmeldungState::ValidationPassed(data),
                other => other,
            },
            LfAbmeldungEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        LfAbmeldungState::ValidationPassed(data) => {
                            LfAbmeldungState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    LfAbmeldungState::Rejected {
                        reason: "Ankündigung abgelehnt".to_owned(),
                    }
                }
            }
            LfAbmeldungEvent::Beendet => match state {
                LfAbmeldungState::AntwortGesendet { data, .. } => LfAbmeldungState::Beendet(data),
                other => other,
            },
            LfAbmeldungEvent::AperakFehlerDispatched { reason, .. } => LfAbmeldungState::Rejected {
                reason: format!("APERAK 29001: {reason}"),
            },
            LfAbmeldungEvent::Rejected { reason } => LfAbmeldungState::Rejected {
                reason: reason.clone(),
            },
            LfAbmeldungEvent::DeadlineExpired { label, .. } => match state {
                LfAbmeldungState::Beendet(_) | LfAbmeldungState::Rejected { .. } => state,
                _ => LfAbmeldungState::Rejected {
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
            LfAbmeldungCommand::ReceiveAnkuendigung {
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
                if !matches!(state, LfAbmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !LF_ABMELDUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected NB-initiated Lieferende PID (55007), got {pid}",
                    )));
                }
                // Clone before move for APERAK emission in the validation-failed path.
                let sender_gln = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![LfAbmeldungEvent::AnkuendigungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                }];
                if validation_passed {
                    events.push(LfAbmeldungEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — mandatory per APERAK AHB 1.0 §2.4.
                    // Strom UTILMD (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_gln.as_str(),
                            serde_json::json!({
                                "sender":        receiver_gln.as_str(),
                                "receiver":      sender_gln.as_str(),
                                "pid":           29001_u32,
                                "document_code": "312",
                            }),
                        )
                        .caused_by(1),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                } else {
                    let reason = validation_errors.join("; ");
                    events.push(LfAbmeldungEvent::Rejected {
                        reason: reason.clone(),
                    });
                    // F-035: APERAK BGM+313 — mandatory per APERAK AHB 1.0 §2.1.1.
                    // Strom UTILMD (weekday): 45 Min; Saturday: Sonntag 12 Uhr (APERAK AHB 1.0 §2.4.1).
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_gln.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_gln.as_str(),
                                "pid":        29001_u32,
                                "error_code": "Z29",
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    Ok(WorkflowOutput::with_outbox(events, outbox))
                }
            }

            LfAbmeldungCommand::SendAntwort { accepted, reason } => {
                match state {
                    LfAbmeldungState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let response_code: u32 = if accepted { 55008 } else { 55009 };
                let response_pid = Pruefidentifikator::new(response_code)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![LfAbmeldungEvent::AntwortGesendet {
                    response_pid,
                    accepted,
                    reason,
                }]
                .into())
            }

            LfAbmeldungCommand::BeendenBestaetigen => {
                if !matches!(state, LfAbmeldungState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![LfAbmeldungEvent::Beendet].into())
            }

            LfAbmeldungCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    LfAbmeldungState::Eingegangen(_) | LfAbmeldungState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                Ok(vec![LfAbmeldungEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            LfAbmeldungCommand::TimeoutExpired { deadline_id, label } => match state {
                LfAbmeldungState::Beendet(_) | LfAbmeldungState::Rejected { .. } => {
                    Ok(vec![].into())
                }
                _ => Ok(vec![LfAbmeldungEvent::DeadlineExpired { deadline_id, label }].into()),
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

    fn ankuendigung_cmd(ok: bool) -> LfAbmeldungCommand {
        LfAbmeldungCommand::ReceiveAnkuendigung {
            pid: pid(55007),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("ABMELD-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    fn apply_all(init: LfAbmeldungState, events: &[LfAbmeldungEvent]) -> LfAbmeldungState {
        events.iter().fold(init, GpkeLfAbmeldungWorkflow::apply)
    }

    #[test]
    fn lf_abmeldung_happy_path_bestaetigung() {
        let out = GpkeLfAbmeldungWorkflow::handle(&LfAbmeldungState::New, ankuendigung_cmd(true))
            .unwrap();
        assert_eq!(out.events.len(), 2); // AnkuendigungErhalten + ValidationPassed
        let state = apply_all(LfAbmeldungState::New, &out.events);
        assert!(matches!(state, LfAbmeldungState::ValidationPassed(_)));

        let out = GpkeLfAbmeldungWorkflow::handle(
            &state,
            LfAbmeldungCommand::SendAntwort {
                accepted: true,
                reason: None,
            },
        )
        .unwrap();
        if let LfAbmeldungEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(accepted);
            assert_eq!(response_pid.as_u32(), 55008);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(state, LfAbmeldungState::AntwortGesendet { .. }));

        let out = GpkeLfAbmeldungWorkflow::handle(&state, LfAbmeldungCommand::BeendenBestaetigen)
            .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, LfAbmeldungState::Beendet(_)));
    }

    #[test]
    fn lf_abmeldung_ablehnung() {
        let out = GpkeLfAbmeldungWorkflow::handle(&LfAbmeldungState::New, ankuendigung_cmd(true))
            .unwrap();
        let state = apply_all(LfAbmeldungState::New, &out.events);
        let out = GpkeLfAbmeldungWorkflow::handle(
            &state,
            LfAbmeldungCommand::SendAntwort {
                accepted: false,
                reason: Some("Widerspruch".to_owned()),
            },
        )
        .unwrap();
        if let LfAbmeldungEvent::AntwortGesendet {
            response_pid,
            accepted,
            ..
        } = &out.events[0]
        {
            assert!(!accepted);
            assert_eq!(response_pid.as_u32(), 55009);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(state, LfAbmeldungState::Rejected { .. }));
    }

    #[test]
    fn lf_abmeldung_wrong_pid_rejected() {
        let result = GpkeLfAbmeldungWorkflow::handle(
            &LfAbmeldungState::New,
            LfAbmeldungCommand::ReceiveAnkuendigung {
                pid: pid(55001),
                sender: mcod("9900357000004"),
                receiver: mcod("4012345000023"),
                location_id: malo("51238696781"),
                document_date: "20251001".to_owned(),
                process_date: "20260101".to_owned(),
                message_ref: mref("X"),
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn timeout_in_beendet_is_noop() {
        let data = LfAbmeldungData {
            location_id: malo("51238696781"),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            pruefidentifikator: pid(55007),
        };
        let state = LfAbmeldungState::Beendet(data);
        let dl_id = DeadlineId::new();
        let out = GpkeLfAbmeldungWorkflow::handle(
            &state,
            LfAbmeldungCommand::TimeoutExpired {
                deadline_id: dl_id,
                label: LF_ABMELDUNG_APERAK_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        assert!(out.events.is_empty());
    }
}
