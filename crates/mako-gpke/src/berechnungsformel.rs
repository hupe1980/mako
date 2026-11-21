//! The NB's Berechnungsformel — UTILTS 25001 to an MSB of the Lokationsbündel,
//! answered by the MSB's 25010 (WiM Strom Teil 2 Kap. 2.3).
//!
//! The NB sends the formula „unverzüglich, jedoch spätester ÜT ist der 3. WT"
//! after the MSB's Zuordnung or the change becoming known; the MSB answers one
//! `E_0218` code per Verwendungszeitraum by the 5. WT, and „Verstreicht die
//! Frist, ohne dass eine Antwort eingeht, gilt dies als Zustimmung" (Kap. 2.3.2
//! Nr. 1–2). An answer is a Zustimmung only when every Zeitraum's code is in
//! `E_0218`'s Zustimmung cluster; a code the tree does not publish is held for
//! a person, never read as either.

use mako_engine::deadline::Deadline;
use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::ids::DeadlineId;
use mako_engine::outbox::PendingOutbox;
use mako_engine::payload::UtiltsPayload;
use mako_engine::types::{MarktpartnerCode, MessageRef};
use mako_engine::{
    error::WorkflowError,
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};
use mako_pruefung::codes::{Cluster, EBD_BERECHNUNGSFORMEL, lookup};

/// Stable workflow name.
pub const WORKFLOW_NAME: &str = "gpke-berechnungsformel";

/// The Berechnungsformel the NB sends.
pub const FORMEL_PID: u32 = 25_001;

/// The MSB's Antwort auf Berechnungsformel.
pub const ANTWORT_PID: u32 = 25_010;

/// The 5-WT window for the MSB's answer.
pub const ANTWORT_WINDOW_LABEL: &str = "gpke-berechnungsformel-antwort";

/// One `STS+E01` of the MSB's 25010: an `E_0218` code for a Zeitraum. Events
/// carry it, so it reads a field it does not know rather than refuse it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ZeitraumAntwort {
    /// DE 9013 — an `E_0218` code.
    pub code: String,
    /// DE 9012 — the Zeitraum-ID it answers.
    pub zeitraum_id: u32,
}

/// What the NB sent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FormelData {
    /// `IDE+24` — the Vorgangsnummer the 25010 names in `RFF+TN`.
    pub vorgangsnummer: String,
    /// The MSB it went to.
    pub empfaenger: MarktpartnerCode,
    /// The Markt- or Netzlokation the formula computes.
    pub lokation: String,
}

/// State of one Berechnungsformel.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum FormelState {
    /// Nothing sent.
    #[default]
    New,
    /// Sent; the MSB's answer is owed.
    Gesendet(FormelData),
    /// Every Zeitraum agreed — by its code, or by the lapse of the window.
    Zugestimmt {
        /// What was sent.
        data: FormelData,
        /// `true` when no answer came within the window.
        stillschweigend: bool,
    },
    /// At least one Zeitraum refused, each with its `E_0218` code.
    Abgelehnt {
        /// What was sent.
        data: FormelData,
        /// The answer, per Zeitraum.
        antworten: Vec<ZeitraumAntwort>,
    },
    /// A code `E_0218` does not publish — held for a person.
    Klaerung {
        /// What was sent.
        data: FormelData,
        /// The answer, per Zeitraum.
        antworten: Vec<ZeitraumAntwort>,
    },
}

impl FormelState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet(_) => "Gesendet",
            Self::Zugestimmt { .. } => "Zugestimmt",
            Self::Abgelehnt { .. } => "Abgelehnt",
            Self::Klaerung { .. } => "Klaerung",
        }
    }
}

/// Live while the answer is owed.
impl mako_engine::workflow::OccupiesBusinessKey for FormelState {
    fn occupies_business_key(&self) -> bool {
        matches!(self, Self::Gesendet(_))
    }
}

/// Commands.
#[derive(Debug, Clone)]
pub enum FormelCommand {
    /// The NB sends its Berechnungsformel to one MSB.
    SendFormel {
        /// The 25001, as edmd computes the Lokation's values from it.
        formel: Box<UtiltsPayload>,
        /// The MSB.
        empfaenger: MarktpartnerCode,
    },
    /// The MSB's 25010 arrived.
    ReceiveAntwort {
        /// One code per Zeitraum.
        antworten: Vec<ZeitraumAntwort>,
        /// `UNH` DE 0062 of the 25010.
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

impl CommandPayload for FormelCommand {}

/// Events.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum FormelEvent {
    /// The 25001 went out.
    FormelGesendet {
        /// What was sent.
        data: FormelData,
    },
    /// The 25010 arrived, with the verdict its codes give.
    AntwortErhalten {
        /// One code per Zeitraum.
        antworten: Vec<ZeitraumAntwort>,
        /// `UNH` DE 0062.
        message_ref: MessageRef,
        /// The verdict.
        urteil: Urteil,
    },
    /// The window lapsed without an answer: Zustimmung.
    StillschweigendZugestimmt {
        /// The deadline.
        deadline_id: DeadlineId,
    },
}

/// What an answer's codes say, read through `E_0218`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Urteil {
    /// Every code is in the Zustimmung cluster.
    Zugestimmt,
    /// At least one code is in the Ablehnung cluster.
    Abgelehnt,
    /// At least one code is not published in `E_0218`.
    Unbekannt,
}

