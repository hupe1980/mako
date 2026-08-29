//! GPKE Zuordnungs-Meldungen — the three one-way notifications the NB owes
//! around a Lieferbeginn.
//!
//! # Why these are their own workflow
//!
//! A **Meldepflicht** is a message a Festlegung obliges a party to send with no
//! answer expected back. That is the whole difficulty: nobody waits for a reply,
//! so a missing one produces no timeout, no dead letter and no alert — it
//! surfaces months later as a counterparty holding a stale view of who supplies
//! the Marktlokation. Modelling them as processes puts the obligation, its
//! Frist and its dispatch in the event log where a `§ 20 EnWG` audit can read
//! them.
//!
//! | PID | Message | NB → | Prozessschritt | Spätester ÜZ |
//! |---|---|---|---|---|
//! | 55036 | Information über existierende Zuordnung | LFN | Nr. 2 | 07:00 Uhr des 1. WT nach dem ÜT |
//! | 55037 | Beendigung der Zuordnung | LFA | Nr. 10 | 12:00 Uhr des 1. WT nach dem ÜT |
//! | 55038 | Aufhebung einer zukünftigen Zuordnung | LFZ | Nr. 13 | 12:00 Uhr des 1. WT nach dem ÜT |
//!
//! All three windows run from the **Eingang der Anmeldung** (Nr. 1), not from
//! the NB's own answer — so all three are resolvable the moment the Anmeldung
//! arrives, and 55036's 07:00 closes four hours *before* the 11:00 Bestätigung
//! of the same message. The catalogue is [`mako_fristen::meldung`].
//!
//! # The branch that decides whether they are owed
//!
//! GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 Prüfschritt 4: „Ist die
//! Marktlokation bzw. Tranche zum Zuordnungsbeginn einem LF zugeordnet, fährt
//! der NB mit Prozessschritt 2 fort, ansonsten mit Prozessschritt 5." So 55036
//! is owed exactly when an LFA exists — the same condition that makes the 55010
//! Abmeldeanfrage owed („Parallel zu Nr. 2"). The Festlegung adds: „Die
//! Information ist auch dann zu versenden, sofern LFA und LFN identisch sind."
//!
//! That condition is the Versorgungsstatus, which lives in `marktd`, so the
//! decision belongs to `processd` — this workflow renders and records what
//! `processd` decides.
//!
//! # Wire facts (UTILMD AHB Strom 2.1 Kap. 8.11 / 2.2 Kap. 8.11)
//!
//! - `BGM+E01` on 55036 (an Anmeldung), `BGM+E02` on 55037 and 55038.
//! - `SG4 STS+7` DE 9013 carries the Grund; the admissible set differs per PID
//!   and is enforced by [`Zuordnungsmeldung::grund_is_admissible`].
//! - `SG4 DTM`: **none** on 55036 — the Anwendungsfall carries no process date
//!   at all — `DTM+93` Vertragsende on 55037, `DTM+92` Vertragsbeginn on 55038
//!   („Ursprünglich vom NB bestätigtes Beginndatum", Bedingung `[507]`).
//! - `SG5 LOC+Z16` Marktlokation **or** `LOC+Z21` Tranche, never both.
//! - `SG6 RFF+Z13` Prüfidentifikator, plus `RFF+TN` with the Vorgangsnummer of
//!   the triggering Anmeldung — Muss on 55036 alone.
//! - `SG12 NAD+VY`: the Altlieferant on 55036 (Bedingung `[518]`: *all* of them,
//!   because Geschäftsvorfall 3 splits a Marktlokation across Tranchen), the
//!   auslösender Marktpartner on 55038 (`[579]`: LFN bei `ZH0`/`ZG9`, NB bei
//!   `ZH1`), and none at all on 55037.
//!
//! # Regulatory basis
//!
//! - **BK6-24-174 GPKE Teil 2** § 2.1.2 SD Lieferbeginn Nr. 2 / 10 / 13
//! - **UTILMD AHB Strom 2.1 / 2.2** Kap. 8.11
//! - **APERAK AHB 1.0 § 2.4.1** — 45 Minuten on a Werktag, a separate clock

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

use edi_codes::{BEGINN_ZUM, ENDE_ZUM, transaktionsgrund as grund};

