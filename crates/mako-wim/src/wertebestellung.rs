//! WiM ESA Wertebestellung — ordering, cancellation and termination of value
//! delivery to an Energieserviceanbieter.
//!
//! Implements **WiM Strom Teil 2 (Anlage 2b zu BK6-22-024), Kapitel 4** —
//! "Anfrage und Übermittlung von Werten durch und an den ESA" — from the MSB
//! side. §34 Abs. 2 S. 2 Nr. 10 MsbG makes serving an ESA a mandatory,
//! non-discriminatory Zusatzleistung, so an MSB deployment must be able to
//! process the Bestellung that authorises delivery and the Abbestellung that
//! stops it.
//!
//! # Message flow
//!
//! ```text
//! ESA ──REQOTE 35003 Anfrage──────────────────────────────────────────▶ MSB
//! ESA ◀─QUOTES 15003 Angebot / Ablehnung──── 5 WT nach ÜT der Anfrage ─ MSB
//! ESA ──ORDERS 17007 Bestellung──────────── bis Ablauf der Bindungsfrist ▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Bestellung ── MSB
//!
//! (before delivery starts)
//! ESA ──ORDCHG 39002 Stornierung──────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19013 / 19014─────────── 2 WT nach ÜT der Stornierung ── MSB
//!
//! (once delivery is running)
//! ESA ──ORDERS 17008 Abbestellung─────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Abbestellung ─ MSB
//! ```
//!
//! # Fristen
//!
//! Every Frist in Kapitel 4 is counted from the **ÜT** — the day the recipient
//! acknowledged the transmission. GPKE Teil 1 defines it as *"Tag des Empfangs
//! der Übertragungsdatei. Dieser Tag ist aus der AS4-Zustellquittung zu
//! entnehmen, die der Empfänger der Übertragungsdatei an den Sender der
//! Übertragungsdatei übermittelt"*, and adds that the day *"nur anwendbar
//! \[ist\], sofern es sich um eine positive Zustellquittung bzw.
//! Response-Nachricht handelt"*.
//!
//! [`Zustellquittung`] therefore carries the acknowledgement explicitly and a
//! negative one cannot start a Frist.
//!
//! | Step | Frist |
//! |---|---|
//! | Angebot / Ablehnung der Anfrage | 5 WT nach ÜT der Anfrage |
//! | Bestellung | bis Ablauf der Bindungsfrist des MSB |
//! | Antwort auf Bestellung | 2 WT nach ÜT der Bestellung |
//! | Antwort auf Stornierung | 2 WT nach ÜT der Stornierung |
//! | Antwort auf Beendigung | 2 WT nach ÜT der Beendigung |

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{HolidayCalendar, deadline_at_werktage};
use time::OffsetDateTime;

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-wertebestellung";

/// REQOTE — "Anfrage von Werten" (ESA → MSB), UC 4.1 Nr. 1.
///
/// ESA-specific. REQOTE AHB 1.1 §4.3 gives the Kommunikation as *ESA an MSB* and
/// labels the `SG1 RFF+Z13` text "35003 Anfrage von Werten für ESA"; the PID
/// overview 4.0 lists it under WiM Strom Teil 2 and nowhere else.
///
/// Do not confuse it with **35002**, which is §4.2 "Anfrage zur Rechnungsabwicklung
/// des Messstellenbetriebs über den LF" — a different process, LF → MSB, in WiM
/// Teil 1. The two never collide: each belongs to one process, so nothing needs
/// a sender-role classifier to tell them apart.
pub const ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(35003);

/// QUOTES — "Angebot zur Anfrage von Werten für ESA" (MSB → ESA), UC 4.1 Nr. 2.
pub const ANGEBOT_PID: Pruefidentifikator = Pruefidentifikator::const_new(15003);

/// ORDERS — "Bestellung von Werten ESA" (ESA → MSB), UC 4.1 Nr. 3.
pub const BESTELLUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(17007);

/// ORDERS — "Abbestellung von Werten ESA" (ESA → MSB), UC 4.3 Nr. 1.
///
/// Distinct from [`BESTELLUNG_PID`]: 17007 orders a delivery, 17008 ends a
/// running one. Both are ORDERS; the Prüfidentifikator in BGM DE 1004 tells
/// them apart.
pub const ABBESTELLUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(17008);

/// ORDCHG — "Stornierung der Bestellung von Werten" (ESA → MSB), UC 4.1 Nr. 5.
pub const STORNIERUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(39002);

/// ORDRSP — "Bestätigung der Ab-/Bestellung von Werten für ESA" (MSB → ESA).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19011);

/// ORDRSP — "Ablehnung der Ab-/Bestellung von Werten für ESA" (MSB → ESA).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19012);

/// ORDRSP — "Bestätigung der Stornierung einer Bestellung für ESA" (MSB → ESA).
pub const STORNO_BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19013);

/// ORDRSP — "Ablehnung der Stornierung einer Bestellung für ESA" (MSB → ESA).
pub const STORNO_ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19014);

/// MSCONS — "Werte nach Typ 2" (MSB → ESA), UC 4.2. The MSB's delivery duty
/// under §60 Abs. 1 MsbG: it transmits the ordered values to the ESA, daily by
/// 09:30. These values are non-authoritative (no billing bearing) and land in
/// the ESA deployment's separate Typ-2 store (`esa_typ2_reads`).
pub const WERTE_UEBERMITTLUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(13027);

/// IFTSTA — "WiM / Umsetzungsstatus (Bestellung WiM)" (MSB → ESA), UC 4.4 Nr. 1.
///
/// The MSB-initiated Beendigung of a running value delivery (§60 MsbG). The
/// termination status travels in SG15 `STS 4405 = 105` ("beendet"). This is the
/// **only** IFTSTA Prüfidentifikator in the ESA Wertebestellung process — every
/// other step is REQOTE/QUOTES/ORDERS/ORDCHG/ORDRSP.
///
/// Authoritative meaning per **IFTSTA AHB 2.0g** (Kap. 6.10 „Bestellung (WiM)",
/// Kommunikation von MSB an ESA). Note: earlier profile metadata mislabelled
/// 21042 as an EnFG Privilegierungsinformation — corrected against the AHB.
pub const BEENDIGUNG_MSB_PID: Pruefidentifikator = Pruefidentifikator::const_new(21042);

/// SG15 `STS 4405` status value carried by [`BEENDIGUNG_MSB_PID`]: „beendet".
pub const STS_BEENDET: &str = "105";

/// Every PID this workflow accepts inbound (ESA → MSB).
pub const INBOUND_PIDS: &[Pruefidentifikator] = &[
    ANFRAGE_PID,
    BESTELLUNG_PID,
    ABBESTELLUNG_PID,
    STORNIERUNG_PID,
];

/// PIDs an **ESA-role** deployment receives inbound (MSB → ESA).
///
/// Disjoint from [`INBOUND_PIDS`], which is the MSB side, so an integrated
/// deployment holding both roles registers both sets without a conflict.
pub const ESA_INBOUND_PIDS: &[Pruefidentifikator] = &[
    ANGEBOT_PID,
    BESTAETIGUNG_PID,
    ABLEHNUNG_PID,
    STORNO_BESTAETIGUNG_PID,
    STORNO_ABLEHNUNG_PID,
    // UC 4.4: the MSB-initiated Beendigung (IFTSTA 21042) arrives at the ESA.
    BEENDIGUNG_MSB_PID,
];

/// Every PID this workflow emits outbound (MSB → ESA).
pub const OUTBOUND_PIDS: &[Pruefidentifikator] = &[
    ANGEBOT_PID,
    BESTAETIGUNG_PID,
    ABLEHNUNG_PID,
    STORNO_BESTAETIGUNG_PID,
    STORNO_ABLEHNUNG_PID,
    // UC 4.4: the MSB emits the Beendigung (IFTSTA 21042) toward the ESA.
    BEENDIGUNG_MSB_PID,
];

// ── Fristen ───────────────────────────────────────────────────────────────────

/// UC 4.1 Nr. 2 — *"spätester ÜT ist der 5. WT nach dem ÜT von Nr. 1"*.
pub const ANGEBOT_FRIST_WT: u32 = 5;

/// UC 4.1 Nr. 4 / Nr. 6 and UC 4.3 Nr. 2 — *"spätester ÜT ist der 2. WT"*.
pub const ANTWORT_FRIST_WT: u32 = 2;

/// Deadline label for the Angebot window (UC 4.1 Nr. 2).
pub const ANGEBOT_WINDOW_LABEL: &str = "wim-wertebestellung-angebot";

/// Deadline label for the Bindungsfrist of the MSB's own Angebot (UC 4.1 Nr. 3).
pub const BINDUNGSFRIST_LABEL: &str = "wim-wertebestellung-bindungsfrist";

/// Deadline label for an outstanding ORDRSP answer (UC 4.1 Nr. 4/6, UC 4.3 Nr. 2).
pub const ANTWORT_WINDOW_LABEL: &str = "wim-wertebestellung-antwort";

