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
//! ESA ──REQOTE 35002 Anfrage──────────────────────────────────────────▶ MSB
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
    fristen::{HolidayCalendar, deadline_at_werktage},
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// ── PID set ───────────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-wertebestellung";

/// REQOTE — Anfrage von Werten (ESA → MSB), UC 4.1 Nr. 1.
///
/// The generic "Anfrage" PID. There is no ESA-specific REQOTE Prüfidentifikator
/// in any published format version; the ESA context is carried by the Messprodukt
/// code and by the ESA-specific QUOTES answer [`ANGEBOT_PID`].
pub const ANFRAGE_PID: u32 = 35002;

/// QUOTES — "Angebot zur Anfrage von Werten für ESA" (MSB → ESA), UC 4.1 Nr. 2.
pub const ANGEBOT_PID: u32 = 15003;

/// ORDERS — "Bestellung von Werten ESA" (ESA → MSB), UC 4.1 Nr. 3.
pub const BESTELLUNG_PID: u32 = 17007;

/// ORDERS — "Abbestellung von Werten ESA" (ESA → MSB), UC 4.3 Nr. 1.
///
/// Distinct from [`BESTELLUNG_PID`]: 17007 orders a delivery, 17008 ends a
/// running one. Both are ORDERS; the Prüfidentifikator in BGM DE 1004 tells
/// them apart.
pub const ABBESTELLUNG_PID: u32 = 17008;

/// ORDCHG — "Stornierung der Bestellung von Werten" (ESA → MSB), UC 4.1 Nr. 5.
pub const STORNIERUNG_PID: u32 = 39002;

/// ORDRSP — "Bestätigung der Ab-/Bestellung von Werten für ESA" (MSB → ESA).
pub const BESTAETIGUNG_PID: u32 = 19011;

/// ORDRSP — "Ablehnung der Ab-/Bestellung von Werten für ESA" (MSB → ESA).
pub const ABLEHNUNG_PID: u32 = 19012;

/// ORDRSP — "Bestätigung der Stornierung einer Bestellung für ESA" (MSB → ESA).
pub const STORNO_BESTAETIGUNG_PID: u32 = 19013;

/// ORDRSP — "Ablehnung der Stornierung einer Bestellung für ESA" (MSB → ESA).
pub const STORNO_ABLEHNUNG_PID: u32 = 19014;

/// MSCONS — "Werte nach Typ 2" (MSB → ESA), UC 4.2. The MSB's delivery duty
/// under §60 Abs. 1 MsbG: it transmits the ordered values to the ESA, daily by
/// 09:30. These values are non-authoritative (no billing bearing) and land in
/// the ESA deployment's separate Typ-2 store (`esa_typ2_reads`).
pub const WERTE_UEBERMITTLUNG_PID: u32 = 13027;

/// Every PID this workflow accepts inbound (ESA → MSB).
pub const INBOUND_PIDS: &[u32] = &[
    ANFRAGE_PID,
    BESTELLUNG_PID,
    ABBESTELLUNG_PID,
    STORNIERUNG_PID,
];

/// PIDs an **ESA-role** deployment receives inbound (MSB → ESA).
///
/// Disjoint from [`INBOUND_PIDS`], which is the MSB side, so an integrated
/// deployment holding both roles registers both sets without a conflict.
pub const ESA_INBOUND_PIDS: &[u32] = &[
    ANGEBOT_PID,
    BESTAETIGUNG_PID,
    ABLEHNUNG_PID,
    STORNO_BESTAETIGUNG_PID,
    STORNO_ABLEHNUNG_PID,
];

/// Every PID this workflow emits outbound (MSB → ESA).
pub const OUTBOUND_PIDS: &[u32] = &[
    ANGEBOT_PID,
    BESTAETIGUNG_PID,
    ABLEHNUNG_PID,
    STORNO_BESTAETIGUNG_PID,
    STORNO_ABLEHNUNG_PID,
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

/// Which level of location the ESA asked for.
///
/// UC 4.1 requires the request to be addressed to the MSB assigned to that
/// exact location, and the identifier differs per level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lokationsebene {
    /// Marktlokation — identified by MaLo-ID.
    Marktlokation,
    /// Messlokation — identified by Zählpunktbezeichnung.
    Messlokation,
    /// Netzlokation — identified by NeLo-ID.
    Netzlokation,
}