/// The `SG4 STS+7` DE 9013 and `SG4 DTM` DE 2005 codes this module writes.
///
/// Spelled out here rather than pulled from `edi-energy`: `mako-gpke` is a
/// domain crate and does not depend on the wire library, and these six codes are
/// what the AHB fixes for this Anwendungsfall.
mod edi_codes {
    /// `SG4 DTM` DE 2005 — `93` Datum Vertragsende (55037).
    pub const ENDE_ZUM: &str = "93";
    /// `SG4 DTM` DE 2005 — `92` Datum Vertragsbeginn (55038).
    pub const BEGINN_ZUM: &str = "92";

    /// `SG4 STS+7` DE 9013 — the Gründe UTILMD AHB Strom Kap. 8.11 admits.
    pub mod transaktionsgrund {
        /// `Z26` — Information über existierende Zuordnung (55036).
        pub const INFO_EXISTIERENDE_ZUORDNUNG: &str = "Z26";
        /// `ZC8` — Beendigung der Zuordnung (55037).
        pub const BEENDIGUNG_ZUORDNUNG: &str = "ZC8";
        /// `ZD9` — Beendigung wegen Rückzuordnungsmeldung (55037).
        pub const BEENDIGUNG_RUECKZUORDNUNG: &str = "ZD9";
        /// `ZG6` — Beendigung der Zuordnung aufgrund EEG 2014 § 38 (55037).
        pub const BEENDIGUNG_EEG38: &str = "ZG6";
        /// `ZG5` — Aufhebung aufgrund § 38 EEG 2014 bzw. § 21b Abs. 1 Nr. 2
        /// EEG 2017 (55038). The one Grund that names no beteiligter
        /// Marktpartner (Bedingung `[206]`).
        pub const AUFHEBUNG_EEG38: &str = "ZG5";
        /// `ZG9` — Aufhebung wegen Auszug des Kunden (55038).
        pub const AUFHEBUNG_AUSZUG: &str = "ZG9";
        /// `ZH0` — Aufhebung wegen Anmeldung eines anderen Lieferanten zu einem
        /// früheren Termin (55038).
        pub const AUFHEBUNG_FRUEHERE_ANMELDUNG: &str = "ZH0";
        /// `ZH1` — Aufhebung wegen Stilllegung (55038).
        pub const AUFHEBUNG_STILLLEGUNG: &str = "ZH1";
    }
}

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-zuordnungsmeldung";

/// PID 55036 — Information über existierende Zuordnung (NB → LFN).
pub const INFORMATION_PID: u32 = 55_036;
/// PID 55037 — Beendigung der Zuordnung (NB → LFA).
pub const BEENDIGUNG_PID: u32 = 55_037;
/// PID 55038 — Aufhebung einer zukünftigen Zuordnung (NB → LFZ).
pub const AUFHEBUNG_PID: u32 = 55_038;

/// Every Zuordnungs-Meldung, in Prozessschritt order — which is also the order
/// their windows close.
pub const ZUORDNUNGSMELDUNG_PIDS: &[u32] = &[INFORMATION_PID, BEENDIGUNG_PID, AUFHEBUNG_PID];

/// Deadline label for the dispatch window.
///
/// One label for all three: the window differs per PID and comes from
/// [`mako_fristen::meldung`], but what expiry means is the same — a Meldepflicht
/// the NB owes went unsent.
pub const MELDUNG_WINDOW_LABEL: &str = "gpke-zuordnungsmeldung-frist";

// ── Which message ─────────────────────────────────────────────────────────────

/// One of the three Zuordnungs-Meldungen, resolved from its Prüfidentifikator.
///
/// The variant decides the BGM code, whether a process date rides in SG4, which
/// DE 2005 qualifier it takes, and which Gründe the AHB admits — four facts a
/// caller must not be able to mix across messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zuordnungsmeldung {
    /// 55036 — „zum Zuordnungsbeginn existiert eine Zuordnung zu einem LFA",
    /// addressed to the **LFN**, naming the LFA's identity.
    Information,
    /// 55037 — the LFA's Zuordnung ends, addressed to the **LFA**, naming the
    /// Zuordnungsende and the Grund.
    Beendigung,
    /// 55038 — a future Zuordnung is cancelled, addressed to the **LFZ**.
    Aufhebung,
}

impl Zuordnungsmeldung {
    /// Resolve from a Prüfidentifikator, or `None` when it is not one of the
    /// three.
    #[must_use]
    pub const fn from_pid(pid: u32) -> Option<Self> {
        match pid {
            INFORMATION_PID => Some(Self::Information),
            BEENDIGUNG_PID => Some(Self::Beendigung),
            AUFHEBUNG_PID => Some(Self::Aufhebung),
            _ => None,
        }
    }

