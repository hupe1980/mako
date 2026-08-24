//! GPKE Teil 4 Stammdatenänderung — master-data change process family.
//!
//! Keeps a downstream role's master data in sync with the Verantwortlicher's
//! authoritative truth. The whole 55615–55694 / 55109 / 55110 band is **one**
//! abstract use-case replicated across (initiator → receiver) × (master-data
//! object). Every instance is a cascade:
//!
//! 1. **Änderung** — the Verantwortlicher notifies the Berechtigter of changed
//!    master data (`resp = --`, no back-reference). *Sofort nach Kenntnisnahme.*
//! 2. **Rückmeldung** — the Berechtigter's **quality feedback** on the change.
//! 3. **Bearbeitungsstand** — IFTSTA PID 21047 (handled by `gpke-supplier-change`).
//!
//! ## Strom is quality feedback, not accept/reject
//!
//! Unlike a Lieferbeginn Anmeldung, a GPKE Teil 4 Stammdatenänderung is
//! **always applied** — the new values are valid from the Änderungsdatum
//! regardless of the Rückmeldung, and a missing Rückmeldung counts as tacit
//! acceptance (GPKE Teil 4 §1.4.2). The Rückmeldung only reports:
//!
//! - **A01** — „Empfänger übernimmt die Stammdaten ohne Anmerkung".
//! - **A02** — „Empfänger übernimmt die Stammdaten, teilt aber mit, dass sie aus
//!   seiner Sicht nicht korrekt sind" (and returns corrected values).
//!
//! There is no hard Ablehnung in Strom — contrast the Gas twin
//! (`mako-geli-gas::stammdatenaenderung`, genuine Zustimmung/Ablehnung).
//!
//! ## Roles are implied by receipt
//!
//! The inbound dispatcher does not need to know the deployment's role: receiving
//! an **Änderung** PID (e.g. 55616, „Änderung Daten der MaLo NB → LF") means we
//! are the *Berechtigter* — we apply the change and send the paired Rückmeldung
//! (55622). Receiving a **Rückmeldung** PID resumes a change **we** initiated
//! (e.g. an LF that sent 55109 to its NB awaits 55137).
//!
//! ## Master-data objects and Fristen
//!
//! Each family carries one object per PID, identified in the message by the
//! SG5 `LOC` qualifier: MaLo (`Z16`), MeLo (`Z17`), NeLo (`Z18`), SR (`Z19`),
//! TR (`Z20`), Tranche (`Z21`), plus the MaLo Paket-ID.
//!
//! ## Object-generic apply
//!
//! The workflow emits an object-tagged `ProcessCompleted` apply intent whenever
//! the `makod` adapter extracted a non-empty attribute patch — `marktd` then
//! dispatches by the `objekt` marker to the matching typed-column patch
//! (`MaLo` → `malo`, `MeLo` → `melo`, `NeLo` → `nelo`, `Tranche` → `tranche`).
//! The extracted attributes are the grounded generic ones (Netzebene, Regelzone,
//! Bilanzierungsgebiet, Energierichtung, …); the §14a-specific SR/TR columns
//! (`steuerkanal`, `ist_fernschaltbar`, Konfigurationsprodukte) travel in
//! specialized characteristic groups whose extraction is gated on the §14a
//! UTILMD AHB (roadmap), so those objects are acknowledged without a typed
//! apply.
//!
//! Frist: Rückmeldung „unverzüglich, spätester ÜT = **2. Werktag** nach dem
//! Eingang" (GPKE Teil 4 § 1.4.2). Not the Teil-2 Lieferbeginn window either —
//! that one is a clock time on the 1. Werktag, and neither is a 24-hour
//! duration, which no GPKE Festlegung contains.
//!
//! # Regulatory basis
//!
//! - **GPKE Teil 4 (BK6-22-024 Anlage 1d)** §1.4 Stammdatenänderung
//! - **UTILMD AHB Strom 2.1** ch. 3 (object → PID → LOC map)
//! - **EBD 4.2** E_0408/E_0409/E_0410/E_0412/E_0415/… (Rückmeldung A01/A02)
//! - **APERAK AHB 1.0 §2.4.1** — Strom UTILMD 45-min APERAK Frist

use mako_engine::types::Pruefidentifikator;
use mako_engine::{
    deadline::Deadline,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MaLo, MarktpartnerCode, MessageRef},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{
    APERAK_STROM_WINDOW_LABEL, HolidayCalendar, aperak_strom_due_at, deadline_at_werktage,
};

// ── PID model ─────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "gpke-stammdatenaenderung";

/// Deadline label for the 2-Werktage Rückmeldung window (GPKE Teil 4 §1.4.2).
pub const RUECKMELDUNG_WINDOW_LABEL: &str = "gpke-stammdaten-rueckmeldung-window";

