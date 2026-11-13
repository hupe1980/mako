//! GPKE Teil 2 Kap. 3.1 — the NB's Bearbeitungsstand on Abrechnungsdaten.
//!
//! The Netzbetreiber sends the Abrechnungsdaten (Netznutzungs- and
//! Bilanzkreisabrechnung) to the Lieferant; the LF answers with a
//! Qualitätsrückmeldung or orders a change, and the NB owes a
//! **Bearbeitungsstandsmeldung** back.
//!
//! | PID | Process (AHB) | Direction |
//! |---|---|---|
//! | 55156 | Rückmeldung / Anfrage Abr.-Daten BK-Abr. verb. MaLo | LF → NB |
//! | 55220 | Rückmeldung / Anfrage Abr.-Daten Netznutzungsabrechnung | LF → NB |
//! | 55673 | Rückmeldung / Anfrage Abr.-Daten BK-Abr. erz. MaLo | LF → NB |
//! | 55614 | Rückmeldung / Anfrage Abr.-Daten BK-Abr. verb. MaLo | ÜNB → NB |
//! | 55675 | Rückmeldung / Anfrage Abr.-Daten BK-Abr. erz. MaLo | ÜNB → NB |
//! | 21047 | Bearbeitungsstandsmeldung (IFTSTA) | NB → LF · NB → ÜNB |
//!
//! **Frist: der 2. Werktag nach dem ÜT** (GPKE Teil 2 Kap. 3.1.1.2 / 3.1.2.2 / 3.1.3.2
//! Prozessschritt 2/3; towards the ÜNB Kap. 3.1.2.2 Nr. 6, S. 76 and 3.1.3.3 Nr. 2,
//! S. 82), from `mako_fristen::antwort`.
//!
//! # Which tree
//!
//! IFTSTA AHB 2.1 Kap. 7.7 (S. 62) names the tree by the recipient's role. To a
//! LF the NB answers out of `E_0595`; to a ÜNB out of `E_0618` „Rückmeldung
//! verarbeiten" (a Qualitätsrückmeldung, EBD 4.3 Kap. 6.16.4, S. 258) or
//! `E_0613` „Bestellung prüfen" (a Bestellung einer Änderung, Kap. 6.18.1, S.
//! 265–267) — the 55614 / 55675 carries either, so the caller names the tree
//! and [`bearbeitungsstand_baeume`] admits it.
//!
//! # The answer is not an agreement
//!
//! `E_0595` „Bestellung prüfen" decides it, and its clusters are „Änderung der
//! Daten" / „keine Änderung der Daten" — whether a Stammdatenänderung follows,
//! not whether the LF's request was granted. `A06` sits in „Änderung der Daten"
//! while stating that no change is made, because the NB still sends its own data
//! back. The IFTSTA 21047 goes out either way, so nothing here derives a PID
//! from the cluster; it carries the code and the operator or ERP picks it.
//!
//! # Regulatory basis
//!
//! - BK6-24-174 GPKE Teil 2 Kap. 3.1.1 / 3.1.2 / 3.1.3
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 6.17.1 (`E_0595`)

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Inbound PIDs this workflow answers — LF → NB and ÜNB → NB, GPKE Teil 2
/// Kap. 3.1.
pub const ABRECHNUNGSDATEN_PIDS: &[u32] = &[55_156, 55_220, 55_673, 55_614, 55_675];

/// The trees the Bearbeitungsstand on the inbound `pid` may name (IFTSTA AHB
/// 2.1 Kap. 7.7, S. 62: `E_0595` to a LF, `[20]`; `E_0613` / `E_0618` to a
/// ÜNB, `[29]`).
#[must_use]
pub fn bearbeitungsstand_baeume(pid: u32) -> &'static [&'static str] {
    use mako_pruefung::codes as c;
    match pid {
        55_156 | 55_220 | 55_673 => &[c::EBD_BESTELLUNG],
        55_614 | 55_675 => &[
            c::EBD_RUECKMELDUNG_ABRECHNUNGSDATEN_UENB,
            c::EBD_BESTELLUNG_ABRECHNUNGSDATEN_UENB,
        ],
        _ => &[],
    }
}

