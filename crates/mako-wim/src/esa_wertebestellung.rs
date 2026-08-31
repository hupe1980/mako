//! WiM ESA Wertebestellung — **ESA origination side**.
//!
//! The mirror of [`super::wertebestellung`] (which is the MSB side). Here the
//! deployment **is** the Energieserviceanbieter: it *originates* the Werteanfrage
//! (REQOTE 35003), Bestellung (ORDERS 17007), Stornierung (ORDCHG 39002) and
//! Abbestellung (ORDERS 17008), and *receives* the MSB's answers (QUOTES 15003,
//! ORDRSP 19011/19012/19013/19014).
//!
//! §49 Abs. 2 Nr. 9 MsbG makes the ESA a consent-derived role: it may request a
//! location's values only while it holds a GDPR-Art.-7 Einwilligung. That guard
//! is enforced at the makod command boundary (the `esa_outbound` consent check)
//! before `SendWerteanfrage`/`SendBestellung` reach this pure workflow. GDPR
//! Art. 7(3) revocation drives `SendAbbestellung` (17008) — the only market
//! mechanism that stops a running delivery — which is therefore **not** gated.
//!
//! # Message flow
//!
//! ```text
//! ESA ──REQOTE 35003 Anfrage──────────────────────────────────────────▶ MSB
//! ESA ◀─QUOTES 15003 Angebot──────────── 5 WT nach ÜT der Anfrage ────── MSB
//! ESA ──ORDERS 17007 Bestellung──────────── bis Ablauf der Bindungsfrist ▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Bestellung ─── MSB
//!
//! (before delivery starts)
//! ESA ──ORDCHG 39002 Stornierung──────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19013 / 19014─────────── 2 WT nach ÜT der Stornierung ─── MSB
//!
//! (once delivery is running — the Art. 7(3) revocation path)
//! ESA ──ORDERS 17008 Abbestellung─────────────────────────────────────▶ MSB
//! ESA ◀─ORDRSP 19011 / 19012──────────── 2 WT nach ÜT der Abbestellung ─ MSB
//! ```

use mako_engine::{
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use time::OffsetDateTime;

// Reuse the shared vocabulary from the MSB side so both directions speak the
// same PIDs, Fristen and location model.
pub use super::wertebestellung::{
    ABBESTELLUNG_PID, ABLEHNUNG_PID, ANFRAGE_PID, ANGEBOT_PID, ANTWORT_FRIST_WT,
    BEENDIGUNG_MSB_PID, BESTAETIGUNG_PID, BESTELLUNG_PID, STORNIERUNG_PID, STORNO_ABLEHNUNG_PID,
    STORNO_BESTAETIGUNG_PID, STS_BEENDET, Zustellquittung,
};
pub use crate::esa::{
    Abonnement, Angebot, Antwort, Bestellgegenstand, Lokationsebene, ProduktFehler, SmgwQuelle,
};

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "esa-wertebestellung";

/// Deadline label for the Angebot the ESA awaits after its Anfrage (5 WT).
pub const ANGEBOT_WINDOW_LABEL: &str = "esa-wertebestellung-angebot";

/// Deadline label for the Bindungsfrist within which the ESA must order.
pub const BINDUNGSFRIST_LABEL: &str = "esa-wertebestellung-bindungsfrist";

/// Deadline label for the ORDRSP answer the ESA awaits (2 WT).
pub const ANTWORT_WINDOW_LABEL: &str = "esa-wertebestellung-antwort";

/// PIDs an ESA deployment receives inbound (MSB → ESA). Identical to
/// [`super::wertebestellung::ESA_INBOUND_PIDS`]; re-exported for routing clarity.
pub use super::wertebestellung::ESA_INBOUND_PIDS;

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the ESA-origination workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum EsaWertebestellungEvent {
    /// REQOTE 35003 sent — the ESA asked the MSB for values.
    AnfrageGesendet {
        /// GLN of this ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB addressed.
        msb: MarktpartnerCode,
        /// Location level requested.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID.
        lokations_id: String,
        /// Messprodukt, Wunschtermin and Abo mode — what is being ordered.
        gegenstand: Box<Bestellgegenstand>,
        /// Reference of the outbound REQOTE. The MSB's QUOTES echoes it in
        /// `RFF+AAV` (Zuordnungsschlüssel `ZG-T16`).
        message_ref: MessageRef,
    },
    /// QUOTES 15003 Angebot received — the ESA may order until `bindungsfrist`.
    AngebotErhalten {
        /// Belegnummer of the inbound QUOTES. Our ORDERS 17007 must echo it in
        /// `RFF+AAG` (Zuordnungsschlüssel `ZG-T24`).
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist, resolved from the `DTM+273`
        /// duration against the day the Angebot arrived.
        bindungsfrist: OffsetDateTime,
        /// `DTM+469` — the earliest start the MSB offers, which may be later
        /// than the ESA's Wunschtermin. **Muss** on the 15003.
        #[serde(default)]
        fruehester_start: Option<OffsetDateTime>,
        /// The commercial substance: currency, per-Artikel-ID prices, the OBIS
        /// registers the subscription will deliver, and the Einrichtungsdauer.
        #[serde(default)]
        angebot: Box<Angebot>,
    },
    /// QUOTES 15003 without a priced position — the MSB will not deliver, and
    /// the process ends.
    ///
    /// UC 4.1 Nr. 2 covers both outcomes („Angebot zur / Ablehnung der
    /// Anfrage") but the QUOTES AHB 1.1a publishes only the Angebot, with
    /// `SG31 PRI` **Muss** inside its position block and `DTM+273` Muss on the
    /// message — so the *prices*, never the Bindungsfrist, tell the two apart.
    /// The MSB states its grounds in `FTX+ACB` (the only free text the 15003
    /// has); the `E_0252` code behind them has no segment to ride.
    AnfrageAbgelehnt {
        /// Reference of the inbound QUOTES.
        message_ref: MessageRef,
        /// `FTX+ACB` — the MSB's stated grounds.
        reason: String,
    },
    /// ORDERS 17007 Bestellung sent.
    BestellungGesendet {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19011 received — delivery is authorised.
    BestellungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0256` Zustimmungscode and its tree.
        #[serde(default)]
        antwort: Option<Antwort>,
        /// `SG27 FTX+Z27`/`FTX+Z28` — where the iMS will push from. **Muss**
        /// on a 19011 confirming a Kapitel-4.6.2 order; the ESA must admit
        /// that source or the confirmed subscription never delivers.
        #[serde(default)]
        smgw_quelle: Option<SmgwQuelle>,
    },
    /// ORDRSP 19012 received — the MSB refused the Bestellung; the process ends.
    BestellungAbgelehnt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0256` Ablehnungscode and its tree. This *is* the
        /// reason; those PIDs publish no free-text segment.
        #[serde(default)]
        antwort: Option<Antwort>,
        /// Human-readable rendering of the Antwortcode, for the operator queue.
        reason: String,
    },
    /// ORDCHG 39002 Stornierung sent (before delivery began).
    StornierungGesendet {
        /// Belegnummer of the outbound ORDCHG. The MSB's ORDRSP 19013/19014
        /// echoes it in `RFF+ACW` (Zuordnungsschlüssel `ZG-T50`).
        message_ref: MessageRef,
    },
    /// ORDRSP 19013 received — the Stornierung was accepted; the order is void.
    StornierungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0257` Zustimmungscode and its tree.
        #[serde(default)]
        antwort: Option<Antwort>,
    },
    /// ORDRSP 19014 received — the Stornierung was refused; the order stands.
    StornierungAbgelehnt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0257` Ablehnungscode and its tree.
        #[serde(default)]
        antwort: Option<Antwort>,
        /// Human-readable rendering of the Antwortcode.
        reason: String,
    },
    /// ORDERS 17008 Abbestellung sent (the Art. 7(3) revocation path).
    AbbestellungGesendet {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// Trigger — typically `einwilligung_widerrufen`.
        grund: String,
    },
    /// ORDRSP 19011 received for the Abbestellung — delivery has ended.
    AbbestellungBestaetigt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0254` Zustimmungscode and its tree.
        #[serde(default)]
        antwort: Option<Antwort>,
    },
    /// ORDRSP 19012 received for the Abbestellung — the MSB refused to stop;
    /// delivery continues. Surfaced so the operator can escalate (refusing a
    /// GDPR-Art.-7(3) Widerruf is a compliance incident).
    AbbestellungAbgelehnt {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0254` Ablehnungscode and its tree. `A01`
        /// („war eine einmalige Übermittlung") and `A02` („ist zu stornieren")
        /// both mean „you used the wrong termination path", which the operator
        /// can only act on if the code survives.
        #[serde(default)]
        antwort: Option<Antwort>,
        /// Human-readable rendering of the Antwortcode.
        reason: String,
    },
    /// First values arrived; the Stornierung window closes.
    LieferungBegonnen,
    /// IFTSTA 21042 received (UC 4.4) — the MSB has ended the value delivery.
    BeendetDurchMsb {
        /// Reference of the inbound IFTSTA.
        message_ref: MessageRef,
        /// Date the MSB stops delivering.
        beendigung_zum: OffsetDateTime,
        /// Reason communicated by the MSB, when present.
        reason: Option<String>,
    },
    /// A regulatory window elapsed without the awaited answer.
    FristVersaeumt {
        /// Deadline label that fired.
        label: String,
    },
    /// An inbound answer contradicted itself: its `SG2 AJT` code sits in the
    /// cluster the *other* PID is for (ORDRSP AHB 1.1b conditions
    /// `[17]`/`[18]`). Recorded and then acted on by PID, since a Bestätigung
    /// that quotes an Ablehnungscode cannot be resolved either way.
    AntwortWidersprichtSich {
        /// PID of the inbound ORDRSP.
        pid: u32,
        /// The code and tree it named.
        antwort: Antwort,
    },
}

