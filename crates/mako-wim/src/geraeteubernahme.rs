//! WiM Geräteübernahme und Gerätewechsel — the two ORDERS/ORDRSP legs the
//! **abgebender** Messstellenbetreiber (MSBA) answers, in both Sparten.
//!
//! Models WiM Strom Teil 1 (Anlage 2a zu BK6-24-174) Kap. 3.1 and 3.2, and the
//! identical AWH WiM Gas 2.0 Kap. 4.1 and 4.2.
//!
//! # Two Use-Cases, one Messlokation
//!
//! The MSBN may run them in parallel, in either order, or only one of them
//! (WiM Teil 1 Kap. 2.3.2 Nr. 5/6). They concern the same devices at the same
//! Messlokation, so one process stream keyed on the MeLo holds both — but they
//! are **separate exchanges with separate Antwortcodes**, not phases of one.
//!
//! ```text
//! Geräteübernahme (Kap. 3.2 / AWH Gas 4.2)
//!   MSBN ──REQOTE 35001 Anforderung Geräteübernahmeangebot─────────────────▶ MSBA
//!   MSBN ◀─QUOTES 15001 Angebot──────────── 4 WT nach dem ÜT von Nr. 1 ───── MSBA
//!   MSBN ──ORDERS 17001 Bestellung───────── 3 WT nach dem ÜT von Nr. 2 ────▶ MSBA
//!   MSBN ◀─ORDRSP 19001 / 19002──────────── 2 WT nach dem ÜT von Nr. 3 ───── MSBA
//!
//! Gerätewechsel (Kap. 3.1 / AWH Gas 4.1)
//!   MSBN ──ORDERS 17009 Anzeige Gerätewechselabsicht───────────────────────▶ MSBA
//!   MSBN ◀─ORDRSP 19015 / 19016──────────── 2 WT *vor* dem Wechseltermin ─── MSBA
//! ```
//!
//! This workflow owns only what arrives as an ORDERS and leaves as an ORDRSP.
//! The REQOTE 35001 Anforderung and the QUOTES 15001 Angebot belong to
//! [`crate::preisanfrage`], so 17001 is the Bestellung and nothing else; ORDERS
//! **17002** „Weiterverpflichtung" is NB → MSBA with its own Frist and
//! Entscheidungsbaum — [`crate::weiterverpflichtung`].
//!
//! # 19015 divides the labour, 19016 refuses
//!
//! `ZB4` „Eigenausbau wird erfolgen" — the MSBA removes its own devices — and
//! `ZB5` „Kein Eigenausbau des MSBA", where the MSBN does, both agree that the
//! Gerätewechsel happens; they divide the labour, and both ride 19015. The
//! refusals are `E17` (Frist) and `Z07` (Berechtigung), and they ride 19016
//! „Ablehnung Gerätewechselabsicht" — ORDRSP AHB 1.1b Kap. 4.10 (S. 46),
//! Bedingungen `[38]` „Möglicher Wert: ZB4 oder ZB5" and `[39]` „Möglicher
//! Wert: E17 oder Z07". The Codeliste in DE 1082 follows the code, not the PID:
//! the EBD 4.3 Kap. 8.4.1 (S. 540) publishes `ZB4` in `S_0065` and `ZB5` in
//! `S_0066`.
//!
//! A 17009 whose Gerätewechseltermin misses the Mindestvorlaufzeit (Kap. 3.1.2
//! Nr. 1, „frühestens am 4. auf diese Aktion … folgenden WT") is answered
//! with that 19016 and `E17` „Ablehnung wg. Fristüberschreitung" at once: the
//! ORDERS is processable, so it is a business answer, and the APERAK beside it
//! is the Anerkennungsmeldung.
//!
//! # One PID, two Sparten
//!
//! ORDERS and ORDRSP are Sparte-neutral AHBs: 17001, 17009 and the answers
//! 19001/19002/19015/19016 carry the Strom **and** the Gas Use-Case. Nothing in
//! the message body says which — the Sparte is the Sparte of the interchange
//! recipient's MP-ID (BDEW Allgemeine Festlegungen §2.13) and it decides the
//! Entscheidungsbaum, hence the Codeliste `SG2 AJT` DE 1082 must name:
//!
//! | Leg | Strom tree | Strom DE 1082 | Gas tree | Gas DE 1082 |
//! |---|---|---|---|---|
//! | 17001 → 19001/19002 | `E_0247` | `S_0067` / `S_0068` | `E_2011` | `G_0061` / `G_0074` |
//! | 17009 → 19015/19016 | `E_0204` | `S_0065` / `S_0066` | `E_2007` | `G_0059` / `G_0060` |
//!
//! # Regulatory basis
//!
//! - **WiM Strom Teil 1 Kap. 3.1/3.2** (Anlage 2a zu BK6-24-174)
//! - **AWH WiM Gas 2.0 Kap. 4.1/4.2** (gültig ab 01.10.2026)
//! - **ORDRSP AHB 1.1b Kap. 4** — `SG2 AJT` DE 4465 (code) und DE 1082 (Codeliste)
//! - **EBD 4.3** Kap. 8.4/8.5 (Strom) und 14.4/14.5 (Gas)

use std::collections::HashMap;

use mako_engine::envelope::{Antwortcode, Richtung, Verlauf};
use mako_engine::payload::AperakPayload;
use mako_engine::{
    envelope::EventEnvelope,
    error::WorkflowError,
    ids::DeadlineId,
    outbox::PendingOutbox,
    projection::Projection,
    types::{DeviceId, MarktpartnerCode, MeLo, MessageRef, Pruefidentifikator, Sparte},
    workflow::{CommandPayload, EventPayload, PendingDeadline, Workflow, WorkflowOutput},
};
use mako_fristen::{HolidayCalendar, deadline_at_werktage};
use time::OffsetDateTime;

// ── PID constants ─────────────────────────────────────────────────────────────

/// Workflow name used for PID routing and `WorkflowId` construction.
pub const WORKFLOW_NAME: &str = "wim-geraeteubernahme";

/// Inbound ORDERS PIDs routed to this workflow, in **both Sparten**.
///
/// | PID | AHB name | Fundstelle Strom | Fundstelle Gas |
/// |---|---|---|---|
/// | 17001 | Bestellung Geräteübernahme | Kap. 3.2.2 Nr. 3 | AWH 4.2.2 Nr. 3 |
/// | 17009 | Anzeige Gerätewechselabsicht | Kap. 3.1.2 Nr. 1 | AWH 4.1.2 Nr. 1 |
///
/// Neighbouring ORDERS PIDs that are deliberately absent: **17002**
/// (Weiterverpflichtung, NB → MSBA — [`crate::weiterverpflichtung`]), **17005**
/// (Rechnungsabwicklung MSB über LF — [`crate::rechnungsabwicklung`]) and
/// **17011** (Änderung der Technik — [`crate::technik_aenderung`]).
pub const GERAETEUBERNAHME_PIDS: &[u32] = &[17001, 17009];

/// ORDERS 17001 — „Bestellung Geräteübernahme".
///
/// The Bestellung against a standing QUOTES 15001 Angebot. It is the **only**
/// ORDERS in the Geräteübernahme: the Anforderung that precedes it is REQOTE
/// 35001 and belongs to [`crate::preisanfrage`].
pub const BESTELLUNG_PIDS: &[u32] = &[17001];

/// ORDERS 17009 — „Anzeige Gerätewechselabsicht".
pub const ANKUENDIGUNG_PIDS: &[u32] = &[17009];

/// The REQOTE that opens a Geräteübernahme, for cross-referencing only.
///
/// Owned by [`crate::preisanfrage`] — this workflow never sees it, and names it
/// only so the two legs of the Use-Case are legible from one place.
pub const ANFRAGE_PID: Pruefidentifikator = Pruefidentifikator::const_new(35001);

/// QUOTES 15001 — „Geräteübernahmeangebot", answered by [`crate::preisanfrage`].
pub const ANGEBOT_PID: Pruefidentifikator = Pruefidentifikator::const_new(15001);

/// ORDRSP 19001 — „Bestellbestätigung" (positive answer to 17001).
pub const BESTAETIGUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19001);

/// ORDRSP 19002 — „Ablehnung der Bestellung" (negative answer to 17001).
pub const ABLEHNUNG_PID: Pruefidentifikator = Pruefidentifikator::const_new(19002);

/// ORDRSP 19015 / 19016 — the MSBA's answer to a Gerätewechselabsicht:
/// 19015 carries `ZB4`/`ZB5`, 19016 the refusals `E17`/`Z07` — see the module
/// docs.
pub const GERAETEWECHSELABSICHT_PIDS: (u32, u32) = (19015, 19016);

/// Werktage for the Geräteübernahmeangebot (Kap. 3.2.2 Nr. 2 / AWH 4.2.2 Nr. 2).
///
/// Owned by [`crate::preisanfrage`]; restated here because the Bestellfrist
/// below is counted from the ÜT of the Angebot.
pub const ANGEBOT_FRIST_WT: u32 = 4;

/// Werktage the MSBN has to order against a standing Angebot
/// (Kap. 3.2.2 Nr. 3 / AWH 4.2.2 Nr. 3).
pub const BESTELLUNG_FRIST_WT: u32 = 3;