/// Judge a 25010 through `E_0218` (EBD 4.3 Kap. 8.17.1): an unpublished code
/// outweighs every other, an Ablehnung outweighs a Zustimmung.
#[must_use]
pub fn urteil(antworten: &[ZeitraumAntwort]) -> Urteil {
    let mut ablehnung = false;
    for a in antworten {
        match lookup(EBD_BERECHNUNGSFORMEL, &a.code).map(|c| c.cluster) {
            None => return Urteil::Unbekannt,
            Some(Cluster::Ablehnung) => ablehnung = true,
            Some(_) => {}
        }
    }
    if antworten.is_empty() {
        Urteil::Unbekannt
    } else if ablehnung {
        Urteil::Abgelehnt
    } else {
        Urteil::Zugestimmt
    }
}

impl EventPayload for FormelEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::FormelGesendet { .. } => "BerechnungsformelGesendet",
            Self::AntwortErhalten { .. } => "BerechnungsformelAntwortErhalten",
            Self::StillschweigendZugestimmt { .. } => "BerechnungsformelStillschweigendZugestimmt",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::FormelGesendet { data } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(FORMEL_PID),
                message_ref: Some(data.vorgangsnummer.clone()),
            }),
            Self::AntwortErhalten { message_ref, .. } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(ANTWORT_PID),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::StillschweigendZugestimmt { .. } => None,
        }
    }

    /// The code that decides: the first refusal, else the first code.
    fn antwort(&self) -> Option<Antwortcode> {
        let Self::AntwortErhalten { antworten, .. } = self else {
            return None;
        };
        let entscheidend = antworten
            .iter()
            .find(|a| {
                lookup(EBD_BERECHNUNGSFORMEL, &a.code)
                    .is_none_or(|c| c.cluster == Cluster::Ablehnung)
            })
            .or_else(|| antworten.first())?;
        Some(Antwortcode::new(
            entscheidend.code.clone(),
            Some(EBD_BERECHNUNGSFORMEL),
        ))
    }
}

/// Whether `label`'s window is still open: the answer window while the
/// answer is owed.
fn window_open(label: &str, state: &FormelState) -> bool {
    label == ANTWORT_WINDOW_LABEL && matches!(state, FormelState::Gesendet(_))
}

/// The NB's Berechnungsformel workflow.
pub struct GpkeBerechnungsformelWorkflow;