impl EventPayload for EsaWertebestellungEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::AnfrageGesendet { .. } => "EsaWertebestellungAnfrageGesendet",
            Self::AngebotErhalten { .. } => "EsaWertebestellungAngebotErhalten",
            Self::BestellungGesendet { .. } => "EsaWertebestellungBestellungGesendet",
            Self::BestellungBestaetigt { .. } => "EsaWertebestellungBestellungBestaetigt",
            Self::AnfrageAbgelehnt { .. } => "EsaWertebestellungAnfrageAbgelehnt",
            Self::BestellungAbgelehnt { .. } => "EsaWertebestellungBestellungAbgelehnt",
            Self::StornierungGesendet { .. } => "EsaWertebestellungStornierungGesendet",
            Self::StornierungBestaetigt { .. } => "EsaWertebestellungStornierungBestaetigt",
            Self::StornierungAbgelehnt { .. } => "EsaWertebestellungStornierungAbgelehnt",
            Self::AbbestellungGesendet { .. } => "EsaWertebestellungAbbestellungGesendet",
            Self::AbbestellungBestaetigt { .. } => "EsaWertebestellungAbbestellungBestaetigt",
            Self::AbbestellungAbgelehnt { .. } => "EsaWertebestellungAbbestellungAbgelehnt",
            Self::LieferungBegonnen => "EsaWertebestellungLieferungBegonnen",
            Self::AntwortWidersprichtSich { .. } => "EsaWertebestellungAntwortWidersprichtSich",
            Self::BeendetDurchMsb { .. } => "EsaWertebestellungBeendetDurchMsb",
            Self::FristVersaeumt { .. } => "EsaWertebestellungFristVersaeumt",
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data carried from the Anfrage through the whole process.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EsaWertebestellungData {
    /// GLN of this ESA.
    pub esa: MarktpartnerCode,
    /// GLN of the MSB.
    pub msb: MarktpartnerCode,
    /// Location level requested.
    pub ebene: Lokationsebene,
    /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID.
    pub lokations_id: String,
    /// What is being ordered: Messprodukt, Wunschtermin, Abo mode and — for a
    /// Kapitel-4.6.2 product — the SM-PKI delivery target.
    ///
    /// Without this the process could not say what a confirmed delivery is
    /// supposed to contain, so a missing daily transmission would be
    /// undetectable and the outbound REQOTE/ORDERS would have to invent a
    /// product code.
    pub gegenstand: Box<Bestellgegenstand>,
    /// Belegnummer of the REQOTE 35003 this ESA sent.
    pub anfrage_ref: String,
    /// Belegnummer of the QUOTES 15003 Angebot. The ORDERS 17007 must echo it
    /// in `RFF+AAG` — its published Zuordnungsschlüssel (`ZG-T24`).
    #[serde(default)]
    pub angebot_ref: Option<String>,
    /// Belegnummer of the ORDERS 17007 Bestellung. Three later messages
    /// reference it: the ORDCHG Stornierung (`RFF+ON`), the ORDERS 17008
    /// Abbestellung (`RFF+ACW`) and the MSB's IFTSTA 21042 (`RFF+AGI`).
    #[serde(default)]
    pub bestellung_ref: Option<String>,
    /// Belegnummer of the ORDCHG 39002 Stornierung, echoed by the ORDRSP
    /// 19013/19014 in `RFF+ACW`.
    #[serde(default)]
    pub stornierung_ref: Option<String>,
    /// Belegnummer of the ORDERS 17008 Abbestellung, echoed by the ORDRSP
    /// 19011/19012 in `RFF+ON`.
    #[serde(default)]
    pub abbestellung_ref: Option<String>,
    /// `true` once the first values arrived under this order.
    ///
    /// Lives here rather than inside the `Beliefert` variant because it is a
    /// fact about the **subscription**, not about the state the handshake
    /// happens to be in. Held in the variant it was lost every time the
    /// process left `Beliefert` for a Storno round trip and had to be
    /// re-invented on the way back — as `true` after a refused Abbestellung
    /// and `false` after a refused Stornierung, neither of which anything had
    /// observed. It is what closes the UC 4.1 Nr. 5 Stornierung window, and
    /// `E_0257` refuses a Stornierung of a started delivery, so guessing it
    /// earns a market rejection either way.
    #[serde(default)]
    pub lieferung_begonnen: bool,
    /// `DTM+469` — the earliest start the MSB offered.
    ///
    /// The ORDERS 17007 `DTM+203` Ausführungsdatum must not precede it: the
    /// MSB has already said it cannot serve an earlier date, so ordering the
    /// original Wunschtermin anyway asks for something the offer excluded.
    #[serde(default)]
    pub fruehester_start: Option<OffsetDateTime>,
    /// What the MSB offered — prices, currency, OBIS registers, Einrichtungs-
    /// dauer. Retained past the Bestellung because the MSB's INVOIC 31009
    /// (UC 4.5) is checked against the offer the ESA accepted, and because the
    /// OBIS list is what a delivery-surveillance sweep compares against.
    #[serde(default)]
    pub angebot: Box<Angebot>,
    /// `SG27 FTX+Z27`/`Z28` on the confirming ORDRSP — where the iMS pushes
    /// from, for a Kapitel-4.6.2 subscription.
    #[serde(default)]
    pub smgw_quelle: Option<SmgwQuelle>,
    /// The last `SG2 AJT` the MSB sent on this process, whichever step it
    /// answered. The operator queue and the § 20 EnWG audit trail both need
    /// the code that was actually stated.
    #[serde(default)]
    pub letzte_antwort: Option<Antwort>,
}

