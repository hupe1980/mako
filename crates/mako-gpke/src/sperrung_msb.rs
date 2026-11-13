//! GPKE Sperrung — MSB side: the NB's Anfrage Sperrung (ORDERS 17116).
//!
//! Where no generelle Zustimmung des MSB exists, the NB asks the MSB to agree
//! to the Sperrung (and the later Entsperrung) before the Sperrtermin
//! (BK6-24-174 GPKE Teil 2 Kap. 3.5.1.2 Nr. 3, S. 106). The MSB answers with
//! ORDRSP 19118 (Bestätigung) or 19119 (Ablehnung) — ORDERS AHB 1.1b Kap. 4.3
//! „NB an MSB" (S. 25), ORDRSP AHB 1.1b Kap. 4.3.1 „MSB an NB" (S. 17).
//!
//! | PID   | Message                    | Direction |
//! |-------|----------------------------|-----------|
//! | 17116 | Anfrage Sperrung           | NB → MSB  |
//! | 19118 | Bestätigung Anfrage Sperrung | MSB → NB (the NB's [`crate::sperrung`] records it) |
//! | 19119 | Ablehnung Anfrage Sperrung | MSB → NB  |
//!
//! # The Frist and its lapse
//!
//! Nr. 4 „Antwort auf Anfrage": „Unverzüglich, jedoch spätester ÜT ist der 3.
//! WT nach dem ÜT von Nr. 3" — [`mako_fristen::antwort`]'s 17116 row. The same
//! step states the lapse's outcome: „Verstreicht die Frist, ohne dass die
//! Antwort auf die Anfrage beim NB eingeht, gilt dies als Zustimmung im Sinne
//! „MSB hat Durchführung der Sperrung und Entsperrung durch NB zugestimmt“.
//! Nach Ablauf der Frist eingehende Antworten sind für den Fortlauf dieses
//! Prozesses unerheblich." (S. 107). So a lapse here is not a
//! `FristAbgelaufen` but a sourced end state, [`SperrungMsbState::ZustimmungFingiert`].
//!
//! # Why its own workflow
//!
//! The NB's [`crate::sperrung`] process runs on the same Marktlokation. A
//! deployment that is NB and MSB at once (the gMSB of § 2 Nr. 6 MsbG is often
//! the NB) would otherwise resume the NB's own Sperrauftrag with the MSB's copy
//! of the Anfrage; under its own workflow name the two sides never meet.
//!
//! # The answer
//!
//! [`SperrungMsbCommand::SendAntwort`] answers out of `E_0488` „Anfrage
//! prüfen" (EBD 4.3 Kap. 6.12.2, S. 242–243): the code's Cluster picks the
//! PID — Zustimmung (`A01`, `A04`, `A05`) rides 19118, Ablehnung (`A02`,
//! `A03`) 19119 — and the ORDRSP carries `BGM+Z51`, `SG1 RFF+ON` the 17116's
//! `BGM` DE 1004 (Hinweis `[529]`), `SG2 AJT` the code with „EBD Nr. E_0488",
//! and beside `A02`/`A03` the problem named in `SG2 FTX+AAP` (`[33]`) —
//! ORDRSP AHB 1.1b Kap. 4.3.1, S. 17–18.

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Workflow name for the MSB side of the Anfrage Sperrung.
pub const WORKFLOW_NAME: &str = "gpke-sperrung-msb";

/// ORDERS 17116 „Anfrage Sperrung", NB → MSB (ORDERS AHB 1.1b Kap. 4.3, S. 25).
pub const ANFRAGE_PID: u32 = 17_116;

/// ORDRSP 19118 „Bestätigung Anfrage Sperrung", MSB → NB.
pub const BESTAETIGUNG_PID: u32 = 19_118;

/// ORDRSP 19119 „Ablehnung Anfrage Sperrung", MSB → NB.
pub const ABLEHNUNG_PID: u32 = 19_119;

/// `BGM` DE 1001 of the answer: `Z51` „Sperrung" — the only value the 19118 /
/// 19119 columns admit (ORDRSP AHB 1.1b Kap. 4.3.1, S. 17).
pub const BGM_SPERRUNG: &str = "Z51";

