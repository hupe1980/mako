//! GPKE Datenabruf — MSB side: the Anfragen and Anforderungen of Werten a MSB
//! receives (ORDERS 17004 / 17102 / 17113, Strom).
//!
//! | PID   | Message | Direction (Strom) | Source |
//! |-------|---------|-------------------|--------|
//! | 17102 | Anfrage von Werten | LF → MSB | ORDERS AHB 1.1b Kap. 4.1.2.2 (S. 15) |
//! | 17004 | Anforderung von Werten | LF · NB · MSB → MSB | ORDERS AHB 1.1b Kap. 4.13 (S. 90) |
//! | 17113 | Reklamation von Werten/Lastgängen | LF · NB · ÜNB · MSB → MSB | ORDERS AHB 1.1b Kap. 4.8 (S. 70) |
//!
//! The requesting side is [`crate::datenabruf`]; this workflow is the party
//! asked. A deployment that is both (a LF and its own MSB) runs the two sides
//! under two workflow names, so its own Anfrage and the received copy never
//! meet on one Marktlokation.
//!
//! # Frist
//!
//! - **17102** is the Geschäftsdatenanfrage of GPKE Teil 4 Kap. 3.2 Nr. 3/4
//!   (Anlage 1d, S. 30): „Unverzüglich, jedoch spätester ÜZ ist 1 WT nach dem
//!   ÜZ von Nr. 3" — [`mako_fristen::antwort::GESCHAEFTSDATENANFRAGE_WERKTAGE`].
//! - **17004** and **17113** are WiM Teil 2 Use-Cases (BK6-24-174 Anlage 2b
//!   Kap. 2.6 resp. 2.7). Their only Frist binds the *Ablehnung* — the
//!   Bearbeitungsstand bei Beendigung des Prozesses, 2. WT (Kap. 2.6.3/2.6.4
//!   Nr. 2, S. 40/42) resp. 3. WT (Kap. 2.7.3/2.7.4 Nr. 2, S. 47/49) — while the
//!   values themselves follow the table „Darstellung der zu übermittelnden
//!   Werte". [`mako_fristen::antwort`] carries both rows
//!   ([`mako_fristen::antwort::WERTE_ANFORDERUNG_ABLEHNUNG_WERKTAGE`],
//!   [`mako_fristen::antwort::WERTE_REKLAMATION_ABLEHNUNG_WERKTAGE`]), and this
//!   workflow registers them under [`ABLEHNUNG_WINDOW_LABEL`]: when it lapses
//!   the MSB can no longer refuse in time, and the values are still owed.
//!
//! # The Ablehnung
//!
//! [`DatenabrufMsbCommand::SendAblehnung`] refuses the Anfrage on its ORDRSP,
//! the code out of a tree the PID admits ([`ablehnungs_baeume`]):
//!
//! | ORDERS | ORDRSP | Trees (EBD 4.3) | Wire Codeliste | Echo |
//! |---|---|---|---|---|
//! | 17102 | 19102 | `E_0442` (Kap. 6.25.2) | `E_0442` | the ORDERS' `BGM` DE 1001 and, under `BGM+7`, its `IMD` (ORDRSP AHB 1.1b Kap. 4.1.3, S. 9–10) |
//! | 17004 | 19007 | `E_0219`–`E_0222`, `E_0225` (Kap. 8.18–8.20) | `S_0073` / `S_0074` / `S_0075` | `BGM+7`, `IMD++Z13` (Kap. 4.13, S. 56–57) |
//! | 17113 | 19114 | `E_0226`–`E_0230` (Kap. 8.21–8.23) | `S_0076`–`S_0079` | `BGM+Z34`, `IMD++Z11`, `SG6 CTA`/`COM` (Kap. 4.8, S. 39–41) |
//!
//! Each echoes the ORDERS' `BGM` DE 1004 in `SG1 RFF+ON`. The values
//! themselves — the positive answer, on MSCONS — are not sent from here.

use mako_engine::envelope::{Richtung, Verlauf};
use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// Workflow name for the MSB side of the GPKE Datenabruf.
pub const WORKFLOW_NAME: &str = "gpke-datenabruf-msb";

/// ORDERS 17102 „Anfrage von Werten" (LF an MSB, Strom).
pub const ANFRAGE_WERTE_PID: u32 = 17_102;

