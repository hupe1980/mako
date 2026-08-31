//! GPKE Teil 2 § 3.1 — the NB's Bearbeitungsstand on Abrechnungsdaten.
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
//! | 21047 | Bearbeitungsstandsmeldung (IFTSTA) | NB → LF |
//!
//! **Frist: der 2. Werktag nach dem ÜT** (§§ 3.1.1.2 / 3.1.2.2 / 3.1.3.2
//! Prozessschritt 2/3), from `mako_fristen::antwort`.
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
//! - BK6-24-174 GPKE Teil 2 §§ 3.1.1 / 3.1.2 / 3.1.3
//! - Entscheidungsbaum-Diagramme und Codelisten 4.3, Kap. 6.17.1 (`E_0595`)

use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

// ── PID set ───────────────────────────────────────────────────────────────────

/// Inbound PIDs this workflow answers — all LF → NB, GPKE Teil 2 § 3.1.
pub const ABRECHNUNGSDATEN_PIDS: &[u32] = &[55_156, 55_220, 55_673];

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
        /// `true` if the message passed AHB validation.
        validation_passed: bool,
        /// Validation issues, for the `Abgebrochen` event.
        validation_errors: Vec<String>,
    },
    /// The IFTSTA 21047 Bearbeitungsstandsmeldung went out.
    BearbeitungsstandGesendet {
        /// The `E_0595` code it states.
        antwort_code: String,
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
        /// The `E_0595` Antwortcode.
        antwort_code: String,
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

/// The NB's Bearbeitungsstand on inbound Abrechnungsdaten (GPKE Teil 2 § 3.1).
#[derive(Debug, Clone, Copy, Default)]
pub struct GpkeAbrechnungsdatenWorkflow;

impl Workflow for GpkeAbrechnungsdatenWorkflow {
    type State = AbrechnungsdatenState;
    type Command = AbrechnungsdatenCommand;
    type Event = AbrechnungsdatenEvent;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        // Only the **business** window ends this process. `makod` registers a
        // second deadline on the same stream — the APERAK 45-minute *delivery*
        // window — which the outbox worker discharges the moment the APERAK
        // goes out. Accepting any label made a late technical acknowledgement
        // fail the Abrechnungsdaten process, which is neither what the Frist
        // means nor what GPKE Teil 2 §§ 3.1.1.2 / 3.1.2.2 / 3.1.3.2 sanction.
        (deadline.label() == BEARBEITUNGSSTAND_WINDOW_LABEL && !state.is_terminal()).then(|| {
            AbrechnungsdatenCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }
        })
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            AbrechnungsdatenCommand::ReceiveRueckmeldung {
                pid,
                sender,
                receiver,
                location_id,
                document_date,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, AbrechnungsdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !ABRECHNUNGSDATEN_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::other(format!(
                        "PID {} is not a GPKE Teil 2 § 3.1 Abrechnungsdaten-Rückmeldung",
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
                    validation_passed,
                    validation_errors,
                }]
                .into())
            }

            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code,
                sendet_stammdatenaenderung,
                bemerkung,
            } => {
                let AbrechnungsdatenState::Eingegangen(data) = state else {
                    return Err(WorkflowError::invalid_state("Eingegangen", state.label()));
                };
                let events = vec![AbrechnungsdatenEvent::BearbeitungsstandGesendet {
                    antwort_code: antwort_code.clone(),
                    sendet_stammdatenaenderung,
                    bemerkung: bemerkung.clone(),
                }];
                // The answer is a message, not just an event.
                let outbox = vec![PendingOutbox::new(
                    "IFTSTA",
                    data.sender.as_str(),
                    serde_json::json!({
                        "pid":          BEARBEITUNGSSTAND_PID,
                        "anfrage_pid":  data.pruefidentifikator.as_u32(),
                        "sender":       data.receiver.as_str(),
                        "receiver":     data.sender.as_str(),
                        "malo":         data.location_id.as_str(),
                        // `SG15 STS` — the E_0595 code and the tree it comes from.
                        "antwort_code": antwort_code,
                        "antwort_codeliste":  BESTELLUNG_EBD,
                        // „Änderung der Daten" means a Stammdatenänderung
                        // follows; the receiver needs to know one is coming.
                        "sendet_stammdatenaenderung": sendet_stammdatenaenderung,
                        "bemerkung":    bemerkung,
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(events, outbox))
            }

            AbrechnungsdatenCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    // A deadline is never cancelled, so it fires for settled
                    // processes too. Nothing went wrong.
                    return Ok(Vec::new().into());
                }
                Ok(vec![AbrechnungsdatenEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            AbrechnungsdatenEvent::RueckmeldungErhalten {
                pruefidentifikator,
                sender,
                receiver,
                location_id,
                document_date,
                message_ref,
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
            AbrechnungsdatenEvent::DeadlineExpired { label, .. } => {
                if state.is_terminal() {
                    state
                } else {
                    AbrechnungsdatenState::Abgebrochen {
                        reason: format!(
                            "{label} expired without a Bearbeitungsstandsmeldung — GPKE Teil 2 \
                             § 3.1 gives the NB until the 2. WT nach dem ÜT"
                        ),
                    }
                }
            }
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

    /// The answer is an IFTSTA 21047 carrying its `E_0595` code — an event with
    /// no outbox entry records the process as answered while the Lieferant
    /// watches its 2-Werktage window expire.
    #[test]
    fn the_bearbeitungsstand_reaches_the_wire_with_its_code() {
        let out = GpkeAbrechnungsdatenWorkflow::handle(
            &eingegangen(55_156),
            AbrechnungsdatenCommand::SendBearbeitungsstand {
                antwort_code: "A05".to_owned(),
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
        assert_eq!(iftsta.payload["sendet_stammdatenaenderung"], true);
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

    /// An unanswered window closes the process, so the operator sees a missed
    /// Frist rather than an open one.
    #[test]
    fn an_expired_window_closes_the_process() {
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
        assert!(matches!(state, AbrechnungsdatenState::Abgebrochen { .. }));
    }
}