/// Werktage for the Bestellbestätigung — „Unverzüglich, jedoch spätester ÜT ist
/// der 2. WT nach dem ÜT von Nr. 3" (Kap. 3.2.2 Nr. 4 / AWH 4.2.2 Nr. 4).
pub const BESTAETIGUNG_FRIST_WT: u32 = 2;

/// The [`mako_fristen::vorlauf`] key for the window in which the MSBA must
/// answer an Anzeige Gerätewechselabsicht
/// (Kap. 3.1.2 Nr. 2 / AWH 4.1.2 Nr. 2 — spätester ÜT ist der 2. WT davor).
///
/// A Vorlauffrist, not an Antwortfrist: it is anchored on the Termin the
/// *message* carries, so it can already be in the past when the Anzeige
/// arrives. That is not an error in the arithmetic — it is a Vorlauffrist the
/// MSBN failed to observe, and `E17` is the code for it.
///
/// Read from the table for the same reason as
/// [`GERAETEWECHSEL_TERMIN_VORLAUF_KEY`]: the `2` was a second copy of the
/// Festlegung here, and the catalogued row — citation and all — was the copy
/// nothing reached.
pub const GERAETEWECHSELABSICHT_ANTWORT_KEY: &str = "wim.antwort-geraetewechselabsicht";

/// The [`mako_fristen::vorlauf`] key for the Mindestvorlaufzeit between the
/// Anzeige der Gerätewechselabsicht and the Gerätewechseltermin
/// (Kap. 3.1.2 Nr. 1 / AWH 4.1.2 Nr. 1 — frühestens am 4. folgenden Werktag).
///
/// The number itself lives in the Vorlauffrist table and is read from there.
/// A copy here was a second place for the Festlegung to be edited, and it was
/// the copy nothing checked against.
pub const GERAETEWECHSEL_TERMIN_VORLAUF_KEY: &str = "wim.geraetewechsel-termin";

/// Deadline label for the ORDRSP answer window on a Bestellung (2 Werktage).
pub const ORDRSP_DEADLINE_LABEL: &str = "wim-geraeteubernahme-ordrsp-deadline";

/// Deadline label for the ORDRSP answer window on a Gerätewechselabsicht.
///
/// Distinct from [`ORDRSP_DEADLINE_LABEL`] because the two are anchored
/// differently — one on the arrival instant, one on a date in the payload — and
/// an operator queue that cannot tell them apart cannot state why an entry is
/// due when it is.
pub const GERAETEWECHSELABSICHT_DEADLINE_LABEL: &str = "wim-geraetewechselabsicht-ordrsp-deadline";

/// The Entscheidungsbaum that answers an inbound ORDERS, per Sparte.
///
/// Returns `None` for a PID this workflow does not answer.
#[must_use]
pub const fn ordrsp_ebd(orders_pid: u32, sparte: Sparte) -> Option<&'static str> {
    use mako_pruefung::codes as c;
    match (orders_pid, sparte) {
        (17001, Sparte::Strom) => Some(c::EBD_BESTELLUNG_GERAETEUEBERNAHME),
        (17001, Sparte::Gas) => Some(c::EBD_BESTELLUNG_GERAETEUEBERNAHME_GAS),
        (17009, Sparte::Strom) => Some(c::EBD_GERAETEWECHSELABSICHT),
        (17009, Sparte::Gas) => Some(c::EBD_GERAETEWECHSELABSICHT_GAS),
        _ => None,
    }
}

/// The ORDRSP answer PID for an inbound ORDERS and the Antwortcode it is
/// answered with.
///
/// The **code picks the PID**, never a boolean the caller passes alongside it.
/// On a Bestellung the code's cluster does: `E_0247`'s Zustimmung rides 19001,
/// its Ablehnung 19002 (ORDRSP AHB 1.1b Kap. 4.9.2). On a Gerätewechselabsicht
/// the cluster does not — `ZB5` sits in the Ablehnungs-Codeliste `S_0066` and
/// still rides the Bestätigung 19015 — so the AHB's own lists decide (Kap.
/// 4.10, S. 46: `[38]` „Möglicher Wert: ZB4 oder ZB5", `[39]` „Möglicher Wert:
/// E17 oder Z07"). `None` for a code neither list names.
#[must_use]
pub fn ordrsp_antwort_pid(
    orders_pid: u32,
    code: &mako_pruefung::codes::AntwortCode,
) -> Option<u32> {
    match (orders_pid, code.code) {
        (17001, _) => Some(if code.ist_zustimmung()? { 19001 } else { 19002 }),
        (17009, "ZB4" | "ZB5") => Some(GERAETEWECHSELABSICHT_PIDS.0),
        (17009, "E17" | "Z07") => Some(GERAETEWECHSELABSICHT_PIDS.1),
        _ => None,
    }
}

/// `true` for the ORDRSP that refuses the order: 19002 „Ablehnung Bestellung"
/// and 19016 „Ablehnung Gerätewechselabsicht". A 19015 with `ZB5` is not one.
#[must_use]
pub const fn ist_ablehnung(antwort_pid: u32) -> bool {
    matches!(antwort_pid, 19002 | 19016)
}

/// `BGM` DE 1001 of the ORDRSP: `Z10` „Geräteübernahme" on 19001/19002 (ORDRSP
/// AHB 1.1b Kap. 4.9.2), `Z13` „Gerätewechselabsicht" on 19015/19016 (Kap.
/// 4.10).
const fn ordrsp_dokumentcode(antwort_pid: u32) -> Option<&'static str> {
    match antwort_pid {
        19001 | 19002 => Some("Z10"),
        19015 | 19016 => Some("Z13"),
        _ => None,
    }
}

// ── Domain events ─────────────────────────────────────────────────────────────

/// Events emitted by the WiM Geräteübernahme workflow.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum GeraeteubernahmeEvent {
    /// An ORDERS 17001 Bestellung or 17009 Anzeige Gerätewechselabsicht arrived.
    OrdersEmpfangen {
        /// ORDERS PID (17001 or 17009).
        pid: Pruefidentifikator,
        /// MP-ID of the incoming Messstellenbetreiber (MSBN) — the sender.
        msbn: MarktpartnerCode,
        /// MP-ID of this party, the abgebender MSB (MSBA) — the receiver.
        msba: MarktpartnerCode,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// Gerätenummer, where the order names one.
        device_id: DeviceId,
        /// Document date from `DTM+137` (YYYYMMDD).
        document_date: String,
        /// The date the order asks for: the Übernahmezeitpunkt on a 17001, the
        /// Gerätewechseltermin on a 17009 (YYYYMMDD).
        termin: Option<String>,
        /// `UNH` DE 0062 of the ORDERS, which the APERAK names in `RFF+ACE`.
        message_ref: MessageRef,
        /// `BGM` DE 1004 of the ORDERS, which the ORDRSP names in `SG1 RFF+ON`
        /// (ORDRSP AHB 1.1b Hinweise `[518]` and `[535]`).
        orders_nummer: String,
        /// Sparte of the interchange — it picks the Entscheidungsbaum.
        sparte: Sparte,
    },
    /// The ORDERS passed AHB validation.
    ValidationPassed {
        /// Reference of the validated message.
        message_ref: MessageRef,
    },
    /// The ORDRSP answer went out.
    AntwortGesendet {
        /// 19001/19002 for a Bestellung, 19015/19016 for a Gerätewechselabsicht.
        pruefidentifikator: Pruefidentifikator,
        /// `SG2 AJT` DE 4465 — the Prüfschritt code.
        antwort_code: String,
        /// The Entscheidungsbaum the code was resolved against.
        antwort_ebd: String,
        /// `true` when the answer refuses the order ([`ist_ablehnung`]): the
        /// process ends. A `ZB5` on 19015 is not a refusal.
        abgelehnt: bool,
        /// What the operator reads beside the code — for `E17`, the earliest
        /// Gerätewechseltermin the MSBN could still have named. Never on the
        /// wire: the 19016 has no `FTX`.
        begruendung: Option<String>,
    },
    /// The physical device transfer completed.
    Abgeschlossen {
        /// Gerätenummer confirmed at transfer.
        device_id: DeviceId,
    },
    /// The order was refused before it could be answered (AHB validation).
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A registered deadline fired.
    DeadlineExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl EventPayload for GeraeteubernahmeEvent {
    fn event_type(&self) -> &'static str {
        match self {
            Self::OrdersEmpfangen { .. } => "WimGeraeteubernahmeOrdersEmpfangen",
            Self::ValidationPassed { .. } => "WimGeraeteubernahmeValidationPassed",
            Self::AntwortGesendet { .. } => "WimGeraeteubernahmeAntwortGesendet",
            Self::Abgeschlossen { .. } => "WimGeraeteubernahmeAbgeschlossen",
            Self::Abgelehnt { .. } => "WimGeraeteubernahmeAbgelehnt",
            Self::DeadlineExpired { .. } => "WimGeraeteubernahmeDeadlineExpired",
        }
    }

    fn verlauf(&self) -> Option<Verlauf> {
        match self {
            Self::OrdersEmpfangen {
                pid, message_ref, ..
            } => Some(Verlauf {
                richtung: Richtung::Eingang,
                pid: Some(pid.as_u32()),
                message_ref: Some(message_ref.as_str().to_owned()),
            }),
            Self::AntwortGesendet {
                pruefidentifikator, ..
            } => Some(Verlauf {
                richtung: Richtung::Ausgang,
                pid: Some(pruefidentifikator.as_u32()),
                message_ref: None,
            }),
            Self::ValidationPassed { .. }
            | Self::Abgeschlossen { .. }
            | Self::Abgelehnt { .. }
            | Self::DeadlineExpired { .. } => None,
        }
    }

    /// The ORDRSP states its `AJT` code and the tree it was resolved against;
    /// the wire Codeliste (`S_`/`G_`) is resolved from it on read.
    fn antwort(&self) -> Option<Antwortcode> {
        match self {
            Self::AntwortGesendet {
                antwort_code,
                antwort_ebd,
                ..
            } => Some(Antwortcode::new(
                antwort_code.clone(),
                Some(antwort_ebd.clone()),
            )),
            _ => None,
        }
    }
}