impl Workflow for GpkeBerechnungsformelWorkflow {
    type State = FormelState;
    type Event = FormelEvent;
    type Command = FormelCommand;

    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];
    const FRIST_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL];

    /// The window lapsed while the answer was owed: „gilt dies als Zustimmung".
    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| FormelCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match (state, event) {
            (_, FormelEvent::FormelGesendet { data }) => FormelState::Gesendet(data.clone()),
            (
                FormelState::Gesendet(data),
                FormelEvent::AntwortErhalten {
                    antworten, urteil, ..
                },
            ) => match urteil {
                Urteil::Zugestimmt => FormelState::Zugestimmt {
                    data,
                    stillschweigend: false,
                },
                Urteil::Abgelehnt => FormelState::Abgelehnt {
                    data,
                    antworten: antworten.clone(),
                },
                Urteil::Unbekannt => FormelState::Klaerung {
                    data,
                    antworten: antworten.clone(),
                },
            },
            (FormelState::Gesendet(data), FormelEvent::StillschweigendZugestimmt { .. }) => {
                FormelState::Zugestimmt {
                    data,
                    stillschweigend: true,
                }
            }
            (other, _) => other,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            FormelCommand::SendFormel { formel, empfaenger } => {
                if !matches!(state, FormelState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if formel.pid != FORMEL_PID {
                    return Err(WorkflowError::rejected(format!(
                        "a Berechnungsformel is UTILTS {FORMEL_PID}, not {}",
                        formel.pid
                    )));
                }
                let Some(lokation) = formel.lokation.clone() else {
                    return Err(WorkflowError::rejected(
                        "a Berechnungsformel names the Lokation it computes (LOC+172)",
                    ));
                };
                if formel.zeitraeume.is_empty() {
                    return Err(WorkflowError::rejected(
                        "a Berechnungsformel names at least one Verwendungszeitraum",
                    ));
                }
                let data = FormelData {
                    vorgangsnummer: formel.vorgangsnummer.clone(),
                    empfaenger: empfaenger.clone(),
                    lokation,
                };
                let outbox = PendingOutbox::typed(empfaenger.as_str(), &*formel);
                Ok(WorkflowOutput::with_outbox(
                    vec![FormelEvent::FormelGesendet { data }],
                    vec![outbox],
                ))
            }
            FormelCommand::ReceiveAntwort {
                antworten,
                message_ref,
            } => {
                if !matches!(state, FormelState::Gesendet(_)) {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                }
                let urteil = urteil(&antworten);
                Ok(vec![FormelEvent::AntwortErhalten {
                    antworten,
                    message_ref,
                    urteil,
                }]
                .into())
            }
            FormelCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, state) {
                    return Ok(Vec::new().into());
                }
                Ok(vec![FormelEvent::StillschweigendZugestimmt { deadline_id }].into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mako_engine::payload::UtiltsZeitraum;

    fn formel() -> UtiltsPayload {
        UtiltsPayload {
            pid: FORMEL_PID,
            vorgangsnummer: "BF0001".into(),
            lokation: Some("51238696012".into()),
            zeitraeume: vec![UtiltsZeitraum {
                id: 1,
                ab: "202610142200".into(),
                bis: None,
                status: "Z33".into(),
                ergebnis_schritt: Some(1),
            }],
            ..UtiltsPayload::default()
        }
    }

    fn gesendet() -> FormelState {
        let out = GpkeBerechnungsformelWorkflow::handle(
            &FormelState::New,
            FormelCommand::SendFormel {
                formel: Box::new(formel()),
                empfaenger: MarktpartnerCode::new("9900259000002"),
            },
        )
        .expect("sends");
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILTS");
        out.events
            .iter()
            .fold(FormelState::New, GpkeBerechnungsformelWorkflow::apply)
    }

    fn antwort(codes: &[(&str, u32)]) -> FormelState {
        let antworten = codes
            .iter()
            .map(|(code, zeitraum_id)| ZeitraumAntwort {
                code: (*code).to_owned(),
                zeitraum_id: *zeitraum_id,
            })
            .collect();
        let out = GpkeBerechnungsformelWorkflow::handle(
            &gesendet(),
            FormelCommand::ReceiveAntwort {
                antworten,
                message_ref: MessageRef::new("ANT1"),
            },
        )
        .expect("receives");
        out.events
            .iter()
            .fold(gesendet(), GpkeBerechnungsformelWorkflow::apply)
    }

    /// `E_0218` decides: `A12` agrees, `A06` refuses, and one refusal among
    /// several Zeiträume refuses the formula.
    #[test]
    fn the_answer_is_read_through_e_0218() {
        assert_eq!(antwort(&[("A12", 1)]).label(), "Zugestimmt");
        assert_eq!(antwort(&[("A06", 1)]).label(), "Abgelehnt");
        assert_eq!(antwort(&[("A12", 1), ("A05", 2)]).label(), "Abgelehnt");
    }

    /// A code `E_0218` does not publish is held for a person — never read as
    /// a Zustimmung, never as an Ablehnung.
    #[test]
    fn an_unpublished_code_is_held_for_a_person() {
        assert_eq!(antwort(&[("Z99", 1)]).label(), "Klaerung");
        assert_eq!(antwort(&[("A12", 1), ("Z99", 2)]).label(), "Klaerung");
    }

    /// „Verstreicht die Frist, ohne dass eine Antwort eingeht, gilt dies als
    /// Zustimmung" (WiM Strom Teil 2 Kap. 2.3.2 Nr. 2).
    #[test]
    fn silence_is_zustimmung() {
        let out = GpkeBerechnungsformelWorkflow::handle(
            &gesendet(),
            FormelCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: ANTWORT_WINDOW_LABEL.into(),
            },
        )
        .expect("lapses");
        let state = out
            .events
            .iter()
            .fold(gesendet(), GpkeBerechnungsformelWorkflow::apply);
        assert_eq!(
            state,
            FormelState::Zugestimmt {
                data: FormelData {
                    vorgangsnummer: "BF0001".into(),
                    empfaenger: MarktpartnerCode::new("9900259000002"),
                    lokation: "51238696012".into(),
                },
                stillschweigend: true,
            }
        );
    }
}
