//! WiM **Weiterverpflichtung des MSB** — ORDERS 17002 → ORDRSP 19003 / 19004.
//!
//! # What this process is for
//!
//! WiM Strom Teil 1 Kap. 2.1.1 forbids a Messlokation ever being unassigned:
//! „Ist eine Messlokation zu einem Zeitpunkt in Bezug auf den
//! Messstellenbetrieb nicht einem wMSB zugeordnet, so ist sie dem gMSB
//! zuzuordnen." When an Ende Messstellenbetrieb has no successor lined up, the
//! NB may keep the outgoing MSB in place while the gMSB prepares to take over —
//! Kap. 2.4.2 Prozessschritte 5 and 6.
//!
//! ```text
//! MSBA ──UTILMD 55051 Abmeldung──────────────────────────────────▶ NB
//! MSBA ◀─UTILMD 55052 vorläufige Bestätigung──── 7 WT ────────────  NB
//!      gMSB ◀─UTILMD 55168 Verpflichtungsanfrage── 8.–5. WT vor ─── NB
//!      gMSB ──UTILMD 55169/55170 Antwort───────── 1 WT ───────────▶ NB
//! MSBA ◀─ORDERS 17002 Weiterverpflichtung─────── 1 WT ────────────  NB
//! MSBA ──ORDRSP 19003/19004 Antwort───────────── 1 WT ───────────▶ NB
//! ```
//!
//! # Not the Geräteübernahme
//!
//! 17001 is MSBN → MSBA and asks to buy equipment; 17002 is NB → MSBA and
//! orders the recipient to keep operating the metering point. Different
//! direction, Frist and Entscheidungsbaum, so a separate workflow.
//!
//! # The cap
//!
//! „Längstens drei Monate" on an Anschlussnutzerwechsel, „längstens einen
//! Monat" otherwise (Kap. 2.4.2 Nr. 4). Which of `Z13` / `Z14` / `Z22` an
//! overshoot earns is decided by
//! [`mako_pruefung::msb::pruefe_weiterverpflichtung`]; this module is the
//! process around it.

use std::collections::HashMap;

use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-weiterverpflichtung";

/// ORDERS 17002 — „Weiterverpflichtung", NB → MSBA (Kap. 2.4.2 Nr. 5).
pub const AUFTRAG_PID: u32 = 17_002;

/// ORDRSP 19003 / 19004 — Bestätigung / Ablehnung Weiterverpflichtung,
/// MSBA → NB (Kap. 2.4.2 Nr. 6), decided by `E_0203`.
pub const ANTWORT_PIDS: (u32, u32) = (19_003, 19_004);

/// Every PID this process family uses, inbound and outbound.
///
/// Only [`AUFTRAG_PID`] is routed: the answers are outbox entries this workflow
/// renders. A deployment acting as the *NB* would send 17002 and receive
/// 19003/19004, and that side is not implemented.
pub const WEITERVERPFLICHTUNG_PIDS: &[u32] = &[AUFTRAG_PID, ANTWORT_PIDS.0, ANTWORT_PIDS.1];

/// Deadline label for the MSBA's answer window (1 Werktag, Kap. 2.4.2 Nr. 6).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-weiterverpflichtung-antwort";

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the Weiterverpflichtung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WeiterverpflichtungEvent {
    /// Inbound ORDERS 17002 accepted — the NB has ordered continuation.
    AuftragEmpfangen {
        /// The Messlokation to keep operating.
        melo_id: MeLo,
        /// MP-ID of the ordering Netzbetreiber.
        nb: MarktpartnerCode,
        /// MP-ID of this party (the abgebender MSB).
        msba: MarktpartnerCode,
        /// The date up to which the NB wants the Messstellenbetrieb continued
        /// („verschobenes Zuordnungsende", YYYYMMDD).
        verschobenes_zuordnungsende: String,
        /// EDIFACT message reference of the ORDERS.
        message_ref: MessageRef,
    },
    /// The ORDRSP answer went out.
    AntwortGesendet {
        /// 19003 (Bestätigung) or 19004 (Ablehnung).
        pruefidentifikator: Pruefidentifikator,
        /// `AJT` DE 4465 — `Z13`, `Z14` or `Z22` from `E_0203`.
        antwort_code: String,
        /// The corrected Abmeldetermin (`DTM` DE 2380), where the code names one.
        abweichender_termin: Option<String>,
    },
    /// The order was refused before it could be answered (validation failure).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
    /// A registered deadline expired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline.
        label: Box<str>,
    },
}