// ── Domain state ──────────────────────────────────────────────────────────────

/// Business data captured from the inbound ORDERS.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeraeteubernahmeData {
    /// ORDERS PID (17001 or 17009).
    pub pid: Pruefidentifikator,
    /// MP-ID of the incoming Messstellenbetreiber (MSBN).
    pub msbn: MarktpartnerCode,
    /// MP-ID of this party, the abgebender MSB (MSBA).
    pub msba: MarktpartnerCode,
    /// Messlokation the devices sit at.
    pub melo_id: MeLo,
    /// Gerätenummer, where the order names one.
    pub device_id: DeviceId,
    /// EDIFACT document date (YYYYMMDD).
    pub document_date: String,
    /// Übernahmezeitpunkt (17001) or Gerätewechseltermin (17009).
    pub termin: Option<String>,
    /// EDIFACT message reference of the ORDERS.
    pub message_ref: MessageRef,
    /// `BGM` DE 1004 of the ORDERS, echoed in the ORDRSP's `SG1 RFF+ON`.
    pub orders_nummer: String,
    /// Sparte of the interchange.
    pub sparte: Sparte,
}

/// State of a single WiM Geräteübernahme / Gerätewechsel process stream.
///
/// ```text
/// New → OrdersEmpfangen → ValidationPassed → Beantwortet → Abgeschlossen
///                       ↘ Abgelehnt (AHB validation)   ↘ Abgelehnt (Ablehnungscode)
///       any non-terminal → Abgelehnt (deadline expired)
/// ```
///
/// `Beantwortet` is reached by a Zustimmungscode and `Abgelehnt` by an
/// Ablehnungscode — the cluster decides, not the caller.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", content = "data")]
#[derive(Default)]
pub enum GeraeteubernahmeState {
    /// No events yet.
    #[default]
    New,
    /// ORDERS received; AHB validation result not yet applied.
    OrdersEmpfangen(GeraeteubernahmeData),
    /// Validation passed; the ORDRSP answer is owed.
    ValidationPassed(GeraeteubernahmeData),
    /// A Zustimmung went out; the physical work is outstanding.
    Beantwortet(GeraeteubernahmeData),
    /// Device transfer completed.
    Abgeschlossen(GeraeteubernahmeData),
    /// Refused — AHB validation or an Ablehnungscode.
    Abgelehnt {
        /// Human-readable rejection reason.
        reason: String,
    },
    /// A window lapsed while the process still waited in `offen`, and no
    /// Festlegung gives that lapse an outcome.
    ///
    /// WiM Teil 1 Kap. 3.1.2 / 3.2.2 give the MSBA its ORDRSP windows and no consequence of missing them. The lapse is a Frist breach for the operator to act on,
    /// not a rejection: every command `offen` accepts is still accepted — a
    /// late answer is still an answer — and the next event moves the process
    /// on from `offen`.
    FristAbgelaufen {
        /// Label of the lapsed window.
        label: String,
        /// The state the process was waiting in.
        offen: Box<GeraeteubernahmeState>,
    },
}

impl GeraeteubernahmeState {
    /// Returns `true` if the process is in a terminal state.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Abgeschlossen(_) | Self::Abgelehnt { .. })
    }

    /// Stable string label for the current variant.
    #[must_use]
    pub fn status_str(&self) -> &'static str {
        match self {
            Self::New => "New",
            Self::OrdersEmpfangen(_) => "OrdersEmpfangen",
            Self::ValidationPassed(_) => "ValidationPassed",
            Self::Beantwortet(_) => "Beantwortet",
            Self::Abgeschlossen(_) => "Abgeschlossen",
            Self::Abgelehnt { .. } => "Abgelehnt",
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
}

impl mako_engine::workflow::OccupiesBusinessKey for GeraeteubernahmeState {
    fn occupies_business_key(&self) -> bool {
        !matches!(self, Self::New) && !self.is_terminal()
    }
}

// ── What the answer states beyond its code ────────────────────────────────────

/// What an ORDRSP answer states beyond its `AJT` code — facts only the MSBA
/// holds, supplied with [`GeraeteubernahmeCommand::DispatchAntwort`].
///
/// ORDRSP AHB 1.1b makes each one Muss on one column and on no other, so the
/// workflow refuses a missing one by name and an inapplicable one too: a
/// segment the column does not list fails the receiver's validation.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AntwortAngaben {
    /// `SG6 CTA`/`COM` — Muss on 19001 and 19002, Hinweis `[500]` „Angabe
    /// eines technischen Ansprechpartners für die Geräteübernahme" (Kap.
    /// 4.9.2, S. 44).
    pub ansprechpartner: Option<Ansprechpartner>,
    /// `SG3 NAD+Z22` „Lieferadresse für Altgeräte" — Muss on 19015 under
    /// `[41]` „wenn SG2 AJT … DE4465 mit Wert ZB5 vorhanden" (Kap. 4.10,
    /// S. 48): where the MSBN removes the devices, it ships them here.
    pub lieferadresse: Option<Lieferadresse>,
    /// `SG3 NAD+VY` — the Eigentümer's MP-ID, Muss on 19016 under `[42]`
    /// „Wenn SG2 AJT+Z07 … vorhanden" (Kap. 4.10, S. 47), Hinweis `[536]`.
    pub eigentuemer: Option<MarktpartnerCode>,
}

/// The MSBA's technischer Ansprechpartner for the Geräteübernahme (`SG6`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ansprechpartner {
    /// `CTA` DE 3412 — the contact's name.
    pub name: String,
    /// One `COM` each; at least one, and each code at most once (`[1P0..1]`).
    pub kommunikation: Vec<Kommunikationsweg>,
}

/// A `COM` channel: DE 3155 the code, DE 3148 the address.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "art", content = "adresse", rename_all = "snake_case")]
pub enum Kommunikationsweg {
    /// `EM` Elektronische Post — must contain „@" and „." (`[939]`).
    Email(String),
    /// `TE` Telefon — „+" then digits only (`[940]`).
    Telefon(String),
    /// `FX` Telefax — „+" then digits only (`[940]`).
    Telefax(String),
    /// `AJ` weiteres Telefon — „+" then digits only (`[940]`).
    WeiteresTelefon(String),
    /// `AL` Handy — „+" then digits only (`[940]`).
    Handy(String),
}

impl Kommunikationsweg {
    /// `COM` DE 3155.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Email(_) => "EM",
            Self::Telefon(_) => "TE",
            Self::Telefax(_) => "FX",
            Self::WeiteresTelefon(_) => "AJ",
            Self::Handy(_) => "AL",
        }
    }

    /// `COM` DE 3148.
    #[must_use]
    pub fn adresse(&self) -> &str {
        match self {
            Self::Email(a)
            | Self::Telefon(a)
            | Self::Telefax(a)
            | Self::WeiteresTelefon(a)
            | Self::Handy(a) => a,
        }
    }
}

/// `SG3 NAD+Z22` „Name und Lieferadresse für Altgeräte" (Kap. 4.10, S. 48).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lieferadresse {
    /// DE 3036 — the company name (DE 3045 `Z02` „Struktur der
    /// Firmenbezeichnung").
    pub name: String,
    /// DE 3042 — Straße und Hausnummer oder Postfach.
    pub strasse: String,
    /// DE 3164 — Ort.
    pub ort: String,
    /// DE 3251 — Postleitzahl; Muss where the country has one (`[40]`).
    pub postleitzahl: Option<String>,
    /// DE 3207 — ISO 3166-1 country code.
    pub land: String,
}

// ── Domain commands ───────────────────────────────────────────────────────────