/// State of an ESA-origination Wertebestellung process.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum EsaWertebestellungState {
    /// No events yet.
    #[default]
    New,
    /// REQOTE sent; the ESA awaits an Angebot within 5 WT.
    AnfrageGesendet(Box<EsaWertebestellungData>),
    /// Angebot received; the ESA may order until the Bindungsfrist lapses.
    AngebotErhalten {
        /// Process data.
        data: Box<EsaWertebestellungData>,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
    },
    /// Bestellung sent; the ESA awaits an ORDRSP within 2 WT.
    BestellungGesendet(Box<EsaWertebestellungData>),
    /// Bestellung confirmed — delivery is authorised and may be running.
    ///
    /// Whether values have actually started arriving is
    /// [`EsaWertebestellungData::lieferung_begonnen`], not a second field
    /// here: it has to survive the Storno and Abbestellung round trips.
    Beliefert(Box<EsaWertebestellungData>),
    /// Stornierung sent; the ESA awaits an ORDRSP 19013/19014 within 2 WT.
    StornierungGesendet(Box<EsaWertebestellungData>),
    /// Abbestellung sent; the ESA awaits an ORDRSP 19011 within 2 WT.
    AbbestellungGesendet(Box<EsaWertebestellungData>),
    /// Order cancelled before delivery began.
    Storniert(Box<EsaWertebestellungData>),
    /// Delivery ended (Abbestellung confirmed).
    Beendet(Box<EsaWertebestellungData>),
    /// Terminal rejection (Anfrage timed out or Bestellung refused).
    Abgelehnt {
        /// Reason.
        reason: String,
    },
}

impl mako_engine::workflow::OccupiesBusinessKey for EsaWertebestellungState {
    fn occupies_business_key(&self) -> bool {
        match self {
            // Every in-flight step of the handshake, plus `Beliefert` — an
            // authorised delivery is live and holds the order.
            Self::AnfrageGesendet(_)
            | Self::AngebotErhalten { .. }
            | Self::BestellungGesendet(_)
            | Self::Beliefert(_)
            | Self::StornierungGesendet(_)
            | Self::AbbestellungGesendet(_) => true,
            // Terminal: cancelled before delivery, delivery ended, or refused.
            Self::New | Self::Storniert(_) | Self::Beendet(_) | Self::Abgelehnt { .. } => false,
        }
    }
}