impl Lokationsebene {
    /// Stable label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Marktlokation => "Marktlokation",
            Self::Messlokation => "Messlokation",
            Self::Netzlokation => "Netzlokation",
        }
    }
}

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
        /// MaLo-ID, ZPB or NeLo-ID depending on `ebene`.
        lokations_id: String,
        /// EDIFACT message reference.
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
    /// MaLo-ID, ZPB or NeLo-ID.
    pub lokations_id: String,
    /// Belegnummer of the most recent inbound ORDERS/ORDCHG. The ORDRSP answer
    /// echoes it in `RFF+ACW` so the ESA can correlate the answer — an ORDRSP
    /// carries no LOC, so the MaLo is not available for correlation.
    #[serde(default)]
    pub inbound_order_ref: Option<String>,
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
#[derive(Clone)]
pub enum WertebestellungCommand {
    /// UC 4.1 Nr. 1 — inbound REQOTE 35002.
    ReceiveAnfrage {
        /// Prüfidentifikator of the inbound message.
        pid: Pruefidentifikator,
        /// GLN of the ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB.
        msb: MarktpartnerCode,
        /// Location level.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB or NeLo-ID.
        lokations_id: String,
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
        /// Reference of the outbound QUOTES.
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
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
        /// AS4 acknowledgement of the inbound message.
        quittung: Zustellquittung,
        /// Consent-registry gate re-checked at ingest — consent can be revoked
        /// between the Angebot and the Bestellung. `Some` carries the Begründung
        /// of a blocked order, answered with an ORDRSP 19012 Ablehnung; `None`
        /// allows it.
        consent_block: Option<String>,
    },
    /// UC 4.1 Nr. 4 — answer the Bestellung.
    AnswerBestellung {
        /// `true` to confirm (ORDRSP 19011), `false` to reject (19012).
        accept: bool,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
        /// Reason, required on rejection.
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
    /// UC 4.1 Nr. 6 — answer the Stornierung.
    AnswerStornierung {
        /// `true` to confirm (ORDRSP 19013), `false` to reject (19014).
        accept: bool,
        /// Reference of the outbound ORDRSP.
        message_ref: MessageRef,
        /// Reason, required on rejection.
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
    /// UC 4.3 Nr. 2 — confirm the Abbestellung.
    AnswerAbbestellung {
        /// Reference of the outbound ORDRSP 19011.
        message_ref: MessageRef,
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

/// Format a datetime as `CCYYMMDD` for a DTM segment value (format code 102).
fn ccyymmdd(dt: OffsetDateTime) -> String {
    format!("{:04}{:02}{:02}", dt.year(), u8::from(dt.month()), dt.day())
}

fn require_pid(pid: Pruefidentifikator, expected: u32, what: &str) -> Result<(), WorkflowError> {
    if pid.as_u32() == expected {
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

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        use WertebestellungEvent as E;
        use WertebestellungState as S;
        match event {
            E::AnfrageEingegangen {
                esa,
                msb,
                ebene,
                lokations_id,
                ..
            } => S::AnfrageEingegangen(Box::new(WertebestellungData {
                esa: esa.clone(),
                msb: msb.clone(),
                ebene: *ebene,
                lokations_id: lokations_id.clone(),
                inbound_order_ref: None,
            })),
            E::AngebotAbgegeben { bindungsfrist, .. } => match state {
                S::AnfrageEingegangen(data) => S::AngebotAbgegeben {
                    data,
                    bindungsfrist: *bindungsfrist,
                },
                other => other,
            },
            E::AnfrageAbgelehnt { reason } | E::BestellungAbgelehnt { reason } => S::Abgelehnt {
                reason: reason.clone(),
            },
            E::BestellungEingegangen { message_ref, .. } => match state {
                S::AngebotAbgegeben { mut data, .. } => {
                    data.inbound_order_ref = Some(message_ref.as_str().to_owned());
                    S::BestellungEingegangen(data)
                }
                other => other,
            },
            E::BestellungBestaetigt { .. } => match state {
                S::BestellungEingegangen(data) => S::BestellungBestaetigt {
                    data,
                    lieferung_begonnen: false,
                },
                other => other,
            },
            E::StornierungEingegangen { message_ref, .. } => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    data.inbound_order_ref = Some(message_ref.as_str().to_owned());
                    S::StornierungEingegangen(data)
                }
                other => other,
            },
            E::StornierungBestaetigt { .. } => match state {
                S::StornierungEingegangen(data) => S::Storniert(data),
                other => other,
            },
            // A refused Stornierung leaves the Bestellung standing.
            E::StornierungAbgelehnt { .. } => match state {
                S::StornierungEingegangen(data) => S::BestellungBestaetigt {
                    data,
                    lieferung_begonnen: false,
                },
                other => other,
            },
            E::AbbestellungEingegangen {
                beendigung_zum,
                message_ref,
                ..
            } => match state {
                S::BestellungBestaetigt { mut data, .. } => {
                    data.inbound_order_ref = Some(message_ref.as_str().to_owned());
                    S::AbbestellungEingegangen {
                        data,
                        beendigung_zum: *beendigung_zum,
                    }
                }
                other => other,
            },
            E::AbbestellungBestaetigt { .. } => match state {
                S::AbbestellungEingegangen { data, .. } => S::Beendet {
                    data,
                    durch_msb: false,
                },
                other => other,
            },
            E::LieferungBegonnen => match state {
                S::BestellungBestaetigt { data, .. } => S::BestellungBestaetigt {
                    data,
                    lieferung_begonnen: true,
                },
                other => other,
            },
            // A delivery closes the Stornierung window (first values have gone
            // out); it never changes the confirmed/winding-down state otherwise.
            E::WerteUebermittelt { .. } => match state {
                S::BestellungBestaetigt { data, .. } => S::BestellungBestaetigt {
                    data,
                    lieferung_begonnen: true,
                },
                other => other,
            },
            E::BeendetDurchMsb { .. } => match state {
                S::BestellungBestaetigt { data, .. } | S::AbbestellungEingegangen { data, .. } => {
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
        fn esa_answer(
            message_type: &'static str,
            pid: u32,
            data: &WertebestellungData,
            message_ref: &MessageRef,
        ) -> PendingOutbox {
            PendingOutbox::new(
                message_type,
                data.esa.as_str(),
                serde_json::json!({
                    "pid": pid,
                    "sender": data.msb.as_str(),
                    "receiver": data.esa.as_str(),
                    "message_ref": message_ref.as_str(),
                    // Echo the location so the ESA can correlate a QUOTES answer
                    // (which still carries a LOC) to the process it started.
                    "location": data.lokations_id,
                    // Echo the Belegnummer of the order this answers. An ORDRSP
                    // carries no LOC, so the ESA correlates it via `RFF+ACW`.
                    "order_reference": data.inbound_order_ref,
                }),
            )
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
                        }),
                    );
                    return Ok(WorkflowOutput::with_outbox(
                        vec![E::AnfrageAbgelehnt { reason }],
                        vec![outbox],
                    ));
                }
                let due = quittung.frist(ANGEBOT_FRIST_WT)?;
                Ok(WorkflowOutput {
                    events: vec![E::AnfrageEingegangen {
                        esa,
                        msb,
                        ebene,
                        lokations_id,
                        message_ref,
                        quittung,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(ANGEBOT_WINDOW_LABEL, due)],
                })
            }

            C::SendAngebot {
                message_ref,
                bindungsfrist,
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
                // The Angebot carries its Bindungsfrist on the wire (DTM+Z12) so
                // the ESA reads the real offer-validity end rather than a
                // synthesised default — and so an Angebot is distinguishable
                // from an Anfrage-Ablehnung (which carries none).
                let outbox = PendingOutbox::new(
                    "QUOTES",
                    data.esa.as_str(),
                    serde_json::json!({
                        "pid": ANGEBOT_PID,
                        "sender": data.msb.as_str(),
                        "receiver": data.esa.as_str(),
                        "location": data.lokations_id,
                        "message_ref": message_ref.as_str(),
                        "bindungsfrist": ccyymmdd(bindungsfrist),
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
                    let outbox = esa_answer("ORDRSP", ABLEHNUNG_PID, data, &message_ref);
                    return Ok(WorkflowOutput::with_outbox(
                        vec![E::BestellungAbgelehnt { reason }],
                        vec![outbox],
                    ));
                }
                let due = quittung.frist(ANTWORT_FRIST_WT)?;
                Ok(WorkflowOutput {
                    events: vec![E::BestellungEingegangen {
                        message_ref,
                        quittung,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerBestellung {
                accept,
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
                if accept {
                    let outbox = esa_answer("ORDRSP", BESTAETIGUNG_PID, data, &message_ref);
                    Ok(WorkflowOutput::with_outbox(
                        vec![E::BestellungBestaetigt { message_ref }],
                        vec![outbox],
                    ))
                } else {
                    reason.map_or_else(
                        || {
                            Err(WorkflowError::rejected(
                                "Ablehnung der Bestellung erfordert eine Begründung \
                                 (UC 4.1 Nr. 4: \"informiert der MSB den ESA über die Gründe\")",
                            ))
                        },
                        |reason| {
                            let outbox = esa_answer("ORDRSP", ABLEHNUNG_PID, data, &message_ref);
                            Ok(WorkflowOutput::with_outbox(
                                vec![E::BestellungAbgelehnt { reason }],
                                vec![outbox],
                            ))
                        },
                    )
                }
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
                Ok(WorkflowOutput {
                    events: vec![E::StornierungEingegangen {
                        message_ref,
                        quittung,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerStornierung {
                accept,
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
                if accept {
                    let outbox = esa_answer("ORDRSP", STORNO_BESTAETIGUNG_PID, data, &message_ref);
                    Ok(WorkflowOutput::with_outbox(
                        vec![E::StornierungBestaetigt { message_ref }],
                        vec![outbox],
                    ))
                } else {
                    reason.map_or_else(
                        || {
                            Err(WorkflowError::rejected(
                                "Ablehnung der Stornierung erfordert eine Begründung",
                            ))
                        },
                        |reason| {
                            let outbox =
                                esa_answer("ORDRSP", STORNO_ABLEHNUNG_PID, data, &message_ref);
                            Ok(WorkflowOutput::with_outbox(
                                vec![E::StornierungAbgelehnt { reason }],
                                vec![outbox],
                            ))
                        },
                    )
                }
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
                Ok(WorkflowOutput {
                    events: vec![E::AbbestellungEingegangen {
                        message_ref,
                        beendigung_zum,
                        quittung,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::AnswerAbbestellung { message_ref } => {
                let Some(data) = state
                    .data()
                    .filter(|_| matches!(state, S::AbbestellungEingegangen { .. }))
                else {
                    return Err(WorkflowError::invalid_state(
                        "AbbestellungEingegangen",
                        state.label(),
                    ));
                };
                let outbox = esa_answer("ORDRSP", BESTAETIGUNG_PID, data, &message_ref);
                Ok(WorkflowOutput::with_outbox(
                    vec![E::AbbestellungBestaetigt { message_ref }],
                    vec![outbox],
                ))
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
                        "reads": reads,
                    }),
                );
                Ok(WorkflowOutput::with_outbox(
                    vec![E::WerteUebermittelt {
                        message_ref,
                        interval_count,
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
                Ok(vec![E::BeendetDurchMsb {
                    message_ref,
                    beendigung_zum,
                    reason,
                }]
                .into())
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

// ── Inbound REQOTE classification ─────────────────────────────────────────────

/// What an inbound REQOTE turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReqoteKind {
    /// UC 4.1 Nr. 1 — an ESA asking for values and their cost.
    EsaWerteanfrage,
    /// A Preisanfrage for MSB/NB services (`wim-preisanfrage`).
    Preisanfrage,
}

/// Classify an inbound REQOTE.
///
/// REQOTE 35002 ("Anfrage") is **not** ESA-specific: no ESA-only REQOTE
/// Prüfidentifikator exists in any published format version, so an ESA
/// Werteanfrage and a Preisanfrage arrive under the same PID. WiM Teil 2 Kap. 4
/// resolves this at the content level — footnote 5 requires *"die entsprechenden
/// Codes der zugehörigen Anwendungsfälle in der Codeliste der Messprodukte"*.
///
/// Two signals are used, strongest first:
///
/// 1. **The sender's market role.** An ESA is a registered market partner
///    (PARTIN 37006, "Kommunikationsdaten des ESA Strom"), so a REQOTE from a
///    party registered in that role is a Werteanfrage. This is decisive.
/// 2. **A Messprodukt identifier in `PIA`.** A Werteanfrage names the product
///    it wants delivered; a Preisanfrage asks for a price sheet and carries no
///    Messprodukt.
///
/// When neither signal is present the REQOTE is classified as a Preisanfrage,
/// which is the safe default: it preserves existing routing, and misrouting an
/// ESA request would silently drop a message the MSB is obliged to answer,
/// whereas the reverse merely fails validation in a workflow that rejects it.
#[must_use]
pub fn classify_reqote(sender_is_esa: bool, has_messprodukt: bool) -> ReqoteKind {
    if sender_is_esa || has_messprodukt {
        ReqoteKind::EsaWerteanfrage
    } else {
        ReqoteKind::Preisanfrage
    }
}

/// `true` when any of the REQOTE's `PIA` product identifiers is non-empty.
///
/// BDEW encodes the Messprodukt in `PIA+5+<code>::<code list>`. A Werteanfrage
/// names the product it wants delivered; a Preisanfrage asks for a price sheet
/// and carries none. The caller extracts the codes, keeping this crate free of a
/// dependency on the EDIFACT parser.
#[must_use]
pub fn has_messprodukt<'a>(pia_codes: impl IntoIterator<Item = &'a str>) -> bool {
    pia_codes.into_iter().any(|c| !c.trim().is_empty())
}
