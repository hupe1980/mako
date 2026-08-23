//! GPKE Kündigung — the incoming supplier terminates the incumbent's contract.
//!
//! GPKE Teil 2 § 1.2: the **LFN sends the Kündigung directly to the LFA**, with
//! no Netzbetreiber in between. Both parties are suppliers, which is what makes
//! this process its own workflow rather than a variant of the supplier change:
//! `gpke-supplier-change` is correlated by Marktlokation and hosts the NB-side
//! Anmeldung, so on one MaLo an integrated deployment would have the grid
//! operator's Anmeldung and this Kündigung contending for the same key.
//!
//! # Prüfidentifikatoren (UTILMD AHB Strom 2.1/2.2)
//!
//! | PID   | Process name (AHB)                     | Direction  |
//! |-------|----------------------------------------|------------|
//! | 55016 | Kündigung                              | LFN → LFA  |
//! | 55017 | Bestätigung Kündigung                  | LFA → LFN  |
//! | 55018 | Ablehnung Kündigung                    | LFA → LFN  |
//!
//! The answer is decided by `mako_pruefung::lf::pruefe_kuendigung` (EBD
//! `E_0614`) and carries its Antwortcode in `SG4 STS+E01`.
//!
//! # Regulatory basis
//!
//! - **BK6-24-174 GPKE Teil 2 § 1.2** — UC/SD Kündigung; the LFA answers by the
//!   **Ablauf des 1. Werktags nach dem ÜT**
//! - **EBD 4.3 Kap. 6.2.1** — `E_0614` „Kündigung Vertrag prüfen"
//! - **APERAK AHB 1.0 § 2.4.1** — the separate 45-minute technical window

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
///
/// Distinct from `gpke-supplier-change` on purpose: both are keyed by MaLo, and
/// an integrated NB+LF deployment runs them on the same Marktlokation.
pub const WORKFLOW_NAME: &str = "gpke-kuendigung";

/// Inbound PIDs handled by [`GpkeKuendigungWorkflow`].
///
/// | PID   | Process (AHB name)                            | AHB profile  |
/// |-------|-----------------------------------------------|--------------|
/// | 55016 | Kündigung (LFN→LFA) | S2.1–S2.2 ✅ |
pub const KUENDIGUNG_PIDS: &[u32] = &[55016];