impl EventPayload for WeiterverpflichtungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AuftragEmpfangen { .. } => "WimWeiterverpflichtungAuftragEmpfangen",
            Self::AntwortGesendet { .. } => "WimWeiterverpflichtungAntwortGesendet",
            Self::Rejected { .. } => "WimWeiterverpflichtungRejected",
            Self::DeadlineExpired { .. } => "WimWeiterverpflichtungDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data carried from `AuftragEmpfangen` onwards.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeiterverpflichtungData {
    /// The Messlokation.
    pub melo_id: MeLo,
    /// The ordering Netzbetreiber.
    pub nb: MarktpartnerCode,
    /// This party.
    pub msba: MarktpartnerCode,
    /// The date the NB asked for.
    pub verschobenes_zuordnungsende: String,
    /// Reference of the inbound ORDERS, echoed in `RFF+ACW` of the answer.
    pub message_ref: MessageRef,
}

/// State of a Weiterverpflichtung process.
///
/// ```text
/// New → AuftragEmpfangen → Beantwortet
///     ↘ Rejected
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum WeiterverpflichtungState {
    /// No events yet.
    #[default]
    New,
    /// Order received; the answer is owed within one Werktag.
    AuftragEmpfangen(WeiterverpflichtungData),
    /// The ORDRSP went out.
    Beantwortet(WeiterverpflichtungData),
    /// Terminal failure.
    Rejected {
        /// Reason.
        reason: String,
    },
}