/// The ORDERS a MSB receives in this workflow.
pub const ORDERS_PIDS: &[u32] = &[17_004, ANFRAGE_WERTE_PID, 17_113];

/// The MSB's answer window on an Anfrage von Werten (17102).
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-datenabruf-msb-antwort";

/// The MSB's window to **refuse** an Anforderung (17004) or a Reklamation
/// (17113) von Werten. Not an answer window: the values that answer either
/// follow the table „Darstellung der zu übermittelnden Werte".
pub const ABLEHNUNG_WINDOW_LABEL: &str = "gpke-datenabruf-msb-ablehnung";

/// The label of the one window a received ORDERS `pid` opens.
#[must_use]
pub const fn window_label(pid: u32) -> &'static str {
    if pid == ANFRAGE_WERTE_PID {
        ANTWORT_WINDOW_LABEL
    } else {
        ABLEHNUNG_WINDOW_LABEL
    }
}

/// The key a received Anfrage runs under: the Anfragender's MP-ID and the
/// ORDERS' Dokumentennummer (`BGM` DE 1004), which the answering ORDRSP echoes
/// in `SG1 RFF+ON` (ORDRSP AHB 1.1b, Hinweis \[529\]).
///
/// Not the Lokation: a MSB is asked for the values of one Messlokation many
/// times, and only 17102 carries a window that ends the process, so a
/// Lokation key would let one unanswered Anforderung refuse every later one.
/// The number is unique only per sender, hence the MP-ID.
#[must_use]
pub fn anfrage_key(anfragender: &str, dokumentennummer: &str) -> String {
    format!("datenabruf-msb:{anfragender}:{dokumentennummer}")
}

/// The window the ORDERS `pid` received at `eingang` opens, as its label and
/// due instant, from the row [`mako_fristen::antwort`] publishes for it:
/// 17102 one Werktag (GPKE Teil 4 Kap. 3.2 Nr. 4), 17004 two and 17113 three
/// for the Ablehnung (WiM Strom Teil 2 Kap. 2.6.3 / 2.7.3 Nr. 2).
///
/// `None` for a PID this workflow does not receive.
#[must_use]
pub fn antwortfrist(
    pid: u32,
    eingang: time::OffsetDateTime,
) -> Option<(&'static str, time::OffsetDateTime)> {
    if !ORDERS_PIDS.contains(&pid) {
        return None;
    }
    mako_fristen::antwort::antwort_deadline(pid, eingang).map(|due| (window_label(pid), due))
}

/// The Anfrage as the MSB received it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WerteAnfrage {
    /// The party asking — `NAD+MS`.
    pub anfragender: MarktpartnerCode,
    /// The MSB asked — `NAD+MR`; the answer's sender.
    pub empfaenger: MarktpartnerCode,
    /// The ORDERS' `BGM` DE 1001 (`7`, `Z28`, `Z48`, `Z34`).
    pub bgm: String,
    /// The ORDERS' `IMD` DE 7081, when it carries one.
    pub leistung: Option<String>,
    /// The ORDERS' Dokumentennummer — `BGM` DE 1004.
    pub dokumentennummer: String,
    /// The Markt- or Messlokation asked about — `SG2 LOC+172`.
    pub lokation_id: String,
    /// Interchange reference of the inbound ORDERS.
    pub message_ref: MessageRef,
}

/// ORDRSP 19102 „Ablehnung Anfrage Werte".
pub const ABLEHNUNG_ANFRAGE_PID: u32 = 19_102;
/// ORDRSP 19007 „Ablehnung Anforderung von Werten".
pub const ABLEHNUNG_ANFORDERUNG_PID: u32 = 19_007;
/// ORDRSP 19114 „Ablehnung der Reklamation von Werten".
pub const ABLEHNUNG_REKLAMATION_PID: u32 = 19_114;