    /// The Prüfidentifikator this message rides.
    #[must_use]
    pub const fn pid(self) -> u32 {
        match self {
            Self::Information => INFORMATION_PID,
            Self::Beendigung => BEENDIGUNG_PID,
            Self::Aufhebung => AUFHEBUNG_PID,
        }
    }

    /// `BGM` DE 1001. 55036 is an Anmeldung (`E01`); the two that end or cancel
    /// an assignment are Abmeldungen (`E02`).
    #[must_use]
    pub const fn bgm_document_code(self) -> &'static str {
        match self {
            Self::Information => "E01",
            Self::Beendigung | Self::Aufhebung => "E02",
        }
    }

    /// The `SG4 DTM` DE 2005 qualifier the process date takes, or `None`.
    ///
    /// 55036 carries **no** SG4 date: the AHB's Kap. 8.11 column is empty for
    /// both „Beginn zum" and „Ende zum". Emitting one there is an unlisted
    /// segment, which is why this is an `Option` rather than a default.
    #[must_use]
    pub const fn dtm_qualifier(self) -> Option<&'static str> {
        match self {
            Self::Information => None,
            Self::Beendigung => Some(ENDE_ZUM),
            Self::Aufhebung => Some(BEGINN_ZUM),
        }
    }

    /// The `SG4 STS+7` DE 9013 codes the AHB admits for this message.
    #[must_use]
    pub const fn admissible_gruende(self) -> &'static [&'static str] {
        match self {
            Self::Information => &[grund::INFO_EXISTIERENDE_ZUORDNUNG],
            Self::Beendigung => &[
                grund::BEENDIGUNG_ZUORDNUNG,
                grund::BEENDIGUNG_RUECKZUORDNUNG,
                grund::BEENDIGUNG_EEG38,
            ],
            Self::Aufhebung => &[
                grund::AUFHEBUNG_EEG38,
                grund::AUFHEBUNG_AUSZUG,
                grund::AUFHEBUNG_FRUEHERE_ANMELDUNG,
                grund::AUFHEBUNG_STILLLEGUNG,
            ],
        }
    }

    /// Whether `code` is an admissible `SG4 STS+7` DE 9013 for this message.
    #[must_use]
    pub fn grund_is_admissible(self, code: &str) -> bool {
        self.admissible_gruende().contains(&code)
    }

    /// Whether `SG12 NAD+VY` must name a beteiligter Marktpartner.
    ///
    /// Unconditional on 55036 (the Altlieferant, Bedingung `[518]`/`[566]`).
    /// On 55038 it is Muss „wenn `SG4 STS+7++ZG5` nicht vorhanden" (`[206]`) —
    /// the § 38 EEG 2014 Aufhebung has no auslösenden Marktpartner to name.
    /// Never on 55037.
    #[must_use]
    pub fn requires_beteiligter(self, transaktionsgrund: &str) -> bool {
        match self {
            Self::Information => true,
            Self::Beendigung => false,
            Self::Aufhebung => transaktionsgrund != grund::AUFHEBUNG_EEG38,
        }
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the Zuordnungs-Meldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ZuordnungsmeldungEvent {
    /// The Meldung was rendered and queued for AS4 delivery.
    Gesendet {
        /// 55036, 55037 or 55038.
        pruefidentifikator: Pruefidentifikator,
        /// Marktlokation or Tranche the Meldung is about.
        location_id: MaLo,
        /// The NB.
        sender: MarktpartnerCode,
        /// LFN (55036), LFA (55037) or LFZ (55038).
        receiver: MarktpartnerCode,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// Zuordnungsende (55037) or ursprünglicher Zuordnungsbeginn (55038),
        /// `YYYYMMDD`. Absent on 55036, which carries no SG4 date.
        #[serde(default)]
        process_date: Option<String>,
        /// `SG12 NAD+VY` — the Altlieferanten (55036) or the auslösender
        /// Marktpartner (55038).
        #[serde(default)]
        beteiligte: Vec<MarktpartnerCode>,
        /// `SG6 RFF+TN` — the Vorgangsnummer of the Anmeldung that triggered
        /// this obligation. Muss on 55036.
        #[serde(default)]
        referenz_vorgangsnummer: Option<String>,
    },
    /// A Meldung arrived from a Netzbetreiber. Recorded, never answered — the
    /// AHB Gas says it in as many words: „Eine Informationsmeldung ist eine
    /// Nachricht, für die keine Antwort vorgesehen ist."
    Empfangen {
        /// 55036, 55037 or 55038.
        pruefidentifikator: Pruefidentifikator,
        /// Marktlokation or Tranche.
        location_id: MaLo,
        /// The NB.
        sender: MarktpartnerCode,
        /// Us.
        receiver: MarktpartnerCode,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
}

impl EventPayload for ZuordnungsmeldungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::Gesendet { .. } => "ZuordnungsmeldungGesendet",
            Self::Empfangen { .. } => "ZuordnungsmeldungEmpfangen",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// State of one Zuordnungs-Meldung.
///
/// There is no „awaiting answer" state, because there is no answer. Both
/// terminal variants exist so a read model can tell an obligation this NB
/// discharged from one a counterparty discharged towards it.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum ZuordnungsmeldungState {
    /// No events yet.
    #[default]
    New,
    /// We sent it.
    Gesendet {
        /// The Prüfidentifikator that went out.
        pruefidentifikator: Pruefidentifikator,
        /// The Marktlokation or Tranche.
        location_id: MaLo,
        /// The party it went to.
        receiver: MarktpartnerCode,
    },
    /// We received it.
    Empfangen {
        /// The Prüfidentifikator that arrived.
        pruefidentifikator: Pruefidentifikator,
        /// The Marktlokation or Tranche.
        location_id: MaLo,
        /// The Netzbetreiber that sent it.
        sender: MarktpartnerCode,
    },
}

impl ZuordnungsmeldungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Gesendet { .. } => "Gesendet",
            Self::Empfangen { .. } => "Empfangen",
        }
    }
}