/// The IFTSTA Bearbeitungsstandsmeldung the NB answers with.
pub const BEARBEITUNGSSTAND_PID: u32 = 21_047;

/// The Entscheidungsbaum that decides the answer — `mako_pruefung::codes`
/// publishes its codes, and the tests hold the two in step.
pub const BESTELLUNG_EBD: &str = "E_0595";

/// Stable workflow name for process routing.
pub const WORKFLOW_NAME: &str = "gpke-abrechnungsdaten";

/// Deadline label for the 2-Werktage Bearbeitungsstand window.
pub const BEARBEITUNGSSTAND_WINDOW_LABEL: &str = "gpke-abrechnungsdaten-bearbeitungsstand";

// ── Events ────────────────────────────────────────────────────────────────────

/// Events emitted by the Abrechnungsdaten workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum AbrechnungsdatenEvent {
    /// A Rückmeldung / Bestellung arrived from the Lieferant.
    RueckmeldungErhalten {
        /// 55156, 55220 or 55673.
        pruefidentifikator: Pruefidentifikator,
        /// MP-ID of the Lieferant.
        sender: MarktpartnerCode,
        /// MP-ID of this Netzbetreiber.
        receiver: MarktpartnerCode,
        /// The Marktlokation the Abrechnungsdaten belong to.
        location_id: MaLo,
        /// EDIFACT document date (`YYYYMMDD`).
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `IDE` DE 7402 of the Rückmeldung — what the Bearbeitungsstand
        /// names in `SG15 RFF+ACW`.
        #[serde(default)]
        vorgangsnummer: String,
        /// `true` if the message passed AHB validation.
        validation_passed: bool,
        /// Validation issues, for the `Abgebrochen` event.
        validation_errors: Vec<String>,
    },
    /// The IFTSTA 21047 Bearbeitungsstandsmeldung went out.
    BearbeitungsstandGesendet {
        /// The Prüfidentifikator the queued IFTSTA carries (21047).
        pid: u32,
        /// The code it states.
        antwort_code: String,
        /// The tree the code comes from — `E_0595`, `E_0613` or `E_0618`.
        codeliste: String,
        /// `true` when a Stammdatenänderung follows — the code's own cluster.
        sendet_stammdatenaenderung: bool,
        /// `FTX` Bemerkung, where one was given.
        bemerkung: Option<String>,
    },
    /// The message could not be processed.
    Abgebrochen {
        /// Why.
        reason: String,
    },
    /// The 2-Werktage window expired without a Bearbeitungsstand.
    DeadlineExpired {
        /// Which deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl EventPayload for AbrechnungsdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::RueckmeldungErhalten { .. } => "AbrechnungsdatenRueckmeldungErhalten",
            Self::BearbeitungsstandGesendet { .. } => "AbrechnungsdatenBearbeitungsstandGesendet",
            Self::Abgebrochen { .. } => "AbrechnungsdatenAbgebrochen",
            Self::DeadlineExpired { .. } => "AbrechnungsdatenDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::RueckmeldungErhalten {
                pruefidentifikator,
                message_ref,
                ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::BearbeitungsstandGesendet { pid, .. } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(*pid),
                message_ref: None,
            }),
            Self::Abgebrochen { .. } | Self::DeadlineExpired { .. } => None,
        }
    }

    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::BearbeitungsstandGesendet {
                antwort_code,
                codeliste,
                ..
            } => Some(Antwortcode::new(
                antwort_code.clone(),
                Some(codeliste.as_str()),
            )),
            _ => None,
        }
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// What the process knows about the inbound Rückmeldung.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AbrechnungsdatenData {
    /// The inbound PID.
    pub pruefidentifikator: Pruefidentifikator,
    /// The Lieferant that sent it.
    pub sender: MarktpartnerCode,
    /// This Netzbetreiber.
    pub receiver: MarktpartnerCode,
    /// The Marktlokation.
    pub location_id: MaLo,
    /// EDIFACT document date.
    pub document_date: String,
    /// EDIFACT message reference.
    pub message_ref: MessageRef,
    /// `IDE` DE 7402 of the Rückmeldung.
    #[serde(default)]
    pub vorgangsnummer: String,
}

