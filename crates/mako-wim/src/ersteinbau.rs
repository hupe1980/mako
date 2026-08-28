//! WiM Strom Teil 1 Kap. 3.5 — **Ersteinbau eines iMS in eine bestehende
//! Messlokation** (IFTSTA 21029 → 21030 / 21031, `E_0233`).
//!
//! The one WiM Use-Case where two Messstellenbetreiber negotiate a rollout
//! rather than a Wechsel. The grundzuständiger MSB carries the § 29 MsbG
//! obligation to fit an intelligentes Messsystem, and it reaches Messlokationen
//! a *wettbewerblicher* MSB operates. Kap. 3.5 is how the two settle who
//! installs, and it runs on IFTSTA in both directions.
//!
//! ```text
//! gMSB ──IFTSTA 21029 Vorabinformation──── 3 Monate + 3 WT vor dem ÜZ ───▶ wMSB
//! gMSB ◀─IFTSTA 21030 Zustimmung (A03)──── 3 WT nach dem ÜT ─────────────  wMSB
//!      └IFTSTA 21031 Bestandsschutz / Eigenausbau (A01/A02/A04)
//! gMSB ──IFTSTA 21029 Vorabinformation──── 3 Monate vor dem ÜZ ──────────▶ LF, NB
//! gMSB ◀─IFTSTA 21027 Information über das Scheitern ────────────────────  wMSB
//! gMSB ──IFTSTA 21025 / 21027 „kein Ersteinbau" ─────────────────────────▶ LF, NB
//! ```
//!
//! # Why this is a workflow and not a status line
//!
//! 21029/21030/21031 look like the other IFTSTA Statusmeldungen and are not:
//! **21030 and 21031 name an Entscheidungsbaum.** The Anwendungsübersicht der
//! Prüfidentifikatoren 4.0 (lfd. Nr. 30800/30810) gives both `E_0233`, and the
//! tree publishes four codes with a real Zustimmungs-/Ablehnungsachse. Treating
//! them as informational drops a decision *and* the three-Werktage window the
//! wMSB has to make it in.
//!
//! # The silent case is a refusal
//!
//! `E_0233` `A04` is „Zum jetzigen Zeitpunkt noch keine Aussage hinsichtlich
//! Selbsteinbau möglich" and the BDEW clusters it as an **Ablehnung**. The
//! gMSB may not roll out against it. So both a stated A04 and an expired
//! Antwortfrist leave the rollout blocked — a Vorabinformation nobody answered
//! is not consent, and [`ErsteinbauState::Abgelehnt`] is where both land.
//!
//! # Sparte
//!
//! Strom only. AWH WiM Gas 2.0 restates WiM Strom Teil 1 use-case for use-case
//! *except* this one: there is no iMS rollout obligation in Gas, so the AWH has
//! no Kap. 3.5 equivalent and the Anwendungsübersicht publishes 21029–21031
//! under „WiM Strom Teil 1" alone.
//!
//! # Regulatory basis
//!
//! - **BK6-22-024 Anlage 2a**, WiM Strom Teil 1 Kap. 3.5
//! - **§§ 5, 19 Abs. 5, 29 MsbG** — freie Wahl, Bestandsschutz, Rolloutpflicht
//! - **Entscheidungsbaum-Diagramme und Codelisten 4.3** Kap. 8.8.2
//! - **Anwendungsübersicht der Prüfidentifikatoren 4.0**, lfd. Nr. 30790–30890

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

/// Stable workflow name used as the `WorkflowId.name` and in the `ProcessRegistry`.
pub const WORKFLOW_NAME: &str = "wim-ersteinbau";

/// IFTSTA 21029 — „Vorabinformation zum Gerätewechsel", gMSB → wMSB / LF / NB.
pub const VORABINFORMATION_PID: u32 = mako_fristen::antwort::ERSTEINBAU_VORABINFORMATION_PID;

/// IFTSTA 21030 — „iMS-Ersteinbauzustimmung", wMSB → gMSB. Carries `E_0233` `A03`.
pub const ZUSTIMMUNG_PID: u32 = 21_030;