/// Commands for the WiM Geräteübernahme workflow.
#[derive(Clone)]
pub enum GeraeteubernahmeCommand {
    /// An inbound ORDERS 17001 or 17009 accepted from the AS4 layer.
    ///
    /// Domain fields are extracted and AHB validation is performed at the
    /// transport boundary **before** this command is constructed.
    ReceiveOrders {
        /// ORDERS PID (17001 or 17009).
        pid: Pruefidentifikator,
        /// MP-ID of the message sender (MSBN).
        sender: MarktpartnerCode,
        /// MP-ID of the message receiver (this party, MSBA).
        receiver: MarktpartnerCode,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// Gerätenummer, where the order names one.
        device_id: DeviceId,
        /// Document date from `DTM+137`.
        document_date: String,
        /// Übernahmezeitpunkt (17001) or Gerätewechseltermin (17009), YYYYMMDD.
        termin: Option<String>,
        /// EDIFACT message reference (`UNH` DE 0062).
        message_ref: MessageRef,
        /// `BGM` DE 1004 of the ORDERS — the ORDRSP names it in `SG1 RFF+ON`.
        orders_nummer: String,
        /// `true` if AHB profile validation succeeded.
        validation_passed: bool,
        /// Validation issues, for the `Abgelehnt` event.
        validation_errors: Vec<String>,
        /// Sparte of the interchange, from the recipient MP-ID.
        sparte: Sparte,
        /// Arrival instant, for the answer deadline.
        received_at: OffsetDateTime,
    },
    /// Send the ORDRSP answer.
    ///
    /// `antwort_code` must be published by the tree [`ordrsp_ebd`] resolves for
    /// this ORDERS and Sparte. The **code** picks 19001 or 19002 (resp. 19015 or
    /// 19016, [`ordrsp_antwort_pid`]) — passing a boolean alongside the code
    /// lets the two disagree, which is how a Zustimmung ends up on an
    /// Ablehnungs-PID.
    DispatchAntwort {
        /// `SG2 AJT` DE 4465 — the Prüfschritt code.
        antwort_code: String,
        /// Free-text Begründung, where the code needs one.
        bemerkung: Option<String>,
        /// What the answer's column makes Muss beyond the code.
        angaben: AntwortAngaben,
    },
    /// Confirm that the physical device transfer is complete.
    ConfirmTransfer {
        /// Gerätenummer confirmed at transfer.
        device_id: DeviceId,
    },
    /// A registered deadline fired.
    TimeoutExpired {
        /// Unique ID of the expired deadline.
        deadline_id: DeadlineId,
        /// Label identifying the deadline type.
        label: Box<str>,
    },
}

impl CommandPayload for GeraeteubernahmeCommand {}

// ── Workflow ──────────────────────────────────────────────────────────────────

/// WiM Geräteübernahme / Gerätewechsel workflow — the **MSBA side**.
pub struct WimGeraeteubernahmeWorkflow;

/// The one guard a fired deadline passes: is the window `label` names still
/// open in `state`?
///
/// Both ORDRSP windows are open while the MSBA still owes its answer
/// (`OrdersEmpfangen`, `ValidationPassed`).
fn window_open(label: &str, state: &GeraeteubernahmeState) -> bool {
    matches!(
        label,
        ORDRSP_DEADLINE_LABEL | GERAETEWECHSELABSICHT_DEADLINE_LABEL
    ) && matches!(
        state,
        GeraeteubernahmeState::OrdersEmpfangen(_) | GeraeteubernahmeState::ValidationPassed(_)
    )
}

impl Workflow for WimGeraeteubernahmeWorkflow {
    type State = GeraeteubernahmeState;
    type Event = GeraeteubernahmeEvent;
    type Command = GeraeteubernahmeCommand;

    const DEADLINE_LABELS: &'static [&'static str] =
        &[ORDRSP_DEADLINE_LABEL, GERAETEWECHSELABSICHT_DEADLINE_LABEL];

    /// Deadline compensation for the two ORDRSP answer windows.
    ///
    /// | Label | Open while | Frist |
    /// |---|---|---|
    /// | [`ORDRSP_DEADLINE_LABEL`] | `OrdersEmpfangen` / `ValidationPassed` | 2 WT nach dem ÜT der Bestellung |
    /// | [`GERAETEWECHSELABSICHT_DEADLINE_LABEL`] | `OrdersEmpfangen` / `ValidationPassed` | 2 WT vor dem Gerätewechseltermin |
    ///
    /// Once the ORDRSP is out the window is closed: a `Beantwortet` process
    /// waits for the physical work, not for an answer.
    fn on_deadline(
        deadline: &mako_engine::deadline::Deadline,
        state: &Self::State,
    ) -> Option<Self::Command> {
        window_open(deadline.label(), state).then(|| GeraeteubernahmeCommand::TimeoutExpired {
            deadline_id: deadline.deadline_id(),
            label: deadline.label().into(),
        })
    }

    fn apply(state: Self::State, event: &Self::Event) -> Self::State {
        // A lapsed window leaves the process waiting where it was; the next
        // event moves it on from there.
        let state = match state {
            GeraeteubernahmeState::FristAbgelaufen { offen, .. } => *offen,
            other => other,
        };
        match event {
            GeraeteubernahmeEvent::OrdersEmpfangen {
                pid,
                msbn,
                msba,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                orders_nummer,
                sparte,
            } => GeraeteubernahmeState::OrdersEmpfangen(GeraeteubernahmeData {
                pid: *pid,
                msbn: msbn.clone(),
                msba: msba.clone(),
                melo_id: melo_id.clone(),
                device_id: device_id.clone(),
                document_date: document_date.clone(),
                termin: termin.clone(),
                message_ref: message_ref.clone(),
                orders_nummer: orders_nummer.clone(),
                sparte: *sparte,
            }),
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeState::OrdersEmpfangen(data) = state {
                    GeraeteubernahmeState::ValidationPassed(data)
                } else {
                    state
                }
            }
            GeraeteubernahmeEvent::AntwortGesendet {
                abgelehnt,
                antwort_code,
                antwort_ebd,
                begruendung,
                ..
            } => match state {
                GeraeteubernahmeState::ValidationPassed(data) => {
                    if *abgelehnt {
                        GeraeteubernahmeState::Abgelehnt {
                            reason: match begruendung {
                                Some(text) => format!("{antwort_ebd} {antwort_code}: {text}"),
                                None => format!("{antwort_ebd} {antwort_code}"),
                            },
                        }
                    } else {
                        GeraeteubernahmeState::Beantwortet(data)
                    }
                }
                other => other,
            },
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => match state {
                GeraeteubernahmeState::Beantwortet(mut data) => {
                    data.device_id = device_id.clone();
                    GeraeteubernahmeState::Abgeschlossen(data)
                }
                other => other,
            },
            GeraeteubernahmeEvent::Abgelehnt { reason } => GeraeteubernahmeState::Abgelehnt {
                reason: reason.clone(),
            },
            GeraeteubernahmeEvent::DeadlineExpired { label, .. } => match state {
                GeraeteubernahmeState::OrdersEmpfangen(_)
                | GeraeteubernahmeState::ValidationPassed(_) => {
                    GeraeteubernahmeState::FristAbgelaufen {
                        label: label.to_string(),
                        offen: Box::new(state),
                    }
                }
                other => other,
            },
        }
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
            GeraeteubernahmeCommand::ReceiveOrders {
                pid,
                sender,
                receiver,
                melo_id,
                device_id,
                document_date,
                termin,
                message_ref,
                orders_nummer,
                validation_passed,
                validation_errors,
                sparte,
                received_at,
            } => {
                if !matches!(state, GeraeteubernahmeState::New) {
                    return Err(WorkflowError::invalid_state("New", state.status_str()));
                }
                if !GERAETEUBERNAHME_PIDS.contains(&pid.as_u32()) {
                    return Err(WorkflowError::rejected(format!(
                        "PID {} is not answered by this workflow (expected {GERAETEUBERNAHME_PIDS:?})",
                        pid.as_u32(),
                    )));
                }
                // Clones for the APERAK, which travels back the other way and
                // names the ORDERS it acknowledges (`SG2 RFF+ACE`, Muss).
                let absender = sender.clone();
                let empfaenger = receiver.clone();
                let orders_ref = message_ref.as_str().to_owned();

                let mut events = vec![GeraeteubernahmeEvent::OrdersEmpfangen {
                    pid,
                    msbn: sender,
                    msba: receiver,
                    melo_id,
                    device_id,
                    document_date,
                    termin: termin.clone(),
                    message_ref: message_ref.clone(),
                    orders_nummer,
                    sparte,
                }];

                if !validation_passed {
                    let reason = validation_errors.join("; ");
                    events.push(GeraeteubernahmeEvent::Abgelehnt {
                        reason: reason.clone(),
                    });
                    return Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![aperak(
                            &empfaenger,
                            &absender,
                            &orders_ref,
                            sparte,
                            Some(&reason),
                        )],
                        vec![],
                    ));
                }

                // The Mindestvorlaufzeit, before any answer window is opened.
                //
                // Kap. 3.1.2 Nr. 1 puts the Gerätewechseltermin at the 4.
                // Werktag after the Anzeige at the earliest, and Nr. 2 puts the
                // answer 2 Werktage *before* that Termin. A Termin closer than
                // four is a Frist the MSBN missed, and the answer is due now:
                // ORDRSP 19016 with `E17` „Ablehnung wg. Fristüberschreitung"
                // (EBD 4.3 Kap. 8.4.1; ORDRSP AHB 1.1b Kap. 4.10 `[39]`). The
                // ORDERS is processable, so the APERAK is the Anerkennung; the
                // operator reads the earliest Termin the MSBN could still reach.
                if pid.as_u32() == 17_009
                    && let Some(mako_fristen::vorlauf::VorlaufVerdict::TooLate {
                        shortfall_wt,
                        earliest_possible,
                    }) = vorlauf_verdict(termin.as_deref(), received_at)
                {
                    let begruendung = format!(
                        "Mindestvorlaufzeit nicht eingehalten — der Gerätewechseltermin \
                         liegt {shortfall_wt} Werktage zu früh; frühestmöglicher Termin ist \
                         {earliest_possible} (WiM Teil 1 Kap. 3.1.2 Nr. 1)"
                    );
                    // The data the answer is built from is the one the
                    // `OrdersEmpfangen` just recorded — one source, not a copy.
                    let GeraeteubernahmeState::OrdersEmpfangen(data) =
                        Self::apply(GeraeteubernahmeState::New, &events[0])
                    else {
                        unreachable!("OrdersEmpfangen applies to OrdersEmpfangen")
                    };
                    let (antwort, ordrsp) = ordrsp_antwort(
                        &data,
                        "E17",
                        None,
                        Some(begruendung),
                        &AntwortAngaben::default(),
                    )?;
                    events.push(GeraeteubernahmeEvent::ValidationPassed { message_ref });
                    events.push(antwort);
                    return Ok(WorkflowOutput::with_outbox_and_deadlines(
                        events,
                        vec![
                            aperak(&empfaenger, &absender, &orders_ref, sparte, None),
                            ordrsp.caused_by(2),
                        ],
                        vec![],
                    ));
                }

                events.push(GeraeteubernahmeEvent::ValidationPassed { message_ref });

                // The business answer is the ORDRSP, and its window depends on
                // which Use-Case the PID opens. The APERAK window beside it is
                // the ingest door's, derived from the message.
                let deadlines = vec![antwort_deadline(
                    pid.as_u32(),
                    termin.as_deref(),
                    received_at,
                )?];

                Ok(WorkflowOutput::with_outbox_and_deadlines(
                    events,
                    vec![aperak(&empfaenger, &absender, &orders_ref, sparte, None)],
                    deadlines,
                ))
            }

            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code,
                bemerkung,
                angaben,
            } => {
                let GeraeteubernahmeState::ValidationPassed(data) = state else {
                    return Err(WorkflowError::invalid_state(
                        "ValidationPassed",
                        state.status_str(),
                    ));
                };
                let (antwort, ordrsp) =
                    ordrsp_antwort(data, &antwort_code, bemerkung.as_deref(), None, &angaben)?;
                Ok(WorkflowOutput::with_outbox(vec![antwort], vec![ordrsp]))
            }

            GeraeteubernahmeCommand::ConfirmTransfer { device_id } => {
                if !matches!(state, GeraeteubernahmeState::Beantwortet(_)) {
                    return Err(WorkflowError::invalid_state(
                        "Beantwortet",
                        state.status_str(),
                    ));
                }
                Ok(vec![GeraeteubernahmeEvent::Abgeschlossen { device_id }].into())
            }

            GeraeteubernahmeCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(WorkflowOutput::events(vec![]));
                }
                Ok(vec![GeraeteubernahmeEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    }
}