/// The MSB's answer window on an Anfrage Sperrung — 3 WT nach dem ÜT
/// (GPKE Teil 2 Kap. 3.5.1.2 Nr. 4), read from [`mako_fristen::antwort`].
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-sperrung-msb-antwort";

/// When the MSB's answer to an Anfrage Sperrung received at `eingang` is due:
/// the 17116 row of [`mako_fristen::antwort`].
#[must_use]
pub fn antwortfrist(eingang: time::OffsetDateTime) -> Option<time::OffsetDateTime> {
    mako_fristen::antwort::antwort_deadline(ANFRAGE_PID, eingang)
}

/// The Anfrage as the MSB received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AnfrageSperrung {
    /// The NB asking — `NAD+MS`.
    pub netzbetreiber: MarktpartnerCode,
    /// The MSB asked — `NAD+MR`; the answer's sender.
    pub msb: MarktpartnerCode,
    /// The Marktlokation to be gesperrt — `SG2 LOC+172`.
    pub malo: MaLo,
    /// The ORDERS' Dokumentennummer — `BGM` DE 1004, which the answer echoes in
    /// `SG1 RFF+ON` (ORDRSP AHB 1.1b Kap. 4.3.1, Hinweis `[529]`).
    pub dokumentennummer: String,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// The MSB's answer as sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AntwortSperrung {
    /// 19118 or 19119 — picked by the code's Cluster.
    pub pid: Pruefidentifikator,
    /// The `E_0488` code.
    pub antwortcode: String,
    /// Reference of the outbound ORDRSP.
    pub message_ref: MessageRef,
}

/// The answer PID and the published code for `antwortcode` out of the tree
/// `codeliste` names.
///
/// # Errors
///
/// The code is not published in `codeliste`, sits off the agreement axis, or
/// needs the problem named and `freitext` is empty.
pub fn antwort_auf_anfrage(
    codeliste: &str,
    antwortcode: &str,
    freitext: Option<&str>,
) -> Result<(u32, &'static mako_pruefung::codes::AntwortCode), WorkflowError> {
    let code = mako_pruefung::codes::lookup(codeliste, antwortcode).ok_or_else(|| {
        WorkflowError::validation(format!(
            "{antwortcode:?} is not published in {codeliste} (EBD 4.3)"
        ))
    })?;
    let pid = match code.ist_zustimmung() {
        Some(true) => BESTAETIGUNG_PID,
        Some(false) => ABLEHNUNG_PID,
        None => {
            return Err(WorkflowError::validation(format!(
                "{antwortcode:?} of {codeliste} is neither a Zustimmung nor an Ablehnung"
            )));
        }
    };
    if code.braucht_bemerkung && freitext.is_none_or(|t| t.trim().is_empty()) {
        return Err(WorkflowError::validation(format!(
            "{antwortcode:?} of {codeliste} requires the problem named — SG2 FTX+AAP is Muss \
             beside it (ORDRSP AHB 1.1b Kap. 4.3.1)"
        )));
    }
    Ok((pid, code))
}

/// The ORDRSP 19118 / 19119 payload — ORDRSP AHB 1.1b Kap. 4.3.1.
#[must_use]
pub fn antwort_outbox(
    anfrage: &AnfrageSperrung,
    antwort: &AntwortSperrung,
    codeliste: &str,
    freitext: Option<&str>,
) -> PendingOutbox {
    PendingOutbox::typed(
        anfrage.netzbetreiber.as_str(),
        &mako_engine::payload::OrdrspPayload {
            pid: Some(antwort.pid.as_u32()),
            sender: Some(anfrage.msb.as_str().to_owned()),
            receiver: Some(anfrage.netzbetreiber.as_str().to_owned()),
            message_ref: Some(antwort.message_ref.as_str().to_owned()),
            document_code: Some(BGM_SPERRUNG.to_owned()),
            document_id: Some(antwort.message_ref.as_str().to_owned()),
            order_reference: Some(anfrage.dokumentennummer.clone()),
            antwort_code: Some(antwort.antwortcode.clone()),
            antwort_codeliste: Some(codeliste.to_owned()),
            antwort_freitext: freitext.map(str::to_owned),
            ..Default::default()
        },
    )
}