/// The AS4 acknowledgement a Frist is counted from.
///
/// GPKE Teil 1 defines the ÜT as the day taken from the AS4-Zustellquittung, and
/// restricts Fristberechnung to a **positive** acknowledgement. A message whose
/// delivery was never positively acknowledged has no ÜT, so no Frist can start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Zustellquittung {
    /// Time the recipient acknowledged the transmission (the ÜZ).
    pub received_at: OffsetDateTime,
    /// `true` for a positive Zustellquittung.
    pub positive: bool,
}

impl Zustellquittung {
    /// A positive acknowledgement at `received_at`.
    #[must_use]
    pub const fn positive(received_at: OffsetDateTime) -> Self {
        Self {
            received_at,
            positive: true,
        }
    }

    /// A negative acknowledgement at `received_at`.
    #[must_use]
    pub const fn negative(received_at: OffsetDateTime) -> Self {
        Self {
            received_at,
            positive: false,
        }
    }

    /// The ÜT-based deadline `werktage` working days out.
    ///
    /// # Errors
    ///
    /// [`WorkflowError::CommandRejected`] when the acknowledgement is negative: GPKE
    /// Teil 1 admits only a positive Zustellquittung for Fristberechnung, and
    /// silently counting from a negative one would produce a deadline the
    /// market partner is not bound by.
    pub fn frist(&self, werktage: u32) -> Result<OffsetDateTime, WorkflowError> {
        if !self.positive {
            return Err(WorkflowError::rejected(
                "Frist cannot start from a negative AS4-Zustellquittung — GPKE Teil 1 \
                 admits only a positive Zustellquittung for Fristberechnung",
            ));
        }
        Ok(deadline_at_werktage(
            self.received_at,
            werktage,
            HolidayCalendar::BdewMaKo,
        ))
    }
}

/// Shared ESA vocabulary. [`Lokationsebene`] lives in [`crate::esa`] so both
/// sides of the handshake — and the Messprodukt catalogue that constrains
/// which level a product may be ordered for — use one type.
pub use crate::esa::{Abonnement, Bestellgegenstand, Lokationsebene};

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the ESA Wertebestellung workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum WertebestellungEvent {
    /// UC 4.1 Nr. 1 — REQOTE Anfrage received from the ESA.
    AnfrageEingegangen {
        /// GLN of the requesting ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB.
        msb: MarktpartnerCode,
        /// Location level the values are requested for.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID depending on `ebene`.
        lokations_id: String,
        /// Messprodukt, Wunschtermin and Abo mode the ESA asked for.
        gegenstand: Box<Bestellgegenstand>,
        /// Belegnummer of the inbound REQOTE — our QUOTES echoes it in `RFF+AAV`.
        message_ref: MessageRef,
        /// AS4 acknowledgement that starts the 5 WT Angebot window.
        quittung: Zustellquittung,
    },
    /// UC 4.1 Nr. 2 — QUOTES Angebot sent to the ESA.
    AngebotAbgegeben {
        /// Reference of the outbound QUOTES.
        message_ref: MessageRef,
        /// End of the MSB's own Bindungsfrist, which bounds the Bestellung.
        bindungsfrist: OffsetDateTime,
    },
    /// UC 4.1 Nr. 2 — the request cannot be served; the process ends.
    AnfrageAbgelehnt {
        /// Reason communicated to the ESA.
        reason: String,
    },
    /// UC 4.1 Nr. 3 — ORDERS Bestellung received.
    BestellungEingegangen {
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `IMD+7081` the order carried — `Z01` Start Abo or `Z03` ohne Abo.
        /// It selects the EBD our ORDRSP must cite and tells a running series
        /// from a single transmission.
        #[serde(default = "default_start_abo")]
        abonnement: Abonnement,
        /// AS4 acknowledgement that starts the 2 WT answer window.
        quittung: Zustellquittung,
    },
    /// UC 4.1 Nr. 4 — Bestellung confirmed; delivery is authorised.
    BestellungBestaetigt {
        /// Reference of the outbound ORDRSP 19011.
        message_ref: MessageRef,
    },
    /// UC 4.1 Nr. 4 — Bestellung rejected; the process ends.
    BestellungAbgelehnt {
        /// Reason communicated to the ESA.
        reason: String,
    },
    /// UC 4.1 Nr. 5 — ORDCHG Stornierung received before delivery began.
    StornierungEingegangen {
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// AS4 acknowledgement that starts the 2 WT answer window.
        quittung: Zustellquittung,
    },
    /// UC 4.1 Nr. 6 — Stornierung accepted.
    StornierungBestaetigt {
        /// Reference of the outbound ORDRSP 19013.
        message_ref: MessageRef,
    },
    /// UC 4.1 Nr. 6 — Stornierung refused; the Bestellung stands.
    StornierungAbgelehnt {
        /// Reason communicated to the ESA.
        reason: String,
    },
    /// UC 4.3 Nr. 1 — ORDERS Abbestellung received while delivery is running.
    AbbestellungEingegangen {
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Date on which delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// AS4 acknowledgement that starts the 2 WT answer window.
        quittung: Zustellquittung,
    },
    /// UC 4.3 Nr. 2 — Abbestellung answered; delivery ends as agreed.
    AbbestellungBestaetigt {
        /// Reference of the outbound ORDRSP 19011.
        message_ref: MessageRef,
    },
    /// UC 4.3 Nr. 2 — Abbestellung refused (ORDRSP 19012); the running
    /// delivery stands.
    ///
    /// `E_0254` publishes four refusals, so this is a normal outcome, not an
    /// error path: most often `A01` („die Bestellung war eine einmalige
    /// Übermittlung — sie ist zu stornieren") or `A02` („die Bestellung ist zu
    /// stornieren", the end date precedes the Abo start).
    AbbestellungAbgelehnt {
        /// Reason communicated to the ESA — the code's Bedeutung, or the
        /// operator's own Erläuterung.
        reason: String,
    },
    /// UC 4.4 Nr. 1 — the MSB itself ends delivery.
    BeendetDurchMsb {
        /// Reference of the outbound notification.
        message_ref: MessageRef,
        /// Date from which delivery stops.
        beendigung_zum: OffsetDateTime,
        /// Trigger (loss of Zuordnung, contract end, technical reason).
        reason: String,
    },
    /// UC 4.2 — a Typ-2 value delivery (MSCONS 13027) was sent to the ESA.
    ///
    /// Emitted once per transmission: the §60 Abs. 1 MsbG delivery duty leaves an
    /// auditable record of each daily Übermittlung. The first one also closes
    /// the Stornierung window (delivery has begun).
    WerteUebermittelt {
        /// Reference of the outbound MSCONS.
        message_ref: MessageRef,
        /// Number of interval values transmitted.
        interval_count: u32,
        /// End of the period the transmitted values cover.
        ///
        /// `E_0254` Prüfschritt 4 refuses a Beendigung dated **before** the
        /// values the MSB has already sent, so it has to remember how far its
        /// own deliveries reach. `None` when the batch carried no readable
        /// period, which is a defect in the caller rather than a date of zero.
        #[serde(default)]
        bis: Option<time::Date>,
    },
    /// First values delivered; the Stornierung window closes (UC 4.3 Vorbedingung).
    LieferungBegonnen,
    /// A regulatory window elapsed without the required answer.
    FristVersaeumt {
        /// Deadline label that fired.
        label: String,
    },
}