/// Deadline label for the **business** answer window — Ablauf des 1. WT nach dem
/// ÜT (GPKE Teil 2 SD Kündigung Prozessschritt 2), resolved by
/// `mako_fristen::antwort`.
///
/// Not the APERAK clock: that is 45 Minuten for a UTILMD (APERAK AHB 1.0
/// § 2.4.1) and rides `mako_fristen::APERAK_STROM_WINDOW_LABEL`.
pub const KUENDIGUNG_ANTWORT_WINDOW_LABEL: &str = "gpke-kuendigung-antwortfrist";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Beendigung-der-Zuordnung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum KuendigungEvent {
    /// PID 55016 Anfrage zur Beendigung der Zuordnung received.
    KuendigungErhalten {
        /// Marktlokation EIC code.
        location_id: MaLo,
        /// MP-ID of the sending LFN.
        sender: MarktpartnerCode,
        /// MP-ID of the receiving LFA.
        receiver: MarktpartnerCode,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// Requested Zuordnungsende date (`YYYYMMDD`).
        process_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// BDEW Prüfidentifikator (55016).
        pruefidentifikator: Pruefidentifikator,
        /// `SG4 IDE+24` DE 7402 — carried into the answer's `SG4 RFF+TN`.
        vorgangsnummer: Option<String>,
    },
    /// EDIFACT message passed profile validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Outbound response (55017 or 55018) dispatched to the LFN.
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

impl EventPayload for KuendigungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::KuendigungErhalten { .. } => "KuendigungKuendigungErhalten",
            Self::ValidationPassed { .. } => "KuendigungValidationPassed",
            Self::AntwortGesendet { .. } => "KuendigungAntwortGesendet",
            Self::Beendet => "KuendigungBeendet",
            Self::AperakFehlerDispatched { .. } => "KuendigungAperakFehlerDispatched",
            Self::Rejected { .. } => "KuendigungRejected",
            Self::DeadlineExpired { .. } => "KuendigungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured at `KuendigungErhalten` time.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KuendigungData {
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
    /// BDEW Prüfidentifikator (55016).
    pub pruefidentifikator: Pruefidentifikator,
    /// `SG4 IDE+24` DE 7402 of the **request**.
    ///
    /// Retained because the answer must carry it back in `SG4 RFF+TN`
    /// („Referenz Vorgangsnummer (aus Anfragenachricht)", Muss on every
    /// Antwortnachricht). It is never reused as the answer's own `IDE+24`.
    pub vorgangsnummer: Option<String>,
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
pub enum KuendigungState {
    /// No events yet.
    #[default]
    New,
    /// Anfrage received.
    Eingegangen(KuendigungData),
    /// Validation passed; response not yet sent.
    ValidationPassed(KuendigungData),
    /// Response dispatched; awaiting Zuordnungsende confirmation.
    AntwortGesendet {
        /// Data from the Anfrage.
        data: KuendigungData,
        /// Response PID sent (55017 or 55018).
        response_pid: Pruefidentifikator,
    },
    /// Zuordnung ended.
    Beendet(KuendigungData),
    /// Process rejected.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl KuendigungState {
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

    /// Return `Some(&KuendigungData)` if the process has been initiated.
    #[must_use]
    pub fn data(&self) -> Option<&KuendigungData> {
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
pub enum KuendigungCommand {
    /// Inbound UTILMD PID 55016 Anfrage received from the AS4 layer.
    ReceiveKuendigung {
        /// BDEW Prüfidentifikator (55016).
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
        /// The `SG4` facts the trees branch on, forwarded to `processd` on
        /// the `de.mako.process.initiated` notification.
        vorgang: crate::lf_antwort::LfVorgangsdaten,
        /// `true` if validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
    },
    /// Send the outbound UTILMD response (55017 = Bestätigung, 55018 = Ablehnung).
    ///
    /// The LFA answers by the **Ablauf des 1. Werktags nach dem ÜT** — GPKE
    /// Teil 2 § 1.2.2 SD Kündigung Prozessschritt 2, resolved by
    /// `mako_fristen::antwort` (trigger PID 55016).
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

impl CommandPayload for KuendigungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// GPKE Kündigung workflow (55016 inbound, 55017/55018
/// outbound).
pub struct GpkeKuendigungWorkflow;

impl Workflow for GpkeKuendigungWorkflow {
    type State = KuendigungState;
    type Event = KuendigungEvent;
    type Command = KuendigungCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (
                KUENDIGUNG_ANTWORT_WINDOW_LABEL,
                KuendigungState::Eingegangen(_) | KuendigungState::ValidationPassed(_),
            ) => Some(KuendigungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            KuendigungEvent::KuendigungErhalten {
                location_id,
                sender,
                receiver,
                document_date,
                process_date,
                pruefidentifikator,
                vorgangsnummer,
                ..
            } => KuendigungState::Eingegangen(KuendigungData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                document_date: document_date.clone(),
                process_date: process_date.clone(),
                pruefidentifikator: *pruefidentifikator,
                vorgangsnummer: vorgangsnummer.clone(),
            }),
            KuendigungEvent::ValidationPassed { .. } => match state {
                KuendigungState::Eingegangen(data) => KuendigungState::ValidationPassed(data),
                other => other,
            },
            KuendigungEvent::AntwortGesendet {
                accepted,
                response_pid,
                ..
            } => {
                if *accepted {
                    match state {
                        KuendigungState::ValidationPassed(data) => {
                            KuendigungState::AntwortGesendet {
                                response_pid: *response_pid,
                                data,
                            }
                        }
                        other => other,
                    }
                } else {
                    KuendigungState::Rejected {
                        reason: "Anfrage abgelehnt".to_owned(),
                    }
                }
            }
            KuendigungEvent::Beendet => match state {
                KuendigungState::AntwortGesendet { data, .. } => KuendigungState::Beendet(data),
                other => other,
            },
            KuendigungEvent::AperakFehlerDispatched { reason, .. } => KuendigungState::Rejected {
                reason: format!("APERAK 29001: {reason}"),
            },
            KuendigungEvent::Rejected { reason } => KuendigungState::Rejected {
                reason: reason.clone(),
            },
            KuendigungEvent::DeadlineExpired { label, .. } => match state {
                KuendigungState::Beendet(_) | KuendigungState::Rejected { .. } => state,
                _ => KuendigungState::Rejected {
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
            KuendigungCommand::ReceiveKuendigung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                process_date,
                message_ref,
                vorgang,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, KuendigungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !KUENDIGUNG_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "expected Anfrage zur Beendigung der Zuordnung PID (55016), got {pid}",
                    )));
                }
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();
                let notify_malo = location_id.clone();
                let notify_termin = process_date.clone();

                let mut events = vec![KuendigungEvent::KuendigungErhalten {
                    location_id,
                    sender,
                    receiver,
                    document_date,
                    process_date,
                    message_ref: message_ref.clone(),
                    pruefidentifikator: pid,
                    vorgangsnummer: vorgang.vorgangsnummer.clone(),
                }];
                if validation_passed {
                    events.push(KuendigungEvent::ValidationPassed { message_ref });
                    // F-038: APERAK BGM+312 (Anerkennungsmeldung) — APERAK AHB 1.0 §2.4.
                    let outbox = vec![
                        // The business notification. `processd`'s LF module
                        // decides this process, and it only ever sees a message
                        // that reaches the ERP fan-out — an APERAK is a
                        // technical acknowledgement, not one.
                        vorgang
                            .process_initiated(
                                pid,
                                &notify_malo,
                                &sender_mp_id,
                                &receiver_gln,
                                &notify_termin,
                                &serde_json::Value::Null,
                            )
                            .caused_by(1),
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
                    events.push(KuendigungEvent::Rejected {
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

            KuendigungCommand::SendAntwort { antwort } => {
                let data = match state {
                    KuendigungState::ValidationPassed(d) => d,
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
                let response_code: u32 = if accepted { 55017 } else { 55018 };
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
                        data.vorgangsnummer.as_deref(),
                    )
                    .caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![KuendigungEvent::AntwortGesendet {
                        response_pid,
                        accepted,
                        antwort,
                    }],
                    outbox,
                ))
            }

            KuendigungCommand::BeendenBestaetigen => {
                if !matches!(state, KuendigungState::AntwortGesendet { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "AntwortGesendet",
                        state.label(),
                    ));
                }
                Ok(vec![KuendigungEvent::Beendet].into())
            }

            KuendigungCommand::DispatchAperakFehler {
                reason,
                outbound_ref,
            } => {
                match state {
                    KuendigungState::Eingegangen(_) | KuendigungState::ValidationPassed(_) => {}
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "Eingegangen or ValidationPassed",
                            state.label(),
                        ));
                    }
                }
                let aperak_pid = Pruefidentifikator::new(29_001)
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                Ok(vec![KuendigungEvent::AperakFehlerDispatched {
                    aperak_pid,
                    reason,
                    outbound_ref,
                }]
                .into())
            }

            KuendigungCommand::TimeoutExpired { deadline_id, label } => match state {
                KuendigungState::Beendet(_) | KuendigungState::Rejected { .. } => Ok(vec![].into()),
                _ => Ok(vec![KuendigungEvent::DeadlineExpired { deadline_id, label }].into()),
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

    fn anfrage_cmd(ok: bool) -> KuendigungCommand {
        KuendigungCommand::ReceiveKuendigung {
            pid: pid(55016),
            sender: mcod("9900357000004"),
            receiver: mcod("4012345000023"),
            location_id: malo("51238696781"),
            document_date: "20251001".to_owned(),
            process_date: "20260101".to_owned(),
            message_ref: mref("BEEND-001"),
            vorgang: crate::LfVorgangsdaten::default(),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
        }
    }

    fn apply_all(init: KuendigungState, events: &[KuendigungEvent]) -> KuendigungState {
        events.iter().fold(init, GpkeKuendigungWorkflow::apply)
    }

    #[test]
    fn happy_path_bestaetigung() {
        let out = GpkeKuendigungWorkflow::handle(&KuendigungState::New, anfrage_cmd(true)).unwrap();
        // KuendigungErhalten + ValidationPassed events; ProcessInitiated + APERAK 312
        // outbox. Both are load-bearing: the APERAK discharges the 45-minute
        // technical clock, the ProcessInitiated is what puts the Vorgang in
        // front of `processd` (or the ERP) at all.
        assert_eq!(out.events.len(), 2);
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "ProcessInitiated");
        assert_eq!(out.outbox[1].payload["document_code"], "312");
        let state = apply_all(KuendigungState::New, &out.events);
        assert!(matches!(state, KuendigungState::ValidationPassed(_)));

        let out = GpkeKuendigungWorkflow::handle(
            &state,
            KuendigungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::zustimmung("A36", "E_0624"),
            },
        )
        .unwrap();
        if let KuendigungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55017);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        let out =
            GpkeKuendigungWorkflow::handle(&state, KuendigungCommand::BeendenBestaetigen).unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, KuendigungState::Beendet(_)));
    }

    #[test]
    fn ablehnung_yields_55018() {
        let out = GpkeKuendigungWorkflow::handle(&KuendigungState::New, anfrage_cmd(true)).unwrap();
        let state = apply_all(KuendigungState::New, &out.events);
        let out = GpkeKuendigungWorkflow::handle(
            &state,
            KuendigungCommand::SendAntwort {
                antwort: crate::lf_antwort::LfAntwort::ablehnung("A35", "E_0624")
                    .with_bemerkung("Widerspruch"),
            },
        )
        .unwrap();
        if let KuendigungEvent::AntwortGesendet { response_pid, .. } = &out.events[0] {
            assert_eq!(response_pid.as_u32(), 55018);
        } else {
            panic!("expected AntwortGesendet");
        }
        let state = apply_all(state, &out.events);
        assert!(matches!(state, KuendigungState::Rejected { .. }));
    }

    #[test]
    fn validation_failure_emits_aperak_313() {
        let out =
            GpkeKuendigungWorkflow::handle(&KuendigungState::New, anfrage_cmd(false)).unwrap();
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(KuendigungState::New, &out.events);
        assert!(matches!(state, KuendigungState::Rejected { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let mut cmd = anfrage_cmd(true);
        if let KuendigungCommand::ReceiveKuendigung { pid: p, .. } = &mut cmd {
            *p = pid(55001);
        }
        assert!(GpkeKuendigungWorkflow::handle(&KuendigungState::New, cmd).is_err());
    }
}