/// The ORDRSP answering `data`'s ORDERS with `antwort_code`, and the event that
/// records it.
///
/// The code must be published by the tree [`ordrsp_ebd`] names for the ORDERS
/// and Sparte; [`ordrsp_antwort_pid`] picks the PID from it, and `SG2 AJT`
/// carries DE 4465 the code and DE 1082 the **Codeliste** it comes from —
/// `S_0067`/`G_0061` and friends, never the EBD number (ORDRSP AHB 1.1b Kap.
/// 4). `SG1 RFF+ON` names the ORDERS by its `BGM` DE 1004 (Hinweise `[518]`,
/// `[535]`), and `BGM` DE 1001 is the Use-Case's own code. The segments a
/// column makes Muss beyond the code are checked by [`angaben_payload`].
fn ordrsp_antwort(
    data: &GeraeteubernahmeData,
    antwort_code: &str,
    bemerkung: Option<&str>,
    begruendung: Option<String>,
    angaben: &AntwortAngaben,
) -> Result<(GeraeteubernahmeEvent, PendingOutbox), WorkflowError> {
    let orders_pid = data.pid.as_u32();
    let tree = ordrsp_ebd(orders_pid, data.sparte).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "ORDERS {orders_pid} has no Entscheidungsbaum in Sparte {}",
            data.sparte
        ))
    })?;
    let code = mako_pruefung::codes::lookup(tree, antwort_code).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "Antwortcode {antwort_code:?} is not published in {tree}"
        ))
    })?;
    if code.braucht_bemerkung && bemerkung.is_none() {
        return Err(WorkflowError::rejected(format!(
            "{tree} {} ({}) requires a written Erläuterung",
            code.code, code.bedeutung
        )));
    }
    let antwort_pid = ordrsp_antwort_pid(orders_pid, code).ok_or_else(|| {
        WorkflowError::rejected(format!(
            "ORDERS {orders_pid} has no ORDRSP answer PID for {tree} {}",
            code.code
        ))
    })?;
    let codeliste = code.wire_codeliste().ok_or_else(|| {
        WorkflowError::rejected(format!("{tree} {} names no Codeliste", code.code))
    })?;

    let mut payload = serde_json::json!({
        "pid":               antwort_pid,
        "sender":            data.msba.as_str(),
        "receiver":          data.msbn.as_str(),
        "melo":              data.melo_id.as_str(),
        "antwort_code":      code.code,
        "antwort_codeliste": codeliste,
        "antwort_tree":      tree,
        "order_reference":   data.orders_nummer,
    });
    if let Some(dokumentcode) = ordrsp_dokumentcode(antwort_pid) {
        payload["document_code"] = serde_json::Value::String(dokumentcode.to_owned());
    }
    if let Some(ref t) = data.termin {
        payload["termin"] = serde_json::Value::String(t.clone());
    }
    if let Some(text) = bemerkung {
        payload["bemerkung"] = serde_json::Value::String(text.to_owned());
    }
    angaben_payload(&mut payload, data, antwort_pid, code.code, angaben)?;

    Ok((
        GeraeteubernahmeEvent::AntwortGesendet {
            pruefidentifikator: Pruefidentifikator::new(antwort_pid)
                .map_err(WorkflowError::rejected)?,
            antwort_code: code.code.to_owned(),
            antwort_ebd: tree.to_owned(),
            abgelehnt: ist_ablehnung(antwort_pid),
            begruendung,
        },
        PendingOutbox::new("ORDRSP", data.msbn.as_str(), payload).caused_by(0),
    ))
}

/// Write the segments `antwort_pid`'s column makes Muss beyond the code into
/// `payload`, or refuse by name the one the MSBA did not supply.
///
/// * **19001 / 19002** — `DTM+203` Ausführungsdatum (Kap. 4.9.2, S. 43) is the
///   date of the Geräteübernahme the ORDRSP confirms or refuses: „Mittels
///   ORDRSP bestätigt der MSBA die bestellte Geräteübernahme aus der ORDERS"
///   (ORDERS AHB 1.1b Kap. 4.9.1, S. 74), so it is the `DTM+203` of that
///   ORDERS 17001, carried as [`GeraeteubernahmeData::termin`]. `SG6
///   CTA`/`COM` is the MSBA's technischer Ansprechpartner (Hinweis `[500]`).
/// * **19015 with `ZB5`** — `SG3 NAD+Z22` Lieferadresse für Altgeräte (`[41]`).
/// * **19016 with `Z07`** — `SG3 NAD+VY` with the Eigentümer's MP-ID (`[42]`).
fn angaben_payload(
    payload: &mut serde_json::Value,
    data: &GeraeteubernahmeData,
    antwort_pid: u32,
    code: &str,
    angaben: &AntwortAngaben,
) -> Result<(), WorkflowError> {
    let refuse = |what: &str| {
        Err(WorkflowError::rejected(format!(
            "ORDRSP {antwort_pid} mit {code}: {what} (ORDRSP AHB 1.1b Kap. {})",
            if matches!(antwort_pid, 19001 | 19002) {
                "4.9.2"
            } else {
                "4.10"
            }
        )))
    };
    let bestellung = matches!(antwort_pid, 19001 | 19002);
    match (bestellung, &angaben.ansprechpartner) {
        (true, None) => {
            return refuse("SG6 CTA/COM, der technische Ansprechpartner, ist Muss und fehlt");
        }
        (true, Some(a)) if a.kommunikation.is_empty() => {
            return refuse("SG6 COM ist Muss — der Ansprechpartner nennt keinen Kommunikationsweg");
        }
        (true, Some(a)) => payload["ansprechpartner"] = serde_json::json!(a),
        (false, Some(_)) => {
            return refuse("SG6 CTA/COM steht nur auf 19001/19002");
        }
        (false, None) => {}
    }
    if bestellung {
        let Some(termin) = &data.termin else {
            return refuse("DTM+203 Ausführungsdatum ist Muss, und die ORDERS 17001 nannte keines");
        };
        payload["ausfuehrungsdatum"] = serde_json::Value::String(termin.clone());
        payload["sparte"] = serde_json::json!(data.sparte);
    }
    match (code, &angaben.lieferadresse) {
        ("ZB5", Some(a)) => payload["lieferadresse"] = serde_json::json!(a),
        ("ZB5", None) => {
            return refuse("SG3 NAD+Z22 Lieferadresse für Altgeräte ist Muss [41] und fehlt");
        }
        (_, Some(_)) => return refuse("SG3 NAD+Z22 steht nur neben ZB5 [41]"),
        (_, None) => {}
    }
    match (code, &angaben.eigentuemer) {
        ("Z07", Some(mp)) => {
            payload["eigentuemer"] = serde_json::Value::String(mp.as_str().to_owned());
        }
        ("Z07", None) => {
            return refuse("SG3 NAD+VY mit der MP-ID des Eigentümers ist Muss [42] und fehlt");
        }
        (_, Some(_)) => return refuse("SG3 NAD+VY steht nur neben Z07 [42]"),
        (_, None) => {}
    }
    Ok(())
}