/// Events of the MSB-side Anfrage Sperrung.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SperrungMsbEvent {
    /// ORDERS 17116 received.
    AnfrageEingegangen {
        /// ORDERS Prüfidentifikator (17116).
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: AnfrageSperrung,
    },
    /// The ORDERS failed its AHB check.
    AnfrageUngueltig {
        /// The findings.
        reason: String,
    },
    /// The MSB answered with ORDRSP 19118 / 19119.
    AntwortGesendet {
        /// The answer.
        antwort: AntwortSperrung,
        /// The tree the code was read from (`E_0488`).
        codeliste: String,
    },
    /// The 3-WT window passed without an answer: the Festlegung counts that as
    /// the Zustimmung „MSB hat Durchführung der Sperrung und Entsperrung durch
    /// NB zugestimmt“ (GPKE Teil 2 Kap. 3.5.1.2 Nr. 4).
    ZustimmungFingiert {
        /// The fired deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl EventPayload for SperrungMsbEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageEingegangen { .. } => "SperrungMsbAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "SperrungMsbAnfrageUngueltig",
            Self::AntwortGesendet { .. } => "SperrungMsbAntwortGesendet",
            Self::ZustimmungFingiert { .. } => "SperrungMsbZustimmungFingiert",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageEingegangen { pid, anfrage } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(anfrage.message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet { antwort, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(antwort.pid.as_u32()),
                message_ref: Some(antwort.message_ref.as_str().to_owned()),
            }),
            Self::AnfrageUngueltig { .. } | Self::ZustimmungFingiert { .. } => None,
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortGesendet { antwort, codeliste } => Some(Antwortcode {
                code: antwort.antwortcode.clone(),
                codeliste: Some(codeliste.clone()),
            }),
            _ => None,
        }
    }
}

/// State of the MSB-side Anfrage Sperrung.
///
/// ```text
/// New → Offen → Beantwortet          (ORDRSP 19118 / 19119 sent)
///            ↘ ZustimmungFingiert   (3 WT passed without an answer)
///     ↘ Ungueltig                    (AHB check failed)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum SperrungMsbState {
    /// Nothing received yet.
    #[default]
    New,
    /// The Anfrage is open; the MSB owes its answer.
    Offen(AnfrageSperrung),
    /// The Anfrage failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// The Frist passed: the Festlegung counts the silence as Zustimmung.
    ZustimmungFingiert(AnfrageSperrung),
    /// The MSB answered with ORDRSP 19118 / 19119.
    Beantwortet {
        /// The Anfrage.
        anfrage: AnfrageSperrung,
        /// The answer.
        antwort: AntwortSperrung,
    },
}

impl SperrungMsbState {
    /// Stable label of the variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Offen(_) => "Offen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::ZustimmungFingiert(_) => "ZustimmungFingiert",
            Self::Beantwortet { .. } => "Beantwortet",
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Ungueltig { .. } | Self::ZustimmungFingiert(_) | Self::Beantwortet { .. }
        )
    }
}

impl mako_engine::workflow::OccupiesBusinessKey for SperrungMsbState {
    /// An open Anfrage holds the Marktlokation; a settled one does not, so the
    /// NB's next Anfrage on it opens a new process.
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::Offen(_))
    }
}

