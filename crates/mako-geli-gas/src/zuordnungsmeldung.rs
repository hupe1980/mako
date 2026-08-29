//! GeLi Gas Informationsmeldungen — the three one-way notifications the GNB owes
//! around a Lieferbeginn.
//!
//! UTILMD AHB Gas 1.1/1.2 Kap. 5.8 states what they are in one line: „Eine
//! Informationsmeldung ist eine Nachricht, für die keine Antwort vorgesehen
//! ist. Die Meldung selbst wird nicht gegenüber dem NB beantwortet, sondern ist
//! als eine Klärungsaufforderung bzw. Information, dass eine früher erfolgte
//! Zuordnung aufgehoben wird, zu verstehen."
//!
//! | PID | Message | NB → | Prozessschritt | Frist |
//! |---|---|---|---|---|
//! | 44036 | Informationsmeldung über existierende Zuordnung | LFN | Nr. 2 | Ablauf des 4. WT nach Eingang |
//! | 44037 | Informationsmeldung zur Beendigung der Zuordnung | LFA | Nr. 6 | am selben Tag wie die Antwort |
//! | 44038 | Informationsmeldung zur Aufhebung einer zuk. Zuordnung | LFZ | Nr. 7 | am selben Tag wie die Antwort |
//!
//! # Two anchors, not one
//!
//! 44036 runs from the **Eingang der Anmeldung** like its Strom twin. 44037 and
//! 44038 are „am selben Tag wie in Prozessschritt 5, wenn die Anmeldung
//! bestätigt wurde" — anchored on the GNB's own **Antwort**, and owed only on a
//! confirmation. Resolving them against the Eingang gives a different day
//! whenever the GNB uses more than a few hours of its four Werktage.
//! [`mako_fristen::meldung::MeldungAnchor`] names which.
//!
//! # Where Gas differs from Strom
//!
//! | | Strom (`mako_gpke::zuordnungsmeldung`) | Gas |
//! |---|---|---|
//! | `BGM` DE 1001 | `E01` / `E02` | **`E44`** Informationsmeldung, on all three |
//! | `SG5 LOC` DE 3227 | `Z16` MaLo / `Z21` Tranche | **`172`** Meldepunkt, „Verwendung der ID der Marktlokation" (`[583]`) |
//! | `NAD` DE 3055 | `293` BDEW | `9` GS1 / **`332`** DVGW — derived from the MP-ID |
//! | Beendigungsgründe | `ZC8`, `ZD9`, `ZG6` | `ZC8` alone |
//! | Aufhebungsgründe | `ZG5`, `ZG9`, `ZH0`, `ZH1` | `ZG9`, `ZH0`, `ZH1` — no `ZG5` |
//! | Bilanzierungsende | — | `SG4 DTM+159`, Soll „wenn eine Bilanzierung stattfindet" (`[29]`) |
//! | `SG12 NAD+VY` on the Aufhebung | Muss unless `ZG5` | **unconditional** |
//!
//! Gas has no `ZG5`, so its Aufhebung always names an auslösenden Marktpartner:
//! LFA bei `ZG9`, LFN bei `ZH0`, NB bei `ZH1` (Bedingung `[571]`).
//!
//! # Regulatory basis
//!
//! - **BDEW/VKU/GEODE/FNB Gas AWH GeLi Gas V1.2** Kap. 2.5.2 SD Lieferbeginn Nr. 2 / 6 / 7
//! - **UTILMD AHB Gas 1.1 / 1.2** Kap. 5.8
//! - **APERAK AHB 1.1 § 2.3.1** — the Gas APERAK windows, a separate clock

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    error::WorkflowError,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, Workflow, WorkflowOutput},
};

/// `BGM` DE 1001 — `E44` Informationsmeldung. All three Gas Meldungen share it,
/// unlike Strom, which splits them across `E01` and `E02`.
pub const BGM_INFORMATIONSMELDUNG: &str = "E44";