/// The APERAK the receiver owes on an inbound ORDERS.
///
/// `reason` present → Verarbeitbarkeitsfehlermeldung (29001, `BGM+313`);
/// absent → Anerkennungsmeldung (29002, `BGM+312`), which Gas keeps off the
/// wire. `orders_ref` is the acknowledged ORDERS' `UNH` DE 0062.
fn aperak(
    from: &MarktpartnerCode,
    to: &MarktpartnerCode,
    orders_ref: &str,
    sparte: Sparte,
    reason: Option<&str>,
) -> PendingOutbox {
    let payload = match reason {
        Some(r) => AperakPayload::fehler(
            from.as_str(),
            to.as_str(),
            orders_ref,
            mako_engine::erc::codes::Z29,
            r,
        ),
        // Gas has no Anerkennungsmeldung: a processable message is acknowledged
        // by the Frist lapsing in silence (APERAK AHB 1.1 §2.3). The decision is
        // still recorded for the ERP; it never reaches the wire.
        None if !mako_fristen::aperak_hat_anerkennungsmeldung(sparte == Sparte::Gas) => {
            AperakPayload::anerkennung(from.as_str(), to.as_str(), orders_ref).off_the_wire()
        }
        None => AperakPayload::anerkennung(from.as_str(), to.as_str(), orders_ref),
    };
    PendingOutbox::typed(to.as_str(), &payload).caused_by(0)
}

/// The ORDRSP answer deadline, which is anchored differently per Use-Case.
///
/// * **17001 Bestellung** — an *Antwortfrist*: 2 Werktage after the arrival ÜT.
/// * **17009 Anzeige Gerätewechselabsicht** — a *Vorlauffrist*: the 2. Werktag
///   **before** the Gerätewechseltermin the message carries. Conflating the two
///   is how an answer that is already overdue on arrival looks on schedule.
///
/// A 17009 without a Termin cannot be scheduled at all; that is a rejected
/// order, not a default window.
/// The Kap. 3.1.2 Nr. 1 Vorlauffrist verdict for a 17009 Termin, or `None` when
/// the Termin is absent or unparseable — [`antwort_deadline`] refuses those with
/// its own message, and reporting them twice would say the same thing two ways.
fn vorlauf_verdict(
    termin: Option<&str>,
    received_at: OffsetDateTime,
) -> Option<mako_fristen::vorlauf::VorlaufVerdict> {
    let datum = termin.and_then(parse_yyyymmdd)?;
    let obligation = mako_fristen::vorlauf::vorlauf(GERAETEWECHSEL_TERMIN_VORLAUF_KEY)
        .expect("the Gerätewechseltermin Vorlauffrist is catalogued");
    Some(obligation.shape.check(
        mako_fristen::berlin_date(received_at),
        datum,
        HolidayCalendar::BdewMaKo,
    ))
}

fn antwort_deadline(
    orders_pid: u32,
    termin: Option<&str>,
    received_at: OffsetDateTime,
) -> Result<PendingDeadline, WorkflowError> {
    match orders_pid {
        17001 => Ok(PendingDeadline::new(
            ORDRSP_DEADLINE_LABEL,
            deadline_at_werktage(
                received_at,
                BESTAETIGUNG_FRIST_WT,
                HolidayCalendar::BdewMaKo,
            ),
        )),
        17009 => {
            let Some(datum) = termin.and_then(parse_yyyymmdd) else {
                return Err(WorkflowError::rejected(
                    "eine Anzeige der Gerätewechselabsicht muss den Gerätewechseltermin nennen — \
                     die Antwortfrist wird von ihm aus zurückgerechnet (WiM Teil 1 Kap. 3.1.2 Nr. 2)"
                        .to_owned(),
                ));
            };
            let mako_fristen::vorlauf::VorlaufShape::LatestWerktageBefore(wt) =
                mako_fristen::vorlauf::vorlauf(GERAETEWECHSELABSICHT_ANTWORT_KEY)
                    .expect("wim.antwort-geraetewechselabsicht is catalogued")
                    .shape
            else {
                unreachable!("{GERAETEWECHSELABSICHT_ANTWORT_KEY} is a LatestWerktageBefore window")
            };
            let due = mako_fristen::sub_werktage(datum, wt, HolidayCalendar::BdewMaKo);
            Ok(PendingDeadline::new(
                GERAETEWECHSELABSICHT_DEADLINE_LABEL,
                mako_fristen::end_of_day_berlin(due),
            ))
        }
        other => Err(WorkflowError::rejected(format!(
            "ORDERS {other} has no ORDRSP answer window in this workflow"
        ))),
    }
}

/// `YYYYMMDD` → a calendar date; `None` on anything else.
fn parse_yyyymmdd(raw: &str) -> Option<time::Date> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 8 {
        return None;
    }
    let year: i32 = digits[0..4].parse().ok()?;
    let month = time::Month::try_from(digits[4..6].parse::<u8>().ok()?).ok()?;
    let day: u8 = digits[6..8].parse().ok()?;
    time::Date::from_calendar_date(year, month, day).ok()
}

// ── Read-model projection ─────────────────────────────────────────────────────

/// Read-model record for a single WiM Geräteübernahme process stream.
#[derive(Debug)]
pub enum GeraeteubernahmeRecord {
    /// No `OrdersEmpfangen` event applied yet.
    New {
        /// Total events applied so far (should be 0).
        event_count: usize,
    },
    /// `OrdersEmpfangen` applied; process fields available.
    Active {
        /// Current lifecycle stage.
        status: &'static str,
        /// Messlokation the devices sit at.
        melo_id: MeLo,
        /// MP-ID of the incoming Messstellenbetreiber.
        msbn: MarktpartnerCode,
        /// MP-ID of this party (the abgebender MSB).
        msba: MarktpartnerCode,
        /// Gerätenummer (updated on `Abgeschlossen`).
        device_id: DeviceId,
        /// ORDERS PID that opened the process.
        pid: Pruefidentifikator,
        /// Sparte of the interchange.
        sparte: Sparte,
        /// Total events applied.
        event_count: usize,
    },
}

impl GeraeteubernahmeRecord {
    /// Current lifecycle status label.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::New { .. } => "New",
            Self::Active { status, .. } => status,
        }
    }

    /// Total events applied to this stream.
    #[must_use]
    pub fn event_count(&self) -> usize {
        match self {
            Self::New { event_count } | Self::Active { event_count, .. } => *event_count,
        }
    }

    /// Domain data for this record, or `None` while it is still `New`.
    #[must_use]
    pub fn active_data(&self) -> Option<GeraeteubernahmeRecordData<'_>> {
        match self {
            Self::New { .. } => None,
            Self::Active {
                melo_id,
                msbn,
                msba,
                device_id,
                pid,
                sparte,
                ..
            } => Some(GeraeteubernahmeRecordData {
                melo_id,
                msbn,
                msba,
                device_id,
                pid,
                sparte: *sparte,
            }),
        }
    }
}

/// Borrowed view of the domain fields in an `Active` [`GeraeteubernahmeRecord`].
#[derive(Debug, Clone, Copy)]
pub struct GeraeteubernahmeRecordData<'a> {
    /// Messlokation the devices sit at.
    pub melo_id: &'a MeLo,
    /// MP-ID of the incoming Messstellenbetreiber.
    pub msbn: &'a MarktpartnerCode,
    /// MP-ID of this party (the abgebender MSB).
    pub msba: &'a MarktpartnerCode,
    /// Gerätenummer.
    pub device_id: &'a DeviceId,
    /// ORDERS PID that opened the process.
    pub pid: &'a Pruefidentifikator,
    /// Sparte of the interchange.
    pub sparte: Sparte,
}

impl Default for GeraeteubernahmeRecord {
    fn default() -> Self {
        Self::New { event_count: 0 }
    }
}

/// In-process read model tracking WiM Geräteübernahme streams.
#[derive(Debug, Default)]
pub struct GeraeteubernahmeProjection {
    /// Map of stream ID → record.
    pub records: HashMap<String, GeraeteubernahmeRecord>,
    /// Highest event sequence number processed.
    pub last_seq: u64,
}