/// Master-data object a Stammdatenänderung targets (SG5 `LOC` qualifier).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StammdatenObjekt {
    /// Marktlokation (`LOC+Z16`) — attributes applied to `marktd`.
    Marktlokation,
    /// Messlokation (`LOC+Z17`).
    Messlokation,
    /// Netzlokation (`LOC+Z18`).
    Netzlokation,
    /// Steuerbare Ressource (`LOC+Z19`, §14a).
    SteuerbareRessource,
    /// Technische Ressource (`LOC+Z20`, §14a/Redispatch).
    TechnischeRessource,
    /// Tranche (`LOC+Z21`).
    Tranche,
    /// Paket-ID der Marktlokation (NB-Wechsel bundling).
    PaketId,
}

impl StammdatenObjekt {
    /// SG5 `LOC` qualifier (DE3227).
    #[must_use]
    pub fn loc_qualifier(self) -> &'static str {
        match self {
            Self::Marktlokation => "Z16",
            Self::Messlokation => "Z17",
            Self::Netzlokation => "Z18",
            Self::SteuerbareRessource => "Z19",
            Self::TechnischeRessource => "Z20",
            Self::Tranche => "Z21",
            Self::PaketId => "Z16", // carried on the MaLo
        }
    }

    /// Stable wire label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Marktlokation => "MARKTLOKATION",
            Self::Messlokation => "MESSLOKATION",
            Self::Netzlokation => "NETZLOKATION",
            Self::SteuerbareRessource => "STEUERBARE_RESSOURCE",
            Self::TechnischeRessource => "TECHNISCHE_RESSOURCE",
            Self::Tranche => "TRANCHE",
            Self::PaketId => "PAKET_ID",
        }
    }
}

/// The authoritative `(Änderung PID, Rückmeldung PID, object)` pairs across all
/// GPKE Teil 4 directions (NB↔LF, NB↔MSB, MSB↔LF/wMSB, ÜNB flows). Source of
/// truth for PID routing and for [`rueckmeldung_pid_for`] / [`objekt_of`].
///
/// Direction is implied by receipt (see module docs); one row per
/// (direction × object). ÜNB-directed rows are defined but currently inert
/// ("ÜNB ist für kein Stammdatum berechtigt").
pub const STAMMDATEN_PAIRS: &[(u32, u32, StammdatenObjekt)] = &[
    // S1 — NB → LF
    (55615, 55621, StammdatenObjekt::Netzlokation),
    (55616, 55622, StammdatenObjekt::Marktlokation),
    (55617, 55623, StammdatenObjekt::TechnischeRessource),
    (55618, 55624, StammdatenObjekt::SteuerbareRessource),
    (55619, 55625, StammdatenObjekt::Tranche),
    (55620, 55626, StammdatenObjekt::Messlokation),
    (55691, 55692, StammdatenObjekt::PaketId),
    // S2 — NB → MSB
    (55627, 55633, StammdatenObjekt::Netzlokation),
    (55628, 55634, StammdatenObjekt::Marktlokation),
    (55629, 55635, StammdatenObjekt::TechnischeRessource),
    (55630, 55636, StammdatenObjekt::SteuerbareRessource),
    (55632, 55638, StammdatenObjekt::Messlokation),
    // S3 — NB → ÜNB (inert)
    (55688, 55689, StammdatenObjekt::Marktlokation),
    // S4 — LF → NB
    (55109, 55137, StammdatenObjekt::Marktlokation),
    (55230, 55232, StammdatenObjekt::Netzlokation),
    (55693, 55694, StammdatenObjekt::TechnischeRessource),
    // S5 — LF → MSB
    (55110, 55136, StammdatenObjekt::Marktlokation),
    // S6 — MSB → NB
    //
    // Two rows carry the Marktlokation: 55640 the ordinary Stammdaten and 55557
    // the **MSB-Abrechnungsdaten**. Same object, different data set — the pair
    // table keys on the Änderung PID, so both resolve.
    (55557, 55559, StammdatenObjekt::Marktlokation),
    (55639, 55644, StammdatenObjekt::Netzlokation),
    (55640, 55645, StammdatenObjekt::Marktlokation),
    (55641, 55646, StammdatenObjekt::SteuerbareRessource),
    (55642, 55647, StammdatenObjekt::Tranche),
    (55643, 55648, StammdatenObjekt::Messlokation),
    // S7 — MSB → LF
    (55649, 55654, StammdatenObjekt::Netzlokation),
    (55650, 55655, StammdatenObjekt::Marktlokation),
    (55651, 55656, StammdatenObjekt::SteuerbareRessource),
    (55652, 55657, StammdatenObjekt::Tranche),
    (55653, 55658, StammdatenObjekt::Messlokation),
    // S8 — MSB → weiterer MSB
    (55659, 55664, StammdatenObjekt::Netzlokation),
    (55660, 55665, StammdatenObjekt::Marktlokation),
    (55661, 55666, StammdatenObjekt::SteuerbareRessource),
    (55662, 55667, StammdatenObjekt::Tranche),
    (55663, 55669, StammdatenObjekt::Messlokation),
    // S9 — MSB → ÜNB (inert)
    (55684, 55685, StammdatenObjekt::Marktlokation),
    (55686, 55687, StammdatenObjekt::Tranche),
    // S11 — ÜNB → NB (Bilanzkreistreue, inert)
    (55670, 55671, StammdatenObjekt::Marktlokation),
];