/// State of an Abrechnungsdaten Bearbeitungsstand process.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum AbrechnungsdatenState {
    /// No events yet.
    #[default]
    New,
    /// Rückmeldung received; the 2-Werktage window is running.
    Eingegangen(AbrechnungsdatenData),
    /// The Bearbeitungsstand went out. Terminal.
    Beantwortet {
        /// The inbound message.
        data: AbrechnungsdatenData,
        /// The `E_0595` code.
        antwort_code: String,
    },
    /// The process ended without an answer. Terminal.
    Abgebrochen {
        /// Why.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// GPKE Teil 2 Kap. 3.1 gives the NB until the 2. WT nach dem ÜT to send the Bearbeitungsstandsmeldung, and no consequence of missing it. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<AbrechnungsdatenState>,
    },
}

impl AbrechnungsdatenState {
    /// Stable label for logs and error messages.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::Abgebrochen { .. } => "Abgebrochen",
            Self::FristAbgelaufen { .. } => "FristAbgelaufen",
        }
    }

    /// The state the process is waiting in — itself, unless a window lapsed.
    #[must_use]
    pub fn offen(&self) -> &Self {
        match self {
            Self::FristAbgelaufen { offen, .. } => offen.offen(),
            other => other,
        }
    }

    /// `true` when no further command can move this process.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Beantwortet { .. } | Self::Abgebrochen { .. })
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Commands the Abrechnungsdaten workflow accepts.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "command", content = "data")]
pub enum AbrechnungsdatenCommand {
    /// An inbound 55156 / 55220 / 55673 arrived.
    ReceiveRueckmeldung {
        /// The inbound PID.
        pid: Pruefidentifikator,
        /// The Lieferant.
        sender: MarktpartnerCode,
        /// This Netzbetreiber.
        receiver: MarktpartnerCode,
        /// The Marktlokation.
        location_id: MaLo,
        /// EDIFACT document date.
        document_date: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `IDE` DE 7402 of the Rückmeldung.
        vorgangsnummer: String,
        /// AHB validation outcome.
        validation_passed: bool,
        /// Validation issues.
        validation_errors: Vec<String>,
    },
    /// Send the IFTSTA 21047 Bearbeitungsstandsmeldung.
    ///
    /// The `E_0595` code is the caller's — `makod` resolves it against the tree
    /// and refuses one the tree does not publish. Its cluster says whether a
    /// Stammdatenänderung follows, which the NB then sends through
    /// `gpke-stammdatenaenderung`; it does **not** select the answer PID, since
    /// 21047 carries both outcomes.
    SendBearbeitungsstand {
        /// The Antwortcode.
        antwort_code: String,
        /// The tree it comes from — one [`bearbeitungsstand_baeume`] admits
        /// for the inbound PID.
        codeliste: String,
        /// `true` when the code's cluster is „Änderung der Daten".
        sendet_stammdatenaenderung: bool,
        /// `FTX` Bemerkung — required alongside `A99`.
        bemerkung: Option<String>,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Which deadline.
        deadline_id: DeadlineId,
        /// Its label.
        label: Box<str>,
    },
}

impl CommandPayload for AbrechnungsdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The NB's Bearbeitungsstand on inbound Abrechnungsdaten (GPKE Teil 2 Kap. 3.1).
#[derive(Debug, Clone, Copy, Default)]
pub struct GpkeAbrechnungsdatenWorkflow;

/// The one guard a fired deadline passes: the Bearbeitungsstand window is open
/// while the answer is owed (`Eingegangen`). `makod` registers the APERAK
/// 45-minute delivery window on the same stream; it is not this workflow's to
/// decide, and a late technical acknowledgement does not fail the process.
fn window_open(label: &str, state: &AbrechnungsdatenState) -> bool {
    label == BEARBEITUNGSSTAND_WINDOW_LABEL
        && matches!(state, AbrechnungsdatenState::Eingegangen(_))
}

impl Workflow for GpkeAbrechnungsdatenWorkflow {
    type State = AbrechnungsdatenState;
    type Command = AbrechnungsdatenCommand;
    type Event = AbrechnungsdatenEvent;