impl Projection for GeraeteubernahmeProjection {
    fn name(&self) -> &'static str {
        "GeraeteubernahmeProjection"
    }

    fn handle_event(&mut self, envelope: &EventEnvelope) {
        self.last_seq = self.last_seq.max(envelope.sequence_number);
        let record = self
            .records
            .entry(envelope.stream_id.as_str().to_owned())
            .or_default();

        let Ok(event) = envelope.decode::<GeraeteubernahmeEvent>() else {
            return;
        };

        match record {
            GeraeteubernahmeRecord::New { event_count }
            | GeraeteubernahmeRecord::Active { event_count, .. } => *event_count += 1,
        }

        match event {
            GeraeteubernahmeEvent::OrdersEmpfangen {
                pid,
                msbn,
                msba,
                melo_id,
                device_id,
                sparte,
                ..
            } => {
                let count = record.event_count();
                *record = GeraeteubernahmeRecord::Active {
                    status: "OrdersEmpfangen",
                    pid,
                    msbn,
                    msba,
                    melo_id,
                    device_id,
                    sparte,
                    event_count: count,
                };
            }
            GeraeteubernahmeEvent::ValidationPassed { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "ValidationPassed";
                }
            }
            GeraeteubernahmeEvent::AntwortGesendet { abgelehnt, .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = if abgelehnt {
                        "Abgelehnt"
                    } else {
                        "Beantwortet"
                    };
                }
            }
            GeraeteubernahmeEvent::Abgeschlossen { device_id } => {
                if let GeraeteubernahmeRecord::Active {
                    status,
                    device_id: d,
                    ..
                } = record
                {
                    *status = "Abgeschlossen";
                    *d = device_id;
                }
            }
            GeraeteubernahmeEvent::Abgelehnt { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "Abgelehnt";
                }
            }
            GeraeteubernahmeEvent::DeadlineExpired { .. } => {
                if let GeraeteubernahmeRecord::Active { status, .. } = record {
                    *status = "FristAbgelaufen";
                }
            }
        }
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const MELO: &str = "DE0000000001234567890000000000001";

    fn orders(pid: u32, sparte: Sparte, termin: Option<&str>) -> GeraeteubernahmeCommand {
        GeraeteubernahmeCommand::ReceiveOrders {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            melo_id: MeLo::new(MELO),
            device_id: DeviceId::new("1ESY1161234567"),
            document_date: "20260302".to_owned(),
            termin: termin.map(str::to_owned),
            message_ref: MessageRef::new("ORD-1"),
            orders_nummer: "ORD-DOC-1".to_owned(),
            validation_passed: true,
            validation_errors: vec![],
            sparte,
            received_at: time::macros::datetime!(2026-03-02 09:00 UTC),
        }
    }

    fn validated(pid: u32, sparte: Sparte, termin: Option<&str>) -> GeraeteubernahmeState {
        let s = GeraeteubernahmeState::default();
        let out = WimGeraeteubernahmeWorkflow::handle(&s, orders(pid, sparte, termin))
            .expect("valid ORDERS");
        out.events
            .iter()
            .fold(s, WimGeraeteubernahmeWorkflow::apply)
    }

    /// What the MSBA states beside `code`: the Ansprechpartner on a
    /// Bestellung, the Lieferadresse beside `ZB5`, the Eigentümer beside `Z07`.
    fn angaben(orders_pid: u32, code: &str) -> AntwortAngaben {
        AntwortAngaben {
            ansprechpartner: (orders_pid == 17001).then(|| Ansprechpartner {
                name: "Technik MSBA".to_owned(),
                kommunikation: vec![Kommunikationsweg::Email("technik@msba.example".to_owned())],
            }),
            lieferadresse: (code == "ZB5").then(|| Lieferadresse {
                name: "MSBA Lager GmbH".to_owned(),
                strasse: "Lagerweg 1".to_owned(),
                ort: "Köln".to_owned(),
                postleitzahl: Some("50667".to_owned()),
                land: "DE".to_owned(),
            }),
            eigentuemer: (code == "Z07").then(|| MarktpartnerCode::new("9900555000005")),
        }
    }

    /// A Gerätewechseltermin closer than the Mindestvorlaufzeit is refused with
    /// `E17` on the ORDRSP 19016 „Ablehnung Gerätewechselabsicht", not with an
    /// APERAK and not with a deadline already behind it.
    ///
    /// Kap. 3.1.2 Nr. 1 puts the Termin at the 4. Werktag after the Anzeige at
    /// the earliest, and Nr. 2 puts the answer 2 Werktage before the Termin. A
    /// Termin closer than four is a Frist the MSBN missed, and `E17` „Ablehnung
    /// wg. Fristüberschreitung" (EBD 4.3 Kap. 8.4.1, S. 540) is its code — a
    /// business answer, which ORDRSP AHB 1.1b Kap. 4.10 (S. 46, Bedingung
    /// `[39]`) puts on 19016. The ORDERS itself is processable, so the APERAK
    /// beside it is the Anerkennungsmeldung, never a Fehlermeldung.
    #[test]
    fn a_termin_inside_the_vorlauffrist_is_refused_with_e17() {
        use mako_engine::payload::OutboundPayload as _;
        // Anzeige on Mon 2026-03-02. The 4. Werktag after it is Fri 2026-03-06.
        for (sparte, codeliste) in [(Sparte::Strom, "S_0066"), (Sparte::Gas, "G_0060")] {
            for (termin, admissible) in [
                ("20260304", false), // Wed — 2 Werktage away
                ("20260305", false), // Thu — 3 Werktage away
                ("20260306", true),  // Fri — exactly 4, „frühestens am 4. WT"
                ("20260401", true),
            ] {
                let out = WimGeraeteubernahmeWorkflow::handle(
                    &GeraeteubernahmeState::default(),
                    orders(17_009, sparte, Some(termin)),
                )
                .expect("the ORDERS is well-formed either way");
                let ordrsp: Vec<_> = out
                    .outbox
                    .iter()
                    .filter(|o| &*o.message_type == "ORDRSP")
                    .collect();
                assert_eq!(
                    ordrsp.len(),
                    usize::from(!admissible),
                    "{sparte} Termin {termin}: expected admissible = {admissible}"
                );
                // No Verarbeitbarkeitsfehlermeldung either way: the ORDERS is
                // processable, and the Frist is a business answer.
                for o in out.outbox.iter().filter(|o| &*o.message_type == "APERAK") {
                    let aperak = AperakPayload::from_payload(&o.payload).expect("an APERAK");
                    assert!(!aperak.is_fehler(), "{sparte} {termin}: {aperak:?}");
                }
                if admissible {
                    continue;
                }
                let payload = &ordrsp[0].payload;
                assert_eq!(payload["pid"], 19_016, "{payload}");
                assert_eq!(payload["antwort_code"], "E17", "{payload}");
                assert_eq!(payload["antwort_codeliste"], codeliste, "{payload}");
                assert_eq!(payload["order_reference"], "ORD-DOC-1", "{payload}");
                assert_eq!(payload["document_code"], "Z13", "{payload}");

                let state = out.events.iter().fold(
                    GeraeteubernahmeState::default(),
                    WimGeraeteubernahmeWorkflow::apply,
                );
                assert!(state.is_terminal(), "a refused Anzeige does not stay open");
                // The operator sees the earliest Termin still reachable.
                let GeraeteubernahmeState::Abgelehnt { reason } = &state else {
                    panic!("refused states are Abgelehnt");
                };
                assert!(
                    reason.contains("E17") && reason.contains("2026-03-06"),
                    "the refusal names its code and the frühestmöglicher Termin: {reason}"
                );
                // The answer is out, so no ORDRSP window is opened.
                assert!(out.deadlines.is_empty(), "{:?}", out.deadlines);
                let antwort = out
                    .events
                    .iter()
                    .find_map(|e| e.antwort())
                    .expect("the ORDRSP states its code");
                assert_eq!(
                    antwort,
                    Antwortcode::new("E17", Some(ordrsp_ebd(17_009, sparte).expect("tree")))
                );
            }
        }
    }

    /// The Bestellung is answered with an ORDRSP that carries the code **and**
    /// the Codeliste it comes from.
    #[test]
    fn a_bestellung_is_answered_with_an_ordrsp() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "Z13".to_owned(),
                bemerkung: None,
                angaben: angaben(17001, "Z13"),
            },
        )
        .expect("Z13");
        assert_eq!(&*out.outbox[0].message_type, "ORDRSP");
        assert_eq!(out.outbox[0].payload["pid"], 19_001);
        assert_eq!(out.outbox[0].payload["antwort_code"], "Z13");
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "S_0067");
        assert_eq!(out.outbox[0].payload["antwort_tree"], "E_0247");
    }

    /// The cluster picks the PID. `Z32` is an Ablehnung, so it rides 19002 —
    /// deriving the PID from a boolean the caller passes lets the two disagree.
    #[test]
    fn the_cluster_picks_the_answer_pid() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "Z32".to_owned(),
                bemerkung: None,
                angaben: angaben(17001, "Z32"),
            },
        )
        .expect("Z32");
        assert_eq!(out.outbox[0].payload["pid"], 19_002);
        assert_eq!(out.outbox[0].payload["antwort_codeliste"], "S_0068");
    }

    /// One Prüfidentifikator, two Sparten, two Codelisten — the recipient MP-ID
    /// is the only thing that says which.
    #[test]
    fn the_sparte_picks_the_tree_on_a_shared_pid() {
        for (sparte, ebd, codeliste) in [
            (Sparte::Strom, "E_0247", "S_0067"),
            (Sparte::Gas, "E_2011", "G_0061"),
        ] {
            let out = WimGeraeteubernahmeWorkflow::handle(
                &validated(17001, sparte, Some("20260401")),
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: "Z13".to_owned(),
                    bemerkung: None,
                    angaben: angaben(17001, "Z13"),
                },
            )
            .expect("Z13");
            assert_eq!(out.outbox[0].payload["antwort_tree"], ebd, "{sparte}");
            assert_eq!(out.outbox[0].payload["antwort_codeliste"], codeliste);
        }
    }

    /// `ZB4` and `ZB5` divide the labour over who removes the old devices;
    /// neither refuses the Gerätewechsel, so both ride the 19015 „Bestätigung
    /// Gerätewechselabsicht" and leave the process open for the physical work.
    /// 19016 carries the refusals `E17` and `Z07` — ORDRSP AHB 1.1b Kap. 4.10
    /// (S. 46): `[38]` „Möglicher Wert: ZB4 oder ZB5", `[39]` „Möglicher Wert:
    /// E17 oder Z07". The Codeliste in DE 1082 still follows the code
    /// (`ZB5` is `S_0066`'s).
    #[test]
    fn the_geraetewechselabsicht_answer_is_a_division_of_labour() {
        for (code, pid, codeliste, offen) in [
            ("ZB4", 19_015, "S_0065", true),
            ("ZB5", 19_015, "S_0066", true),
            ("Z07", 19_016, "S_0066", false),
        ] {
            let state = validated(17009, Sparte::Strom, Some("20260401"));
            let out = WimGeraeteubernahmeWorkflow::handle(
                &state,
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: code.to_owned(),
                    bemerkung: None,
                    angaben: angaben(17009, code),
                },
            )
            .unwrap_or_else(|e| panic!("{code}: {e}"));
            assert_eq!(out.outbox[0].payload["pid"], pid, "{code}");
            assert_eq!(
                out.outbox[0].payload["antwort_codeliste"], codeliste,
                "{code}"
            );
            let state = out
                .events
                .iter()
                .fold(state, WimGeraeteubernahmeWorkflow::apply);
            assert_eq!(
                matches!(state, GeraeteubernahmeState::Beantwortet(_)),
                offen,
                "{code}: {}",
                state.status_str()
            );
        }
    }

    /// The answer to a Gerätewechselabsicht is a **Vorlauffrist** — the 2. WT
    /// before the Termin the message carries — not two Werktage after arrival.
    #[test]
    fn the_geraetewechselabsicht_window_is_anchored_on_the_termin() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17009, Sparte::Strom, Some("20260401")),
        )
        .expect("valid 17009");
        let dl = out
            .deadlines
            .iter()
            .find(|d| &*d.label == GERAETEWECHSELABSICHT_DEADLINE_LABEL)
            .expect("the Gerätewechselabsicht window");
        // 2026-04-01 is a Wednesday; two Werktage before is Monday 2026-03-30.
        assert_eq!(dl.due_at.date(), time::macros::date!(2026 - 03 - 30));
        // The Frist names a day, so it runs to the end of it, not to an hour
        // inside it.
        assert_eq!(
            dl.due_at,
            mako_fristen::end_of_day_berlin(time::macros::date!(2026 - 03 - 30))
        );
        // …and it is nowhere near „2 Werktage after arrival" on 2026-03-02.
        assert!(dl.due_at > time::macros::datetime!(2026-03-04 00:00 UTC));
    }

    /// A 17009 that names no Termin has no answer window at all. Defaulting to
    /// „two Werktage from now" would report a Frist the Festlegung does not
    /// state, on a message that is already malformed.
    #[test]
    fn a_geraetewechselabsicht_without_a_termin_is_refused() {
        let err = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17009, Sparte::Strom, None),
        )
        .expect_err("no Termin");
        assert!(err.to_string().contains("Gerätewechseltermin"), "{err}");
    }

    /// Gas has no Anerkennungsmeldung: a processable Gas ORDERS is acknowledged
    /// by the APERAK Frist lapsing in silence (APERAK AHB 1.1 §2.3). The
    /// decision is still recorded; `suppress_wire` keeps it off the wire.
    #[test]
    fn gas_suppresses_the_positive_aperak() {
        use mako_engine::payload::OutboundPayload as _;
        let strom = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17001, Sparte::Strom, Some("20260401")),
        )
        .expect("valid");
        let strom = AperakPayload::from_payload(&strom.outbox[0].payload).expect("an APERAK");
        assert_eq!(strom.pid, mako_engine::payload::APERAK_PID_ANERKENNUNG);
        assert_eq!(strom.orig_message_ref, "ORD-1");
        assert!(!strom.suppress_wire);

        let gas = WimGeraeteubernahmeWorkflow::handle(
            &GeraeteubernahmeState::default(),
            orders(17001, Sparte::Gas, Some("20260401")),
        )
        .expect("valid");
        assert!(
            AperakPayload::from_payload(&gas.outbox[0].payload)
                .expect("an APERAK")
                .suppress_wire
        );
    }

    /// A code from another tree never reaches the wire. `ZB4` is `E_0204`, not
    /// `E_0247` — and `E_0247` publishes no `ZB4`.
    #[test]
    fn a_foreign_code_is_refused() {
        let err = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Strom, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "ZB4".to_owned(),
                bemerkung: None,
                angaben: angaben(17001, "ZB4"),
            },
        )
        .expect_err("ZB4 is not an E_0247 code");
        assert!(err.to_string().contains("E_0247"), "{err}");
    }

    /// What a column makes Muss beyond the code is refused by name when the
    /// MSBA does not supply it — ORDRSP AHB 1.1b Kap. 4.9.2 (`SG6 CTA`/`COM`)
    /// and Kap. 4.10 (`[41]` `NAD+Z22` beside `ZB5`, `[42]` `NAD+VY` beside
    /// `Z07`) — and a segment the column does not list is refused as well.
    #[test]
    fn a_missing_or_misplaced_angabe_is_refused_by_name() {
        for (orders_pid, code, angaben, names) in [
            (17001, "Z13", AntwortAngaben::default(), "SG6 CTA/COM"),
            (17009, "ZB5", AntwortAngaben::default(), "NAD+Z22"),
            (17009, "Z07", AntwortAngaben::default(), "NAD+VY"),
            (17009, "ZB4", angaben(17009, "ZB5"), "NAD+Z22"),
            (17009, "ZB4", angaben(17001, "ZB4"), "SG6 CTA/COM"),
        ] {
            let err = WimGeraeteubernahmeWorkflow::handle(
                &validated(orders_pid, Sparte::Strom, Some("20260401")),
                GeraeteubernahmeCommand::DispatchAntwort {
                    antwort_code: code.to_owned(),
                    bemerkung: None,
                    angaben,
                },
            )
            .expect_err(code);
            assert!(err.to_string().contains(names), "{code}: {err}");
        }
    }

    /// A Bestellung answer states the Ausführungsdatum of the ORDERS 17001 it
    /// answers, in the Sparte that reads it as a day boundary.
    #[test]
    fn a_bestellung_answer_carries_the_ausfuehrungsdatum() {
        let out = WimGeraeteubernahmeWorkflow::handle(
            &validated(17001, Sparte::Gas, Some("20260401")),
            GeraeteubernahmeCommand::DispatchAntwort {
                antwort_code: "Z32".to_owned(),
                bemerkung: None,
                angaben: angaben(17001, "Z32"),
            },
        )
        .expect("Z32");
        assert_eq!(out.outbox[0].payload["pid"], 19_002);
        assert_eq!(out.outbox[0].payload["ausfuehrungsdatum"], "20260401");
        assert_eq!(out.outbox[0].payload["sparte"], "gas");
        assert_eq!(
            out.outbox[0].payload["ansprechpartner"]["kommunikation"][0]["art"],
            "email"
        );
    }
}

