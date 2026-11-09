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
//! The answer leg — the values on MSCONS, or the Ablehnung on ORDRSP 19102 /
//! 19007 / 19114 out of its Entscheidungsbaum — is not built here.

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
    /// The ORDERS' Dokumentennummer — `BGM` DE 1004.
    pub dokumentennummer: String,
    /// The Markt- or Messlokation asked about — `SG2 LOC+172`.
    pub lokation_id: String,
    /// Interchange reference of the inbound ORDERS.
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
}

impl EventPayload for DatenabrufMsbEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageEingegangen { .. } => "DatenabrufMsbAnfrageEingegangen",
            Self::AnfrageUngueltig { .. } => "DatenabrufMsbAnfrageUngueltig",
            Self::FristAbgelaufen { .. } => "DatenabrufMsbFristAbgelaufen",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::AnfrageEingegangen { pid, anfrage } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(anfrage.message_ref.as_str().to_owned()),
            }),
            Self::AnfrageUngueltig { .. } | Self::FristAbgelaufen { .. } => None,
        }
    }
}

/// State of the MSB-side Datenabruf.
///
/// ```text
/// New → Offen → FristAbgelaufen   (the window passed; still owed)
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
        }
    }

    /// Whether the process has ended.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ungueltig { .. })
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
    /// A registered deadline fired.
    TimeoutExpired {
        /// The deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for DatenabrufMsbCommand {}

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
}