impl EsaWertebestellungState {
    /// Stable string label for the current variant.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::AnfrageGesendet(_) => "AnfrageGesendet",
            Self::AngebotErhalten { .. } => "AngebotErhalten",
            Self::BestellungGesendet(_) => "BestellungGesendet",
            Self::Beliefert(_) => "Beliefert",
            Self::StornierungGesendet(_) => "StornierungGesendet",
            Self::AbbestellungGesendet(_) => "AbbestellungGesendet",
            Self::Storniert(_) => "Storniert",
            Self::Beendet(_) => "Beendet",
            Self::Abgelehnt { .. } => "Abgelehnt",
        }
    }

    /// `true` when delivery to the ESA is authorised (a confirmed Bestellung).
    #[must_use]
    pub const fn beliefert(&self) -> bool {
        matches!(self, Self::Beliefert(_) | Self::AbbestellungGesendet(_))
    }

    /// Process data, when the process has advanced past `New`.
    #[must_use]
    pub const fn data(&self) -> Option<&EsaWertebestellungData> {
        match self {
            Self::AnfrageGesendet(d)
            | Self::BestellungGesendet(d)
            | Self::Beliefert(d)
            | Self::StornierungGesendet(d)
            | Self::AbbestellungGesendet(d)
            | Self::Storniert(d)
            | Self::Beendet(d) => Some(d),
            Self::AngebotErhalten { data, .. } => Some(data),
            Self::New | Self::Abgelehnt { .. } => None,
        }
    }
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the ESA-origination workflow.
#[derive(Clone)]
pub enum EsaWertebestellungCommand {
    /// Originate REQOTE 35003 (UC 4.1 Nr. 1). Consent-gated at the makod
    /// boundary (`esa_outbound`) before it reaches this workflow.
    SendWerteanfrage {
        /// GLN of this ESA.
        esa: MarktpartnerCode,
        /// GLN of the MSB addressed.
        msb: MarktpartnerCode,
        /// Location level.
        ebene: Lokationsebene,
        /// MaLo-ID, ZPB, NeLo-ID or Tranchen-ID.
        lokations_id: String,
        /// What to order. Checked against the Codeliste-4.6 catalogue and the
        /// level before anything leaves the system.
        gegenstand: Box<Bestellgegenstand>,
        /// Belegnummer of the outbound REQOTE.
        message_ref: MessageRef,
    },
    /// QUOTES 15003 Angebot received (UC 4.1 Nr. 2).
    ReceiveAngebot {
        /// Belegnummer of the inbound QUOTES.
        message_ref: MessageRef,
        /// End of the MSB's Bindungsfrist.
        bindungsfrist: OffsetDateTime,
        /// `DTM+469` — earliest start the MSB offers.
        fruehester_start: Option<OffsetDateTime>,
        /// Prices, currency, OBIS registers and Einrichtungsdauer.
        angebot: Box<Angebot>,
    },
    /// QUOTES 15003 with no priced position — the MSB will not deliver.
    ReceiveAnfrageAblehnung {
        /// Belegnummer of the inbound QUOTES.
        message_ref: MessageRef,
        /// `FTX+ACB` — the grounds the MSB stated.
        reason: Option<String>,
    },
    /// Originate ORDERS 17007 Bestellung (UC 4.1 Nr. 3). Consent-gated.
    SendBestellung {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
    },
    /// ORDRSP 19011 received — a **Bestätigung** of the Bestellung (UC 4.1 Nr. 4)
    /// or, once running, of the Abbestellung (UC 4.3 Nr. 2). One PID, resolved
    /// against the current state.
    ReceiveBestaetigung {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the Antwortcode and the EBD it came from. **Muss** on
        /// the wire; `None` only from a non-conformant counterparty.
        antwort: Option<Antwort>,
        /// `SG27 FTX+Z27`/`Z28` — the MSB's push source, for a 4.6.2 order.
        smgw_quelle: Option<SmgwQuelle>,
    },
    /// ORDRSP 19012 received — an **Ablehnung** of the Bestellung (ends the
    /// process) or of the Abbestellung (delivery continues). Resolved against
    /// the current state.
    ReceiveAblehnung {
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the Antwortcode and the EBD it came from. This is the
        /// whole reason: 19012 publishes no free-text segment.
        antwort: Option<Antwort>,
    },
    /// Originate ORDCHG 39002 Stornierung (UC 4.1 Nr. 5) before delivery began.
    SendStornierung {
        /// Reference of the outbound ORDCHG.
        message_ref: MessageRef,
    },
    /// ORDRSP 19013/19014 received answering the Stornierung (UC 4.1 Nr. 6).
    ReceiveStornierungAntwort {
        /// Prüfidentifikator of the inbound ORDRSP (19013 or 19014).
        pid: Pruefidentifikator,
        /// Reference of the inbound ORDRSP.
        message_ref: MessageRef,
        /// `SG2 AJT` — the `E_0257` code and its tree.
        antwort: Option<Antwort>,
    },
    /// Originate ORDERS 17008 Abbestellung (UC 4.3 Nr. 1) — the GDPR Art. 7(3)
    /// revocation path. **Not** consent-gated: it is the act of stopping.
    SendAbbestellung {
        /// Reference of the outbound ORDERS.
        message_ref: MessageRef,
        /// Date delivery is to stop.
        beendigung_zum: OffsetDateTime,
        /// Trigger — typically `einwilligung_widerrufen`.
        grund: String,
    },
    /// IFTSTA 21042 received (UC 4.4) — the MSB has ended the value delivery
    /// (STS 4405 = 105 „beendet"). Terminal; needs no ESA answer.
    ReceiveBeendigungDurchMsb {
        /// Reference of the inbound IFTSTA.
        message_ref: MessageRef,
        /// Date the MSB stops delivering.
        beendigung_zum: OffsetDateTime,
        /// Reason communicated by the MSB, when present.
        reason: Option<String>,
    },
    /// Mark the first values as delivered, closing the Stornierung window.
    MarkLieferungBegonnen,
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique deadline ID.
        deadline_id: DeadlineId,
        /// Deadline label.
        label: Box<str>,
    },
}

impl CommandPayload for EsaWertebestellungCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// ESA-origination Wertebestellung workflow (WiM Strom Teil 2, Kapitel 4).
pub struct EsaWertebestellungWorkflow;

/// Render an inbound `SG2 AJT` as the Begründung an operator reads.
///
/// The ORDRSP ESA answers publish **no free-text segment** (ORDRSP AHB 1.1b
/// §4.15), so the Antwortcode and its EBD are the whole content of a refusal.
/// An answer that names no tree stays unresolved rather than being given a
/// meaning it does not have — `A01` is a different sentence in each of the
/// three trees.
fn antwort_reason(antwort: Option<&Antwort>) -> String {
    antwort.map_or_else(
        || {
            "ORDRSP ohne SG2 AJT — der Antwortcode ist Muss (ORDRSP AHB 1.1b §4.15), \
             die Ablehnung nennt damit keinen Grund"
                .to_owned()
        },
        Antwort::beschreibung,
    )
}

/// Record an answer whose `AJT` Cluster contradicts the PID that carried it.
///
/// ORDRSP AHB 1.1b conditions `[17]`/`[18]` bind the two together, so they can
/// only disagree on a non-conformant message. Returns the event to prepend, or
/// an empty vector when the answer is consistent or cannot be resolved at all
/// (a missing EBD is a different defect, already carried by the answer itself).
fn konflikt_event(
    pid: Pruefidentifikator,
    antwort: Option<&Antwort>,
    pid_ist_zustimmung: bool,
) -> Vec<EsaWertebestellungEvent> {
    match antwort {
        Some(a) if a.widerspricht_pid(pid_ist_zustimmung) => {
            vec![EsaWertebestellungEvent::AntwortWidersprichtSich {
                pid: pid.as_u32(),
                antwort: a.clone(),
            }]
        }
        _ => Vec::new(),
    }
}