/// Return the object of an **Änderung** PID (the initiate direction).
#[must_use]
pub fn objekt_of(aenderung_pid: u32) -> Option<StammdatenObjekt> {
    STAMMDATEN_PAIRS
        .iter()
        .find(|(a, _, _)| *a == aenderung_pid)
        .map(|(_, _, o)| *o)
}

/// Return the **Rückmeldung** PID an inbound Änderung must be answered with.
#[must_use]
pub fn rueckmeldung_pid_for(aenderung_pid: u32) -> Option<u32> {
    STAMMDATEN_PAIRS
        .iter()
        .find(|(a, _, _)| *a == aenderung_pid)
        .map(|(_, r, _)| *r)
}

/// `true` when `pid` is a Rückmeldung PID (resumes a change we initiated).
#[must_use]
pub fn is_rueckmeldung_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(_, r, _)| *r == pid)
}

/// `true` when `pid` is an Änderung PID (an inbound change to apply + answer).
#[must_use]
pub fn is_aenderung_pid(pid: u32) -> bool {
    STAMMDATEN_PAIRS.iter().any(|(a, _, _)| *a == pid)
}

// ── Quality feedback ───────────────────────────────────────────────────────────

/// The Rückmeldung outcome (EBD E_0408 family) — Strom never rejects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Qualitaet {
    /// A01 — übernommen ohne Anmerkung.
    Uebernommen,
    /// A02 — übernommen, aber aus Empfängersicht nicht korrekt (Korrekturwerte
    /// werden als Qualitätsrückmeldung zurückgegeben).
    UebernommenMitKorrektur,
}