    const DEADLINE_LABELS: &'static [&'static str] = &[BEARBEITUNGSSTAND_WINDOW_LABEL];

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| AbrechnungsdatenCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // A lapsed window changes nothing about what the process accepts; only
        // the timeout itself asks about the lapse.
        let lapsed = state;
        let state = state.offen();
        match command {
            AbrechnungsdatenCommand::ReceiveRueckmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                message_ref,
                vorgangsnummer,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, AbrechnungsdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ABRECHNUNGSDATEN_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::other(format!(
                        "PID {} is not a GPKE Teil 2 Kap. 3.1 Abrechnungsdaten-Rückmeldung",
                        pid.as_u32()
                    )));
                }
                Ok(vec![AbrechnungsdatenEvent::RueckmeldungErhalten {
                    pruefidentifikator: pid,
                    sender,
                    receiver,
                    location_id,
                    document_date,
                    message_ref,
                    vorgangsnummer,
                    validation_passed,
                    validation_errors,
                }]
                .into())
            }

            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code,
                codeliste,
                sendet_stammdatenaenderung,
                bemerkung,
            } => {
                let AbrechnungsdatenState::Eingegangen(data) = state else {
                    return Err(WorkflowError::invalid_state("Eingegangen", state.label()));
                };
                let baeume = bearbeitungsstand_baeume(data.pruefidentifikator.as_u32());
                if !baeume.contains(&codeliste.as_str()) {
                    return Err(WorkflowError::validation(format!(
                        "{codeliste:?} does not answer {} (admitted: {baeume:?})",
                        data.pruefidentifikator
                    )));
                }
                let code =
                    mako_pruefung::codes::lookup(&codeliste, &antwort_code).ok_or_else(|| {
                        WorkflowError::validation(format!(
                            "{antwort_code:?} is not published in {codeliste}"
                        ))
                    })?;
                if code.sendet_stammdatenaenderung() != Some(sendet_stammdatenaenderung) {
                    return Err(WorkflowError::validation(format!(
                        "{antwort_code:?} of {codeliste} is clustered \"{}\", which the \
                         Bearbeitungsstand's STS must state",
                        code.cluster.label()
                    )));
                }
                // `SG15 RFF+ACW` is Muss and names the Rückmeldung by its
                // `IDE` DE 7402 — without it the LF cannot tell which of its
                // Rückmeldungen the Bearbeitungsstand answers.
                if data.vorgangsnummer.is_empty() {
                    return Err(WorkflowError::rejected(
                        "the Bearbeitungsstand names the Rückmeldung by its IDE Vorgangsnummer \
                         (IFTSTA 21047 SG15 RFF+ACW), and none was recorded with it"
                            .to_owned(),
                    ));
                }
                let events = vec![AbrechnungsdatenEvent::BearbeitungsstandGesendet {
                    pid: BEARBEITUNGSSTAND_PID,
                    antwort_code: antwort_code.clone(),
                    codeliste: codeliste.clone(),
                    sendet_stammdatenaenderung,
                    bemerkung: bemerkung.clone(),
                }];
                // IFTSTA AHB 2.1 Kap. 7.7 (S. 61–63): `STS+Z43` with `Z47`
                // „Änderung der Daten" or `Z48` „keine Änderung der Daten" —
                // the code's own cluster — the code under `E_0595`, and
                // `RFF+ACW` the Rückmeldung's `IDE` DE 7402 (`[535]`). No
                // `LOC`: the column names no Lokation.
                let payload = mako_engine::payload::IftstaPayload {
                    pid: BEARBEITUNGSSTAND_PID,
                    sender: Some(data.receiver.as_str().to_owned()),
                    receiver: Some(data.sender.as_str().to_owned()),
                    sts_code: Some(
                        if sendet_stammdatenaenderung {
                            "Z47"
                        } else {
                            "Z48"
                        }
                        .to_owned(),
                    ),
                    antwort_code: Some(antwort_code),
                    antwort_codeliste: Some(codeliste),
                    referenz_nachricht: Some(data.vorgangsnummer.clone()),
                    bemerkung,
                    ..Default::default()
                };
                let outbox = vec![PendingOutbox::typed(data.sender.as_str(), &payload)];
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            AbrechnungsdatenCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    // A deadline is never cancelled, so it fires for settled
                    // processes too. Nothing went wrong.
                    return Ok(Vec::new().into());
                }
                Ok(vec![AbrechnungsdatenEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            AbrechnungsdatenState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            AbrechnungsdatenEvent::RueckmeldungErhalten {
                pruefidentifikator,
                sender,
                receiver,
                location_id,
                document_date,
                message_ref,
                vorgangsnummer,
                validation_passed,
                validation_errors,
            } => {
                if *validation_passed {
                    AbrechnungsdatenState::Eingegangen(AbrechnungsdatenData {
                        pruefidentifikator: *pruefidentifikator,
                        sender: sender.clone(),
                        receiver: receiver.clone(),
                        location_id: location_id.clone(),
                        document_date: document_date.clone(),
                        message_ref: message_ref.clone(),
                        vorgangsnummer: vorgangsnummer.clone(),
                    })
                } else {
                    AbrechnungsdatenState::Abgebrochen {
                        reason: format!("AHB validation failed: {}", validation_errors.join("; ")),
                    }
                }
            }
            AbrechnungsdatenEvent::BearbeitungsstandGesendet { antwort_code, .. } => match state {
                AbrechnungsdatenState::Eingegangen(data) => AbrechnungsdatenState::Beantwortet {
                    data,
                    antwort_code: antwort_code.clone(),
                },
                other => other,
            },
            AbrechnungsdatenEvent::Abgebrochen { reason } => AbrechnungsdatenState::Abgebrochen {
                reason: reason.clone(),
            },
            AbrechnungsdatenEvent::DeadlineExpired { label, .. } => match state {
                AbrechnungsdatenState::Eingegangen(_) => AbrechnungsdatenState::FristAbgelaufen {
                    label: label.to_string(),
                    offen: Box::new(state),
                },
                other => other,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).expect("valid PID")
    }

    fn receive(code: u32) -> AbrechnungsdatenCommand {
        AbrechnungsdatenCommand::ReceiveRueckmeldung {
            pid: pid(code),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            location_id: MaLo::new("51238696012"),
            document_date: "20260304".to_owned(),
            message_ref: MessageRef::new("ABR-001"),
            vorgangsnummer: "VG-0815".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
        }
    }

    fn eingegangen(code: u32) -> AbrechnungsdatenState {
        let out =
            GpkeAbrechnungsdatenWorkflow::handle(&AbrechnungsdatenState::default(), receive(code))
                .expect("accepted");
        out.events.iter().fold(
            AbrechnungsdatenState::default(),
            GpkeAbrechnungsdatenWorkflow::apply,
        )
    }

    #[test]
    fn every_routed_pid_is_accepted_and_nothing_else_is() {
        for code in ABRECHNUNGSDATEN_PIDS {
            assert!(matches!(
                eingegangen(*code),
                AbrechnungsdatenState::Eingegangen(_)
            ));
        }
        // 55001 is the Anmeldung; it belongs to another workflow.
        assert!(
            GpkeAbrechnungsdatenWorkflow::handle(
                &AbrechnungsdatenState::default(),
                receive(55_001)
            )
            .is_err()
        );
    }

    #[test]
    fn verlauf_states_the_inbound_rueckmeldung_and_the_outbound_iftsta() {
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &AbrechnungsdatenState::default(),
            receive(55_220),
        )
        .expect("accepted");
        assert_eq!(
            out.events[0].verlauf(),
            Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(55_220),
                message_ref: Some("ABR-001".to_owned()),
            })
        );
        let gesendet = AbrechnungsdatenEvent::BearbeitungsstandGesendet {
            pid: BEARBEITUNGSSTAND_PID,
            antwort_code: "A01".to_owned(),
            codeliste: BESTELLUNG_EBD.to_owned(),
            sendet_stammdatenaenderung: false,
            bemerkung: None,
        };
        assert_eq!(
            gesendet.verlauf(),
            Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(21_047),
                message_ref: None,
            })
        );
        let abgebrochen = AbrechnungsdatenEvent::Abgebrochen {
            reason: "x".to_owned(),
        };
        assert_eq!(abgebrochen.verlauf(), None);
    }

    /// The answer is an IFTSTA 21047 carrying its `E_0595` code — an event with
    /// no outbox entry records the process as answered while the Lieferant
    /// watches its 2-Werktage window expire.
    #[test]
    fn the_bearbeitungsstand_reaches_the_wire_with_its_code() {
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &eingegangen(55_156),
            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A05".to_owned(),
                codeliste: BESTELLUNG_EBD.to_owned(),
                sendet_stammdatenaenderung: true,
                bemerkung: None,
            },
        )
        .expect("answered");

        let iftsta = out
            .outbox
            .iter()
            .find(|o| &*o.message_type == "IFTSTA")
            .expect("the answer must produce an outbound IFTSTA");
        assert_eq!(iftsta.payload["pid"], BEARBEITUNGSSTAND_PID);
        assert_eq!(iftsta.payload["antwort_code"], "A05");
        assert_eq!(iftsta.payload["antwort_codeliste"], "E_0595");
        // „Änderung der Daten" is `STS+Z43+Z47`, and `RFF+ACW` names the
        // Rückmeldung by its `IDE` (IFTSTA AHB 2.1 Kap. 7.7).
        assert_eq!(iftsta.payload["sts_code"], "Z47");
        assert_eq!(iftsta.payload["referenz_nachricht"], "VG-0815");
        // The answer goes back to the Lieferant that asked.
        assert_eq!(iftsta.payload["sender"], "9900357000004");
        assert_eq!(iftsta.payload["receiver"], "4012345000023");
    }

    /// Both clusters ride the same PID — `E_0595` does not select one, which is
    /// why the cluster stayed off the agreement axis.
    #[test]
    fn both_clusters_ride_the_same_pid() {
        for (code, sendet) in [("A02", true), ("A03", false)] {
            let out = GpkeAbrechnungsdatenWorkflow::handle(
                &eingegangen(55_220),
                AbrechnungsdatenCommand::SendBearbeitungsstand {
                    antwort_code: code.to_owned(),
                    codeliste: BESTELLUNG_EBD.to_owned(),
                    sendet_stammdatenaenderung: sendet,
                    bemerkung: None,
                },
            )
            .expect("answered");
            let iftsta = out
                .outbox
                .iter()
                .find(|o| &*o.message_type == "IFTSTA")
                .expect("outbound IFTSTA");
            assert_eq!(iftsta.payload["pid"], BEARBEITUNGSSTAND_PID, "{code}");
        }
    }

    /// Every code this workflow can send comes from the tree's Clearing branch.
    #[test]
    fn the_codes_belong_to_the_clearing_branch() {
        assert_eq!(BESTELLUNG_EBD, mako_pruefung::codes::EBD_BESTELLUNG);
        for code in mako_pruefung::codes::E_0595_CLEARING_CODES {
            assert!(
                mako_pruefung::codes::lookup(BESTELLUNG_EBD, code).is_some(),
                "{code}"
            );
        }
    }

    /// A deadline fires for settled processes too, and that is not a failure.
    #[test]
    fn a_deadline_on_a_settled_process_is_a_no_op() {
        let answered = {
            let out = GpkeAbrechnungsdatenWorkflow::handle(
                &eingegangen(55_673),
                AbrechnungsdatenCommand::SendBearbeitungsstand {
                    antwort_code: "A01".to_owned(),
                    codeliste: BESTELLUNG_EBD.to_owned(),
                    sendet_stammdatenaenderung: false,
                    bemerkung: None,
                },
            )
            .expect("answered");
            out.events
                .iter()
                .fold(eingegangen(55_673), GpkeAbrechnungsdatenWorkflow::apply)
        };
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &answered,
            AbrechnungsdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: BEARBEITUNGSSTAND_WINDOW_LABEL.into(),
            },
        )
        .expect("no-op");
        assert!(out.events.is_empty());
    }

    /// An unanswered window is a missed Frist the operator sees — and the
    /// answer it still owes can still go out.
    #[test]
    fn an_expired_window_leaves_the_answer_owed() {
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &eingegangen(55_156),
            AbrechnungsdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: BEARBEITUNGSSTAND_WINDOW_LABEL.into(),
            },
        )
        .expect("expired");
        let state = out
            .events
            .iter()
            .fold(eingegangen(55_156), GpkeAbrechnungsdatenWorkflow::apply);
        assert!(matches!(
            state,
            AbrechnungsdatenState::FristAbgelaufen { .. }
        ));
        let late = GpkeAbrechnungsdatenWorkflow::handle(
            &state,
            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A01".to_owned(),
                codeliste: BESTELLUNG_EBD.to_owned(),
                sendet_stammdatenaenderung: false,
                bemerkung: None,
            },
        )
        .expect("a late answer still goes out");
        let state = late
            .events
            .iter()
            .fold(state, GpkeAbrechnungsdatenWorkflow::apply);
        assert!(matches!(state, AbrechnungsdatenState::Beantwortet { .. }));
    }

    #[test]
    fn the_bearbeitungsstand_states_its_e_0595_code_and_a_rueckmeldung_none() {
        let gesendet = AbrechnungsdatenEvent::BearbeitungsstandGesendet {
            pid: BEARBEITUNGSSTAND_PID,
            antwort_code: "A01".into(),
            codeliste: BESTELLUNG_EBD.to_owned(),
            sendet_stammdatenaenderung: true,
            bemerkung: None,
        };
        assert_eq!(
            gesendet.antwort(),
            Some(Antwortcode::new("A01", Some(BESTELLUNG_EBD)))
        );
        assert_eq!(
            AbrechnungsdatenEvent::Abgebrochen {
                reason: "AHB".into()
            }
            .antwort(),
            None
        );
    }
    /// A ÜNB's 55614 is answered towards the ÜNB out of `E_0618` or `E_0613`,
    /// never `E_0595`, which IFTSTA AHB 2.1 Kap. 7.7 names for a LF only.
    #[test]
    fn a_uenb_rueckmeldung_is_answered_out_of_e_0618() {
        for code in [55_614, 55_675] {
            let out = GpkeAbrechnungsdatenWorkflow::handle(
                &eingegangen(code),
                AbrechnungsdatenCommand::SendBearbeitungsstand {
                    antwort_code: "A01".to_owned(),
                    codeliste: "E_0618".to_owned(),
                    sendet_stammdatenaenderung: true,
                    bemerkung: None,
                },
            )
            .expect("E_0618 answers the ÜNB");
            let iftsta = &out.outbox[0];
            assert_eq!(iftsta.payload["antwort_codeliste"], "E_0618");
            assert_eq!(iftsta.payload["sts_code"], "Z47");
            let e_0595 = GpkeAbrechnungsdatenWorkflow::handle(
                &eingegangen(code),
                AbrechnungsdatenCommand::SendBearbeitungsstand {
                    antwort_code: "A01".to_owned(),
                    codeliste: BESTELLUNG_EBD.to_owned(),
                    sendet_stammdatenaenderung: false,
                    bemerkung: None,
                },
            );
            assert!(e_0595.is_err(), "{code}: E_0595 is the LF's tree");
        }
        let bestellung = GpkeAbrechnungsdatenWorkflow::handle(
            &eingegangen(55_614),
            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A03".to_owned(),
                codeliste: "E_0613".to_owned(),
                sendet_stammdatenaenderung: false,
                bemerkung: None,
            },
        )
        .expect("E_0613 answers the ÜNB's Bestellung");
        assert_eq!(bestellung.outbox[0].payload["sts_code"], "Z48");
    }

    /// The STS cluster is the code's own: a caller that states the other one
    /// is refused.
    #[test]
    fn the_cluster_is_the_codes_own() {
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &eingegangen(55_614),
            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A02".to_owned(),
                codeliste: "E_0618".to_owned(),
                sendet_stammdatenaenderung: true,
                bemerkung: None,
            },
        );
        assert!(out.is_err(), "A02 of E_0618 is „Keine Änderung der Daten\"");
    }
}