fn require_pid(
    pid: Pruefidentifikator,
    allowed: &[Pruefidentifikator],
    what: &str,
) -> Result<(), WorkflowError> {
    if allowed.contains(&pid) {
        Ok(())
    } else {
        let allowed: Vec<u32> = allowed.iter().map(|a| a.as_u32()).collect();
        Err(WorkflowError::rejected(format!(
            "{what} erwartet PID {allowed:?}, erhielt {pid}"
        )))
    }
}

impl Workflow for EsaWertebestellungWorkflow {
    type State = EsaWertebestellungState;
    type Event = EsaWertebestellungEvent;
    type Command = EsaWertebestellungCommand;

    /// Turn a fired deadline into the
    /// [`EsaWertebestellungCommand::TimeoutExpired`] `handle` already decides.
    ///
    /// Without this hook the three windows this workflow registers fired into
    /// the engine's default `None`: an Angebot that never arrived left the
    /// process in `AnfrageGesendet` indefinitely instead of reaching
    /// `Abgelehnt`, and nothing surfaced the missed Frist.
    ///
    /// Terminal states are filtered here rather than in `handle`, which records
    /// a `FristVersaeumt` for whatever reaches it — a Bindungsfrist lapsing
    /// after the order was already cancelled is not a missed obligation.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        use mako_engine::workflow::OccupiesBusinessKey as _;