impl WeiterverpflichtungState {
    /// Stable label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AuftragEmpfangen(_) => "AuftragEmpfangen",
            Self::Beantwortet(_) => "Beantwortet",
            Self::Rejected { .. } => "Rejected",
        }
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for WeiterverpflichtungState {
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::AuftragEmpfangen(_))
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the Weiterverpflichtung workflow.
#[derive(Clone)]
pub enum WeiterverpflichtungCommand {
    /// Inbound ORDERS 17002 from the NB.
    ReceiveAuftrag {
        /// Must be [`AUFTRAG_PID`].
        pid: Pruefidentifikator,
        /// MP-ID of the ordering Netzbetreiber.
        nb: MarktpartnerCode,
        /// MP-ID of this party.
        msba: MarktpartnerCode,
        /// The Messlokation.
        melo_id: MeLo,
        /// The date the NB wants the Messstellenbetrieb continued to.
        verschobenes_zuordnungsende: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if the ORDERS passed AHB validation.
        validation_passed: bool,
        /// Validation issues, for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Send the ORDRSP answer.
    ///
    /// `antwort_code` must be published by `E_0203`: `Z13` (plain agreement),
    /// `Z14` (agreement to a corrected Abmeldetermin) or `Z22` (refusal, and
    /// only on a further order after the maximum was already reached). The
    /// cluster the code sits in — not a boolean — picks 19003 or 19004.
    DispatchAntwort {
        /// `AJT` DE 4465.
        antwort_code: String,
        /// `DTM` DE 2380 — required with `Z14` and `Z22`, which both name a
        /// corrected date.
        abweichender_termin: Option<String>,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for WeiterverpflichtungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The WiM Weiterverpflichtung workflow (ORDERS 17002 → ORDRSP 19003/19004).
pub struct WimWeiterverpflichtungWorkflow;

impl Workflow for WimWeiterverpflichtungWorkflow {
    type State = WeiterverpflichtungState;
    type Event = WeiterverpflichtungEvent;
    type Command = WeiterverpflichtungCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, WeiterverpflichtungState::AuftragEmpfangen(_)) => {
                Some(WeiterverpflichtungCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            WeiterverpflichtungEvent::AuftragEmpfangen {
                melo_id,
                nb,
                msba,
                verschobenes_zuordnungsende,
                message_ref,
            } => WeiterverpflichtungState::AuftragEmpfangen(WeiterverpflichtungData {
                melo_id: melo_id.clone(),
                nb: nb.clone(),
                msba: msba.clone(),
                verschobenes_zuordnungsende: verschobenes_zuordnungsende.clone(),
                message_ref: message_ref.clone(),
            }),
            WeiterverpflichtungEvent::AntwortGesendet { .. } => match state {
                WeiterverpflichtungState::AuftragEmpfangen(d) => {
                    WeiterverpflichtungState::Beantwortet(d)
                }
                other => other,
            },
            WeiterverpflichtungEvent::Rejected { reason } => WeiterverpflichtungState::Rejected {
                reason: reason.clone(),
            },
            WeiterverpflichtungEvent::DeadlineExpired { label, .. } => match state {
                WeiterverpflichtungState::Beantwortet(_)
                | WeiterverpflichtungState::Rejected { .. } => state,
                _ => WeiterverpflichtungState::Rejected {
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
            WeiterverpflichtungCommand::ReceiveAuftrag {
                pid,
                nb,
                msba,
                melo_id,
                verschobenes_zuordnungsende,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, WeiterverpflichtungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != AUFTRAG_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not the Weiterverpflichtung ({AUFTRAG_PID})",
                        pid.as_u32()
                    )));
                }
                if !validation_passed {
                    return Ok(vec![WeiterverpflichtungEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }
                Ok(vec![WeiterverpflichtungEvent::AuftragEmpfangen {
                    melo_id,
                    nb,
                    msba,
                    verschobenes_zuordnungsende,
                    message_ref,
                }]
                .into())
            }

            WeiterverpflichtungCommand::DispatchAntwort {
                antwort_code,
                abweichender_termin,
            } => {
                let WeiterverpflichtungState::AuftragEmpfangen(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "AuftragEmpfangen",
                        state.label(),
                    ));
                };
                let tree = mako_pruefung::codes::EBD_WEITERVERPFLICHTUNG;
                let code = mako_pruefung::codes::lookup(tree, &antwort_code).ok_or_else(|| {
                    WorkflowError::rejected(format!(
                        "Antwortcode {antwort_code:?} is not published in {tree}"
                    ))
                })?;
                // `Z14` and `Z22` both state the corrected Abmeldetermin, and
                // `Z14`'s Bedingung names the element it goes in.
                if abweichender_termin.is_none() && matches!(code.code, "Z14" | "Z22") {
                    return Err(WorkflowError::rejected(format!(
                        "{tree} {} ({}) requires the corrected Abmeldetermin in DTM DE 2380",
                        code.code, code.bedeutung
                    )));
                }
                let bestaetigt = code.ist_zustimmung().ok_or_else(|| {
                    WorkflowError::rejected(format!("{} sits off the agreement axis", code.code))
                })?;
                let antwort_pid = if bestaetigt {
                    ANTWORT_PIDS.0
                } else {
                    ANTWORT_PIDS.1
                };

                let mut payload = serde_json::json!({
                    "pid":          antwort_pid,
                    "sender":       data.msba.as_str(),
                    "receiver":     data.nb.as_str(),
                    "melo":         data.melo_id.as_str(),
                    "antwort_code": code.code,
                    "antwort_ebd":  tree,
                    "orig_message_ref": data.message_ref.as_str(),
                });
                if let Some(ref t) = abweichender_termin {
                    payload["abmeldetermin"] = serde_json::Value::String(t.clone());
                }

                Ok(WorkflowOutput::with_outbox(
                    vec![WeiterverpflichtungEvent::AntwortGesendet {
                        pruefidentifikator: Pruefidentifikator::new(antwort_pid)
                            .map_err(WorkflowError::rejected)?,
                        antwort_code: code.code.to_owned(),
                        abweichender_termin,
                    }],
                    vec![PendingOutbox::new("ORDRSP", data.nb.as_str(), payload).caused_by(0)],
                ))
            }

            WeiterverpflichtungCommand::TimeoutExpired { deadline_id, label } => {
                if matches!(
                    state,
                    WeiterverpflichtungState::Beantwortet(_)
                        | WeiterverpflichtungState::Rejected { .. }
                ) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![WeiterverpflichtungEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// One Weiterverpflichtung stream, as the read model sees it.
#[derive(Debug, Default)]
pub struct WeiterverpflichtungRecord {
    /// Current lifecycle label.
    pub status: &'static str,
    /// The Messlokation, once known.
    pub melo_id: Option<String>,
    /// The Antwortcode that closed the process, once sent.
    pub antwort_code: Option<String>,
}

/// Read model over [`WeiterverpflichtungEvent`].
#[derive(Debug, Default)]
pub struct WeiterverpflichtungProjection {
    /// One record per process stream.
    pub records: HashMap<String, WeiterverpflichtungRecord>,
}

impl Projection for WeiterverpflichtungProjection {
    fn name(&self) -> &'static str {
        "WeiterverpflichtungProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        let Ok(event) = envelope.decode::<WeiterverpflichtungEvent>() else {
            return;
        };
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        match event {
            WeiterverpflichtungEvent::AuftragEmpfangen { melo_id, .. } => {
                record.status = "AuftragEmpfangen";
                record.melo_id = Some(melo_id.as_str().to_owned());
            }
            WeiterverpflichtungEvent::AntwortGesendet { antwort_code, .. } => {
                record.status = "Beantwortet";
                record.antwort_code = Some(antwort_code);
            }
            WeiterverpflichtungEvent::Rejected { .. }
            | WeiterverpflichtungEvent::DeadlineExpired { .. } => {
                record.status = "Rejected";
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auftrag() -> WeiterverpflichtungCommand {
        WeiterverpflichtungCommand::ReceiveAuftrag {
            pid: Pruefidentifikator::new(AUFTRAG_PID).expect("17002 is a valid PID"),
            nb: MarktpartnerCode::new("9900357000004"),
            msba: MarktpartnerCode::new("4012345000023"),
            melo_id: MeLo::new("DE0000000001234567890000000000001"),
            verschobenes_zuordnungsende: "20260501".to_owned(),
            message_ref: MessageRef::new("ORD-17002-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn empfangen() -> WeiterverpflichtungState {
        let s = WeiterverpflichtungState::default();
        let ev = WimWeiterverpflichtungWorkflow::handle(&s, auftrag()).expect("valid");
        ev.iter().fold(s, WimWeiterverpflichtungWorkflow::apply)
    }

    #[test]
    fn a_plain_agreement_answers_on_19003() {
        let out = WimWeiterverpflichtungWorkflow::handle(
            &empfangen(),
            WeiterverpflichtungCommand::DispatchAntwort {
                antwort_code: "Z13".to_owned(),
                abweichender_termin: None,
            },
        )
        .expect("Z13");
        assert_eq!(&*out.outbox[0].message_type, "ORDRSP");
        assert_eq!(out.outbox[0].payload["pid"], 19_003);
        assert_eq!(out.outbox[0].payload["antwort_ebd"], "E_0203");
    }

    /// `Z22` is the refusal and rides 19004 — the cluster picks the PID, not a
    /// boolean the caller passes alongside the code.
    #[test]
    fn a_refusal_answers_on_19004_and_names_the_capped_date() {
        let out = WimWeiterverpflichtungWorkflow::handle(
            &empfangen(),
            WeiterverpflichtungCommand::DispatchAntwort {
                antwort_code: "Z22".to_owned(),
                abweichender_termin: Some("20260401".to_owned()),
            },
        )
        .expect("Z22");
        assert_eq!(out.outbox[0].payload["pid"], 19_004);
        assert_eq!(out.outbox[0].payload["abmeldetermin"], "20260401");
    }

    /// `Z14` means „to a corrected Abmeldetermin", and its Bedingung names the
    /// element the date goes in. Without it the answer asserts a change it does
    /// not state.
    #[test]
    fn a_terminaenderung_without_its_date_is_refused() {
        let err = WimWeiterverpflichtungWorkflow::handle(
            &empfangen(),
            WeiterverpflichtungCommand::DispatchAntwort {
                antwort_code: "Z14".to_owned(),
                abweichender_termin: None,
            },
        )
        .expect_err("Z14 without a date");
        assert!(err.to_string().contains("DE 2380"), "{err}");
    }

    /// A code from another tree never reaches the wire. `Z34` is `E_0200`.
    #[test]
    fn a_foreign_code_is_refused() {
        let err = WimWeiterverpflichtungWorkflow::handle(
            &empfangen(),
            WeiterverpflichtungCommand::DispatchAntwort {
                antwort_code: "Z34".to_owned(),
                abweichender_termin: None,
            },
        )
        .expect_err("Z34 is E_0200");
        assert!(err.to_string().contains("E_0203"), "{err}");
    }

    /// 17001 is the Geräteübernahme Bestellung and is a different process
    /// entirely — the confusion that had 17002 spawning a Geräteübernahme.
    #[test]
    fn the_geraeteubernahme_bestellung_is_not_a_weiterverpflichtung() {
        let mut cmd = auftrag();
        if let WeiterverpflichtungCommand::ReceiveAuftrag { ref mut pid, .. } = cmd {
            *pid = Pruefidentifikator::new(17_001).expect("valid");
        }
        assert!(
            WimWeiterverpflichtungWorkflow::handle(&WeiterverpflichtungState::default(), cmd)
                .is_err()
        );
    }

    /// The answer window is one Werktag, from the same table `makod` registers
    /// the deadline from and `obsd` raises the breach against.
    #[test]
    fn the_answer_window_is_one_werktag() {
        use mako_fristen::antwort::{FristShape, antwort_obligation};
        let o = antwort_obligation(AUFTRAG_PID).expect("published");
        assert_eq!(o.frist, FristShape::WerktageAtCutoff(1));
        assert_eq!(o.antwort_pids, ANTWORT_PIDS);
        assert_eq!(o.ebd, Some(mako_pruefung::codes::EBD_WEITERVERPFLICHTUNG));
    }
}