/// A Zuordnungs-Meldung never occupies its Marktlokation.
///
/// It is a single-message process: one command, one event, done. Several are
/// owed on the same MaLo around one Lieferbeginn — the Information to the LFN,
/// the Beendigung to the LFA, the Aufhebung to the LFZ — and a counterparty may
/// send more over the life of the Marktlokation. Reporting occupancy would
/// route the second one into the first one's finished process, where `handle`
/// refuses it as an invalid state transition and the message is lost.
impl mako_engine::workflow::OccupiesBusinessKey for ZuordnungsmeldungState {
    fn occupies_business_key(&self) -> bool {
        false
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the Zuordnungs-Meldung workflow.
#[derive(Clone)]
pub enum ZuordnungsmeldungCommand {
    /// Render and queue one Meldung (NB role).
    Senden {
        /// Which of the three.
        meldung: Zuordnungsmeldung,
        /// The NB's own MP-ID.
        sender: MarktpartnerCode,
        /// LFN, LFA or LFZ.
        receiver: MarktpartnerCode,
        /// Marktlokations-ID, or the MaLo-ID of the Tranche.
        location_id: MaLo,
        /// `SG5 LOC` qualifier — `Z16` Marktlokation or `Z21` Tranche.
        tranche: bool,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// `YYYYMMDD`. Required on 55037 and 55038, refused on 55036.
        process_date: Option<String>,
        /// `SG12 NAD+VY` parties.
        beteiligte: Vec<MarktpartnerCode>,
        /// `SG6 RFF+TN` — the triggering Anmeldung's Vorgangsnummer.
        referenz_vorgangsnummer: Option<String>,
    },
    /// An inbound Meldung from a Netzbetreiber (LF role).
    Empfangen {
        /// 55036, 55037 or 55038.
        pid: Pruefidentifikator,
        /// The NB.
        sender: MarktpartnerCode,
        /// Us.
        receiver: MarktpartnerCode,
        /// Marktlokation or Tranche.
        location_id: MaLo,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` when `msg.validate()` returned no errors.
        validation_passed: bool,
        /// Validation issues, for the APERAK.
        validation_errors: Vec<String>,
    },
}

impl CommandPayload for ZuordnungsmeldungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// The GPKE Zuordnungs-Meldung workflow — PIDs 55036 / 55037 / 55038.
#[derive(Debug, Clone, Copy, Default)]
pub struct GpkeZuordnungsmeldungWorkflow;

impl Workflow for GpkeZuordnungsmeldungWorkflow {
    type State = ZuordnungsmeldungState;
    type Event = ZuordnungsmeldungEvent;
    type Command = ZuordnungsmeldungCommand;

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            ZuordnungsmeldungEvent::Gesendet {
                pruefidentifikator,
                location_id,
                receiver,
                ..
            } => ZuordnungsmeldungState::Gesendet {
                pruefidentifikator: *pruefidentifikator,
                location_id: location_id.clone(),
                receiver: receiver.clone(),
            },
            ZuordnungsmeldungEvent::Empfangen {
                pruefidentifikator,
                location_id,
                sender,
                ..
            } => ZuordnungsmeldungState::Empfangen {
                pruefidentifikator: *pruefidentifikator,
                location_id: location_id.clone(),
                sender: sender.clone(),
            },
            #[allow(unreachable_patterns)]
            _ => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            ZuordnungsmeldungCommand::Senden {
                meldung,
                sender,
                receiver,
                location_id,
                tranche,
                transaktionsgrund,
                process_date,
                beteiligte,
                referenz_vorgangsnummer,
            } => {
                if !matches!(state, ZuordnungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !meldung.grund_is_admissible(&transaktionsgrund) {
                    return Err(WorkflowError::rejected(format!(
                        "SG4 STS+7 DE 9013 {transaktionsgrund:?} is not admissible on PID {} — \
                         UTILMD AHB Strom Kap. 8.11 lists {:?}",
                        meldung.pid(),
                        meldung.admissible_gruende(),
                    )));
                }
                // The SG4 date is a per-PID fact, not a caller preference:
                // 55036 has no „Beginn zum" or „Ende zum" column at all, and the
                // other two mark theirs Muss.
                match (meldung.dtm_qualifier(), process_date.as_deref()) {
                    (Some(_), None | Some("")) => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {} requires a process date (SG4 DTM+{})",
                            meldung.pid(),
                            meldung.dtm_qualifier().unwrap_or_default(),
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {} carries no SG4 date — UTILMD AHB Strom Kap. 8.11 leaves both \
                             the „Beginn zum\" and „Ende zum\" columns empty for it",
                            meldung.pid(),
                        )));
                    }
                    _ => {}
                }
                if meldung.requires_beteiligter(&transaktionsgrund) && beteiligte.is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} requires SG12 NAD+VY — {}",
                        meldung.pid(),
                        match meldung {
                            Zuordnungsmeldung::Information =>
                                "every Altlieferant an Abmeldeanfrage was sent to (Bedingung [518])",
                            _ => "the auslösender Marktpartner (Bedingung [579])",
                        },
                    )));
                }
                if meldung == Zuordnungsmeldung::Information
                    && referenz_vorgangsnummer
                        .as_deref()
                        .unwrap_or_default()
                        .is_empty()
                {
                    return Err(WorkflowError::rejected(
                        "PID 55036 requires SG6 RFF+TN — the Vorgangsnummer of the Anmeldung it \
                         answers to (UTILMD AHB Strom Kap. 8.11, „Referenz Vorgangsnummer (aus \
                         Anfragenachricht)\")"
                            .to_owned(),
                    ));
                }

                let pid = Pruefidentifikator::new(meldung.pid())
                    .map_err(|e| WorkflowError::rejected(e.clone()))?;
                let outbox = vec![PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    meldung_payload(
                        meldung,
                        &sender,
                        &receiver,
                        &location_id,
                        tranche,
                        &transaktionsgrund,
                        process_date.as_deref(),
                        &beteiligte,
                        referenz_vorgangsnummer.as_deref(),
                    ),
                )];
                Ok(WorkflowOutput::with_outbox(
                    vec![ZuordnungsmeldungEvent::Gesendet {
                        pruefidentifikator: pid,
                        location_id,
                        sender,
                        receiver,
                        transaktionsgrund,
                        process_date,
                        beteiligte,
                        referenz_vorgangsnummer,
                    }],
                    outbox,
                ))
            }

            ZuordnungsmeldungCommand::Empfangen {
                pid,
                sender,
                receiver,
                location_id,
                transaktionsgrund,
                message_ref,
                validation_passed,
                validation_errors,
            } => {
                if !matches!(state, ZuordnungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if Zuordnungsmeldung::from_pid(pid.as_u32()).is_none() {
                    return Err(WorkflowError::rejected(format!(
                        "expected a Zuordnungs-Meldung PID ({ZUORDNUNGSMELDUNG_PIDS:?}), got {pid}",
                    )));
                }
                // The APERAK is the only thing that goes back — the business
                // answer channel does not exist for these PIDs. Strom owes one
                // either way (APERAK AHB 1.0 § 2.4): `312` Anerkennung on a
                // clean message, `313` Verarbeitbarkeitsfehler otherwise.
                let aperak = if validation_passed {
                    serde_json::json!({
                        "sender":        receiver.as_str(),
                        "receiver":      sender.as_str(),
                        "pid":           29001_u32,
                        "document_code": "312",
                    })
                } else {
                    serde_json::json!({
                        "sender":     receiver.as_str(),
                        "receiver":   sender.as_str(),
                        "pid":        29001_u32,
                        "error_code": mako_engine::erc::codes::Z29,
                        "reason":     validation_errors.join("; "),
                    })
                };
                let outbox = vec![
                    PendingOutbox::new("APERAK", sender.as_str(), aperak),
                    // The ERP/`processd` notification. A Meldung changes what a
                    // supplier believes about its own Zuordnung, so it has to
                    // leave the engine even though nothing is answered.
                    PendingOutbox::new(
                        "ProcessInitiated",
                        receiver.as_str(),
                        serde_json::json!({
                            "pid":               pid.as_u32(),
                            "malo_id":           location_id.as_str(),
                            "grid_operator":     sender.as_str(),
                            "transaktionsgrund": transaktionsgrund,
                        }),
                    ),
                ];
                Ok(WorkflowOutput::with_outbox(
                    vec![ZuordnungsmeldungEvent::Empfangen {
                        pruefidentifikator: pid,
                        location_id,
                        sender,
                        receiver,
                        transaktionsgrund,
                        message_ref,
                    }],
                    outbox,
                ))
            }
        }
    }
}

/// The renderer payload for one Meldung.
#[allow(clippy::too_many_arguments)]
fn meldung_payload(
    meldung: Zuordnungsmeldung,
    sender: &MarktpartnerCode,
    receiver: &MarktpartnerCode,
    location_id: &MaLo,
    tranche: bool,
    transaktionsgrund: &str,
    process_date: Option<&str>,
    beteiligte: &[MarktpartnerCode],
    referenz_vorgangsnummer: Option<&str>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "direction":         "outbound",
        "pid":               meldung.pid(),
        "sender":            sender.as_str(),
        "receiver":          receiver.as_str(),
        "malo":              location_id.as_str(),
        "document_code":     meldung.bgm_document_code(),
        "transaktionsgrund": transaktionsgrund,
        // `SG5 LOC+Z21` when the Vorgang is about a Tranche. Both carry a
        // MaLo-ID in DE 3225 (Bedingung [950]), so the qualifier is the only
        // thing that says which object it is.
        "lokationstyp":      if tranche { "Z21" } else { "Z16" },
    });
    let obj = payload.as_object_mut().expect("json! built an object");
    if let Some(date) = process_date {
        obj.insert("process_date".into(), date.into());
    }
    if let Some(vorgang) = referenz_vorgangsnummer.filter(|v| !v.is_empty()) {
        obj.insert("referenz_vorgangsnummer".into(), vorgang.into());
    }
    if !beteiligte.is_empty() {
        obj.insert(
            "beteiligte_marktpartner".into(),
            serde_json::Value::Array(
                beteiligte
                    .iter()
                    .map(|m| serde_json::Value::String(m.as_str().to_owned()))
                    .collect(),
            ),
        );
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mp(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s.to_owned())
    }

    fn malo() -> MaLo {
        MaLo::new("51238696781".to_owned())
    }

    fn senden(meldung: Zuordnungsmeldung) -> ZuordnungsmeldungCommand {
        let (grund, date, beteiligte, referenz) = match meldung {
            Zuordnungsmeldung::Information => (
                grund::INFO_EXISTIERENDE_ZUORDNUNG,
                None,
                vec![mp("9900555000005")],
                Some("VG-4711".to_owned()),
            ),
            Zuordnungsmeldung::Beendigung => (
                grund::BEENDIGUNG_ZUORDNUNG,
                Some("20260701".to_owned()),
                vec![],
                None,
            ),
            Zuordnungsmeldung::Aufhebung => (
                grund::AUFHEBUNG_FRUEHERE_ANMELDUNG,
                Some("20260801".to_owned()),
                vec![mp("9900111000002")],
                None,
            ),
        };
        ZuordnungsmeldungCommand::Senden {
            meldung,
            sender: mp("9900357000004"),
            receiver: mp("9900123456789"),
            location_id: malo(),
            tranche: false,
            transaktionsgrund: grund.to_owned(),
            process_date: date,
            beteiligte,
            referenz_vorgangsnummer: referenz,
        }
    }

    fn run(cmd: ZuordnungsmeldungCommand) -> WorkflowOutput<ZuordnungsmeldungEvent> {
        GpkeZuordnungsmeldungWorkflow::handle(&ZuordnungsmeldungState::New, cmd)
            .expect("command accepted")
    }

    #[test]
    fn each_meldung_renders_its_own_bgm_code() {
        for (meldung, expected) in [
            (Zuordnungsmeldung::Information, "E01"),
            (Zuordnungsmeldung::Beendigung, "E02"),
            (Zuordnungsmeldung::Aufhebung, "E02"),
        ] {
            let out = run(senden(meldung));
            assert_eq!(out.outbox[0].payload["document_code"], expected);
        }
    }

    /// The AHB leaves both SG4 date columns empty on 55036. A date there is an
    /// unlisted segment, so the workflow refuses one rather than dropping it.
    #[test]
    fn the_information_carries_no_sg4_date() {
        let out = run(senden(Zuordnungsmeldung::Information));
        assert!(out.outbox[0].payload.get("process_date").is_none());

        let ZuordnungsmeldungCommand::Senden {
            meldung,
            sender,
            receiver,
            location_id,
            tranche,
            transaktionsgrund,
            beteiligte,
            referenz_vorgangsnummer,
            ..
        } = senden(Zuordnungsmeldung::Information)
        else {
            unreachable!()
        };
        let err = GpkeZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            ZuordnungsmeldungCommand::Senden {
                meldung,
                sender,
                receiver,
                location_id,
                tranche,
                transaktionsgrund,
                process_date: Some("20260701".to_owned()),
                beteiligte,
                referenz_vorgangsnummer,
            },
        )
        .expect_err("55036 takes no SG4 date");
        assert!(format!("{err}").contains("no SG4 date"), "{err}");
    }

    /// 55037 names a Vertragsende (`93`), 55038 the originally confirmed
    /// Vertragsbeginn (`92`). Swapping them states the wrong kind of date.
    #[test]
    fn the_two_dated_meldungen_take_different_qualifiers() {
        assert_eq!(Zuordnungsmeldung::Beendigung.dtm_qualifier(), Some("93"));
        assert_eq!(Zuordnungsmeldung::Aufhebung.dtm_qualifier(), Some("92"));
    }

    /// The three code spaces are disjoint: `ZC8` closes an assignment and `ZG9`
    /// cancels a future one, and neither is admissible on the other's PID.
    #[test]
    fn a_grund_belongs_to_exactly_one_meldung() {
        let all = [
            Zuordnungsmeldung::Information,
            Zuordnungsmeldung::Beendigung,
            Zuordnungsmeldung::Aufhebung,
        ];
        for m in all {
            for code in m.admissible_gruende() {
                let owners = all.iter().filter(|o| o.grund_is_admissible(code)).count();
                assert_eq!(owners, 1, "{code} is admissible on {owners} Meldungen");
            }
        }
        assert!(!Zuordnungsmeldung::Beendigung.grund_is_admissible(grund::AUFHEBUNG_AUSZUG));
    }

    #[test]
    fn an_inadmissible_grund_is_refused() {
        let err = GpkeZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            ZuordnungsmeldungCommand::Senden {
                meldung: Zuordnungsmeldung::Beendigung,
                sender: mp("9900357000004"),
                receiver: mp("9900123456789"),
                location_id: malo(),
                tranche: false,
                transaktionsgrund: "E03".to_owned(),
                process_date: Some("20260701".to_owned()),
                beteiligte: vec![],
                referenz_vorgangsnummer: None,
            },
        )
        .expect_err("E03 is a Lieferantenwechsel, not a Beendigungsgrund");
        assert!(format!("{err}").contains("not admissible"), "{err}");
    }

    /// Bedingung [206]: the § 38 EEG 2014 Aufhebung is the one Grund with no
    /// auslösenden Marktpartner to name.
    #[test]
    fn only_the_eeg38_aufhebung_may_omit_the_beteiligter() {
        let base =
            |grund: &str, beteiligte: Vec<MarktpartnerCode>| ZuordnungsmeldungCommand::Senden {
                meldung: Zuordnungsmeldung::Aufhebung,
                sender: mp("9900357000004"),
                receiver: mp("9900123456789"),
                location_id: malo(),
                tranche: false,
                transaktionsgrund: grund.to_owned(),
                process_date: Some("20260801".to_owned()),
                beteiligte,
                referenz_vorgangsnummer: None,
            };
        GpkeZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            base(grund::AUFHEBUNG_EEG38, vec![]),
        )
        .expect("ZG5 names no beteiligten Marktpartner");
        GpkeZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            base(grund::AUFHEBUNG_STILLLEGUNG, vec![]),
        )
        .expect_err("ZH1 names the NB itself and must carry SG12");
    }

    /// [518]: „Es sind alle Altlieferanten anzugeben" — Geschäftsvorfall 3
    /// splits a Marktlokation across Tranchen, so SG12 repeats.
    #[test]
    fn the_information_names_every_altlieferant() {
        let out = run(ZuordnungsmeldungCommand::Senden {
            meldung: Zuordnungsmeldung::Information,
            sender: mp("9900357000004"),
            receiver: mp("9900123456789"),
            location_id: malo(),
            tranche: true,
            transaktionsgrund: grund::INFO_EXISTIERENDE_ZUORDNUNG.to_owned(),
            process_date: None,
            beteiligte: vec![mp("9900555000005"), mp("9900111000002")],
            referenz_vorgangsnummer: Some("VG-4711".to_owned()),
        });
        let payload = &out.outbox[0].payload;
        assert_eq!(
            payload["beteiligte_marktpartner"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(payload["lokationstyp"], "Z21");
    }

    /// Without `SG6 RFF+TN` the LFN cannot tie the Information to the Anmeldung
    /// it just sent — the AHB marks it Muss on 55036 and on nothing else here.
    #[test]
    fn the_information_must_reference_the_anmeldung() {
        let err = GpkeZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            ZuordnungsmeldungCommand::Senden {
                meldung: Zuordnungsmeldung::Information,
                sender: mp("9900357000004"),
                receiver: mp("9900123456789"),
                location_id: malo(),
                tranche: false,
                transaktionsgrund: grund::INFO_EXISTIERENDE_ZUORDNUNG.to_owned(),
                process_date: None,
                beteiligte: vec![mp("9900555000005")],
                referenz_vorgangsnummer: None,
            },
        )
        .expect_err("55036 must carry RFF+TN");
        assert!(format!("{err}").contains("RFF+TN"), "{err}");
    }

    /// An inbound Meldung is recorded and acknowledged, never answered.
    #[test]
    fn an_inbound_meldung_is_acknowledged_but_not_answered() {
        let out = run(ZuordnungsmeldungCommand::Empfangen {
            pid: Pruefidentifikator::new(BEENDIGUNG_PID).expect("valid"),
            sender: mp("9900357000004"),
            receiver: mp("9900123456789"),
            location_id: malo(),
            transaktionsgrund: grund::BEENDIGUNG_ZUORDNUNG.to_owned(),
            message_ref: MessageRef::new("MSG-1".to_owned()),
            validation_passed: true,
            validation_errors: vec![],
        });
        let kinds: Vec<&str> = out.outbox.iter().map(|o| &*o.message_type).collect();
        assert_eq!(kinds, vec!["APERAK", "ProcessInitiated"]);
        assert!(
            !kinds.contains(&"UTILMD"),
            "a Meldung has no Antwortnachricht"
        );
    }

    #[test]
    fn every_pid_resolves_to_exactly_one_meldung() {
        for &pid in ZUORDNUNGSMELDUNG_PIDS {
            let m = Zuordnungsmeldung::from_pid(pid).expect("catalogued");
            assert_eq!(m.pid(), pid);
        }
        assert!(Zuordnungsmeldung::from_pid(55_001).is_none());
    }

    /// The catalogue and the workflow must agree on which PIDs exist and who
    /// receives them — they are edited in different crates.
    #[test]
    fn the_workflow_covers_the_catalogued_strom_meldepflichten() {
        let catalogued: Vec<u32> = mako_fristen::meldung::GPKE.iter().map(|m| m.pid).collect();
        assert_eq!(catalogued, ZUORDNUNGSMELDUNG_PIDS.to_vec());
    }
}