        let owned = matches!(
            deadline.label(),
            ANGEBOT_WINDOW_LABEL | ANTWORT_WINDOW_LABEL | BINDUNGSFRIST_LABEL
        );
        (owned && state.occupies_business_key()).then(|| {
            EsaWertebestellungCommand::TimeoutExpired {
                deadline_id: deadline.deadline_id(),
                label: deadline.label().into(),
            }
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        use EsaWertebestellungEvent as E;
        use EsaWertebestellungState as S;
        match event {
            E::AnfrageGesendet {
                esa,
                msb,
                ebene,
                lokations_id,
                gegenstand,
                message_ref,
            } => S::AnfrageGesendet(Box::new(EsaWertebestellungData {
                esa: esa.clone(),
                msb: msb.clone(),
                ebene: *ebene,
                lokations_id: lokations_id.clone(),
                gegenstand: gegenstand.clone(),
                anfrage_ref: message_ref.as_str().to_owned(),
                angebot_ref: None,
                bestellung_ref: None,
                stornierung_ref: None,
                abbestellung_ref: None,
                lieferung_begonnen: false,
                fruehester_start: None,
                angebot: Box::default(),
                smgw_quelle: None,
                letzte_antwort: None,
            })),
            E::AngebotErhalten {
                message_ref,
                bindungsfrist,
                fruehester_start,
                angebot,
            } => match state {
                S::AnfrageGesendet(mut data) => {
                    // The ORDERS 17007 must echo this Belegnummer in `RFF+AAG`.
                    data.angebot_ref = Some(message_ref.as_str().to_owned());
                    data.fruehester_start = *fruehester_start;
                    data.angebot.clone_from(angebot);
                    S::AngebotErhalten {
                        data,
                        bindungsfrist: *bindungsfrist,
                    }
                }
                other => other,
            },
            E::AnfrageAbgelehnt { reason, .. } => match state {
                S::AnfrageGesendet(_) => S::Abgelehnt {
                    reason: reason.clone(),
                },
                other => other,
            },
            E::BestellungGesendet { message_ref } => match state {
                S::AngebotErhalten { mut data, .. } => {
                    data.bestellung_ref = Some(message_ref.as_str().to_owned());
                    S::BestellungGesendet(data)
                }
                other => other,
            },
            E::BestellungBestaetigt {
                antwort,
                smgw_quelle,
                ..
            } => match state {
                S::BestellungGesendet(mut data) => {
                    data.letzte_antwort.clone_from(antwort);
                    data.smgw_quelle.clone_from(smgw_quelle);
                    S::Beliefert(data)
                }
                other => other,
            },
            // Guarded rather than unconditional: `apply` is a replay fold and
            // must not let a stray event collapse a state the handler would
            // never have produced it from.
            E::BestellungAbgelehnt {
                reason, antwort, ..
            } => match state {
                S::BestellungGesendet(_) => S::Abgelehnt {
                    reason: antwort
                        .as_ref()
                        .map_or_else(|| reason.clone(), Antwort::beschreibung),
                },
                other => other,
            },
            E::StornierungGesendet { message_ref } => match state {
                S::Beliefert(mut data) => {
                    // The ORDRSP 19013/19014 echoes this in `RFF+ACW`.
                    data.stornierung_ref = Some(message_ref.as_str().to_owned());
                    S::StornierungGesendet(data)
                }
                other => other,
            },
            E::StornierungBestaetigt { antwort, .. } => match state {
                S::StornierungGesendet(mut data) => {
                    data.letzte_antwort.clone_from(antwort);
                    S::Storniert(data)
                }
                other => other,
            },
            // A refused Stornierung leaves the delivery exactly as it was —
            // `lieferung_begonnen` rides in the data and is not re-asserted.
            E::StornierungAbgelehnt { antwort, .. } => match state {
                S::StornierungGesendet(mut data) => {
                    data.letzte_antwort.clone_from(antwort);
                    S::Beliefert(data)
                }
                other => other,
            },
            E::AbbestellungGesendet { message_ref, .. } => match state {
                S::Beliefert(mut data) => {
                    // The ORDRSP 19011/19012 echoes this in `RFF+ON`.
                    data.abbestellung_ref = Some(message_ref.as_str().to_owned());
                    S::AbbestellungGesendet(data)
                }
                other => other,
            },
            E::AbbestellungBestaetigt { antwort, .. } => match state {
                S::AbbestellungGesendet(mut data) => {
                    data.letzte_antwort.clone_from(antwort);
                    S::Beendet(data)
                }
                other => other,
            },
            // UC 4.4: the MSB ended the delivery — terminal from any
            // delivery-authorised state.
            E::BeendetDurchMsb { .. } => match state {
                S::Beliefert(data)
                | S::AbbestellungGesendet(data)
                | S::StornierungGesendet(data) => S::Beendet(data),
                other => other,
            },
            // A refused Abbestellung leaves delivery running, unchanged.
            E::AbbestellungAbgelehnt { antwort, .. } => match state {
                S::AbbestellungGesendet(mut data) => {
                    data.letzte_antwort.clone_from(antwort);
                    S::Beliefert(data)
                }
                other => other,
            },
            E::LieferungBegonnen => match state {
                // Recorded from any state that still holds an authorised
                // order: a first delivery landing while a Stornierung is in
                // flight is exactly the case `E_0257` `A02` exists for.
                S::Beliefert(mut data) => {
                    data.lieferung_begonnen = true;
                    S::Beliefert(data)
                }
                S::StornierungGesendet(mut data) => {
                    data.lieferung_begonnen = true;
                    S::StornierungGesendet(data)
                }
                S::AbbestellungGesendet(mut data) => {
                    data.lieferung_begonnen = true;
                    S::AbbestellungGesendet(data)
                }
                other => other,
            },
            E::FristVersaeumt { label } => match state {
                // An outstanding Angebot that never came ends the process.
                S::AnfrageGesendet(_) => S::Abgelehnt {
                    reason: "Angebot nicht innerhalb der Frist erhalten".to_owned(),
                },
                // …and so does an offer whose Bindungsfrist ran out. UC 4.1
                // Nr. 3 admits no order after it, so `AngebotErhalten` can
                // never advance again — and a process that cannot advance must
                // release its (Meldepunkt, Messprodukt) business key, or the
                // ESA can never request those values again.
                S::AngebotErhalten { bindungsfrist, .. } if label == BINDUNGSFRIST_LABEL => {
                    S::Abgelehnt {
                        reason: format!(
                            "Bindungsfrist des Angebots am {bindungsfrist} abgelaufen, ohne dass \
                             bestellt wurde"
                        ),
                    }
                }
                // A missed ORDRSP is a process anomaly the event surfaces; it
                // does not void an authorised delivery.
                other => other,
            },
            // Recorded for the audit trail; the PID decides what happens next.
            E::AntwortWidersprichtSich { .. } => state,
        }
    }

    fn handle(
        state: &Self::State,
        command: Self::Command,
    ) -> Result<WorkflowOutput<Self::Event>, WorkflowError> {
        // Build the outbound render intent that puts a message on the wire to
        // the MSB. The renderer turns this into REQOTE/ORDERS/ORDCHG with the
        // PID in BGM DE 1004 and the location in LOC.
        // `korrelation_ref` is the Belegnummer this message must echo under the
        // PID's published Zuordnungsschlüssel; the renderer picks the `RFF`
        // qualifier from [`crate::esa::korrelation`], so the two cannot drift.
        //
        // Only the REQOTE carries a `LOC` — `ZO-T17` is the *only* location
        // key in the process, and a conformant ORDERS/ORDCHG of Kapitel 4 has
        // no `LOC` segment at all (ORDERS AHB 1.1b §4.15, ORDCHG AHB 1.1 §3.2).
        fn esa_send(
            message_type: &'static str,
            pid: Pruefidentifikator,
            data: &EsaWertebestellungData,
            message_ref: &MessageRef,
            korrelation_ref: Option<&str>,
            ausfuehrungsdatum: Option<OffsetDateTime>,
            abonnement: Abonnement,
        ) -> PendingOutbox {
            let traegt_location = pid == ANFRAGE_PID;
            PendingOutbox::new(
                message_type,
                data.msb.as_str(),
                serde_json::json!({
                    "pid": pid,
                    "sender": data.esa.as_str(),
                    "receiver": data.msb.as_str(),
                    "message_ref": message_ref.as_str(),
                    "location": traegt_location.then(|| data.lokations_id.clone()),
                    "ebene": data.ebene,
                    "korrelation_ref": korrelation_ref,
                    "messprodukt": data.gegenstand.messprodukt,
                    "wunschtermin": data.gegenstand.wunschtermin.to_string(),
                    "zeitraum_bis": data.gegenstand.zeitraum_bis.map(|d| d.to_string()),
                    "abonnement": abonnement.imd_code(),
                    "ausfuehrungsdatum": ausfuehrungsdatum.map(|d| d.date().to_string()),
                    "smgw": data.gegenstand.smgw,
                }),
            )
        }

        use EsaWertebestellungCommand as C;
        use EsaWertebestellungEvent as E;
        use EsaWertebestellungState as S;

        match command {
            C::SendWerteanfrage {
                esa,
                msb,
                ebene,
                lokations_id,
                gegenstand,
                message_ref,
            } => {
                if !matches!(state, S::New) {
                    return Err(WorkflowError::invalid_state("New", state.label()));
                }
                if lokations_id.trim().is_empty() {
                    return Err(WorkflowError::rejected(format!(
                        "Werteanfrage auf Ebene {} ohne Lokations-ID",
                        ebene.as_str()
                    )));
                }
                // Catalogue check before anything leaves the system: the
                // Messprodukt must exist in Codeliste der Konfigurationen 1.4
                // Kapitel 4.6, be defined for the level being addressed, be
                // usable by the Wunschtermin, and carry its SM-PKI target when
                // it is a 4.6.2 product.
                gegenstand
                    .validate(ebene)
                    .map_err(|e| WorkflowError::rejected(e.to_string()))?;
                let data = EsaWertebestellungData {
                    esa: esa.clone(),
                    msb: msb.clone(),
                    ebene,
                    lokations_id: lokations_id.clone(),
                    gegenstand: gegenstand.clone(),
                    anfrage_ref: message_ref.as_str().to_owned(),
                    angebot_ref: None,
                    bestellung_ref: None,
                    stornierung_ref: None,
                    abbestellung_ref: None,
                    lieferung_begonnen: false,
                    fruehester_start: None,
                    angebot: Box::default(),
                    smgw_quelle: None,
                    letzte_antwort: None,
                };
                let wunschtermin = gegenstand.wunschtermin.midnight().assume_utc();
                let outbox = esa_send(
                    "REQOTE",
                    ANFRAGE_PID,
                    &data,
                    &message_ref,
                    None,
                    Some(wunschtermin),
                    gegenstand.abonnement,
                );
                // The MSB owes an Angebot within 5 WT; arm the window from now
                // (the AS4 Receipt for our REQOTE is issued in the same request).
                let due = mako_fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    super::wertebestellung::ANGEBOT_FRIST_WT,
                    mako_fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::AnfrageGesendet {
                        esa,
                        msb,
                        ebene,
                        lokations_id,
                        gegenstand,
                        message_ref,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANGEBOT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveAngebot {
                message_ref,
                bindungsfrist,
                fruehester_start,
                angebot,
            } => {
                if !matches!(state, S::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                // An offer is a **priced** position: `SG31 PRI` is Muss inside
                // the `SG27 LIN` block and the OBIS list says which registers
                // will arrive. A 15003 carrying neither is the MSB declining,
                // and `AngebotErhalten` means „an offer stands that the ESA may
                // order against" — a state this message does not create.
                //
                // The ingest adapter already routes an unpriced 15003 to
                // `ReceiveAnfrageAblehnung`; refusing it here too is what keeps
                // a second caller (a replay, a hand-built command) from parking
                // the process in a state whose Bindungsfrist can only ever
                // expire, holding the (Meldepunkt, Messprodukt) business key.
                if angebot.ist_leer() {
                    return Err(WorkflowError::rejected(
                        "QUOTES 15003 ohne bepreiste Position ist kein Angebot, sondern die \
                         Ablehnung der Anfrage (SG31 PRI und die PIA+5 …:SRW OBIS-Kennzahlen \
                         sind Muss, QUOTES AHB 1.1a §4.3) — nutze ReceiveAnfrageAblehnung",
                    ));
                }
                Ok(WorkflowOutput {
                    events: vec![E::AngebotErhalten {
                        message_ref,
                        bindungsfrist,
                        fruehester_start,
                        angebot,
                    }],
                    outbox: Vec::new(),
                    deadlines: vec![PendingDeadline::new(BINDUNGSFRIST_LABEL, bindungsfrist)],
                })
            }

            C::ReceiveAnfrageAblehnung {
                message_ref,
                reason,
            } => {
                if !matches!(state, S::AnfrageGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "AnfrageGesendet",
                        state.label(),
                    ));
                }
                Ok(WorkflowOutput::events(vec![E::AnfrageAbgelehnt {
                    message_ref,
                    reason: reason.unwrap_or_else(|| {
                        "QUOTES 15003 ohne bepreiste Position — der MSB nennt keine Gründe"
                            .to_owned()
                    }),
                }]))
            }

            C::SendBestellung { message_ref } => {
                let S::AngebotErhalten {
                    data,
                    bindungsfrist,
                } = state
                else {
                    return Err(WorkflowError::invalid_state(
                        "AngebotErhalten",
                        state.label(),
                    ));
                };
                // UC 4.1 Nr. 3: order only within the MSB's Bindungsfrist.
                if OffsetDateTime::now_utc() > *bindungsfrist {
                    return Err(WorkflowError::rejected(format!(
                        "Bindungsfrist des Angebots endete am {bindungsfrist}"
                    )));
                }
                // ORDERS AHB 1.1b §4.15: `SG1 RFF+AAG` is Muss on 17007 and
                // carries the QUOTES Angebot's Dokumentennummer — it is the
                // order's published Zuordnungsschlüssel (`ZG-T24`), so an
                // order without it cannot be matched to the offer it accepts.
                let angebot_ref = data.angebot_ref.as_deref().ok_or_else(|| {
                    WorkflowError::rejected(
                        "Bestellung ohne Angebotsnummer — RFF+AAG ist Muss (ORDERS AHB 1.1b §4.15)",
                    )
                })?;
                // `DTM+203` Ausführungsdatum is **Muss** on the 17007
                // (ORDERS AHB 1.1b §4.15) and states when delivery is to
                // begin. The MSB has already answered that question with
                // `DTM+469` „Startdatum, frühestes/r" — Muss on its Angebot —
                // so ordering the original Wunschtermin when the offer named a
                // later one asks for a date the MSB has said it cannot serve.
                let wunsch = data.gegenstand.wunschtermin.midnight().assume_utc();
                let ausfuehrungsdatum = data.fruehester_start.map_or(wunsch, |f| wunsch.max(f));
                let outbox = esa_send(
                    "ORDERS",
                    BESTELLUNG_PID,
                    data,
                    &message_ref,
                    Some(angebot_ref),
                    Some(ausfuehrungsdatum),
                    data.gegenstand.abonnement,
                );
                let due = mako_fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::BestellungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveBestaetigung {
                message_ref,
                antwort,
                smgw_quelle,
            } => {
                // ORDRSP AHB 1.1b conditions `[17]`/`[18]`: the `AJT` code has
                // to sit in the Zustimmungs-Cluster of the tree it names, and
                // 19011 is the Zustimmungs-PID. A 19011 quoting an
                // Ablehnungscode is a message whose halves disagree; it is
                // recorded and then read by PID, because acting on the code
                // instead would silently turn a confirmation into a refusal.
                let mut events = konflikt_event(BESTAETIGUNG_PID, antwort.as_ref(), true);
                match state {
                    // ORDRSP 19011 confirms the Bestellung → delivery authorised.
                    S::BestellungGesendet(_) => {
                        events.push(E::BestellungBestaetigt {
                            message_ref,
                            antwort,
                            smgw_quelle,
                        });
                    }
                    // ORDRSP 19011 confirms the Abbestellung → delivery ended.
                    S::AbbestellungGesendet(_) => {
                        events.push(E::AbbestellungBestaetigt {
                            message_ref,
                            antwort,
                        });
                    }
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "BestellungGesendet|AbbestellungGesendet",
                            state.label(),
                        ));
                    }
                }
                Ok(WorkflowOutput::events(events))
            }

            C::ReceiveAblehnung {
                message_ref,
                antwort,
            } => {
                let mut events = konflikt_event(ABLEHNUNG_PID, antwort.as_ref(), false);
                // The `AJT` **is** the reason — ORDRSP AHB 1.1b §4.15 gives
                // 19011–19014 no free-text segment at all, and the only `FTX`
                // a conformant 19011 may carry is `SG27 FTX+Z27`, the MSB's IP
                // address.
                let reason = antwort_reason(antwort.as_ref());
                match state {
                    // ORDRSP 19012 refuses the Bestellung → the process ends.
                    S::BestellungGesendet(_) => {
                        events.push(E::BestellungAbgelehnt {
                            message_ref,
                            antwort,
                            reason,
                        });
                    }
                    // ORDRSP 19012 refuses the Abbestellung → delivery continues.
                    S::AbbestellungGesendet(_) => {
                        events.push(E::AbbestellungAbgelehnt {
                            message_ref,
                            antwort,
                            reason,
                        });
                    }
                    _ => {
                        return Err(WorkflowError::invalid_state(
                            "BestellungGesendet|AbbestellungGesendet",
                            state.label(),
                        ));
                    }
                }
                Ok(WorkflowOutput::events(events))
            }

            C::SendStornierung { message_ref } => {
                let S::Beliefert(data) = state else {
                    return Err(WorkflowError::invalid_state("Beliefert", state.label()));
                };
                if data.lieferung_begonnen {
                    return Err(WorkflowError::rejected(
                        "Stornierung ist nach Lieferbeginn nicht mehr möglich \
                         (UC 4.3 Vorbedingung) — nutze die Abbestellung (17008)",
                    ));
                }
                // ORDCHG AHB 1.1 §3.2: `SG1 RFF+ON` is Muss and carries the
                // ORDERS' Dokumentennummer (`ZG-T51`).
                let bestellung_ref = data.bestellung_ref.as_deref().ok_or_else(|| {
                    WorkflowError::rejected(
                        "Stornierung ohne Auftragsnummer — RFF+ON ist Muss (ORDCHG AHB 1.1 §3.2)",
                    )
                })?;
                let outbox = esa_send(
                    "ORDCHG",
                    STORNIERUNG_PID,
                    data,
                    &message_ref,
                    Some(bestellung_ref),
                    None,
                    data.gegenstand.abonnement,
                );
                let due = mako_fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::StornierungGesendet { message_ref }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::ReceiveStornierungAntwort {
                pid,
                message_ref,
                antwort,
            } => {
                if !matches!(state, S::StornierungGesendet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "StornierungGesendet",
                        state.label(),
                    ));
                }
                require_pid(
                    pid,
                    &[STORNO_BESTAETIGUNG_PID, STORNO_ABLEHNUNG_PID],
                    "Antwort auf Stornierung",
                )?;
                let bestaetigt = pid == STORNO_BESTAETIGUNG_PID;
                let mut events = konflikt_event(pid, antwort.as_ref(), bestaetigt);
                events.push(if bestaetigt {
                    E::StornierungBestaetigt {
                        message_ref,
                        antwort,
                    }
                } else {
                    E::StornierungAbgelehnt {
                        reason: antwort_reason(antwort.as_ref()),
                        message_ref,
                        antwort,
                    }
                });
                Ok(WorkflowOutput::events(events))
            }

            C::SendAbbestellung {
                message_ref,
                beendigung_zum,
                grund,
            } => {
                let S::Beliefert(data) = state else {
                    return Err(WorkflowError::invalid_state("Beliefert", state.label()));
                };
                // UC 4.3's Vorbedingung is „Es findet eine turnusmäßige/
                // regelmäßige Übermittlung von Werten statt". A one-shot has
                // none, and `E_0254` Prüfschritt 1 refuses its Beendigung with
                // `A01` by construction — „es handelte sich um eine einmalige
                // Übermittlung, sie ist zu stornieren". Sending it anyway
                // spends a 2-Werktage answer window to be told what the
                // Codeliste already says, and after the refusal the Abo mode
                // has still not changed, so the ESA can only repeat it.
                if !data.gegenstand.abonnement.ist_abo() {
                    return Err(WorkflowError::rejected(
                        "einmalige Übermittlung (IMD++Z03) ist stornierbar, nicht abbestellbar — \
                         nutze die Stornierung (ORDCHG 39002); E_0254 Prüfschritt 1 lehnt eine \
                         Abbestellung mit A01 ab",
                    ));
                }
                // ORDERS AHB 1.1b §4.15 makes `SG1 RFF+ACW` Muss —
                // it carries the 17007's Dokumentennummer (`ZG-T41`) and is
                // the only way the MSB can tell which order is being ended.
                let bestellung_ref = data.bestellung_ref.as_deref().ok_or_else(|| {
                    WorkflowError::rejected(
                        "Abbestellung ohne Referenz auf die Bestellung — RFF+ACW ist Muss \
                         (ORDERS AHB 1.1b §4.15)",
                    )
                })?;
                let outbox = esa_send(
                    "ORDERS",
                    ABBESTELLUNG_PID,
                    data,
                    &message_ref,
                    Some(bestellung_ref),
                    Some(beendigung_zum),
                    // `IMD++Z02` Ende Abo — which is also what selects EBD
                    // `E_0254` for the MSB's answer.
                    Abonnement::EndeAbo,
                );
                let due = mako_fristen::deadline_at_werktage(
                    OffsetDateTime::now_utc(),
                    ANTWORT_FRIST_WT,
                    mako_fristen::HolidayCalendar::BdewMaKo,
                );
                Ok(WorkflowOutput {
                    events: vec![E::AbbestellungGesendet {
                        message_ref,
                        beendigung_zum,
                        grund,
                    }],
                    outbox: vec![outbox],
                    deadlines: vec![PendingDeadline::new(ANTWORT_WINDOW_LABEL, due)],
                })
            }

            C::MarkLieferungBegonnen => {
                // Driven by an inbound MSCONS 13027, which is a fact rather
                // than a request: values arrived. Anything but a running
                // delivery is a no-op instead of an error — a batch that lands
                // after the subscription ended must not fail the ingest, and
                // repeating the event on every daily delivery would bloat the
                // stream for no state change.
                match state {
                    S::Beliefert(d) | S::StornierungGesendet(d) | S::AbbestellungGesendet(d)
                        if !d.lieferung_begonnen =>
                    {
                        Ok(WorkflowOutput::events(vec![E::LieferungBegonnen]))
                    }
                    _ => Ok(WorkflowOutput::events(Vec::new())),
                }
            }

            C::ReceiveBeendigungDurchMsb {
                message_ref,
                beendigung_zum,
                reason,
            } => {
                // UC 4.4: the MSB ends the delivery unilaterally — on an MSB
                // change at the Messlokation, on the MSB↔AN contract ending, or
                // on technical grounds. Its Vorbedingung is that a delivery is
                // running and the ESA has not already ended it, and it is *not*
                // an answer: the ESA sent nothing the MSB is replying to, so
                // whatever the ESA has in flight does not gate it.
                //
                // `StornierungGesendet` is therefore included. An ESA waiting
                // out its 2-Werktage Storno window while the MeLo moves to
                // another MSB is an ordinary race, and refusing the IFTSTA left
                // the process `Beliefert` for a subscription that had ended —
                // holding its business key and reporting a delivery gap that is
                // not one.
                if !state.beliefert() && !matches!(state, S::StornierungGesendet(_) | S::Beendet(_))
                {
                    return Err(WorkflowError::invalid_state(
                        "Beliefert|StornierungGesendet",
                        state.label(),
                    ));
                }
                Ok(WorkflowOutput::events(vec![E::BeendetDurchMsb {
                    message_ref,
                    beendigung_zum,
                    reason,
                }]))
            }

            C::TimeoutExpired { label, .. } => {
                Ok(WorkflowOutput::events(vec![E::FristVersaeumt {
                    label: label.to_string(),
                }]))
            }
        }
    }
}