#[cfg(test)]
mod verlauf_tests {
    use super::*;
    use mako_engine::types::{MessageRef, Pruefidentifikator};

    fn pid(p: u32) -> Pruefidentifikator {
        Pruefidentifikator::new(p).expect("valid PID")
    }

    fn stated(richtung: Richtung, pid: Option<u32>, message_ref: Option<&str>) -> Verlauf {
        Verlauf {
            richtung,
            pid,
            message_ref: message_ref.map(str::to_owned),
        }
    }

    #[test]
    fn the_ordrsp_states_its_code_and_tree_and_a_completion_states_none() {
        let antwort = GeraeteubernahmeEvent::AntwortGesendet {
            pruefidentifikator: pid(19_001),
            antwort_code: "A01".into(),
            antwort_ebd: "E_0247".into(),
            abgelehnt: false,
            begruendung: None,
        };
        assert_eq!(
            antwort.antwort(),
            Some(Antwortcode::new("A01", Some("E_0247")))
        );
        assert_eq!(
            GeraeteubernahmeEvent::Abgelehnt { reason: "x".into() }.antwort(),
            None
        );
    }

    #[test]
    fn verlauf_states_the_messages_and_only_the_messages() {
        let antwort = GeraeteubernahmeEvent::AntwortGesendet {
            pruefidentifikator: pid(19_001),
            antwort_code: "A01".into(),
            antwort_ebd: "E_0247".into(),
            abgelehnt: false,
            begruendung: None,
        };
        assert_eq!(
            antwort.verlauf(),
            Some(stated(Richtung::Ausgang, Some(19_001), None))
        );
        let validiert = GeraeteubernahmeEvent::ValidationPassed {
            message_ref: MessageRef::new("ORD-1"),
        };
        assert_eq!(validiert.verlauf(), None);
        assert_eq!(
            GeraeteubernahmeEvent::Abgelehnt { reason: "x".into() }.verlauf(),
            None
        );
    }
}