/// `SG5 LOC` DE 3227 — `172` Meldepunkt.
///
/// Gas names one polymorphic Lokation qualifier where Strom names `Z16`/`Z21`;
/// Bedingung `[583]` fixes the content to the **MaLo-ID**.
pub const LOC_MELDEPUNKT: &str = "172";

/// `SG4 DTM` DE 2005 — `93` Datum Vertragsende (44037).
pub const DTM_ENDE_ZUM: &str = "93";
/// `SG4 DTM` DE 2005 — `92` Datum Vertragsbeginn (44038).
pub const DTM_BEGINN_ZUM: &str = "92";
/// `SG4 DTM` DE 2005 — `159` Bilanzierungsende, Soll on 44037 and 44038
/// „wenn eine Bilanzierung stattfindet" (Bedingung `[29]`).
pub const DTM_BILANZIERUNGSENDE: &str = "159";

/// `SG4 STS+7` DE 9013 — the Gründe UTILMD AHB Gas Kap. 5.8 admits.
pub mod grund {
    /// `Z26` — Information über existierende Zuordnung (44036).
    pub const INFO_EXISTIERENDE_ZUORDNUNG: &str = "Z26";
    /// `ZC8` — Beendigung der Zuordnung (44037). The only one Gas defines.
    pub const BEENDIGUNG_ZUORDNUNG: &str = "ZC8";
    /// `ZG9` — Aufhebung wegen Auszug des Kunden (44038); the LFA triggers it.
    pub const AUFHEBUNG_AUSZUG: &str = "ZG9";
    /// `ZH0` — Aufhebung wegen Anmeldung eines anderen Lieferanten zu einem
    /// früheren Termin (44038); the LFN triggers it.
    pub const AUFHEBUNG_FRUEHERE_ANMELDUNG: &str = "ZH0";
    /// `ZH1` — Aufhebung wegen Stilllegung (44038); the NB triggers it.
    pub const AUFHEBUNG_STILLLEGUNG: &str = "ZH1";
}

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "geli-gas-zuordnungsmeldung";

/// PID 44036 — Informationsmeldung über existierende Zuordnung (NB → LFN).
pub const INFORMATION_PID: u32 = 44_036;
/// PID 44037 — Informationsmeldung zur Beendigung der Zuordnung (NB → LFA).
pub const BEENDIGUNG_PID: u32 = 44_037;
/// PID 44038 — Informationsmeldung zur Aufhebung einer zuk. Zuordnung (NB → LFZ).
pub const AUFHEBUNG_PID: u32 = 44_038;

/// Every Gas Informationsmeldung, in Prozessschritt order.
pub const ZUORDNUNGSMELDUNG_PIDS: &[u32] = &[INFORMATION_PID, BEENDIGUNG_PID, AUFHEBUNG_PID];

/// Deadline label for the dispatch window.
pub const MELDUNG_WINDOW_LABEL: &str = "geli-gas-zuordnungsmeldung-frist";

// ── Which message ─────────────────────────────────────────────────────────────

/// One of the three Gas Informationsmeldungen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zuordnungsmeldung {
    /// 44036 — an LFA exists at the Zuordnungsbeginn; addressed to the LFN.
    Information,
    /// 44037 — the LFA's Zuordnung ends; addressed to the LFA.
    Beendigung,
    /// 44038 — a future Zuordnung is cancelled; addressed to the LFZ.
    Aufhebung,
}

impl Zuordnungsmeldung {
    /// Resolve from a Prüfidentifikator, or `None` when it is not one of the three.
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