impl EventPayload for WertebestellungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageEingegangen { .. } => "WertebestellungAnfrageEingegangen",
            Self::AngebotAbgegeben { .. } => "WertebestellungAngebotAbgegeben",
            Self::AnfrageAbgelehnt { .. } => "WertebestellungAnfrageAbgelehnt",
            Self::BestellungEingegangen { .. } => "WertebestellungBestellungEingegangen",
            Self::BestellungBestaetigt { .. } => "WertebestellungBestellungBestaetigt",
            Self::BestellungAbgelehnt { .. } => "WertebestellungBestellungAbgelehnt",
            Self::StornierungEingegangen { .. } => "WertebestellungStornierungEingegangen",
            Self::StornierungBestaetigt { .. } => "WertebestellungStornierungBestaetigt",
            Self::StornierungAbgelehnt { .. } => "WertebestellungStornierungAbgelehnt",
            Self::AbbestellungEingegangen { .. } => "WertebestellungAbbestellungEingegangen",
            Self::AbbestellungBestaetigt { .. } => "WertebestellungAbbestellungBestaetigt",
            Self::AbbestellungAbgelehnt { .. } => "WertebestellungAbbestellungAbgelehnt",
            Self::WerteUebermittelt { .. } => "WertebestellungWerteUebermittelt",
            Self::LieferungBegonnen => "WertebestellungLieferungBegonnen",
            Self::BeendetDurchMsb { .. } => "WertebestellungBeendetDurchMsb",
            Self::FristVersaeumt { .. } => "WertebestellungFristVersaeumt",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data carried from the Anfrage through the whole process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WertebestellungData {
    /// GLN of the ESA.
    pub esa: MarktpartnerCode,
    /// GLN of the MSB.
    pub msb: MarktpartnerCode,
    /// Location level requested.
    pub ebene: Lokationsebene,
    /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID.
    pub lokations_id: String,
    /// What the ESA asked for. Without it a confirmed Bestellung would not
    /// tell the MSB which Messprodukt to deliver, at what cadence, or whether
    /// it is a running subscription or a single transmission.
    pub gegenstand: Box<Bestellgegenstand>,
    /// Belegnummer of the inbound REQOTE 35003. Our QUOTES must echo it in
    /// `RFF+AAV` (Zuordnungsschlüssel `ZG-T16`).
    #[serde(default)]
    pub anfrage_ref: Option<String>,
    /// Belegnummer of our own QUOTES 15003 Angebot. The ESA's ORDERS 17007
    /// echoes it in `RFF+AAG`, which is how the order is matched to the offer.
    #[serde(default)]
    pub angebot_ref: Option<String>,
    /// Belegnummer of the inbound ORDERS 17007 Bestellung. Our ORDRSP echoes
    /// it in `RFF+ON` (`ZG-T14`) and the UC-4.4 IFTSTA in `RFF+AGI` (`ZG-T47`).
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    /// Belegnummer of the ORDERS the answer currently owed refers to — the
    /// 17007 or the 17008, whichever arrived last. `RFF+ON` on the ORDRSP.
    #[serde(default)]
    pub inbound_order_ref: Option<String>,
    /// Belegnummer of the inbound ORDCHG 39002. Our ORDRSP 19013/19014 echoes
    /// it in `RFF+ACW` (`ZG-T50`).
    #[serde(default)]
    pub stornierung_ref: Option<String>,
    /// `IMD+7081` of the answer currently owed. `Z01`/`Z03` for a Bestellung,
    /// `Z02` for an Abbestellung — it selects the EBD the ORDRSP must cite.
    #[serde(default)]
    pub offene_antwort_abo: Option<Abonnement>,
    /// End of the Bindungsfrist this MSB stated in its own Angebot.
    ///
    /// Held here rather than only in the `AngebotAbgegeben` variant because
    /// `E_0256` Prüfschritt 1 asks about it at **Bestellung** time, one state
    /// later — and a Prüfschritt whose input was dropped in transit escalates
    /// to an operator on every single order.
    #[serde(default)]
    pub bindungsfrist: Option<OffsetDateTime>,
    /// `DTM+203` of the confirmed Bestellung — when the Abo starts.
    ///
    /// `E_0254` Prüfschritt 2 compares the requested Beendigungsdatum against
    /// it. It is not the Bindungsfrist (that is when the *offer* expires) and
    /// not the Abbestellung's own Ausführungsdatum (that *is* the requested
    /// end, so comparing the two can only ever refuse).
    #[serde(default)]
    pub abo_beginn: Option<time::Date>,
    /// End of the period the most recent delivery covered (`E_0254`
    /// Prüfschritt 4).
    #[serde(default)]
    pub juengste_lieferung: Option<time::Date>,
    /// Date the delivery was already ended, if it was (`E_0254` Prüfschritt 3).
    #[serde(default)]
    pub bereits_beendet_zum: Option<time::Date>,
    /// `true` once the first values have gone out, which closes the UC 4.1
    /// Nr. 5 Stornierung window (`E_0257` Prüfschritte 3/4).
    ///
    /// A fact about the subscription, so it lives with the rest of them rather
    /// than inside whichever state variant happens to be current.
    #[serde(default)]
    pub lieferung_begonnen: bool,
}

/// State of an ESA Wertebestellung process.
///
/// ```text
/// New
///  └─▶ AnfrageEingegangen ──┬─▶ AngebotAbgegeben ──▶ BestellungEingegangen
///                           │                          ├─▶ BestellungBestaetigt ─┐
///                           │                          └─▶ Abgelehnt             │
///                           └─▶ Abgelehnt                                        │
///                                                                                │
///   ┌────────────────────────────────────────────────────────────────────────────┘
///   ├─▶ StornierungEingegangen ─▶ Storniert          (delivery not yet started)
///   ├─▶ AbbestellungEingegangen ─▶ Beendet           (delivery running)
///   └─▶ Beendet                                       (UC 4.4, MSB-initiated)
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum WertebestellungState {
    /// No events yet.
    #[default]
    New,
    /// UC 4.1 Nr. 1 done; the MSB owes an Angebot or Ablehnung within 5 WT.
    AnfrageEingegangen(Box<WertebestellungData>),
    /// UC 4.1 Nr. 2 done; the ESA may order until the Bindungsfrist lapses.
    AngebotAbgegeben {
        /// Process data.
        data: Box<WertebestellungData>,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
    },
    /// UC 4.1 Nr. 3 done; the MSB owes an ORDRSP within 2 WT.
    BestellungEingegangen(Box<WertebestellungData>),
    /// UC 4.1 Nr. 4 confirmed — delivery is authorised and may be running.
    BestellungBestaetigt {
        /// Process data.
        data: Box<WertebestellungData>,
        /// `true` once the first values have gone out, which closes the
        /// Stornierung window per UC 4.3 Vorbedingung.
        lieferung_begonnen: bool,
    },
    /// UC 4.1 Nr. 5 done; the MSB owes an ORDRSP 19013/19014 within 2 WT.
    StornierungEingegangen(Box<WertebestellungData>),
    /// UC 4.3 Nr. 1 done; the MSB owes an ORDRSP 19011/19012 within 2 WT.
    AbbestellungEingegangen {
        /// Process data.
        data: Box<WertebestellungData>,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
    },
    /// Bestellung cancelled before delivery began.
    Storniert(Box<WertebestellungData>),
    /// Delivery ended, by ESA (UC 4.3) or by MSB (UC 4.4).
    Beendet {
        /// Process data.
        data: Box<WertebestellungData>,
        /// `true` when the MSB ended it (UC 4.4).
        durch_msb: bool,
    },
    /// Terminal rejection.
    Abgelehnt {
        /// Reason.
        reason: String,
    },
}