/// The ORDRSP that refuses the received ORDERS `pid` and the trees its `SG2
/// AJT` may name (ORDRSP AHB 1.1b Kap. 4.1.3, 4.13, 4.8).
#[must_use]
pub fn ablehnungs_baeume(pid: u32) -> Option<(u32, &'static [&'static str])> {
    use mako_pruefung::codes as c;
    match pid {
        ANFRAGE_WERTE_PID => Some((ABLEHNUNG_ANFRAGE_PID, &[c::EBD_GDA_WERTE_MSB])),
        17_004 => Some((
            ABLEHNUNG_ANFORDERUNG_PID,
            &[
                c::EBD_ANFORDERUNG_WERT_NB,
                c::EBD_ANFORDERUNG_WERT_NB_MSB,
                c::EBD_ANFORDERUNG_WERT_LF,
                c::EBD_ANFORDERUNG_WERT_LF_MSB,
                c::EBD_ANFORDERUNG_WERT_MSB,
            ],
        )),
        17_113 => Some((
            ABLEHNUNG_REKLAMATION_PID,
            &[
                c::EBD_REKLAMATION_WERTE_NB,
                c::EBD_REKLAMATION_WERTE_NB_MSB,
                c::EBD_REKLAMATION_WERTE_LF,
                c::EBD_REKLAMATION_WERTE_LF_MSB,
                c::EBD_REKLAMATION_WERTE_UENB,
            ],
        )),
        _ => None,
    }
}

/// The Ablehnung as sent.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WerteAblehnung {
    /// 19102, 19007 or 19114.
    pub pid: Pruefidentifikator,
    /// The code.
    pub antwortcode: String,
    /// The tree it was read from.
    pub codeliste: String,
    /// Reference of the outbound ORDRSP.
    pub message_ref: MessageRef,
}

/// Events of the MSB-side Datenabruf.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum DatenabrufMsbEvent {
    /// ORDERS 17004 / 17102 / 17113 received.
    AnfrageEingegangen {
        /// ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: WerteAnfrage,
    },
    /// The ORDERS failed its AHB check.
    AnfrageUngueltig {
        /// The findings.
        reason: String,
    },
    /// The answer window passed while the Anfrage was open.
    FristAbgelaufen {
        /// The fired deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
    /// The MSB refused the Anfrage on its ORDRSP.
    AblehnungGesendet {
        /// The Ablehnung.
        ablehnung: WerteAblehnung,
    },
}

impl EventPayload for DatenabrufMsbEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageEingegangen { .. } => "DatenabrufMsbAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "DatenabrufMsbAnfrageUngueltig",
            Self::FristAbgelaufen { .. } => "DatenabrufMsbFristAbgelaufen",
            Self::AblehnungGesendet { .. } => "DatenabrufMsbAblehnungGesendet",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageEingegangen { pid, anfrage } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(anfrage.message_ref.as_str().to_owned()),
            }),
            Self::AblehnungGesendet { ablehnung } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(ablehnung.pid.as_u32()),
                message_ref: Some(ablehnung.message_ref.as_str().to_owned()),
            }),
            Self::AnfrageUngueltig { .. } | Self::FristAbgelaufen { .. } => None,
        }
    }

    fn antwort(&self) -> Option<mako_engine::envelope::Antwortcode> {
        match self {
            Self::AblehnungGesendet { ablehnung } => Some(mako_engine::envelope::Antwortcode {
                code: ablehnung.antwortcode.clone(),
                codeliste: Some(ablehnung.codeliste.clone()),
            }),
            _ => None,
        }
    }
}

/// State of the MSB-side Datenabruf.
///
/// ```text
/// New → Offen → Abgelehnt         (ORDRSP 19102 / 19007 / 19114 sent)
///            ↘ FristAbgelaufen   (the window passed; still owed)
///     ↘ Ungueltig                 (AHB check failed)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum DatenabrufMsbState {
    /// Nothing received yet.
    #[default]
    New,
    /// The Anfrage is open; the MSB owes the values or the Ablehnung.
    Offen {
        /// ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: WerteAnfrage,
    },
    /// The Anfrage failed its AHB check.
    Ungueltig {
        /// The findings.
        reason: String,
    },
    /// The MSB refused the Anfrage.
    Abgelehnt {
        /// The Anfrage.
        anfrage: WerteAnfrage,
        /// The Ablehnung.
        ablehnung: WerteAblehnung,
    },
    /// The window lapsed while the Anfrage was open. No Festlegung gives the
    /// lapse an outcome: it is a Frist breach for the operator to act on, and
    /// the answer is still owed.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// ORDERS Prüfidentifikator.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: WerteAnfrage,
    },
}