/// Commands of the MSB-side Anfrage Sperrung.
#[derive(Debug, Clone)]
pub enum SperrungMsbCommand {
    /// Inbound ORDERS 17116.
    ReceiveAnfrage {
        /// Prüfidentifikator read from `SG1 RFF+Z13`.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: AnfrageSperrung,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// The MSB answers out of `E_0488`.
    SendAntwort {
        /// The `E_0488` code; its Cluster picks 19118 or 19119.
        antwortcode: String,
        /// `SG2 FTX+AAP` — the problem named, Muss beside `A02`/`A03`.
        freitext: Option<String>,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for SperrungMsbCommand {}

/// MSB side of the GPKE Anfrage Sperrung (ORDERS 17116).
pub struct GpkeSperrungMsbWorkflow;

/// The one guard a fired deadline passes: the answer window is open while the
/// Anfrage is.
fn window_open(label: &str, state: &SperrungMsbState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, SperrungMsbState::Offen(_))
}

impl Workflow for GpkeSperrungMsbWorkflow {
    type State = SperrungMsbState;
    type Event = SperrungMsbEvent;
    type Command = SperrungMsbCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| SperrungMsbCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            SperrungMsbEvent::AnfrageEingegangen { anfrage, .. } => {
                SperrungMsbState::Offen(anfrage.clone())
            }
            SperrungMsbEvent::AnfrageUngueltig { reason } => SperrungMsbState::Ungueltig {
                reason: reason.clone(),
            },
            SperrungMsbEvent::ZustimmungFingiert { .. } => match state {
                SperrungMsbState::Offen(anfrage) => SperrungMsbState::ZustimmungFingiert(anfrage),
                other => other,
            },
            SperrungMsbEvent::AntwortGesendet { antwort, .. } => match state {
                SperrungMsbState::Offen(anfrage) => SperrungMsbState::Beantwortet {
                    anfrage,
                    antwort: antwort.clone(),
                },
                other => other,
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            SperrungMsbCommand::ReceiveAnfrage {
                pid,
                anfrage,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, SperrungMsbState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if pid.as_u32() != ANFRAGE_PID {
                    return Err(WorkflowError::rejected(format!(
                        "expected the Anfrage Sperrung {ANFRAGE_PID}, got {pid}"
                    )));
                }
                let mut events = vec![SperrungMsbEvent::AnfrageEingegangen { pid, anfrage }];
                if !validation_passed {
                    events.push(SperrungMsbEvent::AnfrageUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            SperrungMsbCommand::SendAntwort {
                antwortcode,
                freitext,
                message_ref,
            } => {
                let SperrungMsbState::Offen(anfrage) = state else {
                    return Err(WorkflowError::invalid_state("Offen", state.label()));
                };
                let codeliste = mako_pruefung::codes::EBD_ANFRAGE_SPERRUNG_MSB;
                let (pid, _) = antwort_auf_anfrage(codeliste, &antwortcode, freitext.as_deref())?;
                let antwort = AntwortSperrung {
                    pid: Pruefidentifikator::new(pid)
                        .map_err(|e| WorkflowError::validation(format!("answer PID {pid}: {e}")))?,
                    antwortcode,
                    message_ref,
                };
                let outbox = vec![
                    antwort_outbox(anfrage, &antwort, codeliste, freitext.as_deref()).caused_by(0),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![SperrungMsbEvent::AntwortGesendet {
                        antwort,
                        codeliste: codeliste.to_owned(),
                    }],
                    outbox,
                ))
            }
            SperrungMsbCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, state) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![SperrungMsbEvent::ZustimmungFingiert { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anfrage() -> SperrungMsbCommand {
        SperrungMsbCommand::ReceiveAnfrage {
            pid: Pruefidentifikator::new(ANFRAGE_PID).expect("five digits"),
            anfrage: AnfrageSperrung {
                netzbetreiber: MarktpartnerCode::new("9900357000004"),
                msb: MarktpartnerCode::new("9900555000005"),
                malo: MaLo::new("51238696012"),
                dokumentennummer: "SPERR-4711".to_owned(),
                message_ref: MessageRef::new("ORD-17116"),
            },
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn run(state: SperrungMsbState, cmd: SperrungMsbCommand) -> SperrungMsbState {
        let out = GpkeSperrungMsbWorkflow::handle(&state, cmd).expect("accepted");
        out.events
            .iter()
            .fold(state, GpkeSperrungMsbWorkflow::apply)
    }

    /// Three Werktage after the ÜT: received Freitag 06.11.2026, due by the end
    /// of Mittwoch 11.11.2026.
    #[test]
    fn the_answer_is_due_by_the_end_of_the_third_werktag() {
        let eingang = time::macros::datetime!(2026-11-06 09:00 UTC);
        assert_eq!(
            antwortfrist(eingang),
            Some(mako_fristen::end_of_day_berlin(time::macros::date!(
                2026 - 11 - 11
            )))
        );
    }

    /// The lapse is the Zustimmung the Festlegung states, not an open breach.
    #[test]
    fn a_lapsed_window_is_the_fingierte_zustimmung() {
        let offen = run(SperrungMsbState::New, anfrage());
        assert!(matches!(offen, SperrungMsbState::Offen(_)));
        let deadline_id = DeadlineId::new();
        let ende = run(
            offen,
            SperrungMsbCommand::TimeoutExpired {
                deadline_id,
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        );
        assert!(
            matches!(ende, SperrungMsbState::ZustimmungFingiert(_)),
            "{ende:?}"
        );
        assert!(ende.is_terminal());
        // A second firing finds the window closed.
        let out = GpkeSperrungMsbWorkflow::handle(
            &ende,
            SperrungMsbCommand::TimeoutExpired {
                deadline_id,
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        assert!(out.events.is_empty());
    }

    #[test]
    fn only_the_anfrage_sperrung_opens_the_process() {
        let SperrungMsbCommand::ReceiveAnfrage { anfrage, .. } = anfrage() else {
            unreachable!()
        };
        let err = GpkeSperrungMsbWorkflow::handle(
            &SperrungMsbState::New,
            SperrungMsbCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::new(17_115).expect("five digits"),
                anfrage,
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(err.is_err());
    }

    fn antwort(code: &str, freitext: Option<&str>) -> SperrungMsbCommand {
        SperrungMsbCommand::SendAntwort {
            antwortcode: code.to_owned(),
            freitext: freitext.map(str::to_owned),
            message_ref: MessageRef::new("ORDRSP-1"),
        }
    }

    /// A Zustimmung of `E_0488` rides 19118 and ends the process before the
    /// window; the ORDRSP names the 17116 by its `BGM` DE 1004.
    #[test]
    fn a_zustimmung_is_answered_with_19118_and_ends_the_process() {
        let offen = run(SperrungMsbState::New, anfrage());
        let out = GpkeSperrungMsbWorkflow::handle(&offen, antwort("A05", None)).expect("accepted");
        assert_eq!(out.outbox.len(), 1);
        let p = &out.outbox[0].payload;
        assert_eq!(&*out.outbox[0].message_type, "ORDRSP");
        assert_eq!(p["pid"], 19_118);
        assert_eq!(p["document_code"], "Z51");
        assert_eq!(p["order_reference"], "SPERR-4711");
        assert_eq!(p["antwort_codeliste"], "E_0488");
        assert_eq!(p["sender"], "9900555000005");
        let ende = out
            .events
            .iter()
            .fold(offen, GpkeSperrungMsbWorkflow::apply);
        assert!(
            matches!(ende, SperrungMsbState::Beantwortet { .. }),
            "{ende:?}"
        );
        assert!(ende.is_terminal());
        // The window finds the process answered and records nothing.
        let fired = GpkeSperrungMsbWorkflow::handle(
            &ende,
            SperrungMsbCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        assert!(fired.events.is_empty());
    }

    /// An Ablehnung rides 19119, and `A02`/`A03` need the problem named.
    #[test]
    fn an_ablehnung_rides_19119_and_names_the_problem() {
        let offen = run(SperrungMsbState::New, anfrage());
        assert!(GpkeSperrungMsbWorkflow::handle(&offen, antwort("A02", None)).is_err());
        let out = GpkeSperrungMsbWorkflow::handle(
            &offen,
            antwort(
                "A02",
                Some("Verhinderungsgrund an der Marktlokation angezeigt"),
            ),
        )
        .expect("accepted");
        assert_eq!(out.outbox[0].payload["pid"], 19_119);
        assert_eq!(
            out.outbox[0].payload["antwort_freitext"],
            "Verhinderungsgrund an der Marktlokation angezeigt"
        );
    }

    /// A code `E_0488` does not publish is refused.
    #[test]
    fn a_code_outside_e_0488_is_refused() {
        let offen = run(SperrungMsbState::New, anfrage());
        assert!(GpkeSperrungMsbWorkflow::handle(&offen, antwort("A06", None)).is_err());
        assert!(
            GpkeSperrungMsbWorkflow::handle(&SperrungMsbState::New, antwort("A05", None)).is_err()
        );
    }
}