impl Qualitaet {
    /// EBD DE1131 status code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Uebernommen => "A01",
            Self::UebernommenMitKorrektur => "A02",
        }
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the GPKE Stammdatenänderung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum StammdatenEvent {
    /// Inbound Änderung received (we are the Berechtigter).
    AenderungErhalten {
        /// Marktlokation the change concerns (LOC/IDE object id).
        location_id: MaLo,
        /// GLN of the Verantwortlicher (sender).
        sender: MarktpartnerCode,
        /// GLN of the receiving Berechtigter.
        receiver: MarktpartnerCode,
        /// Änderung PID (e.g. 55616).
        pruefidentifikator: Pruefidentifikator,
        /// Which master-data object changed.
        objekt: StammdatenObjekt,
        /// Änderungsdatum (`YYYYMMDD`) — the values are valid from here.
        aenderungsdatum: String,
        /// EDIFACT message reference.
        message_ref: MessageRef,
    },
    /// EDIFACT message passed structural validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// Our Rückmeldung (quality feedback) dispatched to the Verantwortlicher.
    RueckmeldungGesendet {
        /// Rückmeldung PID (e.g. 55622).
        response_pid: Pruefidentifikator,
        /// A01 / A02.
        qualitaet: Qualitaet,
    },
    /// NB/initiator: we dispatched an Änderung (e.g. LF → NB 55109).
    AenderungGesendet {
        /// Marktlokation.
        location_id: MaLo,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter (receiver).
        receiver: MarktpartnerCode,
        /// Änderung PID.
        pruefidentifikator: Pruefidentifikator,
        /// Object.
        objekt: StammdatenObjekt,
        /// Änderungsdatum.
        aenderungsdatum: String,
    },
    /// Initiator: the Berechtigter's Rückmeldung arrived.
    RueckmeldungErhalten {
        /// Rückmeldung PID.
        response_pid: Pruefidentifikator,
        /// A01 / A02.
        qualitaet: Qualitaet,
    },
    /// The Rückmeldung window elapsed with no answer — tacit acceptance
    /// (GPKE Teil 4 §1.4.2).
    StillschweigendAngenommen {
        /// The expired deadline.
        deadline_id: DeadlineId,
    },
    /// Structural validation failed — APERAK 313 dispatched.
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl EventPayload for StammdatenEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AenderungErhalten { .. } => "StammdatenAenderungErhalten",
            Self::ValidationPassed { .. } => "StammdatenValidationPassed",
            Self::RueckmeldungGesendet { .. } => "StammdatenRueckmeldungGesendet",
            Self::AenderungGesendet { .. } => "StammdatenAenderungGesendet",
            Self::RueckmeldungErhalten { .. } => "StammdatenRueckmeldungErhalten",
            Self::StillschweigendAngenommen { .. } => "StammdatenStillschweigendAngenommen",
            Self::Rejected { .. } => "StammdatenRejected",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured when the process starts (either role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StammdatenData {
    /// Marktlokation.
    pub location_id: MaLo,
    /// GLN of the Verantwortlicher.
    pub sender: MarktpartnerCode,
    /// GLN of the Berechtigter.
    pub receiver: MarktpartnerCode,
    /// Änderung PID.
    pub pruefidentifikator: Pruefidentifikator,
    /// Object.
    pub objekt: StammdatenObjekt,
    /// Änderungsdatum (`YYYYMMDD`).
    pub aenderungsdatum: String,
}

/// State of a GPKE Stammdatenänderung process.
///
/// ```text
/// Berechtigter: New → Eingegangen → ValidationPassed → Beantwortet
///                                  ↘ Rejected (structural)
/// Verantwortlicher: New → Gesendet → Abgeschlossen (Rückmeldung / tacit)
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum StammdatenState {
    /// No events yet.
    #[default]
    New,
    /// Berechtigter: inbound Änderung received.
    Eingegangen(StammdatenData),
    /// Berechtigter: validation passed; Rückmeldung not yet sent.
    ValidationPassed(StammdatenData),
    /// Berechtigter: Rückmeldung dispatched (terminal).
    Beantwortet {
        /// Data from the Änderung.
        data: StammdatenData,
        /// Quality reported.
        qualitaet: Qualitaet,
    },
    /// Verantwortlicher: Änderung dispatched; awaiting Rückmeldung.
    Gesendet(StammdatenData),
    /// Verantwortlicher: closed by explicit Rückmeldung or tacit acceptance.
    Abgeschlossen {
        /// Data from the Änderung.
        data: StammdatenData,
        /// Quality reported, or `None` on tacit acceptance.
        qualitaet: Option<Qualitaet>,
    },
    /// Process rejected (structural validation failure).
    Rejected {
        /// Human-readable reason.
        reason: String,
    },
}

impl StammdatenState {
    /// Stable label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::Eingegangen(_) => "Eingegangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Beantwortet { .. } => "Beantwortet",
            Self::Gesendet(_) => "Gesendet",
            Self::Abgeschlossen { .. } => "Abgeschlossen",
            Self::Rejected { .. } => "Rejected",
        }
    }

    /// `true` when terminal.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Beantwortet { .. } | Self::Abgeschlossen { .. } | Self::Rejected { .. }
        )
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the GPKE Stammdatenänderung workflow.
#[derive(Clone)]
pub enum StammdatenCommand {
    /// Berechtigter: inbound UTILMD Änderung received from the AS4 layer.
    ReceiveAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher.
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
        /// The applied MaLo attribute patch (only for MaLo objects; the makod
        /// adapter carries the extracted values, serialized as JSON).
        patch: serde_json::Value,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `true` if structural validation returned no errors.
        validation_passed: bool,
        /// Validation error strings.
        validation_errors: Vec<String>,
        /// Receipt timestamp (drives the APERAK + Rückmeldung deadlines).
        received_at: time::OffsetDateTime,
    },
    /// Berechtigter: send the Rückmeldung (quality feedback).
    ///
    /// Defaults to A01 (übernommen) via the ingest auto-path; an operator may
    /// send A02 with corrected values from the ERP.
    SendRueckmeldung {
        /// A01 / A02.
        qualitaet: Qualitaet,
    },
    /// Verantwortlicher: dispatch an Änderung (ERP-initiated, e.g. LF → NB).
    SendAenderung {
        /// Änderung PID.
        pid: Pruefidentifikator,
        /// GLN of the Verantwortlicher (us).
        sender: MarktpartnerCode,
        /// GLN of the Berechtigter.
        receiver: MarktpartnerCode,
        /// Marktlokation.
        location_id: MaLo,
        /// Änderungsdatum (`YYYYMMDD`).
        aenderungsdatum: String,
    },
    /// Verantwortlicher: the Berechtigter's Rückmeldung arrived.
    ReceiveRueckmeldung {
        /// Rückmeldung PID.
        response_pid: Pruefidentifikator,
        /// A01 / A02.
        qualitaet: Qualitaet,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for StammdatenCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// Build the `ProcessCompleted` outbox payload that drives the marktd apply.
///
/// Emitted for any object carrying a non-empty patch; `marktd` dispatches by the
/// `objekt` marker to the matching typed-column patch (`patch_stammdaten` on the
/// MaLo/MeLo/NeLo/Tranche repository). The `malo_id` key carries the object's own
/// location id (the SG5 `LOC`/IDE object id), which is the MaLo id for MaLo
/// objects and the MeLo/NeLo/Tranche id otherwise.
fn apply_outbox(data: &StammdatenData, patch: &serde_json::Value) -> PendingOutbox {
    PendingOutbox::new(
        "ProcessCompleted",
        "",
        serde_json::json!({
            "pid":             data.pruefidentifikator.as_u32(),
            "malo_id":         data.location_id.as_str(),
            "objekt":          data.objekt.as_str(),
            "aenderungsdatum": data.aenderungsdatum,
            "stammdaten_patch": patch,
        }),
    )
}

/// GPKE Teil 4 Stammdatenänderung workflow (PIDs 55615–55694, 55109/55110).
pub struct GpkeStammdatenaenderungWorkflow;

impl Workflow for GpkeStammdatenaenderungWorkflow {
    type State = StammdatenState;
    type Event = StammdatenEvent;
    type Command = StammdatenCommand;

    fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
        match (deadline.label(), state) {
            // The Rückmeldung window elapsed while we still owe an answer, or a
            // change we sent went unanswered → tacit acceptance / close.
            (
                RUECKMELDUNG_WINDOW_LABEL | APERAK_STROM_WINDOW_LABEL,
                StammdatenState::Eingegangen(_)
                | StammdatenState::ValidationPassed(_)
                | StammdatenState::Gesendet(_),
            ) => Some(StammdatenCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }),
            _ => None,
        }
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        match event {
            StammdatenEvent::AenderungErhalten {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                objekt,
                aenderungsdatum,
                ..
            } => StammdatenState::Eingegangen(StammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                objekt: *objekt,
                aenderungsdatum: aenderungsdatum.clone(),
            }),
            StammdatenEvent::ValidationPassed { .. } => match state {
                StammdatenState::Eingegangen(data) => StammdatenState::ValidationPassed(data),
                other => other,
            },
            StammdatenEvent::RueckmeldungGesendet { qualitaet, .. } => match state {
                StammdatenState::ValidationPassed(data) => StammdatenState::Beantwortet {
                    data,
                    qualitaet: *qualitaet,
                },
                other => other,
            },
            StammdatenEvent::AenderungGesendet {
                location_id,
                sender,
                receiver,
                pruefidentifikator,
                objekt,
                aenderungsdatum,
            } => StammdatenState::Gesendet(StammdatenData {
                location_id: location_id.clone(),
                sender: sender.clone(),
                receiver: receiver.clone(),
                pruefidentifikator: *pruefidentifikator,
                objekt: *objekt,
                aenderungsdatum: aenderungsdatum.clone(),
            }),
            StammdatenEvent::RueckmeldungErhalten { qualitaet, .. } => match state {
                StammdatenState::Gesendet(data) => StammdatenState::Abgeschlossen {
                    data,
                    qualitaet: Some(*qualitaet),
                },
                other => other,
            },
            StammdatenEvent::StillschweigendAngenommen { .. } => match state {
                StammdatenState::Gesendet(data) => StammdatenState::Abgeschlossen {
                    data,
                    qualitaet: None,
                },
                // Berechtigter side: our own Rückmeldung Frist lapsed — record
                // as answered (tacit) so the process closes cleanly.
                StammdatenState::Eingegangen(data) | StammdatenState::ValidationPassed(data) => {
                    StammdatenState::Beantwortet {
                        data,
                        qualitaet: Qualitaet::Uebernommen,
                    }
                }
                other => other,
            },
            StammdatenEvent::Rejected { reason } => StammdatenState::Rejected {
                reason: reason.clone(),
            },
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        match command {
            // ── Berechtigter: inbound Änderung ───────────────────────────────
            StammdatenCommand::ReceiveAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
                patch,
                message_ref,
                validation_passed,
                validation_errors,
                received_at,
            } => {
                if !matches!(state, StammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some(objekt) = objekt_of(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a Stammdatenänderung Änderung PID",
                    )));
                };
                let sender_mp_id = sender.clone();
                let receiver_gln = receiver.clone();

                let mut events = vec![StammdatenEvent::AenderungErhalten {
                    location_id: location_id.clone(),
                    sender,
                    receiver,
                    pruefidentifikator: pid,
                    objekt,
                    aenderungsdatum: aenderungsdatum.clone(),
                    message_ref: message_ref.clone(),
                }];

                if !validation_passed {
                    let reason = if validation_errors.is_empty() {
                        "structural validation failed".to_owned()
                    } else {
                        validation_errors.join("; ")
                    };
                    events.push(StammdatenEvent::Rejected {
                        reason: reason.clone(),
                    });
                    let outbox = vec![
                        PendingOutbox::new(
                            "APERAK",
                            sender_mp_id.as_str(),
                            serde_json::json!({
                                "sender":     receiver_gln.as_str(),
                                "receiver":   sender_mp_id.as_str(),
                                "pid":        29001_u32,
                                "error_code": mako_engine::erc::codes::Z29,
                                "reason":     reason,
                            }),
                        )
                        .caused_by(0),
                    ];
                    return Ok(WorkflowOutput::with_outbox(events, outbox));
                }

                events.push(StammdatenEvent::ValidationPassed { message_ref });

                // APERAK 312 (Anerkennung).
                let mut outbox = vec![
                    PendingOutbox::new(
                        "APERAK",
                        sender_mp_id.as_str(),
                        serde_json::json!({
                            "sender":        receiver_gln.as_str(),
                            "receiver":      sender_mp_id.as_str(),
                            "pid":           29001_u32,
                            "document_code": "312",
                        }),
                    )
                    .caused_by(1),
                ];

                // Apply the extracted attribute change to marktd. Object-generic:
                // any object with a non-empty patch emits an `objekt`-tagged
                // apply intent, and `marktd` routes it to the matching
                // typed-column `patch_stammdaten` (MaLo/MeLo/NeLo/Tranche). SR/TR
                // carry no grounded generic attributes (source-gated), so their
                // patch is empty and they are acknowledged without an apply.
                if patch.as_object().is_some_and(|o| !o.is_empty()) {
                    let data = StammdatenData {
                        location_id,
                        sender: sender_mp_id,
                        receiver: receiver_gln,
                        pruefidentifikator: pid,
                        objekt,
                        aenderungsdatum,
                    };
                    outbox.push(apply_outbox(&data, &patch).caused_by(1));
                }

                let deadlines = vec![
                    PendingDeadline::new(
                        APERAK_STROM_WINDOW_LABEL,
                        aperak_strom_due_at(received_at),
                    ),
                    PendingDeadline::new(
                        RUECKMELDUNG_WINDOW_LABEL,
                        deadline_at_werktage(
                            received_at,
                            mako_fristen::antwort::STAMMDATEN_RUECKMELDUNG_WERKTAGE,
                            HolidayCalendar::BdewMaKo,
                        ),
                    ),
                ];
                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    events, outbox, deadlines,
                ))
            }

            StammdatenCommand::SendRueckmeldung { qualitaet } => {
                let data = match state {
                    StammdatenState::ValidationPassed(d) => d,
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "ValidationPassed",
                            state.label(),
                        ));
                    }
                };
                let response_pid = rueckmeldung_pid_for(data.pruefidentifikator.as_u32())
                    .and_then(|p| Pruefidentifikator::new(p).ok())
                    .ok_or_else(|| {
                        WorkflowError::rejected(format!(
                            "no Rückmeldung PID for Änderung {}",
                            data.pruefidentifikator
                        ))
                    })?;
                let outbox = vec![PendingOutbox::new(
                    "UTILMD",
                    data.sender.as_str(),
                    serde_json::json!({
                        "direction":    "outbound",
                        "pid":          response_pid.as_u32(),
                        "sender":       data.receiver.as_str(),
                        "receiver":     data.sender.as_str(),
                        "malo":         data.location_id.as_str(),
                        "process_date": data.aenderungsdatum,
                        "qualitaet":    qualitaet.code(),
                    }),
                )];
                Ok(WorkflowOutput::with_outbox(
                    vec![StammdatenEvent::RueckmeldungGesendet {
                        response_pid,
                        qualitaet,
                    }],
                    outbox,
                ))
            }

            // ── Verantwortlicher: outbound Änderung ──────────────────────────
            StammdatenCommand::SendAenderung {
                pid,
                sender,
                receiver,
                location_id,
                aenderungsdatum,
            } => {
                if !matches!(state, StammdatenState::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                let Some(objekt) = objekt_of(pid.as_u32()) else {
                    return Err(WorkflowError::rejected(format!(
                        "PID {pid} is not a Stammdatenänderung Änderung PID",
                    )));
                };
                let utilmd = PendingOutbox::new(
                    "UTILMD",
                    receiver.as_str(),
                    serde_json::json!({
                        "direction":    "outbound",
                        "pid":          pid.as_u32(),
                        "sender":       sender.as_str(),
                        "receiver":     receiver.as_str(),
                        "malo":         location_id.as_str(),
                        "process_date": aenderungsdatum,
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![StammdatenEvent::AenderungGesendet {
                        location_id,
                        sender,
                        receiver,
                        pruefidentifikator: pid,
                        objekt,
                        aenderungsdatum,
                    }],
                    vec![utilmd],
                ))
            }

            StammdatenCommand::ReceiveRueckmeldung {
                response_pid,
                qualitaet,
            } => {
                if !matches!(state, StammdatenState::Gesendet(_)) {
                    return Err(WorkflowError::invalid_state("Gesendet", state.label()));
                }
                if !is_rueckmeldung_pid(response_pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {response_pid} is not a Stammdatenänderung Rückmeldung PID",
                    )));
                }
                Ok(vec![StammdatenEvent::RueckmeldungErhalten {
                    response_pid,
                    qualitaet,
                }]
                .into())
            }

            StammdatenCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    Ok(vec![].into())
                } else if label.as_ref() == RUECKMELDUNG_WINDOW_LABEL {
                    Ok(vec![StammdatenEvent::StillschweigendAngenommen { deadline_id }].into())
                } else {
                    // APERAK sending window is monitoring-only; do not close.
                    Ok(vec![].into())
                }
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use mako_engine::{ids::DeadlineId, workflow::Workflow};

    use super::*;

    fn pid(code: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(code).unwrap()
    }
    fn mcod(s: &str) -> MarktpartnerCode {
        MarktpartnerCode::new(s)
    }
    fn malo(s: &str) -> MaLo {
        MaLo::new(s)
    }
    fn mref(s: &str) -> MessageRef {
        MessageRef::new(s)
    }
    fn now() -> time::OffsetDateTime {
        time::macros::datetime!(2026-07-01 10:00:00 UTC)
    }

    fn receive_malo_cmd(patch: serde_json::Value, ok: bool) -> StammdatenCommand {
        StammdatenCommand::ReceiveAenderung {
            pid: pid(55616), // NB → LF, MaLo
            sender: mcod("9900357000004"),
            receiver: mcod("9900357000011"),
            location_id: malo("51238696781"),
            aenderungsdatum: "20260801".to_owned(),
            patch,
            message_ref: mref("SD-001"),
            validation_passed: ok,
            validation_errors: if ok {
                vec![]
            } else {
                vec!["missing mandatory segment".to_owned()]
            },
            received_at: now(),
        }
    }

    fn apply_all(init: StammdatenState, events: &[StammdatenEvent]) -> StammdatenState {
        events
            .iter()
            .fold(init, GpkeStammdatenaenderungWorkflow::apply)
    }

    // ── Classification ─────────────────────────────────────────────────────────

    #[test]
    fn pid_classification_and_pairing() {
        assert_eq!(objekt_of(55616), Some(StammdatenObjekt::Marktlokation));
        assert_eq!(objekt_of(55620), Some(StammdatenObjekt::Messlokation));
        assert_eq!(rueckmeldung_pid_for(55616), Some(55622));
        assert_eq!(rueckmeldung_pid_for(55109), Some(55137));
        assert!(is_aenderung_pid(55616));
        assert!(is_rueckmeldung_pid(55622));
        assert!(!is_rueckmeldung_pid(55616));
        // 55230 „Änderung Blindabr.-Daten der NeLo" (LF → NB) and 55557
        // „Änderung MSB-Abr.-Daten der MaLo" (MSB → NB) are ordinary GPKE
        // Teil 4 Stammdatenänderungen — PID overview 4.0, „Stammdatenänderung
        // vom LF / vom MSB (verantwortlich) ausgehend" Prozessschritte 1/2.
        assert_eq!(rueckmeldung_pid_for(55230), Some(55232));
        assert_eq!(objekt_of(55230), Some(StammdatenObjekt::Netzlokation));
        assert_eq!(rueckmeldung_pid_for(55557), Some(55559));
        assert_eq!(objekt_of(55557), Some(StammdatenObjekt::Marktlokation));
        // The IFTSTA Bearbeitungsstandsmeldung is not a Stammdaten pair.
        assert!(objekt_of(21047).is_none());
    }

    #[test]
    fn pairs_table_has_no_duplicate_pids() {
        let mut seen = std::collections::HashSet::new();
        for (a, r, _) in STAMMDATEN_PAIRS {
            assert!(seen.insert(*a), "duplicate Änderung PID {a}");
            assert!(seen.insert(*r), "duplicate Rückmeldung PID {r}");
        }
    }

    // ── Berechtigter (receive → apply → Rückmeldung) ────────────────────────────

    #[test]
    fn malo_change_applied_and_answered_a01() {
        let patch = serde_json::json!({ "bilanzierungsmethode": "RLM", "netzebene": "NSP" });
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            receive_malo_cmd(patch, true),
        )
        .unwrap();
        // APERAK 312 + ProcessCompleted apply outbox.
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessCompleted");
        assert_eq!(out.outbox[1].payload["objekt"], "MARKTLOKATION");
        assert_eq!(
            out.outbox[1].payload["stammdaten_patch"]["bilanzierungsmethode"],
            "RLM"
        );
        assert_eq!(out.deadlines.len(), 2); // APERAK + 2-WT Rückmeldung
        let state = apply_all(StammdatenState::New, &out.events);
        assert!(matches!(state, StammdatenState::ValidationPassed(_)));

        let out = GpkeStammdatenaenderungWorkflow::handle(
            &state,
            StammdatenCommand::SendRueckmeldung {
                qualitaet: Qualitaet::Uebernommen,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["pid"], 55622);
        assert_eq!(out.outbox[0].payload["qualitaet"], "A01");
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            StammdatenState::Beantwortet {
                qualitaet: Qualitaet::Uebernommen,
                ..
            }
        ));
    }

    #[test]
    fn empty_patch_acknowledged_without_apply_outbox() {
        // A MaLo change carrying no extractable attributes still gets APERAK +
        // Rückmeldung, but no marktd apply.
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            receive_malo_cmd(serde_json::json!({}), true),
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
    }

    #[test]
    fn nelo_change_emits_object_tagged_apply() {
        // NeLo change (55615) with a grounded attribute now emits an
        // `objekt`-tagged apply intent alongside the APERAK — marktd routes it
        // to NeLoRepository::patch_stammdaten.
        let mut cmd = receive_malo_cmd(serde_json::json!({ "netzebene": "NSP" }), true);
        if let StammdatenCommand::ReceiveAenderung { pid: p, .. } = &mut cmd {
            *p = pid(55615); // NeLo
        }
        let out = GpkeStammdatenaenderungWorkflow::handle(&StammdatenState::New, cmd).unwrap();
        assert_eq!(out.outbox.len(), 2);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[1].message_type.as_ref(), "ProcessCompleted");
        assert_eq!(out.outbox[1].payload["objekt"], "NETZLOKATION");
        assert_eq!(
            out.outbox[1].payload["stammdaten_patch"]["netzebene"],
            "NSP"
        );
    }

    #[test]
    fn non_malo_object_without_grounded_attributes_is_acknowledged_only() {
        // An SR change (55618) whose §14a-specific attributes the adapter cannot
        // yet ground carries an empty patch → APERAK only, no apply intent.
        let mut cmd = receive_malo_cmd(serde_json::json!({}), true);
        if let StammdatenCommand::ReceiveAenderung { pid: p, .. } = &mut cmd {
            *p = pid(55618); // Steuerbare Ressource
        }
        let out = GpkeStammdatenaenderungWorkflow::handle(&StammdatenState::New, cmd).unwrap();
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
    }

    #[test]
    fn a02_reports_correction() {
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            receive_malo_cmd(serde_json::json!({ "regelzone": "10YDE-EON------1" }), true),
        )
        .unwrap();
        let state = apply_all(StammdatenState::New, &out.events);
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &state,
            StammdatenCommand::SendRueckmeldung {
                qualitaet: Qualitaet::UebernommenMitKorrektur,
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].payload["qualitaet"], "A02");
    }

    #[test]
    fn validation_failure_rejects_with_aperak_313() {
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            receive_malo_cmd(serde_json::json!({}), false),
        )
        .unwrap();
        assert_eq!(out.outbox.len(), 1);
        assert_eq!(out.outbox[0].message_type.as_ref(), "APERAK");
        assert_eq!(out.outbox[0].payload["error_code"], "Z29");
        let state = apply_all(StammdatenState::New, &out.events);
        assert!(matches!(state, StammdatenState::Rejected { .. }));
    }

    #[test]
    fn rueckmeldung_timeout_is_tacit_acceptance() {
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            receive_malo_cmd(serde_json::json!({ "netzebene": "NSP" }), true),
        )
        .unwrap();
        let state = apply_all(StammdatenState::New, &out.events);
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &state,
            StammdatenCommand::TimeoutExpired {
                deadline_id: DeadlineId::new(),
                label: RUECKMELDUNG_WINDOW_LABEL.into(),
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(
            state,
            StammdatenState::Beantwortet {
                qualitaet: Qualitaet::Uebernommen,
                ..
            }
        ));
    }

    // ── Verantwortlicher (send → receive) ───────────────────────────────────────

    #[test]
    fn initiator_send_and_receive_rueckmeldung() {
        let out = GpkeStammdatenaenderungWorkflow::handle(
            &StammdatenState::New,
            StammdatenCommand::SendAenderung {
                pid: pid(55109), // LF → NB, MaLo
                sender: mcod("9900357000011"),
                receiver: mcod("9900357000004"),
                location_id: malo("51238696781"),
                aenderungsdatum: "20260801".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(out.outbox[0].message_type.as_ref(), "UTILMD");
        assert_eq!(out.outbox[0].payload["pid"], 55109);
        let state = apply_all(StammdatenState::New, &out.events);
        assert!(matches!(state, StammdatenState::Gesendet(_)));

        let out = GpkeStammdatenaenderungWorkflow::handle(
            &state,
            StammdatenCommand::ReceiveRueckmeldung {
                response_pid: pid(55137),
                qualitaet: Qualitaet::Uebernommen,
            },
        )
        .unwrap();
        let state = apply_all(state, &out.events);
        assert!(matches!(state, StammdatenState::Abgeschlossen { .. }));
    }

    #[test]
    fn wrong_pid_rejected() {
        let mut cmd = receive_malo_cmd(serde_json::json!({}), true);
        if let StammdatenCommand::ReceiveAenderung { pid: p, .. } = &mut cmd {
            *p = pid(55001); // Lieferbeginn — not a Stammdaten PID
        }
        assert!(GpkeStammdatenaenderungWorkflow::handle(&StammdatenState::New, cmd).is_err());
    }
}