impl DatenabrufMsbState {
    /// Stable label of the variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Offen { .. } => "Offen",
            Self::Ungueltig { .. } => "Ungueltig",
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ungueltig { .. } | Self::Abgelehnt { .. })
    }
}

/// Commands of the MSB-side Datenabruf.
#[derive(Debug, Clone)]
pub enum DatenabrufMsbCommand {
    /// Inbound ORDERS 17004 / 17102 / 17113.
    ReceiveAnfrage {
        /// Prüfidentifikator read from `SG1 RFF+Z13`.
        pid: Pruefidentifikator,
        /// The Anfrage.
        anfrage: WerteAnfrage,
        /// `true` when the AHB check passed.
        validation_passed: bool,
        /// The findings when it did not.
        validation_errors: Vec<String>,
    },
    /// The MSB refuses the Anfrage on its ORDRSP.
    SendAblehnung {
        /// The tree the code is read from — one [`ablehnungs_baeume`] admits.
        codeliste: String,
        /// The code.
        antwortcode: String,
        /// `SG6 CTA`/`COM` — Muss on the 19114 (ORDRSP AHB 1.1b Kap. 4.8, S. 40).
        ansprechpartner: Option<serde_json::Value>,
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

impl CommandPayload for DatenabrufMsbCommand {}

/// The ORDRSP refusing the Anfrage `pid` (see the module table).
fn ablehnung(
    pid: Pruefidentifikator,
    anfrage: &WerteAnfrage,
    codeliste: &str,
    antwortcode: String,
    ansprechpartner: Option<serde_json::Value>,
    message_ref: MessageRef,
) -> Result<WorkflowOutput<DatenabrufMsbEvent>, WorkflowError> {
    let (ordrsp, baeume) = ablehnungs_baeume(pid.as_u32())
        .ok_or_else(|| WorkflowError::validation(format!("no Ablehnung answers ORDERS {pid}")))?;
    if !baeume.contains(&codeliste) {
        return Err(WorkflowError::validation(format!(
            "{codeliste:?} does not answer ORDERS {pid} (admitted: {baeume:?})"
        )));
    }
    let code = mako_pruefung::codes::lookup(codeliste, &antwortcode).ok_or_else(|| {
        WorkflowError::validation(format!("{antwortcode:?} is not published in {codeliste}"))
    })?;
    let wire = code.wire_codeliste().unwrap_or(codeliste).to_owned();
    let (bgm, leistung) = match ordrsp {
        ABLEHNUNG_ANFRAGE_PID => (
            anfrage.bgm.clone(),
            (anfrage.bgm == "7")
                .then(|| anfrage.leistung.clone())
                .flatten(),
        ),
        ABLEHNUNG_ANFORDERUNG_PID => ("7".to_owned(), Some("Z13".to_owned())),
        _ => ("Z34".to_owned(), Some("Z11".to_owned())),
    };
    if ordrsp == ABLEHNUNG_ANFRAGE_PID && bgm == "7" && leistung.is_none() {
        return Err(WorkflowError::validation(
            "a 19102 under BGM+7 echoes the Anfrage's IMD (ORDRSP AHB 1.1b Kap. 4.1.3, [1]); \
             the received ORDERS carried none",
        ));
    }
    if ordrsp == ABLEHNUNG_REKLAMATION_PID && ansprechpartner.is_none() {
        return Err(WorkflowError::validation(
            "SG6 CTA/COM is Muss on 19114 (ORDRSP AHB 1.1b Kap. 4.8): name the Ansprechpartner",
        ));
    }
    let mut payload = serde_json::json!({
        "pid": ordrsp,
        "sender": anfrage.empfaenger.as_str(),
        "receiver": anfrage.anfragender.as_str(),
        "message_ref": message_ref.as_str(),
        "document_code": bgm,
        "document_id": message_ref.as_str(),
        "order_reference": anfrage.dokumentennummer,
        "antwort_code": antwortcode,
        "antwort_codeliste": wire,
    });
    if let Some(l) = leistung {
        payload["leistung"] = serde_json::json!(l);
    }
    if let Some(a) = ansprechpartner {
        payload["ansprechpartner"] = a;
    }
    let pid = Pruefidentifikator::new(ordrsp)
        .map_err(|e| WorkflowError::validation(format!("answer PID {ordrsp}: {e}")))?;
    let outbox = vec![
        mako_engine::outbox::PendingOutbox::new("ORDRSP", anfrage.anfragender.as_str(), payload)
            .caused_by(0),
    ];
    Ok(WorkflowOutput::with_outbox(
        vec![DatenabrufMsbEvent::AblehnungGesendet {
            ablehnung: WerteAblehnung {
                pid,
                antwortcode,
                codeliste: wire,
                message_ref,
            },
        }],
        outbox,
    ))
}

/// MSB side of the GPKE Datenabruf (ORDERS 17004 / 17102 / 17113).
pub struct GpkeDatenabrufMsbWorkflow;

/// The one guard a fired deadline passes: the window is open while the
/// Anfrage is, and only the window its PID opens.
fn window_open(label: &str, state: &DatenabrufMsbState) -> bool {
    let DatenabrufMsbState::Offen { pid, .. } = state else {
        return false;
    };
    let anfrage_von_werten = pid.as_u32() == ANFRAGE_WERTE_PID;
    (label == ANTWORT_WINDOW_LABEL && anfrage_von_werten)
        || (label == ABLEHNUNG_WINDOW_LABEL && !anfrage_von_werten)
}

impl Workflow for GpkeDatenabrufMsbWorkflow {
    type State = DatenabrufMsbState;
    type Event = DatenabrufMsbEvent;
    type Command = DatenabrufMsbCommand;

    const DEADLINE_LABELS: &'static [&'static str] =
        &[ANTWORT_WINDOW_LABEL, ABLEHNUNG_WINDOW_LABEL];

    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| DatenabrufMsbCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            DatenabrufMsbEvent::AnfrageEingegangen { pid, anfrage } => DatenabrufMsbState::Offen {
                pid: *pid,
                anfrage: anfrage.clone(),
            },
            DatenabrufMsbEvent::AnfrageUngueltig { reason } => DatenabrufMsbState::Ungueltig {
                reason: reason.clone(),
            },
            DatenabrufMsbEvent::AblehnungGesendet { ablehnung } => match state {
                DatenabrufMsbState::Offen { anfrage, .. }
                | DatenabrufMsbState::FristAbgelaufen { anfrage, .. } => {
                    DatenabrufMsbState::Abgelehnt {
                        anfrage,
                        ablehnung: ablehnung.clone(),
                    }
                }
                other => other,
            },
            DatenabrufMsbEvent::FristAbgelaufen { label, .. } => match state {
                DatenabrufMsbState::Offen { pid, anfrage } => DatenabrufMsbState::FristAbgelaufen {
                    label: label.to_string(),
                    pid,
                    anfrage,
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
            DatenabrufMsbCommand::ReceiveAnfrage {
                pid,
                anfrage,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, DatenabrufMsbState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ORDERS_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "not an ORDERS a MSB receives in the Datenabruf: {pid}"
                    )));
                }
                let mut events = vec![DatenabrufMsbEvent::AnfrageEingegangen { pid, anfrage }];
                if !validation_passed {
                    events.push(DatenabrufMsbEvent::AnfrageUngueltig {
                        reason: validation_errors.join("; "),
                    });
                }
                Ok(events.into())
            }
            DatenabrufMsbCommand::SendAblehnung {
                codeliste,
                antwortcode,
                ansprechpartner,
                message_ref,
            } => {
                let (DatenabrufMsbState::Offen { pid, anfrage }
                | DatenabrufMsbState::FristAbgelaufen { pid, anfrage, .. }) = state
                else {
                    return Err(WorkflowError::invalid_state("Offen", state.label()));
                };
                ablehnung(
                    *pid,
                    anfrage,
                    &codeliste,
                    antwortcode,
                    ansprechpartner,
                    message_ref,
                )
            }
            DatenabrufMsbCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, state) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![DatenabrufMsbEvent::FristAbgelaufen { deadline_id, label }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every ORDERS this workflow receives opens its window: the Anfrage von
    /// Werten the answer by the end of the next Werktag (GPKE Teil 4 Kap. 3.2
    /// Nr. 4), the Anforderung and the Reklamation the Ablehnung by the end of
    /// the 2. resp. 3. Werktag (WiM Strom Teil 2 Kap. 2.6.3 / 2.7.3 Nr. 2).
    #[test]
    fn every_received_orders_has_its_window() {
        // Freitag 06.11.2026 → Montag 09., Dienstag 10., Mittwoch 11.11.2026.
        let eingang = time::macros::datetime!(2026-11-06 09:00 UTC);
        let end = |d| Some(mako_fristen::end_of_day_berlin(d));
        assert_eq!(
            antwortfrist(ANFRAGE_WERTE_PID, eingang).map(|(_, d)| d),
            end(time::macros::date!(2026 - 11 - 09))
        );
        assert_eq!(
            antwortfrist(17_004, eingang),
            Some((
                ABLEHNUNG_WINDOW_LABEL,
                end(time::macros::date!(2026 - 11 - 10)).unwrap()
            ))
        );
        assert_eq!(
            antwortfrist(17_113, eingang),
            Some((
                ABLEHNUNG_WINDOW_LABEL,
                end(time::macros::date!(2026 - 11 - 11)).unwrap()
            ))
        );
        assert_eq!(antwortfrist(19_102, eingang), None);
    }

    /// A fired deadline is admitted only under the label its PID opens.
    #[test]
    fn a_window_fires_only_under_its_own_label() {
        let offen = |pid: u32| DatenabrufMsbState::Offen {
            pid: Pruefidentifikator::new(pid).expect("five digits"),
            anfrage: WerteAnfrage {
                anfragender: MarktpartnerCode::new("9900357000004"),
                empfaenger: MarktpartnerCode::new("9900555000005"),
                bgm: "7".to_owned(),
                leistung: Some("Z13".to_owned()),
                dokumentennummer: "DOK".to_owned(),
                lokation_id: "51238696012".to_owned(),
                message_ref: MessageRef::new("ORD"),
            },
        };
        assert!(window_open(ABLEHNUNG_WINDOW_LABEL, &offen(17_004)));
        assert!(window_open(ABLEHNUNG_WINDOW_LABEL, &offen(17_113)));
        assert!(!window_open(ANTWORT_WINDOW_LABEL, &offen(17_004)));
        assert!(window_open(ANTWORT_WINDOW_LABEL, &offen(ANFRAGE_WERTE_PID)));
        assert!(!window_open(
            ABLEHNUNG_WINDOW_LABEL,
            &offen(ANFRAGE_WERTE_PID)
        ));
        assert!(!window_open(
            ABLEHNUNG_WINDOW_LABEL,
            &DatenabrufMsbState::New
        ));
    }

    #[test]
    fn a_lapse_leaves_the_anfrage_owed() {
        let pid = Pruefidentifikator::new(ANFRAGE_WERTE_PID).expect("five digits");
        let anfrage = WerteAnfrage {
            anfragender: MarktpartnerCode::new("9900357000004"),
            empfaenger: MarktpartnerCode::new("9900555000005"),
            bgm: "7".to_owned(),
            leistung: Some("Z12".to_owned()),
            dokumentennummer: "DOK17102".to_owned(),
            lokation_id: "51238696012".to_owned(),
            message_ref: MessageRef::new("ORD-17102"),
        };
        let out = GpkeDatenabrufMsbWorkflow::handle(
            &DatenabrufMsbState::New,
            DatenabrufMsbCommand::ReceiveAnfrage {
                pid,
                anfrage,
                validation_passed: true,
                validation_errors: vec![],
            },
        )
        .expect("accepted");
        assert_eq!(
            out.events[0].verlauf().and_then(|v| v.pid),
            Some(ANFRAGE_WERTE_PID)
        );
        let offen = out
            .events
            .iter()
            .fold(DatenabrufMsbState::New, GpkeDatenabrufMsbWorkflow::apply);
        let out = GpkeDatenabrufMsbWorkflow::handle(
            &offen,
            DatenabrufMsbCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("accepted");
        let lapsed = out
            .events
            .iter()
            .fold(offen, GpkeDatenabrufMsbWorkflow::apply);
        assert!(matches!(lapsed, DatenabrufMsbState::FristAbgelaufen { .. }));
        assert!(!lapsed.is_terminal(), "the answer is still owed");
    }

    #[test]
    fn a_datenabruf_ordrsp_does_not_open_the_process() {
        let out = GpkeDatenabrufMsbWorkflow::handle(
            &DatenabrufMsbState::New,
            DatenabrufMsbCommand::ReceiveAnfrage {
                pid: Pruefidentifikator::new(19_102).expect("five digits"),
                anfrage: WerteAnfrage {
                    anfragender: MarktpartnerCode::new("9900357000004"),
                    empfaenger: MarktpartnerCode::new("9900555000005"),
                    bgm: "7".to_owned(),
                    leistung: None,
                    dokumentennummer: "DOK19102".to_owned(),
                    lokation_id: "51238696012".to_owned(),
                    message_ref: MessageRef::new("ORDRSP-1"),
                },
                validation_passed: true,
                validation_errors: vec![],
            },
        );
        assert!(out.is_err());
    }

    fn offen(pid: u32, bgm: &str, leistung: Option<&str>) -> DatenabrufMsbState {
        DatenabrufMsbState::Offen {
            pid: Pruefidentifikator::new(pid).expect("five digits"),
            anfrage: WerteAnfrage {
                anfragender: MarktpartnerCode::new("9900357000004"),
                empfaenger: MarktpartnerCode::new("9900555000005"),
                bgm: bgm.to_owned(),
                leistung: leistung.map(str::to_owned),
                dokumentennummer: "DOK-1".to_owned(),
                lokation_id: "51238696012".to_owned(),
                message_ref: MessageRef::new("ORD"),
            },
        }
    }

    fn ablehnen(tree: &str, code: &str, ansprechpartner: bool) -> DatenabrufMsbCommand {
        DatenabrufMsbCommand::SendAblehnung {
            codeliste: tree.to_owned(),
            antwortcode: code.to_owned(),
            ansprechpartner: ansprechpartner.then(|| {
                serde_json::json!({
                    "name": "Messwesen",
                    "kommunikation": [{ "art": "email", "adresse": "messwesen@example.de" }],
                })
            }),
            message_ref: MessageRef::new("ORDRSP-1"),
        }
    }

    /// Each Anfrage is refused on its own ORDRSP out of a tree its PID admits,
    /// the Codeliste going out under its wire number, and the process ends.
    #[test]
    fn an_answered_anfrage_reaches_its_end_state() {
        let state = offen(17_004, "7", Some("Z13"));
        let out = GpkeDatenabrufMsbWorkflow::handle(&state, ablehnen("E_0221", "Z19", false))
            .expect("refused");
        let p = &out.outbox[0].payload;
        assert_eq!(p["pid"], 19_007);
        assert_eq!(p["antwort_codeliste"], "S_0075");
        assert_eq!(p["leistung"], "Z13");
        assert_eq!(p["order_reference"], "DOK-1");
        let ende = out
            .events
            .iter()
            .fold(state, GpkeDatenabrufMsbWorkflow::apply);
        assert!(ende.is_terminal(), "{ende:?}");

        let out = GpkeDatenabrufMsbWorkflow::handle(
            &offen(ANFRAGE_WERTE_PID, "7", Some("Z12")),
            ablehnen("E_0442", "A04", false),
        )
        .expect("refused");
        assert_eq!(out.outbox[0].payload["pid"], 19_102);
        assert_eq!(out.outbox[0].payload["leistung"], "Z12");

        let reklamation = offen(17_113, "Z34", Some("Z11"));
        assert!(
            GpkeDatenabrufMsbWorkflow::handle(&reklamation, ablehnen("E_0226", "Z54", false))
                .is_err(),
            "the 19114 names its Ansprechpartner"
        );
        let out = GpkeDatenabrufMsbWorkflow::handle(&reklamation, ablehnen("E_0226", "Z54", true))
            .expect("refused");
        assert_eq!(out.outbox[0].payload["pid"], 19_114);
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "S_0076");
    }

    /// A tree the PID does not admit is refused.
    #[test]
    fn a_foreign_tree_is_refused() {
        assert!(
            GpkeDatenabrufMsbWorkflow::handle(
                &offen(17_004, "7", Some("Z13")),
                ablehnen("E_0442", "A04", false)
            )
            .is_err()
        );
    }
}