impl WertebestellungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageEingegangen(_) => "AnfrageEingegangen",
            Self::AngebotAbgegeben { .. } => "AngebotAbgegeben",
            Self::BestellungEingegangen(_) => "BestellungEingegangen",
            Self::BestellungBestaetigt { .. } => "BestellungBestaetigt",
            Self::StornierungEingegangen(_) => "StornierungEingegangen",
            Self::AbbestellungEingegangen { .. } => "AbbestellungEingegangen",
            Self::Storniert(_) => "Storniert",
            Self::Beendet { .. } => "Beendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// `true` when the MSB is authorised to deliver values to the ESA.
    ///
    /// The Übermittlung use-case (UC 4.2) has this as its Vorbedingung, so a
    /// delivery path should gate on it rather than on the presence of a
    /// Bestellung alone.
    #[must_use]
    pub const fn lieferung_erlaubt(&self) -> bool {
        matches!(
            self,
            Self::BestellungBestaetigt { .. } | Self::AbbestellungEingegangen { .. }
        )
    }

    /// Process data, when the process has advanced past `New`.
    #[must_use]
    pub const fn data(&self) -> Option<&WertebestellungData> {
        match self {
            Self::AnfrageEingegangen(d)
            | Self::BestellungEingegangen(d)
            | Self::StornierungEingegangen(d)
            | Self::Storniert(d) => Some(d),
            Self::AngebotAbgegeben { data, .. }
            | Self::BestellungBestaetigt { data, .. }
            | Self::AbbestellungEingegangen { data, .. }
            | Self::Beendet { data, .. } => Some(data),
            Self::New | Self::Abgelehnt { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the ESA Wertebestellung workflow.
///
/// Not `Eq`: [`Bestellgegenstand`] carries no `Eq` because the SMGW thresholds
/// are decimal strings, and comparing orders for exact equality is never what
/// the domain wants.
#[derive(Debug, Clone, PartialEq)]
pub enum WertebestellungCommand {
    /// UC 4.1 Nr. 1 — inbound REQOTE 35003.
    ReceiveAnfrage {
        /// Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// GLN of the ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB.
        msb: MarktpartnerCode,
        /// Location level.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID.
        lokations_id: String,
        /// Messprodukt, Wunschtermin and Abo mode extracted from the REQOTE.
        gegenstand: Box<Bestellgegenstand>,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// AS4 acknowledgement of the inbound message.
        quittung: Zustellquittung,
        /// Consent-registry gate, checked at the makod ingest boundary. `Some`
        /// carries the Begründung of a blocked delivery (revoked consent or an
        /// unestablished framework agreement) — the Anfrage is answered with a
        /// QUOTES 15003 Ablehnung instead of proceeding. `None` allows it (an
        /// active consent, self-assertion, or no gate configured).
        consent_block: Option<String>,
    },
    /// UC 4.1 Nr. 2 — send QUOTES 15003 with a Bindungsfrist.
    SendAngebot {
        /// Belegnummer of the outbound QUOTES — the ESA's ORDERS echoes it in
        /// `RFF+AAG`.
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
        /// `DTM+469` — earliest start we can deliver from. Defaults to the
        /// ESA's Wunschtermin when the MSB can meet it.
        fruehester_start: Option<OffsetDateTime>,
        /// The commercial terms — currency, per-Artikel-ID prices and the OBIS
        /// registers the subscription will deliver.
        ///
        /// `SG4 CUX`, the `SG27 PIA+Z02` Artikel-IDs, the `SG31 PRI+CAL` prices
        /// and one to 23 `PIA+5 …:SRW` OBIS-Kennzahlen are all **Muss** on the
        /// 15003 (QUOTES AHB 1.1a §4.3). UC 4.1.1 has the ESA asking for „die
        /// Übermittlung von Werten **und die damit verbundenen Kosten**", so an
        /// offer that prices nothing is not an offer — and pricing is exactly
        /// what distinguishes the Angebot from the Ablehnung, since `DTM+273`
        /// is Muss on both.
        angebot: Box<crate::esa::Angebot>,
    },
    /// UC 4.1 Nr. 2 — refuse the Anfrage; the process ends.
    RejectAnfrage {
        /// Reason communicated to the ESA.
        reason: String,
    },
    /// UC 4.1 Nr. 3 — inbound ORDERS 17007 ordering delivery.
    ReceiveBestellung {
        /// Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// `IMD+7081` the order carried (`Z01` Start Abo / `Z03` ohne Abo).
        abonnement: Abonnement,
        /// AS4 acknowledgement of the inbound message.
        quittung: Zustellquittung,
        /// Consent-registry gate re-checked at ingest — consent can be revoked
        /// between the Angebot and the Bestellung. `Some` carries the Begründung
        /// of a blocked order, answered with an ORDRSP 19012 Ablehnung; `None`
        /// allows it.
        consent_block: Option<String>,
    },
    /// UC 4.1 Nr. 4 — answer the Bestellung (ORDRSP 19011/19012).
    ///
    /// The answer is a **published Antwortcode**, not a boolean: `SG2 AJT` is
    /// Muss on this PID and conditions `[17]`/`[18]` of ORDRSP AHB 1.1b §4.15
    /// require the code to sit in the tree's Zustimmungs- resp.
    /// Ablehnungs-Cluster. The cluster therefore selects the PID — deriving it
    /// from a separate `accept` flag lets the two disagree.
    ///
    /// Run [`mako_pruefung::esa::wertebestellung::pruefe_bestellung`] to obtain the code;
    /// the tree is `E_0256` for a Bestellung and `E_0254` for an Abbestellung,
    /// selected here by the `IMD+7081` the order carried.
    AnswerBestellung {
        /// `AJT` DE 4465 — a code published by the tree the order's Abo mode
        /// selects (`A11` accepts, `A01`/`A04`–`A10` refuse in `E_0256`).
        antwort_code: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
        /// Written Erläuterung. Required for a code whose Codeliste entry sets
        /// `braucht_bemerkung`, and useful on any refusal.
        reason: Option<String>,
    },
    /// UC 4.1 Nr. 5 — inbound ORDCHG 39002 cancelling a confirmed Bestellung.
    ReceiveStornierung {
        /// Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// AS4 acknowledgement of the inbound message.
        quittung: Zustellquittung,
    },
    /// UC 4.1 Nr. 6 — answer the Stornierung (ORDRSP 19013/19014).
    ///
    /// Code from `E_0257` ([`mako_pruefung::esa::wertebestellung::pruefe_stornierung`]):
    /// `A04` confirms, `A01`–`A03` refuse. Note that a started delivery is
    /// refused with **different codes** for the two Abo modes.
    AnswerStornierung {
        /// `AJT` DE 4465 — a code published by `E_0257`.
        antwort_code: String,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
        /// Written Erläuterung.
        reason: Option<String>,
    },
    /// UC 4.3 Nr. 1 — inbound ORDERS 17007 terminating a running delivery.
    ReceiveAbbestellung {
        /// Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// EDIFACT message reference.
        message_ref: MessageRef,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// AS4 acknowledgement of the inbound message.
        quittung: Zustellquittung,
    },
    /// UC 4.3 Nr. 2 — answer the Abbestellung (ORDRSP 19011/19012).
    ///
    /// Code from `E_0254` ([`mako_pruefung::esa::wertebestellung::pruefe_beendigung`]):
    /// `A05` confirms, `A01`–`A04` refuse. A refusal is not merely allowed but
    /// required in four cases — most importantly when the order was a one-shot
    /// (`A01`), which is stornierbar rather than abbestellbar.
    AnswerAbbestellung {
        /// `AJT` DE 4465 — a code published by `E_0254`.
        antwort_code: String,
        /// Reference of the outbound ORDRSP 19011/19012.
        message_ref: MessageRef,
        /// Written Erläuterung.
        reason: Option<String>,
    },
    /// UC 4.2 — deliver Typ-2 values to the ESA (outbound MSCONS 13027).
    ///
    /// Admissible only once delivery is authorised ([`WertebestellungState::lieferung_erlaubt`]),
    /// which is the §60 Abs. 1 MsbG guard: the MSB must hold a confirmed
    /// Bestellung before it may transmit — so it cannot accept an order it
    /// cannot fulfil, nor deliver without one.
    LiefereWerte {
        /// Reference of the outbound MSCONS.
        message_ref: MessageRef,
        /// Interval values to transmit — a JSON array of
        /// `{ dtm_from, dtm_to, quantity_kwh, obis_code, ersatzwert? }`. Passed
        /// through verbatim into the MSCONS render intent; the workflow only
        /// gates and addresses it.
        reads: serde_json::Value,
    },
    /// Mark the first values as delivered, closing the Stornierung window.
    ///
    /// UC 4.3 Vorbedingung: an Abbestellung presupposes that *"eine Stornierung
    /// der Bestellung ist nicht mehr möglich"*.
    MarkLieferungBegonnen,
    /// UC 4.4 Nr. 1 — the MSB ends delivery on its own initiative.
    BeendenDurchMsb {
        /// Reference of the outbound notification.
        message_ref: MessageRef,
        /// Date from which delivery stops.
        beendigung_zum: OffsetDateTime,
        /// Trigger (loss of Zuordnung, contract end, technical reason).
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

impl CommandPayload for WertebestellungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// ESA Wertebestellung workflow (WiM Strom Teil 2, Kapitel 4).
pub struct WimWertebestellungWorkflow;

/// Default for a persisted `BestellungEingegangen` written before `IMD+7081`
/// was modelled: those were all Abo starts, since nothing else was renderable.
const fn default_start_abo() -> Abonnement {
    Abonnement::StartAbo
}

/// Format a datetime as `CCYYMMDD` for a DTM segment value (format code 102).
fn ccyymmdd(dt: OffsetDateTime) -> String {
    format!("{:04}{:02}{:02}", dt.year(), u8::from(dt.month()), dt.day())
}

fn require_pid(
    pid: Pruefidentifikator,
    expected: Pruefidentifikator,
    what: &str,
) -> Result<(), WorkflowError> {
    if pid == expected {
        Ok(())
    } else {
        Err(WorkflowError::rejected(format!(
            "{what} expects PID {expected}, got {pid}"
        )))
    }
}

impl Workflow for WimWertebestellungWorkflow {
    type State = WertebestellungState;
    type Event = WertebestellungEvent;
    type Command = WertebestellungCommand;

    /// Turn a fired deadline into the [`WertebestellungCommand::TimeoutExpired`]
    /// that `handle` already knows how to decide.
    ///
    /// Without this hook the three windows this workflow registers —
    /// [`ANGEBOT_WINDOW_LABEL`], [`ANTWORT_WINDOW_LABEL`] and
    /// [`BINDUNGSFRIST_LABEL`] — fired into the engine's default `None` and the
    /// process sat in `AnfrageEingegangen` forever. `handle` decides which of
    /// them is a Fristversäumnis and which merely ends an offer; this only has
    /// to make sure the three reach it and nothing else does.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        _state: &Self::State,
    ) -> Option<Self::Command> {
        let owned = matches!(
            deadline.label(),
            ANGEBOT_WINDOW_LABEL | ANTWORT_WINDOW_LABEL | BINDUNGSFRIST_LABEL
        );
        // `handle` already checks whether the window was actually outstanding in
        // the current state and returns no events when it was not, so the state
        // filter belongs there and not here.
        owned.then(|| WertebestellungCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        use WertebestellungEvent as E;
        use WertebestellungState as S;
        match event {
            E::AnfrageEingegangen {
                esa,
                msb,
                ebene,
                lokations_id,
                gegenstand,
                message_ref,
                ..
            } => S::AnfrageEingegangen(Box::new(WertebestellungData {
                esa: esa.clone(),
                msb: msb.clone(),
                ebene: *ebene,
                lokations_id: lokations_id.clone(),
                gegenstand: gegenstand.clone(),
                anfrage_ref: Some(message_ref.as_str().to_owned()),
                angebot_ref: None,
                bestellung_ref: None,
                inbound_order_ref: None,
                stornierung_ref: None,
                offene_antwort_abo: None,
                bindungsfrist: None,
                abo_beginn: None,
                juengste_lieferung: None,
                bereits_beendet_zum: None,
                lieferung_begonnen: false,
            })),
            E::AngebotAbgegeben {
                bindungsfrist,
                message_ref,
            } => match state {
                S::AnfrageEingegangen(mut data) => {
                    // The ESA's ORDERS 17007 will echo this in `RFF+AAG`.
                    data.angebot_ref = Some(message_ref.as_str().to_owned());
                    // `E_0256` Prüfschritt 1 asks about this one state later.
                    data.bindungsfrist = Some(*bindungsfrist);
                    S::AngebotAbgegeben {
                        data,
                        bindungsfrist: *bindungsfrist,
                    }
                }
                other => other,
            },
            E::AnfrageAbgelehnt { reason } | E::BestellungAbgelehnt { reason } => S::Abgelehnt {
                reason: reason.clone(),
            },
            E::BestellungEingegangen {
                message_ref,
                abonnement,
                ..
            } => match state {
                S::AngebotAbgegeben { mut data, .. } => {
                    let r = message_ref.as_str().to_owned();
                    data.bestellung_ref = Some(r.clone());
                    data.inbound_order_ref = Some(r);
                    data.gegenstand.abonnement = *abonnement;
                    data.offene_antwort_abo = Some(*abonnement);
                    S::BestellungEingegangen(data)
                }
                other => other,
            },
            E::BestellungBestaetigt { .. } => match state {
                S::BestellungEingegangen(mut data) => {
                    // `DTM+203` of the order is when the Abo starts — the date
                    // `E_0254` Prüfschritt 2 compares a later Beendigung with.
                    data.abo_beginn = Some(data.gegenstand.wunschtermin);
                    S::BestellungBestaetigt {
                        data,
                        lieferung_begonnen: false,
                    }
                }
                other => other,
            },
            E::StornierungEingegangen { message_ref, .. } => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    // The Storno-Antwort references the *ORDCHG* in `RFF+ACW`
                    // (`ZG-T50`), not the ORDERS — a separate slot so the
                    // ORDERS reference survives for a later Abbestellung.
                    data.stornierung_ref = Some(message_ref.as_str().to_owned());
                    S::StornierungEingegangen(data)
                }
                other => other,
            },
            E::StornierungBestaetigt { .. } => match state {
                S::StornierungEingegangen(data) => S::Storniert(data),
                other => other,
            },
            // A refused Stornierung leaves the Bestellung exactly as it was.
            // `lieferung_begonnen` rides in the data and is never re-asserted:
            // asserting `false` here claimed the delivery had not started, on a
            // path taken precisely when `E_0257` said it had (`A02`/`A03`).
            E::StornierungAbgelehnt { .. } => match state {
                S::StornierungEingegangen(data) => {
                    let lieferung_begonnen = data.lieferung_begonnen;
                    S::BestellungBestaetigt {
                        data,
                        lieferung_begonnen,
                    }
                }
                other => other,
            },
            E::AbbestellungEingegangen {
                beendigung_zum,
                message_ref,
                ..
            } => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    data.inbound_order_ref = Some(message_ref.as_str().to_owned());
                    data.offene_antwort_abo = Some(Abonnement::EndeAbo);
                    S::AbbestellungEingegangen {
                        data,
                        beendigung_zum: *beendigung_zum,
                    }
                }
                other => other,
            },
            E::AbbestellungBestaetigt { .. } => match state {
                S::AbbestellungEingegangen {
                    mut data,
                    beendigung_zum,
                } => {
                    // `E_0254` Prüfschritt 3 refuses a second Beendigung for
                    // the same or an earlier date.
                    data.bereits_beendet_zum = Some(beendigung_zum.date());
                    S::Beendet {
                        data,
                        durch_msb: false,
                    }
                }
                other => other,
            },
            // A refused Beendigung leaves the running delivery in place; the
            // ESA has to act on the `E_0254` code it was given.
            E::AbbestellungAbgelehnt { .. } => match state {
                S::AbbestellungEingegangen { mut data, .. } => {
                    data.offene_antwort_abo = None;
                    let lieferung_begonnen = data.lieferung_begonnen;
                    S::BestellungBestaetigt {
                        data,
                        lieferung_begonnen,
                    }
                }
                other => other,
            },
            // A delivery closes the Stornierung window (first values have gone
            // out). Recorded from every state that still holds an authorised
            // order, since values may land while a Storno or an Abbestellung
            // is in flight — which is exactly the case `E_0257` `A02` and
            // `E_0254` `A04` exist for.
            E::LieferungBegonnen => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    data.lieferung_begonnen = true;
                    S::BestellungBestaetigt {
                        data,
                        lieferung_begonnen: true,
                    }
                }
                S::StornierungEingegangen(mut data) => {
                    data.lieferung_begonnen = true;
                    S::StornierungEingegangen(data)
                }
                S::AbbestellungEingegangen {
                    mut data,
                    beendigung_zum,
                } => {
                    data.lieferung_begonnen = true;
                    S::AbbestellungEingegangen {
                        data,
                        beendigung_zum,
                    }
                }
                other => other,
            },
            E::WerteUebermittelt { bis, .. } => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    data.lieferung_begonnen = true;
                    // `E_0254` Prüfschritt 4 refuses a Beendigung dated before
                    // the values already sent, so the MSB has to remember how
                    // far its own deliveries reach.
                    data.juengste_lieferung = data.juengste_lieferung.max(*bis);
                    S::BestellungBestaetigt {
                        data,
                        lieferung_begonnen: true,
                    }
                }
                S::StornierungEingegangen(mut data) => {
                    data.lieferung_begonnen = true;
                    data.juengste_lieferung = data.juengste_lieferung.max(*bis);
                    S::StornierungEingegangen(data)
                }
                S::AbbestellungEingegangen {
                    mut data,
                    beendigung_zum,
                } => {
                    data.lieferung_begonnen = true;
                    data.juengste_lieferung = data.juengste_lieferung.max(*bis);
                    S::AbbestellungEingegangen {
                        data,
                        beendigung_zum,
                    }
                }
                other => other,
            },
            E::BeendetDurchMsb { beendigung_zum, .. } => match state {
                S::BestellungBestaetigt { mut data, .. }
                | S::AbbestellungEingegangen { mut data, .. } => {
                    data.bereits_beendet_zum = Some(beendigung_zum.date());
                    S::Beendet {
                        data,
                        durch_msb: true,
                    }
                }
                other => other,
            },
            // A missed Frist is recorded for supervision; it does not by itself
            // change the obligation, which stays outstanding until answered.
            E::FristVersaeumt { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // Build the outbound render intent that answers the ESA on the wire.
        // The renderer turns this into QUOTES/ORDRSP with the PID in BGM DE 1004.
        // Build the ORDRSP answer. The Belegnummer it echoes is chosen by the
        // PID's published Zuordnungsschlüssel, not by "the last thing we saw":
        //
        // - 19011/19012 answer an **ORDERS** and echo it in `RFF+ON` (`ZG-T14`)
        // - 19013/19014 answer an **ORDCHG** and echo it in `RFF+ACW` (`ZG-T50`)
        //
        // Both also carry `IMD+7081` and an `AJT` naming the EBD the
        // Prüfschritt code belongs to — Muss on all four (ORDRSP AHB 1.1b
        // §4.15). An ORDRSP carries **no LOC**, so no location travels here.
        /// The `de.mako.process.initiated` notification an inbound ESA step
        /// owes its observers.
        ///
        /// WiM Teil 2 Kapitel 4 obliges the **MSB** to answer four inbound
        /// PIDs — 35003 within 5 Werktage, 17007/17008/39002 within 2 — and
        /// §34 Abs. 2 S. 2 Nr. 10 MsbG makes serving an ESA a mandatory
        /// Zusatzleistung, so none of them is optional. This notification is
        /// the entire input to `processd`'s ESA module: the four
        /// `mako-pruefung` walks, the operator queue and its Fristen.
        ///
        /// The payload is the contract `processd::esa_module::EsaOrderPayload`
        /// parses, and every field it reads is a Prüfschritt input. An omitted
        /// one does not fail — it escalates, so a decision that could have been
        /// answered lands on an operator's desk instead.
        fn esa_process_initiated(
            pid: Pruefidentifikator,
            data: &WertebestellungData,
            beendigung_zum: Option<OffsetDateTime>,
        ) -> PendingOutbox {
            PendingOutbox::new(
                "ProcessInitiated",
                data.msb.as_str(),
                serde_json::json!({
                    "pid": pid.as_u32(),
                    "malo_id": data.lokations_id,
                    "lokations_id": data.lokations_id,
                    "esa_mp_id": data.esa.as_str(),
                    "msb_mp_id": data.msb.as_str(),
                    "ebene": data.ebene,
                    // Half of the subscription's business key, and what
                    // `E_0252` Prüfschritt 1 and `E_0256` Prüfschritte 4/5 ask
                    // about.
                    "messprodukt": data.gegenstand.messprodukt,
                    // `IMD+7081` — both termination trees branch on it with
                    // different codes per side, so it is never defaulted.
                    "abonnement": data.gegenstand.abonnement.imd_code(),
                    // `DTM+203` of the message that just arrived: the delivery
                    // start on a Bestellung, the stop date on an Abbestellung.
                    "ausfuehrungsdatum": beendigung_zum
                        .map(|d| d.date())
                        .unwrap_or(data.gegenstand.wunschtermin)
                        .to_string(),
                    // `E_0256` Prüfschritt 1.
                    "bindungsfrist": data.bindungsfrist.and_then(|b| {
                        b.format(&time::format_description::well_known::Rfc3339).ok()
                    }),
                    // `E_0257` Prüfschritte 3/4 and `E_0254` Prüfschritt 4.
                    "lieferung_begonnen": data.lieferung_begonnen,
                    // `E_0254` Prüfschritte 2/3/4 — the three dates that walk
                    // cannot be run without.
                    "abo_beginn": data.abo_beginn.map(|d| d.to_string()),
                    "bereits_beendet_zum": data.bereits_beendet_zum.map(|d| d.to_string()),
                    "juengste_lieferung": data.juengste_lieferung.map(|d| d.to_string()),
                }),
            )
        }

        fn esa_answer(
            message_type: &'static str,
            pid: Pruefidentifikator,
            data: &WertebestellungData,
            message_ref: &MessageRef,
            antwort: Option<(&'static str, &mako_pruefung::codes::AntwortCode)>,
            reason: Option<&str>,
        ) -> PendingOutbox {
            let ist_storno_antwort = pid == STORNO_BESTAETIGUNG_PID || pid == STORNO_ABLEHNUNG_PID;
            let korrelation_ref = if ist_storno_antwort {
                data.stornierung_ref.clone()
            } else {
                data.inbound_order_ref.clone()
            };
            let abo = data
                .offene_antwort_abo
                .unwrap_or(data.gegenstand.abonnement);
            PendingOutbox::new(
                message_type,
                data.esa.as_str(),
                serde_json::json!({
                    "pid": pid,
                    "sender": data.msb.as_str(),
                    "receiver": data.esa.as_str(),
                    "message_ref": message_ref.as_str(),
                    "korrelation_ref": korrelation_ref,
                    "abonnement": abo.imd_code(),
                    // `SG2 AJT` — DE 4465 the Prüfschritt code, DE 1082 the
                    // tree that publishes it. Muss on all four answer PIDs.
                    "antwort_code": antwort.map(|(_, c)| c.code),
                    "antwort_ebd": antwort.map(|(t, _)| t),
                    "reason": reason,
                    "messprodukt": data.gegenstand.messprodukt,
                }),
            )
        }

        /// Resolve an `AJT` code against the tree that publishes it, and let
        /// its **Cluster** pick the answer PID.
        ///
        /// This is the rule ORDRSP AHB 1.1b §4.15 conditions `[17]`/`[18]` state.
        /// A code off the Zustimmung/Ablehnung axis, or one the tree does not
        /// publish, is refused rather than guessed at.
        fn resolve_antwort(
            tree: &'static str,
            antwort_code: &str,
            bestaetigung_pid: Pruefidentifikator,
            ablehnung_pid: Pruefidentifikator,
        ) -> Result<
            (
                Pruefidentifikator,
                &'static mako_pruefung::codes::AntwortCode,
                bool,
            ),
            WorkflowError,
        > {
            let code = mako_pruefung::codes::lookup(tree, antwort_code).ok_or_else(|| {
                WorkflowError::rejected(format!(
                    "Antwortcode {antwort_code:?} ist in {tree} nicht veröffentlicht"
                ))
            })?;
            let zustimmung = code.ist_zustimmung().ok_or_else(|| {
                WorkflowError::rejected(format!(
                    "{} liegt nicht auf der Zustimmungs-/Ablehnungsachse von {tree}",
                    code.code
                ))
            })?;
            Ok((
                if zustimmung {
                    bestaetigung_pid
                } else {
                    ablehnung_pid
                },
                code,
                zustimmung,
            ))
        }

        use WertebestellungCommand as C;
        use WertebestellungEvent as E;
        use WertebestellungState as S;

        match command {
            C::ReceiveAnfrage {
                pid,
                esa,
                msb,
                ebene,
                lokations_id,
                gegenstand,
                message_ref,
                quittung,
                consent_block,
            } => {
                if !matches!(state, S::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                require_pid(pid, ANFRAGE_PID, "Anfrage von Werten")?;
                if lokations_id.trim().is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "Anfrage auf Ebene {} ohne Lokations-ID",
                        ebene.as_str()
                    )));
                }
                // Consent gate (checked at ingest): a revoked consent or an
                // unestablished framework agreement blocks the Anfrage. Answer
                // with a QUOTES 15003 Ablehnung built from the command's parties
                // (state is still New, so there is no `data` to draw from yet).
                if let Some(reason) = consent_block {
                    let outbox = PendingOutbox::new(
                        "QUOTES",
                        esa.as_str(),
                        serde_json::json!({
                            "pid": ANGEBOT_PID,
                            "sender": msb.as_str(),
                            "receiver": esa.as_str(),
                            "message_ref": message_ref.as_str(),
                            "location": lokations_id,
                            // QUOTES AHB 1.1a §4.3: `RFF+AAV` echoes the
                            // REQOTE's Belegnummer — the Angebot's published
                            // Zuordnungsschlüssel, and equally the Ablehnung's.
                            "korrelation_ref": message_ref.as_str(),
                            "reason": reason.clone(),
                        }),
                    );
                    return Ok(WorkflowOutput::with_outbox(
                        vec![E::AnfrageAbgelehnt { reason }],
                        vec![outbox],
                    ));
                }
                // The Messprodukt must be orderable at the level the Anfrage
                // addresses. UC 4.1.1 lists a mis-addressed request among the
                // Fehlerfälle, and answering it with an Angebot would commit
                // the MSB to a delivery it cannot make.
                if let Err(e) = gegenstand.validate(ebene) {
                    let outbox = PendingOutbox::new(
                        "QUOTES",
                        esa.as_str(),
                        serde_json::json!({
                            "pid": ANGEBOT_PID,
                            "sender": msb.as_str(),
                            "receiver": esa.as_str(),
                            "message_ref": message_ref.as_str(),
                            "location": lokations_id,
                            "korrelation_ref": message_ref.as_str(),
                            "reason": e.to_string(),
                        }),
                    );
                    return Ok(WorkflowOutput::with_outbox(
                        vec![E::AnfrageAbgelehnt {
                            reason: e.to_string(),
                        }],
                        vec![outbox],
                    ));
                }
                let due = quittung.frist(ANGEBOT_FRIST_WT)?;
                // The MSB owes an answer within 5 Werktage and `E_0252` decides
                // what it is; nothing else in the platform learns that an
                // Anfrage arrived.
                let initiated = esa_process_initiated(
                    pid,
                    &WertebestellungData {
                        esa: esa.clone(),
                        msb: msb.clone(),
                        ebene,
                        lokations_id: lokations_id.clone(),
                        gegenstand: gegenstand.clone(),
                        anfrage_ref: Some(message_ref.as_str().to_owned()),
                        angebot_ref: None,
                        bestellung_ref: None,
                        inbound_order_ref: None,
                        stornierung_ref: None,
                        offene_antwort_abo: None,
                        bindungsfrist: None,
                        abo_beginn: None,
                        juengste_lieferung: None,
                        bereits_beendet_zum: None,
                        lieferung_begonnen: false,
                    },
                    None,
                );
                Ok(WorkflowOutput {
                    events: vec![E::AnfrageEingegangen {
                        esa,
                        msb,
                        ebene,
                        lokations_id,
                        gegenstand,
                        message_ref,
                        quittung,
                    }],
                    outbox: vec![initiated],
                    deadlines: vec![PendingDeadline::new(ANGEBOT_WINDOW_LABEL, due)],
                })
            }

            C::SendAngebot {
                message_ref,
                bindungsfrist,
                fruehester_start,
                angebot,
            } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::AnfrageEingegangen(_)))
                else {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageEingegangen",
                        state.label(),
                    ));
                };
                // The Angebot carries its Bindungsfrist on the wire (`DTM+273`)
                // so the ESA reads the real offer-validity end rather than a
                // synthesised default — and so an Angebot is distinguishable
                // from an Anfrage-Ablehnung (which carries none).
                //
                // `DTM+273` is a **duration**, not a date (QUOTES AHB 1.1a
                // §4.3: DE 2380 „Zeitraum“, DE 2379 ∈ {802 Monat, 803 Woche,
                // 804 Tag}). The workflow holds the absolute end it will
                // enforce and hands the renderer the day count to the wire.
                let bindungsfrist_tage = (bindungsfrist - OffsetDateTime::now_utc())
                    .whole_days()
                    .max(1);
                let start = fruehester_start
                    .unwrap_or_else(|| data.gegenstand.wunschtermin.midnight().assume_utc());
                // An Angebot has to price something: `SG31 PRI` is Muss inside
                // the `SG27 LIN` position block, and a 15003 that prices nothing
                // is the Ablehnung — which `RejectAnfrage` sends instead.
                if angebot.ist_leer() {
                    return Err(WorkflowError::rejected(
                        "Angebot ohne Preisangabe — SG31 PRI und die OBIS-Kennzahlen sind Muss \
                         auf der QUOTES 15003 (QUOTES AHB 1.1a §4.3); eine Anfrage ohne Angebot \
                         wird mit RejectAnfrage abgelehnt",
                    ));
                }
                let artikel_ids: Vec<&str> = {
                    let mut ids: Vec<&str> = Vec::new();
                    for p in &angebot.preise {
                        if !ids.contains(&p.artikel_id.as_str()) {
                            ids.push(p.artikel_id.as_str());
                        }
                    }
                    ids
                };
                let preise: Vec<serde_json::Value> = angebot
                    .preise
                    .iter()
                    .map(|p| {
                        serde_json::json!({
                            "betrag": p.betrag,
                            "art": p.preistyp.pri_code(),
                            "einheit": p.einheit,
                        })
                    })
                    .collect();
                let outbox = PendingOutbox::new(
                    "QUOTES",
                    data.esa.as_str(),
                    serde_json::json!({
                        "pid": ANGEBOT_PID,
                        "sender": data.msb.as_str(),
                        "receiver": data.esa.as_str(),
                        "location": data.lokations_id,
                        "message_ref": message_ref.as_str(),
                        // `RFF+AAV` — the REQOTE this answers (`ZG-T16`).
                        "korrelation_ref": data.anfrage_ref,
                        "bindungsfrist_tage": bindungsfrist_tage,
                        "fruehester_start": ccyymmdd(start),
                        "messprodukt": data.gegenstand.messprodukt,
                        "currency": angebot.waehrung.as_deref().unwrap_or("EUR"),
                        "artikel_ids": artikel_ids,
                        "obis": angebot.obis_kennzahlen,
                        "preise": preise,
                    }),
                );
                Ok(WorkflowOutput {
                    events: vec![E::AngebotAbgegeben {
                        message_ref,
                        bindungsfrist,
                    }],
                    outbox: vec![outbox],
                    // UC 4.1 Nr. 3 bounds the Bestellung by the MSB's own
                    // Bindungsfrist rather than by a fixed Werktage count.
                    deadlines: vec![PendingDeadline::new(BINDUNGSFRIST_LABEL, bindungsfrist)],
                })
            }

            C::RejectAnfrage { reason } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::AnfrageEingegangen(_)))
                else {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageEingegangen",
                        state.label(),
                    ));
                };
                // Ablehnung der Anfrage is answered with QUOTES 15003 (the
                // renderer derives the message reference from the event id).
                // Ablehnung der Anfrage: QUOTES 15003 with the reason (FTX) and
                // *no* Bindungsfrist — its absence is what tells the ESA this is
                // a rejection, not an Angebot.
                let outbox = PendingOutbox::new(
                    "QUOTES",
                    data.esa.as_str(),
                    serde_json::json!({
                        "pid": ANGEBOT_PID,
                        "sender": data.msb.as_str(),
                        "receiver": data.esa.as_str(),
                        "location": data.lokations_id,
                        "korrelation_ref": data.anfrage_ref,
                        "reason": reason.clone(),
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![E::AnfrageAbgelehnt { reason }],
                    vec![outbox],
                ))
            }

            C::ReceiveBestellung {
                pid,
                message_ref,
                abonnement,
                quittung,
                consent_block,
            } => {
                let S::AngebotAbgegeben { bindungsfrist, .. } = state else {
                    return Err(WorkflowError::invalid_state(
                        "AngebotAbgegeben",
                        state.label(),
                    ));
                };
                require_pid(pid, BESTELLUNG_PID, "Bestellung von Werten")?;
                // UC 4.1 Nr. 3: "spätestens bis zum Ablauf der Bindungsfrist".
                if quittung.received_at > *bindungsfrist {
                    return Err(WorkflowError::rejected(format!(
                        "Bestellung ging am {} ein, die Bindungsfrist des Angebots endete am {}",
                        quittung.received_at, bindungsfrist
                    )));
                }
                // Consent can be revoked between the Angebot and the Bestellung:
                // re-gate at ingest and answer a blocked order with an ORDRSP
                // 19012 Ablehnung (the state carries the parties for the wire).
                if let Some(reason) = consent_block {
                    let data = state.data().ok_or_else(|| {
                        WorkflowError::invalid_state("AngebotAbgegeben", state.label())
                    })?;
                    // `E_0256` A08 is exactly this case: „Der Anschlussnutzer
                    // hat gegenüber dem ESA seine Einwilligung widerrufen oder
                    // ihre Gültigkeit ist abgelaufen" (Prüfschritt 8).
                    let tree = crate::esa::EBD_ESA_BESTELLUNG;
                    let code = mako_pruefung::codes::lookup(tree, "A08")
                        .expect("A08 is published in E_0256");
                    let outbox = esa_answer(
                        "ORDRSP",
                        ABLEHNUNG_PID,
                        data,
                        &message_ref,
                        Some((tree, code)),
                        Some(reason.as_str()),
                    );
                    return Ok(WorkflowOutput::with_outbox(
                        vec![E::BestellungAbgelehnt { reason }],
                        vec![outbox],
                    ));
                }
                let due = quittung.frist(ANTWORT_FRIST_WT)?;
                // `E_0256` runs on this. The Abo mode arrives with the order
                // rather than with the Anfrage, so it is stated here.
                let mut fuer_meldung = state
                    .data()
                    .cloned()
                    .ok_or_else(|| WorkflowError::invalid_state("AngebotAbgegeben", "New"))?;
                fuer_meldung.gegenstand.abonnement = abonnement;
                let initiated = esa_process_initiated(pid, &fuer_meldung, None);
                Ok(WorkflowOutput {
                    events: vec![E::BestellungEingegangen {
                        message_ref,
                        abonnement,
                        quittung,
                    }],
                    outbox: vec![initiated],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerBestellung {
                antwort_code,
                message_ref,
                reason,
            } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::BestellungEingegangen(_)))
                else {
                    return Err(WorkflowError::invalid_state(
                        "BestellungEingegangen",
                        state.label(),
                    ));
                };
                // The tree is chosen by the `IMD+7081` the order carried, since
                // 19011/19012 answer both the Bestellung (`E_0256`) and the
                // Beendigung (`E_0254`).
                let tree = data
                    .offene_antwort_abo
                    .unwrap_or(data.gegenstand.abonnement)
                    .antwort_ebd();
                let (pid, code, zustimmung) =
                    resolve_antwort(tree, &antwort_code, BESTAETIGUNG_PID, ABLEHNUNG_PID)?;
                // UC 4.1 Nr. 4: "informiert der MSB den ESA über die Gründe".
                if !zustimmung && reason.is_none() && code.braucht_bemerkung {
                    return Err(WorkflowError::rejected(format!(
                        "{tree} {} ({}) verlangt eine schriftliche Erläuterung",
                        code.code, code.bedeutung
                    )));
                }
                let outbox = esa_answer(
                    "ORDRSP",
                    pid,
                    data,
                    &message_ref,
                    Some((tree, code)),
                    reason.as_deref(),
                );
                let event = if zustimmung {
                    E::BestellungBestaetigt { message_ref }
                } else {
                    E::BestellungAbgelehnt {
                        reason: reason.unwrap_or_else(|| code.bedeutung.to_owned()),
                    }
                };
                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            C::ReceiveStornierung {
                pid,
                message_ref,
                quittung,
            } => {
                let S::BestellungBestaetigt {
                    lieferung_begonnen, ..
                } = state
                else {
                    return Err(WorkflowError::invalid_state(
                        "BestellungBestaetigt",
                        state.label(),
                    ));
                };
                require_pid(pid, STORNIERUNG_PID, "Stornierung einer Bestellung")?;
                // UC 4.1 Nr. 5 admits a Stornierung only while the einmalige
                // Übermittlung has not happened, or the turnusmäßige has not
                // begun. Once values have gone out the ESA must use the
                // Abbestellung (UC 4.3) instead.
                if *lieferung_begonnen {
                    return Err(WorkflowError::rejected(
                        "Stornierung nicht mehr möglich — die Übermittlung von Werten hat \
                         bereits begonnen; die Beendigung erfolgt über die Abbestellung \
                         (WiM Teil 2, UC 4.3)",
                    ));
                }
                let due = quittung.frist(ANTWORT_FRIST_WT)?;
                let initiated = esa_process_initiated(
                    pid,
                    state.data().ok_or_else(|| {
                        WorkflowError::invalid_state("BestellungBestaetigt", "New")
                    })?,
                    None,
                );
                Ok(WorkflowOutput {
                    events: vec![E::StornierungEingegangen {
                        message_ref,
                        quittung,
                    }],
                    outbox: vec![initiated],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerStornierung {
                antwort_code,
                message_ref,
                reason,
            } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::StornierungEingegangen(_)))
                else {
                    return Err(WorkflowError::invalid_state(
                        "StornierungEingegangen",
                        state.label(),
                    ));
                };
                let tree = crate::esa::EBD_ESA_STORNIERUNG;
                let (pid, code, zustimmung) = resolve_antwort(
                    tree,
                    &antwort_code,
                    STORNO_BESTAETIGUNG_PID,
                    STORNO_ABLEHNUNG_PID,
                )?;
                let outbox = esa_answer(
                    "ORDRSP",
                    pid,
                    data,
                    &message_ref,
                    Some((tree, code)),
                    reason.as_deref(),
                );
                let event = if zustimmung {
                    E::StornierungBestaetigt { message_ref }
                } else {
                    E::StornierungAbgelehnt {
                        reason: reason.unwrap_or_else(|| code.bedeutung.to_owned()),
                    }
                };
                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            C::ReceiveAbbestellung {
                pid,
                message_ref,
                beendigung_zum,
                quittung,
            } => {
                if !matches!(state, S::BestellungBestaetigt { .. }) {
                    return Err(WorkflowError::invalid_state(
                        "BestellungBestaetigt",
                        state.label(),
                    ));
                }
                require_pid(pid, ABBESTELLUNG_PID, "Abbestellung von Werten")?;
                let due = quittung.frist(ANTWORT_FRIST_WT)?;
                // `E_0254` compares `beendigung_zum` against the Abo start and
                // the values already delivered; both ride in the data.
                let mut fuer_meldung = state
                    .data()
                    .cloned()
                    .ok_or_else(|| WorkflowError::invalid_state("BestellungBestaetigt", "New"))?;
                fuer_meldung.gegenstand.abonnement = Abonnement::EndeAbo;
                let initiated = esa_process_initiated(pid, &fuer_meldung, Some(beendigung_zum));
                Ok(WorkflowOutput {
                    events: vec![E::AbbestellungEingegangen {
                        message_ref,
                        beendigung_zum,
                        quittung,
                    }],
                    outbox: vec![initiated],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerAbbestellung {
                antwort_code,
                message_ref,
                reason,
            } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::AbbestellungEingegangen { .. }))
                else {
                    return Err(WorkflowError::invalid_state(
                        "AbbestellungEingegangen",
                        state.label(),
                    ));
                };
                // `E_0254` publishes four refusals, so a Beendigung is not
                // always confirmable — most notably `A01`, which says the order
                // was a one-shot and must be storniert instead.
                let tree = crate::esa::EBD_ESA_BEENDIGUNG;
                let (pid, code, zustimmung) =
                    resolve_antwort(tree, &antwort_code, BESTAETIGUNG_PID, ABLEHNUNG_PID)?;
                let outbox = esa_answer(
                    "ORDRSP",
                    pid,
                    data,
                    &message_ref,
                    Some((tree, code)),
                    reason.as_deref(),
                );
                let event = if zustimmung {
                    E::AbbestellungBestaetigt { message_ref }
                } else {
                    // A refused Abbestellung leaves the delivery running — the
                    // ESA must act on the code (storniere, or re-date).
                    E::AbbestellungAbgelehnt {
                        reason: reason.unwrap_or_else(|| code.bedeutung.to_owned()),
                    }
                };
                Ok(WorkflowOutput::with_outbox(vec![event], vec![outbox]))
            }

            C::LiefereWerte { message_ref, reads } => {
                // §60 Abs. 1 MsbG delivery duty: the MSB may transmit only once
                // it holds a confirmed Bestellung. This gate is what stops it
                // accepting an order it cannot fulfil, and stops delivery
                // without one.
                let Some(data) = state.data().filter(|_| state.lieferung_erlaubt()) else {
                    return Err(WorkflowError::invalid_state(
                        "BestellungBestaetigt|AbbestellungEingegangen",
                        state.label(),
                    ));
                };
                let intervals = reads.as_array().filter(|a| !a.is_empty()).ok_or_else(|| {
                    WorkflowError::rejected(
                        "Werteübermittlung ohne Intervallwerte — reads muss ein nicht-leeres \
                         Array sein",
                    )
                })?;
                let interval_count = u32::try_from(intervals.len()).unwrap_or(u32::MAX);
                // How far this batch reaches. `E_0254` Prüfschritt 4 refuses a
                // Beendigung dated before it, so the answer to that Prüfschritt
                // is assembled here rather than guessed later.
                let bis = intervals
                    .iter()
                    .filter_map(|iv| {
                        let raw = iv.get("dtm_to").or_else(|| iv.get("bis"))?.as_str()?;
                        // Both `CCYYMMDD…` from the wire and ISO from the API.
                        let digits: String =
                            raw.chars().filter(char::is_ascii_digit).take(8).collect();
                        (digits.len() == 8).then_some(())?;
                        time::Date::from_calendar_date(
                            digits[0..4].parse().ok()?,
                            time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?,
                            digits[6..8].parse().ok()?,
                        )
                        .ok()
                    })
                    .max();
                // Outbound MSCONS 13027 addressed to the ESA (NAD+MR = ESA).
                let outbox = PendingOutbox::new(
                    "MSCONS",
                    data.esa.as_str(),
                    serde_json::json!({
                        "pid": WERTE_UEBERMITTLUNG_PID,
                        "sender_mp_id": data.msb.as_str(),
                        "receiver_mp_id": data.esa.as_str(),
                        "malo_id": data.lokations_id,
                        "message_ref": message_ref.as_str(),
                        // `SG1 RFF+AGI` — hint `[574]`: the Belegnummer of the
                        // ORDERS that ordered the values. It is what lets the
                        // ESA tie a delivery to the subscription that
                        // authorised it, since a MaLo may carry several.
                        "korrelation_ref": data.bestellung_ref,
                        "messprodukt": data.gegenstand.messprodukt,
                        "reads": reads,
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![E::WerteUebermittelt {
                        message_ref,
                        interval_count,
                        bis,
                    }],
                    vec![outbox],
                ))
            }

            C::MarkLieferungBegonnen => match state {
                // Idempotent: the delivery path may report this per batch.
                S::BestellungBestaetigt {
                    lieferung_begonnen: true,
                    ..
                } => Ok(Vec::new().into()),
                S::BestellungBestaetigt { .. } => Ok(vec![E::LieferungBegonnen].into()),
                other => Err(WorkflowError::invalid_state(
                    "BestellungBestaetigt",
                    other.label(),
                )),
            },

            C::BeendenDurchMsb {
                message_ref,
                beendigung_zum,
                reason,
            } => {
                if !state.lieferung_erlaubt() {
                    return Err(WorkflowError::invalid_state(
                        "BestellungBestaetigt",
                        state.label(),
                    ));
                }
                // UC 4.4: notify the ESA on the wire via IFTSTA 21042
                // (WiM Umsetzungsstatus, STS 4405 = 105 „beendet").
                let data = state
                    .data()
                    .expect("lieferung_erlaubt() implies process data is present");
                let outbox = PendingOutbox::new(
                    "IFTSTA",
                    data.esa.as_str(),
                    serde_json::json!({
                        "pid": BEENDIGUNG_MSB_PID,
                        "sender": data.msb.as_str(),
                        "receiver": data.esa.as_str(),
                        "message_ref": message_ref.as_str(),
                        "sts_code": STS_BEENDET,
                        // `SG15 RFF+AGI` — „Aus ORDERS BGM DE1004" (`ZG-T47`),
                        // i.e. the Bestellung, which is what the ESA indexed
                        // its process under. The IFTSTA carries no LOC.
                        "korrelation_ref": data.bestellung_ref,
                        "beendigung_zum": beendigung_zum,
                        "reason": reason,
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![E::BeendetDurchMsb {
                        message_ref,
                        beendigung_zum,
                        reason,
                    }],
                    vec![outbox],
                ))
            }

            C::TimeoutExpired { label, .. } => {
                let outstanding = matches!(
                    (state, label.as_ref()),
                    (S::AnfrageEingegangen(_), ANGEBOT_WINDOW_LABEL)
                        | (S::BestellungEingegangen(_), ANTWORT_WINDOW_LABEL)
                        | (S::StornierungEingegangen(_), ANTWORT_WINDOW_LABEL)
                        | (S::AbbestellungEingegangen { .. }, ANTWORT_WINDOW_LABEL)
                );
                if outstanding {
                    return Ok(vec![E::FristVersaeumt {
                        label: label.to_string(),
                    }]
                    .into());
                }
                // The Bindungsfrist lapsing without a Bestellung simply ends the
                // offer; that is not a Fristversäumnis by either party.
                Ok(Vec::new().into())
            }
        }
    }
}