    /// The `SG4 DTM` DE 2005 qualifier the process date takes, or `None`.
    ///
    /// 44036 carries no SG4 date — the AHB's „Beginn zum" and „Ende zum" columns
    /// are empty for it.
    #[must_use]
    pub const fn dtm_qualifier(self) -> Option<&'static str> {
        match self {
            Self::Information => None,
            Self::Beendigung => Some(DTM_ENDE_ZUM),
            Self::Aufhebung => Some(DTM_BEGINN_ZUM),
        }
    }

    /// The `SG4 STS+7` DE 9013 codes the AHB admits for this message.
    #[must_use]
    pub const fn admissible_gruende(self) -> &'static [&'static str] {
        match self {
            Self::Information => &[grund::INFO_EXISTIERENDE_ZUORDNUNG],
            Self::Beendigung => &[grund::BEENDIGUNG_ZUORDNUNG],
            Self::Aufhebung => &[
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
    /// Unconditional on 44036 (Altlieferant, `[566]`) and 44038 (auslösender
    /// Marktpartner, `[571]`) — Gas has no `ZG5`, the one Strom Grund that
    /// exempts it. Never on 44037.
    #[must_use]
    pub const fn requires_beteiligter(self) -> bool {
        matches!(self, Self::Information | Self::Aufhebung)
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the Gas Informationsmeldung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ZuordnungsmeldungEvent {
    /// The Meldung was rendered and queued for AS4 delivery.
    Gesendet {
        /// 44036, 44037 or 44038.
        pruefidentifikator: Pruefidentifikator,
        /// The Marktlokation (`SG5 LOC+172`, MaLo-ID per `[583]`).
        location_id: MaLo,
        /// The GNB.
        sender: MarktpartnerCode,
        /// LFN (44036), LFA (44037) or LFZ (44038).
        receiver: MarktpartnerCode,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// Zuordnungsende (44037) or ursprünglicher Zuordnungsbeginn (44038).
        #[serde(default)]
        process_date: Option<String>,
        /// `SG4 DTM+159` Bilanzierungsende, when a Bilanzierung takes place.
        #[serde(default)]
        bilanzierungsende: Option<String>,
        /// `SG12 NAD+VY`.
        #[serde(default)]
        beteiligte: Vec<MarktpartnerCode>,
        /// `SG6 RFF+TN` — the Vorgangsnummer of the triggering Anmeldung.
        #[serde(default)]
        referenz_vorgangsnummer: Option<String>,
    },
    /// A Meldung arrived from a Gasnetzbetreiber. Recorded, never answered.
    Empfangen {
        /// 44036, 44037 or 44038.
        pruefidentifikator: Pruefidentifikator,
        /// The Marktlokation.
        location_id: MaLo,
        /// The GNB.
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
            Self::Gesendet { .. } => "GasZuordnungsmeldungGesendet",
            Self::Empfangen { .. } => "GasZuordnungsmeldungEmpfangen",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// State of one Gas Informationsmeldung. There is no „awaiting answer" state,
/// because the AHB says there is no answer.
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
        /// The Marktlokation.
        location_id: MaLo,
        /// The party it went to.
        receiver: MarktpartnerCode,
    },
    /// We received it.
    Empfangen {
        /// The Prüfidentifikator that arrived.
        pruefidentifikator: Pruefidentifikator,
        /// The Marktlokation.
        location_id: MaLo,
        /// The Gasnetzbetreiber that sent it.
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

/// Commands for the Gas Informationsmeldung workflow.
#[derive(Clone)]
pub enum ZuordnungsmeldungCommand {
    /// Render and queue one Informationsmeldung (GNB role).
    Senden {
        /// Which of the three.
        meldung: Zuordnungsmeldung,
        /// The GNB's own MP-ID.
        sender: MarktpartnerCode,
        /// LFN, LFA or LFZ.
        receiver: MarktpartnerCode,
        /// Marktlokations-ID.
        location_id: MaLo,
        /// `SG4 STS+7` DE 9013.
        transaktionsgrund: String,
        /// `YYYYMMDD`. Required on 44037 and 44038, refused on 44036.
        process_date: Option<String>,
        /// `SG4 DTM+159` Bilanzierungsende, `YYYYMMDD`.
        bilanzierungsende: Option<String>,
        /// `SG12 NAD+VY` parties.
        beteiligte: Vec<MarktpartnerCode>,
        /// `SG6 RFF+TN` — the triggering Anmeldung's Vorgangsnummer.
        referenz_vorgangsnummer: Option<String>,
    },
    /// An inbound Informationsmeldung from a Gasnetzbetreiber (LF role).
    Empfangen {
        /// 44036, 44037 or 44038.
        pid: Pruefidentifikator,
        /// The GNB.
        sender: MarktpartnerCode,
        /// Us.
        receiver: MarktpartnerCode,
        /// The Marktlokation.
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

/// The GeLi Gas Informationsmeldung workflow — PIDs 44036 / 44037 / 44038.
#[derive(Debug, Clone, Copy, Default)]
pub struct GeliGasZuordnungsmeldungWorkflow;

impl Workflow for GeliGasZuordnungsmeldungWorkflow {
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
                transaktionsgrund,
                process_date,
                bilanzierungsende,
                beteiligte,
                referenz_vorgangsnummer,
            } => {
                if !matches!(state, ZuordnungsmeldungState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if !meldung.grund_is_admissible(&transaktionsgrund) {
                    return Err(WorkflowError::rejected(format!(
                        "SG4 STS+7 DE 9013 {transaktionsgrund:?} is not admissible on PID {} — \
                         UTILMD AHB Gas Kap. 5.8 lists {:?}",
                        meldung.pid(),
                        meldung.admissible_gruende(),
                    )));
                }
                match (meldung.dtm_qualifier(), process_date.as_deref()) {
                    (Some(q), None | Some("")) => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {} requires a process date (SG4 DTM+{q})",
                            meldung.pid(),
                        )));
                    }
                    (None, Some(_)) => {
                        return Err(WorkflowError::rejected(format!(
                            "PID {} carries no SG4 date — UTILMD AHB Gas Kap. 5.8 leaves both the \
                             „Beginn zum\" and „Ende zum\" columns empty for it",
                            meldung.pid(),
                        )));
                    }
                    _ => {}
                }
                if meldung.requires_beteiligter() && beteiligte.is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} requires SG12 NAD+VY — {}",
                        meldung.pid(),
                        match meldung {
                            Zuordnungsmeldung::Information => "the Altlieferant (Bedingung [566])",
                            _ =>
                                "the auslösender Marktpartner (Bedingung [571]): LFA bei ZG9, \
                                  LFN bei ZH0, NB bei ZH1",
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
                        "PID 44036 requires SG6 RFF+TN — the Vorgangsnummer of the Anmeldung it \
                         refers to (UTILMD AHB Gas Kap. 5.8, „Referenz Vorgangsnummer (aus \
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
                        &transaktionsgrund,
                        process_date.as_deref(),
                        bilanzierungsende.as_deref(),
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
                        bilanzierungsende,
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
                        "expected a Gas Informationsmeldung PID ({ZUORDNUNGSMELDUNG_PIDS:?}), \
                         got {pid}",
                    )));
                }
                // Gas APERAK semantics are the inverse of Strom's: silence means
                // acceptance, so only a failure is acknowledged. Sending an
                // Anerkennung here would put a message on the wire the GeLi Gas
                // AHB does not ask for.
                let mut outbox = Vec::new();
                if !validation_passed {
                    outbox.push(PendingOutbox::new(
                        "APERAK",
                        sender.as_str(),
                        serde_json::json!({
                            "sender":     receiver.as_str(),
                            "receiver":   sender.as_str(),
                            "pid":        29002_u32,
                            "error_code": mako_engine::erc::codes::Z29,
                            "reason":     validation_errors.join("; "),
                        }),
                    ));
                }
                outbox.push(PendingOutbox::new(
                    "ProcessInitiated",
                    receiver.as_str(),
                    serde_json::json!({
                        "pid":               pid.as_u32(),
                        "malo_id":           location_id.as_str(),
                        "grid_operator":     sender.as_str(),
                        "transaktionsgrund": transaktionsgrund,
                    }),
                ));
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

/// The renderer payload for one Gas Informationsmeldung.
#[allow(clippy::too_many_arguments)]
fn meldung_payload(
    meldung: Zuordnungsmeldung,
    sender: &MarktpartnerCode,
    receiver: &MarktpartnerCode,
    location_id: &MaLo,
    transaktionsgrund: &str,
    process_date: Option<&str>,
    bilanzierungsende: Option<&str>,
    beteiligte: &[MarktpartnerCode],
    referenz_vorgangsnummer: Option<&str>,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "direction":         "outbound",
        "pid":               meldung.pid(),
        "sender":            sender.as_str(),
        "receiver":          receiver.as_str(),
        "malo":              location_id.as_str(),
        "document_code":     BGM_INFORMATIONSMELDUNG,
        "transaktionsgrund": transaktionsgrund,
        "lokationstyp":      LOC_MELDEPUNKT,
    });
    let obj = payload.as_object_mut().expect("json! built an object");
    if let Some(date) = process_date {
        obj.insert("process_date".into(), date.into());
    }
    if let Some(date) = bilanzierungsende.filter(|d| !d.is_empty()) {
        obj.insert("bilanzierungsende".into(), date.into());
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
        let (g, date, beteiligte, referenz) = match meldung {
            Zuordnungsmeldung::Information => (
                grund::INFO_EXISTIERENDE_ZUORDNUNG,
                None,
                vec![mp("9871234567897")],
                Some("VG-4711".to_owned()),
            ),
            Zuordnungsmeldung::Beendigung => (
                grund::BEENDIGUNG_ZUORDNUNG,
                Some("20260701".to_owned()),
                vec![],
                None,
            ),
            Zuordnungsmeldung::Aufhebung => (
                grund::AUFHEBUNG_STILLLEGUNG,
                Some("20260801".to_owned()),
                vec![mp("9870123456789")],
                None,
            ),
        };
        ZuordnungsmeldungCommand::Senden {
            meldung,
            sender: mp("9870123456789"),
            receiver: mp("9871234567897"),
            location_id: malo(),
            transaktionsgrund: g.to_owned(),
            process_date: date,
            bilanzierungsende: None,
            beteiligte,
            referenz_vorgangsnummer: referenz,
        }
    }

    fn run(cmd: ZuordnungsmeldungCommand) -> WorkflowOutput<ZuordnungsmeldungEvent> {
        GeliGasZuordnungsmeldungWorkflow::handle(&ZuordnungsmeldungState::New, cmd)
            .expect("command accepted")
    }

    /// All three Gas Meldungen are `BGM+E44`, and all three name a Meldepunkt —
    /// where Strom splits `E01`/`E02` and `Z16`/`Z21`.
    #[test]
    fn every_gas_meldung_is_an_informationsmeldung_at_a_meldepunkt() {
        for meldung in [
            Zuordnungsmeldung::Information,
            Zuordnungsmeldung::Beendigung,
            Zuordnungsmeldung::Aufhebung,
        ] {
            let payload = &run(senden(meldung)).outbox[0].payload;
            assert_eq!(payload["document_code"], "E44");
            assert_eq!(payload["lokationstyp"], "172");
        }
    }

    /// Gas defines no `ZG5`, `ZD9` or `ZG6`: the Strom Gründe are not
    /// interchangeable with the Gas ones even though the PIDs mirror each other.
    #[test]
    fn the_strom_only_gruende_are_refused() {
        for (meldung, code) in [
            (Zuordnungsmeldung::Beendigung, "ZD9"),
            (Zuordnungsmeldung::Beendigung, "ZG6"),
            (Zuordnungsmeldung::Aufhebung, "ZG5"),
        ] {
            assert!(
                !meldung.grund_is_admissible(code),
                "{code} is a Strom-only Grund but PID {} accepted it",
                meldung.pid()
            );
        }
    }

    /// Gas has no `ZG5`, so its Aufhebung always names an auslösenden
    /// Marktpartner — unlike Strom, where `ZG5` exempts it.
    #[test]
    fn the_gas_aufhebung_always_names_a_beteiligten() {
        assert!(Zuordnungsmeldung::Aufhebung.requires_beteiligter());
        let err = GeliGasZuordnungsmeldungWorkflow::handle(
            &ZuordnungsmeldungState::New,
            ZuordnungsmeldungCommand::Senden {
                meldung: Zuordnungsmeldung::Aufhebung,
                sender: mp("9870123456789"),
                receiver: mp("9871234567897"),
                location_id: malo(),
                transaktionsgrund: grund::AUFHEBUNG_AUSZUG.to_owned(),
                process_date: Some("20260801".to_owned()),
                bilanzierungsende: None,
                beteiligte: vec![],
                referenz_vorgangsnummer: None,
            },
        )
        .expect_err("44038 must carry SG12 NAD+VY");
        assert!(format!("{err}").contains("SG12"), "{err}");
    }

    #[test]
    fn the_information_carries_no_sg4_date() {
        assert_eq!(Zuordnungsmeldung::Information.dtm_qualifier(), None);
        assert_eq!(Zuordnungsmeldung::Beendigung.dtm_qualifier(), Some("93"));
        assert_eq!(Zuordnungsmeldung::Aufhebung.dtm_qualifier(), Some("92"));
        assert!(
            run(senden(Zuordnungsmeldung::Information)).outbox[0]
                .payload
                .get("process_date")
                .is_none()
        );
    }

    /// `[29]`: the Bilanzierungsende is Soll „wenn eine Bilanzierung
    /// stattfindet" — a Gas-only slot with no Strom counterpart.
    #[test]
    fn the_bilanzierungsende_rides_when_supplied() {
        let out = run(ZuordnungsmeldungCommand::Senden {
            meldung: Zuordnungsmeldung::Beendigung,
            sender: mp("9870123456789"),
            receiver: mp("9871234567897"),
            location_id: malo(),
            transaktionsgrund: grund::BEENDIGUNG_ZUORDNUNG.to_owned(),
            process_date: Some("20260701".to_owned()),
            bilanzierungsende: Some("20260701".to_owned()),
            beteiligte: vec![],
            referenz_vorgangsnummer: None,
        });
        assert_eq!(out.outbox[0].payload["bilanzierungsende"], "20260701");
    }

    /// Gas APERAK is silence-means-acceptance, so a clean inbound Meldung is
    /// recorded without one — the opposite of the Strom twin.
    #[test]
    fn a_clean_inbound_meldung_is_not_acknowledged() {
        let out = run(ZuordnungsmeldungCommand::Empfangen {
            pid: Pruefidentifikator::new(BEENDIGUNG_PID).expect("valid"),
            sender: mp("9870123456789"),
            receiver: mp("9871234567897"),
            location_id: malo(),
            transaktionsgrund: grund::BEENDIGUNG_ZUORDNUNG.to_owned(),
            message_ref: MessageRef::new("MSG-1".to_owned()),
            validation_passed: true,
            validation_errors: vec![],
        });
        let kinds: Vec<&str> = out.outbox.iter().map(|o| &*o.message_type).collect();
        assert_eq!(kinds, vec!["ProcessInitiated"]);
    }

    #[test]
    fn a_failed_inbound_meldung_is_rejected_with_an_aperak() {
        let out = run(ZuordnungsmeldungCommand::Empfangen {
            pid: Pruefidentifikator::new(INFORMATION_PID).expect("valid"),
            sender: mp("9870123456789"),
            receiver: mp("9871234567897"),
            location_id: malo(),
            transaktionsgrund: grund::INFO_EXISTIERENDE_ZUORDNUNG.to_owned(),
            message_ref: MessageRef::new("MSG-2".to_owned()),
            validation_passed: false,
            validation_errors: vec!["missing SG12".to_owned()],
        });
        assert_eq!(&*out.outbox[0].message_type, "APERAK");
        assert_eq!(out.outbox[0].payload["pid"], 29002);
    }

    /// The catalogue and the workflow are edited in different crates and must
    /// agree on which PIDs exist.
    #[test]
    fn the_workflow_covers_the_catalogued_gas_meldepflichten() {
        let catalogued: Vec<u32> = mako_fristen::meldung::GELI_GAS
            .iter()
            .map(|m| m.pid)
            .collect();
        assert_eq!(catalogued, ZUORDNUNGSMELDUNG_PIDS.to_vec());
    }
}