/// IFTSTA 21031 — „Bestandsschutz / Eigenausbau iMS", wMSB → gMSB.
/// Carries `E_0233` `A01`, `A02` or `A04`.
pub const ABLEHNUNG_PID: u32 = 21_031;

/// IFTSTA 21027 — „Information über das Scheitern", wMSB → gMSB (Kap. 3.5.2
/// Nr. 6), and „Information kein Ersteinbau", gMSB → NB (Nr. 10).
///
/// Shared with the Messlokationsänderung, where it carries `E_0286`; here it
/// carries no code at all.
pub const SCHEITERN_PID: u32 = 21_027;

/// IFTSTA 21025 — „Information kein Ersteinbau", gMSB → LF (Kap. 3.5.2 Nr. 9).
pub const KEIN_ERSTEINBAU_LF_PID: u32 = 21_025;

/// Every Prüfidentifikator this workflow routes.
pub const ERSTEINBAU_PIDS: &[u32] = &[VORABINFORMATION_PID, ZUSTIMMUNG_PID, ABLEHNUNG_PID];

/// Deadline label for the wMSB's three-Werktage answer window.
pub const ANTWORT_WINDOW_LABEL: &str = "wim-ersteinbau-antwort-frist";

/// The Entscheidungsbaum every answer in this Use-Case resolves against.
pub const ERSTEINBAU_EBD: &str = mako_pruefung::codes::EBD_ERSTEINBAU_IMS;

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the Ersteinbau workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ErsteinbauEvent {
    /// A Vorabinformation arrived — this party is the wMSB and owes an answer.
    VorabinformationEmpfangen {
        /// The Messlokation the gMSB wants to fit an iMS at.
        melo_id: MeLo,
        /// MP-ID of the grundzuständiger MSB.
        gmsb: MarktpartnerCode,
        /// MP-ID of this party (the wettbewerblicher MSB).
        wmsb: MarktpartnerCode,
        /// The planned Umstellungszeitpunkt the Vorabinformation names.
        umstellungszeitpunkt: String,
        /// Reference of the inbound IFTSTA, echoed by the answer.
        message_ref: MessageRef,
    },
    /// A Vorabinformation went out — this party is the gMSB.
    VorabinformationGesendet {
        /// The Messlokation.
        melo_id: MeLo,
        /// MP-ID of this party (the grundzuständiger MSB).
        gmsb: MarktpartnerCode,
        /// MP-ID of the wettbewerblicher MSB addressed.
        wmsb: MarktpartnerCode,
        /// The planned Umstellungszeitpunkt.
        umstellungszeitpunkt: String,
        /// Reference of the outbound IFTSTA.
        message_ref: MessageRef,
    },
    /// The wMSB's answer went out or came back.
    AntwortGesendet {
        /// 21030 (Zustimmung) or 21031 (Ablehnung).
        pruefidentifikator: Pruefidentifikator,
        /// `E_0233` `A01`–`A04`.
        antwort_code: String,
    },
    /// The rollout failed on site (21027, Kap. 3.5.2 Nr. 6).
    ScheiternGemeldet {
        /// Human-readable reason, for the audit trail.
        reason: String,
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

impl EventPayload for ErsteinbauEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::VorabinformationEmpfangen { .. } => "WimErsteinbauVorabinformationEmpfangen",
            Self::VorabinformationGesendet { .. } => "WimErsteinbauVorabinformationGesendet",
            Self::AntwortGesendet { .. } => "WimErsteinbauAntwortGesendet",
            Self::ScheiternGemeldet { .. } => "WimErsteinbauScheiternGemeldet",
            Self::Rejected { .. } => "WimErsteinbauRejected",
            Self::DeadlineExpired { .. } => "WimErsteinbauDeadlineExpired",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data carried from the first event onwards.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErsteinbauData {
    /// The Messlokation.
    pub melo_id: MeLo,
    /// The grundzuständiger MSB — sender on the inbound branch, this party on
    /// the outbound one.
    pub gmsb: MarktpartnerCode,
    /// The wettbewerblicher MSB — this party on the inbound branch.
    pub wmsb: MarktpartnerCode,
    /// The planned Umstellungszeitpunkt the Vorabinformation names.
    pub umstellungszeitpunkt: String,
    /// Reference of the IFTSTA that opened the Vorgang.
    pub message_ref: MessageRef,
}

/// State of an Ersteinbau process.
///
/// ```text
/// New ─┬─ VorabinformationEmpfangen ─▶ Angekündigt ─┬─ Zugestimmt
///      │  (inbound, we are the wMSB)                ├─ Abgelehnt
///      │                                            └─ Gescheitert
///      └─ VorabinformationGesendet ──▶ Angekündigt
///         (outbound, we are the gMSB)
/// ```
///
/// Both branches share `Angekündigt` because the three-Werktage window is the
/// same fact from either side: the wMSB owes the answer, the gMSB awaits it,
/// and neither may roll out until it arrives.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
#[serde(tag = "status", content = "data")]
pub enum ErsteinbauState {
    /// No events yet.
    #[default]
    New,
    /// The Vorabinformation is out; the wMSB owes an answer in 3 Werktagen.
    Angekuendigt(ErsteinbauData),
    /// `A03` — the wMSB waived its Selbsteinbau; the gMSB may fit the iMS.
    Zugestimmt(ErsteinbauData),
    /// `A01`, `A02` or `A04`, or an expired Antwortfrist. The gMSB may **not**
    /// fit the iMS.
    Abgelehnt {
        /// The `E_0233` code, or `None` when the window simply expired.
        antwort_code: Option<String>,
        /// Why, for the operator queue.
        reason: String,
    },
    /// The rollout was consented to and then failed on site (21027).
    Gescheitert {
        /// Why.
        reason: String,
    },
    /// Terminal failure before any answer — a malformed Vorabinformation.
    Rejected {
        /// Reason.
        reason: String,
    },
}

impl ErsteinbauState {
    /// Stable label for the current variant.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Angekuendigt(_) => "Angekuendigt",
            Self::Zugestimmt(_) => "Zugestimmt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Gescheitert { .. } => "Gescheitert",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// Whether the gMSB is cleared to fit the iMS.
    ///
    /// Only an explicit `A03` clears it. Silence does not, and neither does
    /// `A04` — see the module note.
    #[must_use]
    pub const fn rollout_freigegeben(&self) -> bool {
        matches!(self, Self::Zugestimmt(_))
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for ErsteinbauState {
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::Angekuendigt(_))
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the Ersteinbau workflow.
#[derive(Clone)]
pub enum ErsteinbauCommand {
    /// Inbound IFTSTA 21029 from the gMSB — this party is the wMSB.
    ReceiveVorabinformation {
        /// Must be [`VORABINFORMATION_PID`].
        pid: Pruefidentifikator,
        /// MP-ID of the grundzuständiger MSB.
        gmsb: MarktpartnerCode,
        /// MP-ID of this party.
        wmsb: MarktpartnerCode,
        /// The Messlokation.
        melo_id: MeLo,
        /// The planned Umstellungszeitpunkt.
        umstellungszeitpunkt: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if the IFTSTA passed AHB validation.
        validation_passed: bool,
        /// Validation issues, for the `Rejected` event.
        validation_errors: Vec<String>,
    },
    /// Send an IFTSTA 21029 — this party is the gMSB.
    ///
    /// The Vorlauffrist („3 Monate und 3 WT vor dem geplanten Umstellungs­
    /// zeitpunkt", Kap. 3.5.2 Nr. 1) is checked by the caller against
    /// `mako_fristen::vorlauf::vorlauf("wim.vorabinformation-ersteinbau-ims")`;
    /// a workflow cannot know today's date.
    SendVorabinformation {
        /// MP-ID of this party.
        gmsb: MarktpartnerCode,
        /// MP-ID of the wettbewerblicher MSB.
        wmsb: MarktpartnerCode,
        /// The Messlokation.
        melo_id: MeLo,
        /// The planned Umstellungszeitpunkt.
        umstellungszeitpunkt: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// Send or record the `E_0233` answer.
    ///
    /// The code's **cluster** picks 21030 or 21031 — never a boolean beside it.
    DispatchAntwort {
        /// `E_0233` `A01`–`A04`.
        antwort_code: String,
    },
    /// The rollout failed on site — IFTSTA 21027 (Kap. 3.5.2 Nr. 6).
    MeldeScheitern {
        /// Why.
        reason: String,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for ErsteinbauCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The WiM Ersteinbau workflow (IFTSTA 21029 → 21030 / 21031).
pub struct WimErsteinbauWorkflow;

impl Workflow for WimErsteinbauWorkflow {
    type State = ErsteinbauState;
    type Event = ErsteinbauEvent;
    type Command = ErsteinbauCommand;

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        match (deadline.label(), state) {
            (ANTWORT_WINDOW_LABEL, ErsteinbauState::Angekuendigt(_)) => {
                Some(ErsteinbauCommand::TimeoutExpired {
                    deadline_id: deadline.deadline_id(),
                    label: deadline.label().into(),
                })
            }
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ErsteinbauEvent::VorabinformationEmpfangen {
                melo_id,
                gmsb,
                wmsb,
                umstellungszeitpunkt,
                message_ref,
            }
            | ErsteinbauEvent::VorabinformationGesendet {
                melo_id,
                wmsb,
                umstellungszeitpunkt,
                message_ref,
                gmsb,
            } => ErsteinbauState::Angekuendigt(ErsteinbauData {
                melo_id: melo_id.clone(),
                gmsb: gmsb.clone(),
                wmsb: wmsb.clone(),
                umstellungszeitpunkt: umstellungszeitpunkt.clone(),
                message_ref: message_ref.clone(),
            }),
            ErsteinbauEvent::AntwortGesendet {
                pruefidentifikator,
                antwort_code,
            } => match state {
                ErsteinbauState::Angekuendigt(d) => {
                    if pruefidentifikator.as_u32() == ZUSTIMMUNG_PID {
                        ErsteinbauState::Zugestimmt(d)
                    } else {
                        ErsteinbauState::Abgelehnt {
                            antwort_code: Some(antwort_code.clone()),
                            reason: format!(
                                "E_0233 {antwort_code} — der Ersteinbau eines iMS durch den gMSB \
                                 darf nicht erfolgen"
                            ),
                        }
                    }
                }
                other => other,
            },
            ErsteinbauEvent::ScheiternGemeldet { reason } => ErsteinbauState::Gescheitert {
                reason: reason.clone(),
            },
            ErsteinbauEvent::Rejected { reason } => ErsteinbauState::Rejected {
                reason: reason.clone(),
            },
            // An unanswered Vorabinformation blocks the rollout exactly as an
            // `A04` does: the gMSB never received the wMSB's waiver, so there
            // is nothing to install against.
            ErsteinbauEvent::DeadlineExpired { label, .. } => match state {
                ErsteinbauState::Angekuendigt(_) | ErsteinbauState::New => {
                    ErsteinbauState::Abgelehnt {
                        antwort_code: None,
                        reason: format!(
                            "Antwortfrist abgelaufen ({label}) — ohne Zustimmung des wMSB darf \
                             der gMSB kein iMS einbauen"
                        ),
                    }
                }
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ErsteinbauCommand::ReceiveVorabinformation {
                pid,
                gmsb,
                wmsb,
                melo_id,
                umstellungszeitpunkt,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ErsteinbauState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != VORABINFORMATION_PID {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not the Vorabinformation zum Ersteinbau ({VORABINFORMATION_PID})",
                        pid.as_u32()
                    )));
                }
                if !validation_passed {
                    return Ok(vec![ErsteinbauEvent::Rejected {
                        reason: validation_errors.join("; "),
                    }]
                    .into());
                }
                Ok(vec![ErsteinbauEvent::VorabinformationEmpfangen {
                    melo_id,
                    gmsb,
                    wmsb,
                    umstellungszeitpunkt,
                    message_ref,
                }]
                .into())
            }

            ErsteinbauCommand::SendVorabinformation {
                gmsb,
                wmsb,
                melo_id,
                umstellungszeitpunkt,
                message_ref,
            } => {
                if !matches!(state, ErsteinbauState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let payload = serde_json::json!({
                    "pid":                  VORABINFORMATION_PID,
                    "sender":               gmsb.as_str(),
                    "receiver":             wmsb.as_str(),
                    "melo":                 melo_id.as_str(),
                    "umstellungszeitpunkt": umstellungszeitpunkt,
                });
                Ok(WorkflowOutput::with_outbox(
                    vec![ErsteinbauEvent::VorabinformationGesendet {
                        melo_id,
                        gmsb,
                        wmsb: wmsb.clone(),
                        umstellungszeitpunkt,
                        message_ref,
                    }],
                    vec![PendingOutbox::new("IFTSTA", wmsb.as_str(), payload).caused_by(0)],
                ))
            }

            ErsteinbauCommand::DispatchAntwort { antwort_code } => {
                let ErsteinbauState::Angekuendigt(data) = state else {
                    return Err(WorkflowError::invalid_state("Angekuendigt", state.label()));
                };
                let code = mako_pruefung::codes::lookup(ERSTEINBAU_EBD, &antwort_code).ok_or_else(
                    || {
                        WorkflowError::rejected(format!(
                            "Antwortcode {antwort_code:?} is not published in {ERSTEINBAU_EBD}"
                        ))
                    },
                )?;
                let zustimmung = code.ist_zustimmung().ok_or_else(|| {
                    WorkflowError::rejected(format!("{} sits off the agreement axis", code.code))
                })?;
                let antwort_pid = if zustimmung {
                    ZUSTIMMUNG_PID
                } else {
                    ABLEHNUNG_PID
                };
                let payload = serde_json::json!({
                    "pid":              antwort_pid,
                    "sender":           data.wmsb.as_str(),
                    "receiver":         data.gmsb.as_str(),
                    "melo":             data.melo_id.as_str(),
                    "antwort_code":     code.code,
                    "antwort_ebd":      ERSTEINBAU_EBD,
                    "orig_message_ref": data.message_ref.as_str(),
                });
                Ok(WorkflowOutput::with_outbox(
                    vec![ErsteinbauEvent::AntwortGesendet {
                        pruefidentifikator: Pruefidentifikator::new(antwort_pid)
                            .map_err(WorkflowError::rejected)?,
                        antwort_code: code.code.to_owned(),
                    }],
                    vec![PendingOutbox::new("IFTSTA", data.gmsb.as_str(), payload).caused_by(0)],
                ))
            }

            ErsteinbauCommand::MeldeScheitern { reason } => {
                // Kap. 3.5.2 Nr. 6 puts the Scheitermeldung after the rollout
                // was consented to. A Vorgang that never got a `A03` has no
                // rollout to fail.
                let ErsteinbauState::Zugestimmt(data) = state else {
                    return Err(WorkflowError::invalid_state("Zugestimmt", state.label()));
                };
                let payload = serde_json::json!({
                    "pid":      SCHEITERN_PID,
                    "sender":   data.wmsb.as_str(),
                    "receiver": data.gmsb.as_str(),
                    "melo":     data.melo_id.as_str(),
                    "grund":    reason,
                });
                Ok(WorkflowOutput::with_outbox(
                    vec![ErsteinbauEvent::ScheiternGemeldet { reason }],
                    vec![PendingOutbox::new("IFTSTA", data.gmsb.as_str(), payload).caused_by(0)],
                ))
            }

            ErsteinbauCommand::TimeoutExpired { deadline_id, label } => {
                if !matches!(state, ErsteinbauState::Angekuendigt(_)) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![ErsteinbauEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// One Ersteinbau stream, as the read model sees it.
#[derive(Debug, Default)]
pub struct ErsteinbauRecord {
    /// Current lifecycle label.
    pub status: &'static str,
    /// The Messlokation, once known.
    pub melo_id: Option<String>,
    /// The `E_0233` code that closed the process, once answered.
    pub antwort_code: Option<String>,
    /// Whether the gMSB is cleared to fit the iMS.
    pub rollout_freigegeben: bool,
}

/// Read model over [`ErsteinbauEvent`].
#[derive(Debug, Default)]
pub struct ErsteinbauProjection {
    /// One record per process stream.
    pub records: HashMap<String, ErsteinbauRecord>,
}

impl Projection for ErsteinbauProjection {
    fn name(&self) -> &'static str {
        "ErsteinbauProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        let Ok(event) = envelope.decode::<ErsteinbauEvent>() else {
            return;
        };
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();
        match event {
            ErsteinbauEvent::VorabinformationEmpfangen { melo_id, .. }
            | ErsteinbauEvent::VorabinformationGesendet { melo_id, .. } => {
                record.status = "Angekuendigt";
                record.melo_id = Some(melo_id.as_str().to_owned());
            }
            ErsteinbauEvent::AntwortGesendet {
                pruefidentifikator,
                antwort_code,
            } => {
                let zustimmung = pruefidentifikator.as_u32() == ZUSTIMMUNG_PID;
                record.status = if zustimmung {
                    "Zugestimmt"
                } else {
                    "Abgelehnt"
                };
                record.rollout_freigegeben = zustimmung;
                record.antwort_code = Some(antwort_code);
            }
            ErsteinbauEvent::ScheiternGemeldet { .. } => {
                record.status = "Gescheitert";
                record.rollout_freigegeben = false;
            }
            ErsteinbauEvent::Rejected { .. } => record.status = "Rejected",
            ErsteinbauEvent::DeadlineExpired { .. } => {
                if record.status == "Angekuendigt" {
                    record.status = "Abgelehnt";
                    record.rollout_freigegeben = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }

    fn melo() -> MeLo {
        MeLo::new("DE0000000001234567890000000000001")
    }

    fn vorabinformation() -> ErsteinbauCommand {
        ErsteinbauCommand::ReceiveVorabinformation {
            pid: Pruefidentifikator::new(VORABINFORMATION_PID).expect("valid PID"),
            gmsb: mp("9900000000001"),
            wmsb: mp("9900000000003"),
            melo_id: melo(),
            umstellungszeitpunkt: "20261201".to_owned(),
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn angekuendigt() -> ErsteinbauState {
        let out = WimErsteinbauWorkflow::handle(&ErsteinbauState::New, vorabinformation())
            .expect("accepted");
        out.events
            .iter()
            .fold(ErsteinbauState::New, WimErsteinbauWorkflow::apply)
    }

    #[test]
    fn a_vorabinformation_opens_the_answer_window() {
        assert!(matches!(angekuendigt(), ErsteinbauState::Angekuendigt(_)));
    }

    #[test]
    fn a03_clears_the_rollout_and_rides_21030() {
        let out = WimErsteinbauWorkflow::handle(
            &angekuendigt(),
            ErsteinbauCommand::DispatchAntwort {
                antwort_code: "A03".to_owned(),
            },
        )
        .expect("A03 is published in E_0233");
        let ErsteinbauEvent::AntwortGesendet {
            pruefidentifikator, ..
        } = &out.events[0]
        else {
            panic!("expected an AntwortGesendet");
        };
        assert_eq!(pruefidentifikator.as_u32(), ZUSTIMMUNG_PID);
        let state = out
            .events
            .iter()
            .fold(angekuendigt(), WimErsteinbauWorkflow::apply);
        assert!(state.rollout_freigegeben());
    }

    /// The finding this workflow exists for: `A04` reads like a deferral and is
    /// clustered as an Ablehnung, so it rides 21031 and blocks the rollout.
    #[test]
    fn a04_is_a_refusal_and_blocks_the_rollout() {
        let out = WimErsteinbauWorkflow::handle(
            &angekuendigt(),
            ErsteinbauCommand::DispatchAntwort {
                antwort_code: "A04".to_owned(),
            },
        )
        .expect("A04 is published in E_0233");
        let ErsteinbauEvent::AntwortGesendet {
            pruefidentifikator, ..
        } = &out.events[0]
        else {
            panic!("expected an AntwortGesendet");
        };
        assert_eq!(pruefidentifikator.as_u32(), ABLEHNUNG_PID);
        let state = out
            .events
            .iter()
            .fold(angekuendigt(), WimErsteinbauWorkflow::apply);
        assert!(!state.rollout_freigegeben());
        assert!(matches!(state, ErsteinbauState::Abgelehnt { .. }));
    }

    #[test]
    fn every_ablehnungscode_rides_21031() {
        for c in ["A01", "A02", "A04"] {
            let out = WimErsteinbauWorkflow::handle(
                &angekuendigt(),
                ErsteinbauCommand::DispatchAntwort {
                    antwort_code: c.to_owned(),
                },
            )
            .expect("published in E_0233");
            let ErsteinbauEvent::AntwortGesendet {
                pruefidentifikator, ..
            } = &out.events[0]
            else {
                panic!("expected an AntwortGesendet");
            };
            assert_eq!(pruefidentifikator.as_u32(), ABLEHNUNG_PID, "code {c}");
        }
    }

    /// Silence is not consent — an expired window leaves the rollout blocked.
    #[test]
    fn an_expired_antwortfrist_blocks_the_rollout() {
        let state = WimErsteinbauWorkflow::apply(
            angekuendigt(),
            &ErsteinbauEvent::DeadlineExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        );
        assert!(!state.rollout_freigegeben());
        let ErsteinbauState::Abgelehnt { antwort_code, .. } = &state else {
            panic!("expected Abgelehnt, got {}", state.label());
        };
        assert_eq!(*antwort_code, None, "no code was ever stated");
    }

    #[test]
    fn a_code_from_another_tree_is_refused() {
        // `E15` is the WiM MSB-Wechsel Zustimmung and has no meaning in E_0233.
        assert!(
            WimErsteinbauWorkflow::handle(
                &angekuendigt(),
                ErsteinbauCommand::DispatchAntwort {
                    antwort_code: "E15".to_owned(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn a_scheitermeldung_needs_a_consented_rollout() {
        assert!(
            WimErsteinbauWorkflow::handle(
                &angekuendigt(),
                ErsteinbauCommand::MeldeScheitern {
                    reason: "kein Zugang".to_owned(),
                },
            )
            .is_err(),
            "a rollout that was never consented to cannot fail"
        );
    }

    #[test]
    fn the_wrong_pid_is_refused() {
        let mut cmd = vorabinformation();
        if let ErsteinbauCommand::ReceiveVorabinformation { ref mut pid, .. } = cmd {
            *pid = Pruefidentifikator::new(21_007).expect("valid PID");
        }
        assert!(WimErsteinbauWorkflow::handle(&ErsteinbauState::New, cmd).is_err());
    }

    /// The window this workflow arms and the one `mako-fristen` publishes must
    /// be one number.
    #[test]
    fn the_answer_window_is_the_published_one() {
        assert_eq!(mako_fristen::antwort::ERSTEINBAU_ANTWORT_WERKTAGE, 3);
        let o = mako_fristen::antwort::WIM
            .iter()
            .find(|o| o.trigger_pid == VORABINFORMATION_PID)
            .expect("21029 is in the WiM Antwortfrist table");
        assert_eq!(o.antwort_pids, (ZUSTIMMUNG_PID, ABLEHNUNG_PID));
        assert_eq!(o.ebd, Some(ERSTEINBAU_EBD));

        // Being in that table is also what makes the window observable:
        // `obsd::derive_family` reads `antwort_obligation` first, so the
        // Ersteinbau breach is bucketed as WiM without obsd knowing this
        // workflow exists.
        let via_lookup = mako_fristen::antwort::antwort_obligation(VORABINFORMATION_PID)
            .expect("21029 is discoverable by PID");
        assert_eq!(via_lookup.family.as_str(), "wim");
    }
}
