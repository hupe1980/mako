//! LF answer automation — the processes a supplier is *asked* about.
//!
//! Seven inbound messages put this deployment in the answering seat. Each one
//! has a published set of Prüfschritte, its own Codeliste, and its own
//! Antwortfrist:
//!
//! | Sparte | Inbound | Process | EBD | Answers | Frist |
//! |---|---|---|---|---|---|
//! | Strom | **55007** | Lieferende von NB an LF | `E_0609` | 55008 / 55009 | 05:00 Uhr des 1. WT nach dem ÜT |
//! | Strom | **55010** | Beendigung der Zuordnung | `E_0624` | 55011 / 55012 | 09:00 Uhr des 1. WT nach dem ÜT |
//! | Strom | **55016** | Kündigung (LFN → LFA) | `E_0614` | 55017 / 55018 | Ablauf des 1. WT nach dem ÜT |
//! | Strom | **55607** | Ankündigung Zuordnung LF (erz. MaLo / Tranche) | `E_0603`–`E_0606` | 55608 / 55609 | 15:00 Uhr **am ÜT** |
//! | Gas | **44007** | Abmeldung NN vom NB | `E_3002` | 44008 / 44009 | Ablauf des 3. WT |
//! | Gas | **44010** | Abmeldeanfrage des NB | `E_3020` | 44011 / 44012 | Ablauf des 3. WT |
//! | Gas | **44016** | Kündigung beim alten Lieferanten | `E_3001` | 44017 / 44018 | Ablauf des 3. WT |
//!
//! **55607 is the one where silence is not a lapsed Frist.** Prozessschritt 3
//! of GPKE Teil 2 § 2.4 has the NB assign the supplier to the erzeugende
//! Marktlokation „aufgrund fehlender Antwort" anyway, using whichever
//! Bilanzkreis the supplier last communicated. Not answering does not stop it;
//! it only stops the supplier from choosing the Bilanzkreis.
//!
//! ## Two clocks
//!
//! An inbound UTILMD starts two independent timers:
//!
//! | Clock | Window | Owner |
//! |-------|--------|-------|
//! | Technical acknowledgement (APERAK) | **45 min** on weekdays, Sunday 12:00 Berlin for a Saturday arrival (APERAK AHB 1.0 § 2.4.1) | **`makod`**, automatically |
//! | Business answer | the per-PID Frist above | this module / the operator |
//!
//! The queue is bounded by the **business** window, from [`mako_fristen::antwort`].
//!
//! ## How the decision is made
//!
//! [`mako_pruefung`] walks the Prüfschritte; this module assembles the facts
//! they ask about — supply state from `marktd`, contract state from `vertragd` —
//! and routes the outcome:
//!
//! ```text
//! de.mako.process.initiated (an answerable PID)
//!   → build LfAnfrage from the CloudEvent
//!   → build LfVertragslage from marktd + vertragd
//!   → mako_pruefung::lf::pruefe_*(…)
//!       Antwort   → dispatch the makod command carrying the Antwortcode
//!       Eskalation → approval_queue, expiring an hour before the Frist
//! ```
//!
//! A fact the deployment cannot supply is [`Bekannt::Unbekannt`] and escalates,
//! naming the Prüfschritt — not "assume the ordinary case". A supplier that
//! agrees to every Beendigung der Zuordnung hands over its customers on request.
//!
//! ## Regulatory basis
//!
//! - **BK6-24-174 GPKE Teil 2**, **BK7-24-01-009 GeLi Gas 3.0** — the processes
//! - **EBD 4.3** — the Prüfschritte and Codelisten, in [`mako_pruefung`]
//! - **APERAK AHB 1.0 § 2.4.1** — the separate 45-minute technical window

use crate::config::BilanzkreisEintrag;
use mako_gpke::LfVorgangsdaten;
use mako_markt::makod_client::{ForwardCommand, MakodClient};
use mako_markt::repository::{LieferStatus, VersorgungsStatusRecord};
use mako_pruefung::{
    Bekannt, LfAnfrage, LfAntwort, LfEntscheidung, LfVertragslage, Lokationsart, Terminart,
    Vollmacht,
};
use secrecy::SecretString;
use time::{Date, OffsetDateTime};
use tracing::{info, warn};
use uuid::Uuid;

use crate::pg::approval::{ApprovalQueueEntry, PgApprovalQueue};

// ── Configuration ─────────────────────────────────────────────────────────────

/// Runtime configuration for the LF module.
#[derive(Debug, Clone)]
pub struct LfModuleConfig {
    pub marktd_url: String,
    pub marktd_api_key: SecretString,
    /// Base URL of `vertragd`, when this deployment runs the retail layer.
    ///
    /// Without it the contract-side Prüfschritte — Vertragsbindung, customer
    /// identity, Kündbarkeit — cannot be answered, and every decision that
    /// reaches one of them escalates. That is the honest outcome: those
    /// questions are about a contract, and a deployment with no contract
    /// database does not know the answer.
    pub vertragd_url: Option<String>,
    pub vertragd_api_key: Option<SecretString>,
    pub own_mp_id: String,
    pub tenant: String,
    /// When `true`, dispatch the resolved answer automatically.
    pub auto_respond: bool,
    /// This supplier's Bilanzkreise, by Regelzone and regime.
    ///
    /// Empty means every 55607 escalates. That is not a gap: the NB assigns the
    /// supplier regardless of the answer, so the only thing an invented
    /// Bilanzkreis would buy is generation booked into the wrong balancing
    /// circle rather than into the one the NB already had on file.
    pub bilanzkreise: Vec<BilanzkreisEintrag>,
}

/// Pick the Bilanzkreis a 55607 Zustimmung names, or `None` to let an operator
/// decide.
///
/// Two coordinates narrow the candidates: the [`Bilanzkreisart`] the
/// Anwendungsfall prescribes, and the **Bilanzierungsgebiet** of the
/// Marktlokation being assigned — the key MaBiS grants the
/// Zuordnungsermächtigung on. A row without a `bilanzierungsgebiet` is the
/// fallback, used only when no row names the MaLo's own.
///
/// `None` — and so an escalation — in four cases, all of them a real absence of
/// an answer rather than a gap:
///
/// - the Anwendungsfall does not fix the regime (`E_0603` covers EEG *and*
///   KWKG plants);
/// - the Bilanzierungsgebiet is unknown and there is no fallback row;
/// - the matching row lists no BK for that regime;
/// - the row lists **several**, which is the case the Festlegung leaves to the
///   supplier. Picking the first would be inventing a decision, and a BK the
///   BKV has not authorised for this (ZRT, BG, LF) comes back as a rejected
///   Zuordnung.
///
/// [`Bilanzkreisart`]: mako_pruefung::Bilanzkreisart
#[must_use]
pub fn resolve_bilanzkreis(
    eintraege: &[BilanzkreisEintrag],
    fall: Option<mako_pruefung::ZuordnungsFall>,
    bilanzierungsgebiet: Option<&str>,
) -> Option<String> {
    let art = fall?.bilanzkreisart()?;
    let row = bilanzierungsgebiet
        .and_then(|bg| {
            eintraege
                .iter()
                .find(|e| e.bilanzierungsgebiet.as_deref() == Some(bg))
        })
        .or_else(|| eintraege.iter().find(|e| e.bilanzierungsgebiet.is_none()))?;
    match row.candidates(art) {
        [only] => Some(only.clone()),
        _ => None,
    }
}

// ── Process descriptors ───────────────────────────────────────────────────────

/// One inbound process this module answers on the supplier's behalf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LfAntwortProcess {
    /// The inbound PID that triggers it.
    pub trigger_pid: u32,
    /// Human-readable process name, for logs and queue reasons.
    pub name: &'static str,
    /// The EBD whose Codeliste the answer is drawn from.
    pub ebd: &'static str,
    /// `makod` command for a Zustimmung.
    pub bestaetigen: &'static str,
    /// `makod` command for an Ablehnung.
    pub ablehnen: &'static str,
    /// Which walk decides it.
    pub walk: Walk,
}

/// Which published tree governs a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Lieferende von NB an LF — `E_0609` (Strom) / `E_3002` (Gas).
    Abmeldung,
    /// Beendigung der Zuordnung — `E_0624` (Strom) / `E_3020` (Gas).
    BeendigungZuordnung,
    /// Kündigung — `E_0614` (Strom) / `E_3001` (Gas).
    Kuendigung,
    /// Ankündigung Zuordnung LF — `E_0603`–`E_0606` (Strom only).
    ///
    /// GeLi Gas has no counterpart: assigning a supplier to an erzeugende
    /// Marktlokation is a Strom Bilanzkreis mechanic.
    ZuordnungLf,
}

/// Sparte, derived from the PID range.
const fn ist_gas(pid: u32) -> bool {
    pid >= 44_000 && pid < 45_000
}

/// **55007** Ankündigung der Beendigung der Zuordnung (Lieferende von NB an LF).
pub const NB_LIEFERENDE: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_007,
    name: "Lieferende von NB an LF",
    ebd: mako_pruefung::codes::EBD_ABMELDUNG,
    bestaetigen: mako_markt::commands::GPKE_NB_LIEFERENDE_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_NB_LIEFERENDE_ABLEHNEN,
    walk: Walk::Abmeldung,
};

/// **55010** Anfrage zur Beendigung der Zuordnung.
///
/// Despite the name this is Prozessschritt 3 of the *Lieferbeginn*: a new
/// supplier registered and the NB asks the incumbent to release the MaLo.
pub const BEENDIGUNG_ZUORDNUNG: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_010,
    name: "Beendigung der Zuordnung",
    ebd: mako_pruefung::codes::EBD_BEENDIGUNG_ZUORDNUNG,
    bestaetigen: mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    walk: Walk::BeendigungZuordnung,
};

/// **55016** Kündigung, sent LFN → LFA without the NB in between.
pub const KUENDIGUNG: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_016,
    name: "Kündigung",
    ebd: mako_pruefung::codes::EBD_KUENDIGUNG,
    bestaetigen: mako_markt::commands::GPKE_KUENDIGUNG_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_KUENDIGUNG_ABLEHNEN,
    walk: Walk::Kuendigung,
};

/// **55607** Ankündigung der Zuordnung des LF zur Marktlokation bzw. Tranche.
pub const ZUORDNUNG_LF: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 55_607,
    name: "Ankündigung Zuordnung LF",
    // The Anwendungsfall's own EBD replaces this once the inbound message names
    // one in `SG4 STS+E01` DE 1131; all four publish the same two codes.
    ebd: mako_pruefung::codes::EBD_ZUORDNUNG_LF[0],
    bestaetigen: mako_markt::commands::GPKE_ZUORDNUNG_LF_BESTAETIGEN,
    ablehnen: mako_markt::commands::GPKE_ZUORDNUNG_LF_ABLEHNEN,
    walk: Walk::ZuordnungLf,
};

/// **44007** Abmeldung NN vom NB.
pub const NB_LIEFERENDE_GAS: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 44_007,
    name: "Abmeldung NN vom NB (Gas)",
    ebd: mako_pruefung::codes::EBD_ABMELDUNG_GAS,
    bestaetigen: mako_markt::commands::GELI_NB_LIEFERENDE_BESTAETIGEN,
    ablehnen: mako_markt::commands::GELI_NB_LIEFERENDE_ABLEHNEN,
    walk: Walk::Abmeldung,
};

/// **44010** Abmeldungsanfrage des NB.
pub const BEENDIGUNG_ZUORDNUNG_GAS: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 44_010,
    name: "Abmeldeanfrage des NB (Gas)",
    ebd: mako_pruefung::codes::EBD_ABMELDUNGSANFRAGE_GAS,
    bestaetigen: mako_markt::commands::GELI_BEENDIGUNG_ZUORDNUNG_BESTAETIGEN,
    ablehnen: mako_markt::commands::GELI_BEENDIGUNG_ZUORDNUNG_ABLEHNEN,
    walk: Walk::BeendigungZuordnung,
};

/// **44016** Kündigung beim alten Lieferanten.
pub const KUENDIGUNG_GAS: LfAntwortProcess = LfAntwortProcess {
    trigger_pid: 44_016,
    name: "Kündigung beim alten Lieferanten (Gas)",
    ebd: mako_pruefung::codes::EBD_KUENDIGUNG_GAS,
    bestaetigen: mako_markt::commands::GELI_KUENDIGUNG_BESTAETIGEN,
    ablehnen: mako_markt::commands::GELI_KUENDIGUNG_ABLEHNEN,
    walk: Walk::Kuendigung,
};

/// The GPKE processes, compiled in only for `role-lf-strom`.
#[cfg(feature = "role-lf-strom")]
const STROM_PROCESSES: &[LfAntwortProcess] = &[
    NB_LIEFERENDE,
    BEENDIGUNG_ZUORDNUNG,
    KUENDIGUNG,
    ZUORDNUNG_LF,
];
#[cfg(not(feature = "role-lf-strom"))]
const STROM_PROCESSES: &[LfAntwortProcess] = &[];

/// The GeLi Gas processes, compiled in only for `role-lf-gas`.
#[cfg(feature = "role-lf-gas")]
const GAS_PROCESSES: &[LfAntwortProcess] =
    &[NB_LIEFERENDE_GAS, BEENDIGUNG_ZUORDNUNG_GAS, KUENDIGUNG_GAS];
#[cfg(not(feature = "role-lf-gas"))]
const GAS_PROCESSES: &[LfAntwortProcess] = &[];

/// Every process *this build* answers.
///
/// Sparte-scoped: a `role-lf-strom` deployment must not answer a GeLi Gas
/// Abmeldung, because it holds no Gas Lieferverhältnis to answer from. The
/// gate is on the routing table rather than the walks, which are shared.
pub fn lf_antwort_processes() -> impl Iterator<Item = &'static LfAntwortProcess> {
    STROM_PROCESSES.iter().chain(GAS_PROCESSES)
}

// ── CloudEvent → LfAnfrage ────────────────────────────────────────────────────

/// A parsed inbound event, ready for a walk.
#[derive(Debug, Clone)]
pub struct LfAnfragePayload {
    /// Which process this event belongs to.
    pub process: LfAntwortProcess,
    /// The request, in the shape [`mako_pruefung`] takes.
    pub anfrage: LfAnfrage,
    /// The business answer deadline and the operator window derived from it.
    pub window: mako_fristen::antwort::OperatorWindow,
    /// Which of the four 55607 Anwendungsfälle this is, when the deployment
    /// could name it.
    ///
    /// The inbound 55607 does **not** carry an EBD — the Anwendungsübersicht
    /// leaves the column empty for it and names `E_0603`–`E_0606` only on the
    /// answers. Which one applies follows from the Marktlokation being EEG or
    /// KWKG, with or without Direktvermarktungspflicht, tranchiert or not —
    /// facts `marktd` does not record. A deployment that enriches the event
    /// with `anwendungsfall` gets an automatic answer; otherwise the decision
    /// escalates with its Frist attached.
    pub zuordnungsfall: Option<mako_pruefung::ZuordnungsFall>,
}

impl LfAnfragePayload {
    /// Parse a `de.mako.process.initiated`, or `None` when the PID is not ours.
    #[must_use]
    pub fn parse(event: &serde_json::Value) -> Option<Self> {
        let data = &event["data"];
        let pid = event
            .get("makopid")
            .and_then(serde_json::Value::as_u64)
            .or_else(|| data.get("pid").and_then(serde_json::Value::as_u64))?;
        let process = *lf_antwort_processes().find(|p| u64::from(p.trigger_pid) == pid)?;

        let process_id: Uuid = event["subject"].as_str()?.parse().ok()?;
        let malo_id = data.get("malo_id")?.as_str()?.to_owned();

        let str_field = |key: &str| {
            data.get(key)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        };

        // The Sparte-neutral `SG4` contract both workflow crates publish. Read
        // through the shared type rather than key by key, so a producer that
        // renames a field breaks the build instead of silently escalating every
        // decision. A key that is absent, `null` or empty is all the same
        // thing: the message did not carry it.
        let vorgang = LfVorgangsdaten {
            transaktionsgrund: str_field("transaktionsgrund"),
            transaktionsgrund_ergaenzung: str_field("transaktionsgrund_ergaenzung"),
            vorgangsnummer: str_field("vorgangsnummer"),
            uet_lieferanmeldung: str_field("uet_lieferanmeldung"),
            naechstmoeglicher_termin: str_field("naechstmoeglicher_termin"),
        };

        // `STS+7` DE 9013 element 3 — the Transaktionsgrundergänzung, which is
        // the first thing every tree branches on. There is no safe default:
        // the two branches answer from different code ranges, so reading a
        // missing Ergänzung as „verbrauchende Marktlokation" sends `A10` where
        // the counterparty expects `A29`. It stays `None` and the walk
        // escalates at Prüfschritt 10.
        let lokationsart = vorgang
            .transaktionsgrund_ergaenzung
            .as_deref()
            .and_then(Lokationsart::from_ergaenzung);

        let event_time = event["time"]
            .as_str()
            .and_then(|s| {
                OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
            })
            .unwrap_or_else(OffsetDateTime::now_utc);

        // `SG4 DTM+93` „Ende zum" and `SG4 DTM+471` „Ende zum nächstmöglichen
        // Termin" are mutually exclusive on a 55016 / 44016, and `E_0614`
        // Prüfschritt 60 branches on which one arrived: only a Kündigung to a
        // fixed date may be refused for Vertragsbindung.
        //
        // The branch is decided by the *presence* of `DTM+471`, so it must be
        // read from the parsed value, not from key presence: `serde_json`
        // renders an absent `Option` as an explicit `null`, and
        // `Value::Null.is_some()` is `true`. Read that way every Kündigung looks
        // „zum nächstmöglichen Termin", which puts `A05`/`A06` out of reach and
        // releases customers still inside a Laufzeitvertrag.
        let terminart = if vorgang.naechstmoeglicher_termin.is_some() {
            Terminart::Naechstmoeglich
        } else {
            Terminart::Fix
        };

        let anfrage = LfAnfrage {
            pid: process.trigger_pid,
            process_id,
            malo_id,
            vorgangsnummer: vorgang.vorgangsnummer.clone(),
            absender_mp_id: str_field("grid_operator")
                .or_else(|| str_field("sender"))
                .unwrap_or_default(),
            empfaenger_mp_id: str_field("receiver").unwrap_or_default(),
            lokationsart,
            transaktionsgrund: vorgang.transaktionsgrund.clone(),
            termin: str_field("lieferende")
                .or_else(|| str_field("termin"))
                .or(vorgang.naechstmoeglicher_termin.clone())
                .or_else(|| str_field("process_date"))
                .as_deref()
                .and_then(parse_date),
            terminart,
            uet_lieferanmeldung: vorgang.uet_lieferanmeldung.as_deref().and_then(parse_date),
            eingang: event_time,
        };

        let window = mako_fristen::antwort::operator_window(process.trigger_pid, event_time);
        let zuordnungsfall = str_field("anwendungsfall")
            .or_else(|| str_field("antwort_ebd"))
            .as_deref()
            .and_then(mako_pruefung::ZuordnungsFall::from_ebd);
        Some(Self {
            process,
            anfrage,
            window,
            zuordnungsfall,
        })
    }
}

/// Accept both `YYYYMMDD` (EDIFACT) and `YYYY-MM-DD` (JSON) dates.
fn parse_date(s: &str) -> Option<Date> {
    if s.len() == 8 {
        Date::parse(s, time::macros::format_description!("[year][month][day]")).ok()
    } else {
        Date::parse(
            &s[..s.len().min(10)],
            time::macros::format_description!("[year]-[month]-[day]"),
        )
        .ok()
    }
}

// ── Fact gathering ────────────────────────────────────────────────────────────

/// Build the supply half of [`LfVertragslage`] from `marktd`.
///
/// Everything the contract database owns stays [`Bekannt::Unbekannt`] here and
/// is filled in by [`apply_vertrag`].
#[must_use]
pub fn lage_from_versorgung(
    anfrage: &LfAnfrage,
    versorgung: Option<&VersorgungsStatusRecord>,
    own_mp_id: &str,
) -> LfVertragslage {
    let termin = anfrage.termin;
    let Some(vs) = versorgung else {
        return LfVertragslage {
            vorlauffrist_eingehalten: abmeldung_vorlauffrist(anfrage, termin),
            ..LfVertragslage::default()
        };
    };

    // „Beliefert" for these trees means *we* hold the assignment — under our own
    // MP-ID, in any of the three states that are a supply.
    let ours = vs.lf_mp_id.as_deref() == Some(own_mp_id);
    let beliefert = ours
        && matches!(
            vs.lieferstatus,
            LieferStatus::Beliefert
                | LieferStatus::Grundversorgung
                | LieferStatus::Ersatzversorgung
        );

    // „Besteht zum Folgetag des genannten Termins eine Zuordnung?" — the
    // question E_0624 Prüfschritt 20 asks. A confirmed Lieferende on or before
    // the requested date answers it; otherwise the supply continues.
    let zuordnung_am_folgetag = match (beliefert, vs.lieferende, termin) {
        (false, _, _) => Bekannt::Nein,
        (true, Some(ende), Some(t)) if ende <= t => Bekannt::Nein,
        (true, _, _) => Bekannt::Ja,
    };

    // `E_0609` / `E_3002` Prüfschritt 40 — the one question in the LF's trees
    // that is arithmetic rather than a contract fact. Left unevaluated it
    // escalates every single Abmeldung, so the deployment answers it from the
    // dates the message itself carries.
    let vorlauffrist_eingehalten = abmeldung_vorlauffrist(anfrage, termin);

    LfVertragslage {
        beliefert,
        vorlauffrist_eingehalten,
        zuordnung_am_folgetag,
        // `lieferende` is set once the termination is agreed with the NB.
        bestaetigtes_zuordnungsende: vs.lieferende,
        in_ersatzversorgung_am_folgetag: Bekannt::from_option(Some(matches!(
            vs.lieferstatus,
            LieferStatus::Ersatzversorgung
        ))),
        ist_grundversorger: matches!(
            vs.lieferstatus,
            LieferStatus::Grundversorgung | LieferStatus::Ersatzversorgung
        ) && ours,
        ..LfVertragslage::default()
    }
}

/// `E_0609` Prüfschritt 40 / `E_3002` `E17` — was the Abmeldung announced far
/// enough ahead?
///
/// Only the two Abmeldung trees ask it, and only from the two dates the message
/// already carries: the Übertragungstag and the Zuordnungsende. Everything else
/// — which of GPKE Teil 2's four windows applies — follows from the
/// Transaktionsgrund and from whether the object is an EEG-Marktlokation.
///
/// A verbrauchende or ruhende Marktlokation can never be one, so the ordinary
/// Stilllegung decides here instead of escalating. An erzeugende Marktlokation
/// or a Tranche might be, and `marktd` does not record it, so those keep
/// escalating until a deployment can say — which is the honest answer, not a
/// gap: the EEG window is a month and the general one is a day, and guessing
/// the looser of the two accepts announcements the supplier could have refused.
fn abmeldung_vorlauffrist(anfrage: &LfAnfrage, termin: Option<Date>) -> Bekannt {
    use mako_fristen::abmeldung::AbmeldungVorlauf;

    if !matches!(anfrage.pid, 55_007 | 44_007) {
        return Bekannt::Unbekannt;
    }
    let (Some(grund), Some(zuordnungsende)) = (anfrage.transaktionsgrund.as_deref(), termin) else {
        return Bekannt::Unbekannt;
    };
    let eeg = match anfrage.lokationsart {
        Some(Lokationsart::VerbrauchendeMalo | Lokationsart::RuhendeMalo) => Some(false),
        _ => None,
    };
    AbmeldungVorlauf::fuer(grund, eeg)
        .check(
            anfrage.eingang.date(),
            zuordnungsende,
            mako_fristen::HolidayCalendar::BdewMaKo,
        )
        .map_or(Bekannt::Unbekannt, |verdict| {
            Bekannt::from_option(Some(verdict.is_ok()))
        })
}

/// Fold the contract facts from `vertragd` into the supply-derived Lage.
///
/// `vertrag` is the `GET /api/v1/vertraege/by-malo/{malo_id}` body.
#[must_use]
pub fn apply_vertrag(
    mut lage: LfVertragslage,
    vertrag: &serde_json::Value,
    termin: Option<Date>,
) -> LfVertragslage {
    let v = &vertrag["vertrag"];
    let vertragsende = v
        .get("vertragsende")
        .and_then(|x| x.as_str())
        .and_then(parse_date);
    lage.vertragsende = vertragsende;

    // „Bleibt das Vertragsverhältnis zum Tag nach dem Enddatum bestehen?"
    // A contract with no end date does; one ending on or before the requested
    // date does not. Without a requested date the question is unanswerable.
    lage.vertragsbindung_am_folgetag = match (termin, vertragsende) {
        (None, _) => Bekannt::Unbekannt,
        (Some(_), None) => Bekannt::Ja,
        (Some(t), Some(ende)) => Bekannt::from_option(Some(ende > t)),
    };

    // The next admissible termination date, which `vertragd` computes from the
    // Vertragsart, the Kündigungsfrist and § 41b EnWG. It is a *separate* fact
    // from `vertragsende`: `E_0614` Prüfschritt 80 distinguishes a contract
    // somebody already terminated to a later date (`A05`, whose 55018 carries
    // that date in `DTM+Z05`/`Z06`) from one merely still running (`A06`,
    // `DTM+157`). Writing it into `vertragsende` answered `A05` for every
    // running contract.
    let naechster = vertrag
        .get("naechstmoeglicher_kuendigungstermin")
        .and_then(|x| x.as_str())
        .and_then(parse_date);
    lage.naechstmoeglicher_kuendigungstermin = naechster;
    if let (Some(naechster), Some(t)) = (naechster, termin)
        && naechster > t
    {
        // Notice to a date earlier than the next admissible one is a
        // Vertragsbindung, whatever the stored Vertragsende says.
        lage.vertragsbindung_am_folgetag = Bekannt::Ja;
    }

    lage.kunde_identisch = Bekannt::from_option(
        v.get("kunde_identisch_mit_anfrage")
            .and_then(serde_json::Value::as_bool),
    );
    lage.kunde_nicht_ausgezogen = Bekannt::from_option(
        v.get("kunde_nicht_ausgezogen")
            .and_then(serde_json::Value::as_bool),
    );
    lage.vollmacht = Vollmacht::NichtAngefordert;
    lage
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Handle one `de.mako.process.initiated` for a PID this module answers.
///
/// Returns `true` when the event was handled — including when it escalated —
/// and `false` when the PID belongs to another module.
///
/// # Errors
///
/// Propagates `marktd` and `makod` transport failures so the fan-out redelivers
/// rather than silently dropping an obligation.
pub async fn process_lf_antwort(
    event: &serde_json::Value,
    config: &LfModuleConfig,
    reader: &mako_markt::marktd_client::MarktdClient,
    makod: &MakodClient,
    queue: &PgApprovalQueue,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let Some(payload) = LfAnfragePayload::parse(event) else {
        return Ok(false);
    };
    let anfrage = &payload.anfrage;

    info!(
        process_id = %anfrage.process_id,
        malo_id = %anfrage.malo_id,
        pid = payload.process.trigger_pid,
        process = payload.process.name,
        ebd = payload.process.ebd,
        "processd LF: evaluating"
    );

    let versorgung = reader.get_versorgung(&anfrage.malo_id).await.inspect_err(
        |e| warn!(%e, malo_id = %anfrage.malo_id, "processd LF: marktd fetch failed"),
    )?;

    let mut lage = lage_from_versorgung(anfrage, versorgung.as_ref(), &config.own_mp_id);

    // The contract half. A deployment without `vertragd` leaves these facts
    // unknown, and the walk escalates when it reaches one — rather than
    // assuming the supplier has no contract, which would agree to everything.
    if let Some(vertrag) = fetch_vertrag(config, &anfrage.malo_id).await {
        lage = apply_vertrag(lage, &vertrag, anfrage.termin);
    }

    // The Zuordnungsermächtigung is granted per Bilanzierungsgebiet (MaBiS
    // § 10.2.1), which is a Marktlokations-Stammdatum rather than a supply
    // state — so it is fetched only for the one process that needs it. A lookup
    // failure leaves it unknown, and the decision escalates rather than naming a
    // BK authorised for a different Bilanzierungsgebiet.
    let bilanzierungsgebiet = if payload.process.walk == Walk::ZuordnungLf {
        reader
            .get_malo(&anfrage.malo_id)
            .await
            .ok()
            .flatten()
            .and_then(|m| m.bilanzierungsgebiet)
    } else {
        None
    };

    let entscheidung = entscheide(
        &payload,
        &lage,
        &mako_pruefung::ZuordnungsLage {
            bilanzkreis: resolve_bilanzkreis(
                &config.bilanzkreise,
                payload.zuordnungsfall,
                bilanzierungsgebiet.as_deref(),
            ),
            fehler: None,
        },
    );

    info!(
        process_id = %anfrage.process_id,
        malo_id = %anfrage.malo_id,
        process = payload.process.name,
        outcome = ?entscheidung,
        "processd LF: decision"
    );

    let enqueue = async |reason: String| -> Result<(), sqlx::Error> {
        let entry = ApprovalQueueEntry::pending(
            anfrage.process_id,
            i32::try_from(payload.process.trigger_pid).unwrap_or(0),
            Some(anfrage.malo_id.clone()),
            format!(
                "{reason} (Antwortfrist {}: {})",
                payload.window.deadline, payload.window.source
            ),
            payload.window.expires_at,
            config.tenant.clone(),
        )
        .with_commands(
            payload.process.bestaetigen,
            payload.process.ablehnen,
            Some("LF"),
        );
        queue
            .enqueue(&entry)
            .await
            .inspect_err(|e| warn!(%e, "processd LF: failed to enqueue approval entry"))
    };

    match &entscheidung {
        LfEntscheidung::Antwort(antwort) => {
            if config.auto_respond {
                dispatch_antwort(makod, payload.process, anfrage, antwort).await?;
            } else {
                // auto_respond off means "an operator decides", not "nobody
                // answers": without a queue row the process goes unanswered and
                // unseen.
                enqueue(format!(
                    "auto_respond disabled — {} für MaLo {} ist entschieden: {} {} ({})",
                    payload.process.name,
                    anfrage.malo_id,
                    if antwort.zustimmung {
                        "Zustimmung"
                    } else {
                        "Ablehnung"
                    },
                    antwort.code,
                    antwort.bedeutung,
                ))
                .await?;
            }
        }
        LfEntscheidung::Eskalation {
            grund,
            pruefschritt,
        } => {
            warn!(
                process_id = %anfrage.process_id,
                malo_id = %anfrage.malo_id,
                ebd = payload.process.ebd,
                pruefschritt,
                %grund,
                "processd LF: escalated — creating approval_queue entry"
            );
            enqueue(format!(
                "{} Prüfschritt {pruefschritt}: {grund}",
                payload.process.ebd
            ))
            .await?;
        }
    }

    Ok(true)
}

/// Run the walk this process is governed by.
#[must_use]
pub fn entscheide(
    payload: &LfAnfragePayload,
    lage: &LfVertragslage,
    zuordnung: &mako_pruefung::ZuordnungsLage,
) -> LfEntscheidung {
    let process = payload.process;
    let anfrage = &payload.anfrage;
    let gas = ist_gas(process.trigger_pid);
    match (process.walk, gas) {
        (Walk::Abmeldung, false) => mako_pruefung::pruefe_abmeldung(anfrage, lage),
        (Walk::Abmeldung, true) => mako_pruefung::pruefe_abmeldung_gas(anfrage, lage),
        (Walk::BeendigungZuordnung, false) => {
            mako_pruefung::pruefe_beendigung_zuordnung(anfrage, lage)
        }
        (Walk::BeendigungZuordnung, true) => {
            mako_pruefung::pruefe_abmeldungsanfrage_gas(anfrage, lage)
        }
        (Walk::Kuendigung, false) => mako_pruefung::pruefe_kuendigung(anfrage, lage),
        (Walk::Kuendigung, true) => mako_pruefung::pruefe_kuendigung_gas(anfrage, lage),
        // Strom only, and the one walk whose Anwendungsfall the message does
        // not state. Without it there is no EBD to put in DE 1131, so the
        // answer would name no tree at all.
        (Walk::ZuordnungLf, _) => {
            mako_pruefung::pruefe_zuordnung(anfrage, payload.zuordnungsfall, zuordnung)
        }
    }
}

/// Send the resolved answer to `makod`.
///
/// The command is chosen by the Antwortcode's published Cluster, so a
/// Zustimmungscode can never be dispatched down the Ablehnung path.
async fn dispatch_antwort(
    makod: &MakodClient,
    process: LfAntwortProcess,
    anfrage: &LfAnfrage,
    antwort: &LfAntwort,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let command = if antwort.zustimmung {
        process.bestaetigen
    } else {
        process.ablehnen
    };
    let mut payload = serde_json::json!({
        "process_id":   anfrage.process_id,
        "malo_id":      anfrage.malo_id,
        "antwort_code": antwort.code,
        "zustimmung":   antwort.zustimmung,
    });
    if let Some(ebd) = &antwort.ebd {
        payload["antwort_ebd"] = serde_json::Value::String(ebd.clone());
    }
    if let Some(bemerkung) = &antwort.bemerkung {
        payload["bemerkung"] = serde_json::Value::String(bemerkung.clone());
    }
    if let Some(termin) = antwort.termin {
        payload["termin"] = serde_json::Value::String(
            termin
                .format(time::macros::format_description!("[year][month][day]"))
                .unwrap_or_default(),
        );
    }

    let cmd = ForwardCommand {
        marktrolle: None,
        command: command.to_owned(),
        malo_id: Some(anfrage.malo_id.clone()),
        melo_id: None,
        payload,
    };
    makod
        .post_command(
            &format!("processd-lf-{}-{}", antwort.code, anfrage.process_id),
            &cmd,
        )
        .await
        .inspect_err(|e| warn!(%e, command, "processd LF: answer dispatch failed"))?;
    info!(
        process_id = %anfrage.process_id,
        command,
        antwort_code = %antwort.code,
        "processd LF: dispatched answer"
    );
    Ok(())
}

/// Fetch the contract for a MaLo, or `None` when `vertragd` is not configured
/// or has no contract on file.
///
/// A transport failure is deliberately *not* an error: the walk then reaches an
/// unknown fact and escalates, which is the right outcome for "we could not
/// find out". Returning `Err` would instead make the fan-out retry an event
/// whose answer window may be minutes wide.
async fn fetch_vertrag(config: &LfModuleConfig, malo_id: &str) -> Option<serde_json::Value> {
    use secrecy::ExposeSecret;

    let base = config.vertragd_url.as_ref()?;
    let url = format!(
        "{}/api/v1/vertraege/by-malo/{malo_id}",
        base.trim_end_matches('/')
    );
    let mut req = reqwest::Client::new().get(&url);
    if let Some(key) = &config.vertragd_api_key {
        req = req.bearer_auth(key.expose_secret());
    }
    match req.send().await {
        Ok(resp) if resp.status().is_success() => resp.json().await.ok(),
        Ok(resp) => {
            if resp.status() != reqwest::StatusCode::NOT_FOUND {
                warn!(
                    status = %resp.status(), malo_id,
                    "processd LF: vertragd lookup failed — contract facts stay unknown \
                     and the decision escalates"
                );
            }
            None
        }
        Err(e) => {
            warn!(%e, malo_id, "processd LF: vertragd unreachable — the decision escalates");
            None
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use mako_markt::domain::MaloId;
    use time::macros::{date, datetime};

    const OWN: &str = "9900357000004";

    fn make_vs(status: LieferStatus, lf_mp_id: Option<&str>) -> VersorgungsStatusRecord {
        VersorgungsStatusRecord {
            malo_id: "51238696012".parse::<MaloId>().expect("valid MaLo"),
            lieferstatus: status,
            lf_mp_id: lf_mp_id.map(ToOwned::to_owned),
            lf_mp_id_next: None,
            lf_next_lieferbeginn: None,
            lieferbeginn: None,
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: "9900000000001".to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: OffsetDateTime::now_utc(),
            tenant: "9900000000002".to_owned(),
            version: 1,
        }
    }

    /// The 55010 request the `E_0624` tests share — a Wechsel to 2026-09-01
    /// on a verbrauchende Marktlokation.
    fn beendigung_anfrage() -> LfAnfrage {
        LfAnfragePayload::parse(&event(
            55_010,
            serde_json::json!({
                "transaktionsgrund": "E03",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20260901",
            }),
        ))
        .expect("parses")
        .anfrage
    }

    fn event(pid: u32, extra: serde_json::Value) -> serde_json::Value {
        let mut data = serde_json::json!({ "malo_id": "51238696012" });
        if let (Some(d), Some(e)) = (data.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                d.insert(k.clone(), v.clone());
            }
        }
        serde_json::json!({
            "makopid": pid,
            "subject": Uuid::nil().to_string(),
            "time": "2026-08-20T09:00:00Z",
            "data": data,
        })
    }

    /// Every answerable PID must parse.
    #[test]
    fn every_answerable_pid_is_routed() {
        for p in lf_antwort_processes() {
            let parsed = LfAnfragePayload::parse(&event(p.trigger_pid, serde_json::json!({})));
            assert!(
                parsed.is_some(),
                "PID {} ({}) is not routed",
                p.trigger_pid,
                p.name
            );
        }
    }

    /// A PID belonging to another module is not ours.
    #[test]
    fn a_foreign_pid_is_declined() {
        assert!(LfAnfragePayload::parse(&event(55_001, serde_json::json!({}))).is_none());
        // 55008 is the *answer* to 55007, never an inbound trigger.
        assert!(LfAnfragePayload::parse(&event(55_008, serde_json::json!({}))).is_none());
    }

    /// The Transaktionsgrundergänzung selects the branch, and therefore the
    /// code range.
    #[test]
    fn the_ergaenzung_selects_the_lokationsart() {
        let p = LfAnfragePayload::parse(&event(
            55_007,
            serde_json::json!({ "transaktionsgrund_ergaenzung": "ZW5" }),
        ))
        .expect("parses");
        assert_eq!(p.anfrage.lokationsart, Some(Lokationsart::Tranche));
    }

    /// A message without the Ergänzung has no branch, and the two branches
    /// answer from different code ranges. It stays unknown and the walk
    /// escalates rather than picking one.
    #[test]
    fn a_missing_ergaenzung_is_not_a_verbrauchende_malo() {
        let p = LfAnfragePayload::parse(&event(55_007, serde_json::json!({}))).expect("parses");
        assert_eq!(p.anfrage.lokationsart, None);
        let d = entscheide(&p, &LfVertragslage::default(), &Default::default());
        assert!(d.ist_eskalation(), "{d:?}");
    }

    /// The Zuordnungsermächtigung is granted per Bilanzierungsgebiet, so the row
    /// naming the Marktlokation's own BG wins over the fallback row.
    #[test]
    fn the_bilanzkreis_is_picked_per_bilanzierungsgebiet_and_regime() {
        let table = vec![
            BilanzkreisEintrag {
                bilanzierungsgebiet: None,
                eeg: vec!["BK-FALLBACK-EEG".into()],
                kwkg: vec!["BK-FALLBACK-KWKG".into()],
                standard: vec!["BK-FALLBACK-STD".into()],
            },
            BilanzkreisEintrag {
                bilanzierungsgebiet: Some("11YN-BG-EON---X".into()),
                eeg: vec!["BK-EON-EEG".into()],
                kwkg: vec![],
                standard: vec!["BK-EON-STD".into()],
            },
        ];
        let eeg = Some(mako_pruefung::ZuordnungsFall::EegMitDvPflicht);
        let ordinary = Some(mako_pruefung::ZuordnungsFall::KwkgNichtTranchiert);

        assert_eq!(
            resolve_bilanzkreis(&table, eeg, Some("11YN-BG-EON---X")).as_deref(),
            Some("BK-EON-EEG")
        );
        assert_eq!(
            resolve_bilanzkreis(&table, ordinary, Some("11YN-BG-EON---X")).as_deref(),
            Some("BK-EON-STD")
        );
        // A Bilanzierungsgebiet with no row of its own falls back, as does an
        // unknown one.
        assert_eq!(
            resolve_bilanzkreis(&table, eeg, Some("11YN-BG-OTHER-Y")).as_deref(),
            Some("BK-FALLBACK-EEG")
        );
        assert_eq!(
            resolve_bilanzkreis(&table, eeg, None).as_deref(),
            Some("BK-FALLBACK-EEG")
        );
    }

    /// **Several authorised BKs is a choice, not a default.** MaBiS § 10.2.1
    /// lets the BKV authorise more than one BK for the same (ZRT, BG, LF); the
    /// supplier picks. Picking the first in code would invent that decision.
    #[test]
    fn several_authorised_bilanzkreise_are_an_operator_choice() {
        let table = vec![BilanzkreisEintrag {
            bilanzierungsgebiet: None,
            eeg: vec!["BK-EEG-A".into(), "BK-EEG-B".into()],
            ..BilanzkreisEintrag::default()
        }];
        assert_eq!(
            resolve_bilanzkreis(
                &table,
                Some(mako_pruefung::ZuordnungsFall::EegMitDvPflicht),
                None
            ),
            None
        );
    }

    /// The other three ways the Bilanzkreis stays unknown. Reaching for a
    /// plausible neighbour books generation into a balancing circle that will
    /// not settle it, or into one the BKV never authorised.
    #[test]
    fn an_unresolvable_bilanzkreis_is_none_rather_than_a_neighbour() {
        let table = vec![BilanzkreisEintrag {
            bilanzierungsgebiet: Some("11YN-BG-EON---X".into()),
            eeg: vec!["BK-EON-EEG".into()],
            ..BilanzkreisEintrag::default()
        }];

        // `E_0603` covers EEG *and* KWKG plants, so the Anwendungsfall alone
        // does not fix the regime.
        assert_eq!(
            resolve_bilanzkreis(
                &table,
                Some(mako_pruefung::ZuordnungsFall::EegOhneDvPflicht),
                Some("11YN-BG-EON---X")
            ),
            None
        );
        // The matching row lists no BK for this regime, and there is no fallback.
        assert_eq!(
            resolve_bilanzkreis(
                &table,
                Some(mako_pruefung::ZuordnungsFall::KwkgNichtTranchiert),
                Some("11YN-BG-EON---X")
            ),
            None
        );
        // No Anwendungsfall at all.
        assert_eq!(
            resolve_bilanzkreis(&table, None, Some("11YN-BG-EON---X")),
            None
        );
    }

    /// 55607 is answered by the LFN, and it is the one process where missing
    /// the Frist does not simply lapse: the NB assigns the supplier anyway.
    /// Without the Anwendungsfall there is no EBD for DE 1131, so it escalates
    /// — with the 15:00-Uhr window attached, which is the point of routing it
    /// here at all.
    #[test]
    fn an_ankuendigung_zuordnung_without_its_anwendungsfall_escalates() {
        let p = LfAnfragePayload::parse(&event(
            55_607,
            serde_json::json!({ "transaktionsgrund_ergaenzung": "ZW3" }),
        ))
        .expect("parses");
        assert_eq!(p.process.trigger_pid, 55_607);
        assert!(p.window.is_regulatory, "55607 has a published window");

        let d = entscheide(
            &p,
            &LfVertragslage::default(),
            &mako_pruefung::ZuordnungsLage {
                bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                fehler: None,
            },
        );
        assert!(d.ist_eskalation(), "{d:?}");
    }

    /// Named the Anwendungsfall and the Bilanzkreis, it answers `A01` from that
    /// Fall's own EBD, and the Bilanzkreis rides the answer.
    #[test]
    fn a_named_anwendungsfall_answers_a01_with_its_bilanzkreis() {
        let p = LfAnfragePayload::parse(&event(
            55_607,
            serde_json::json!({
                "transaktionsgrund_ergaenzung": "ZW3",
                "anwendungsfall": "E_0604",
            }),
        ))
        .expect("parses");
        let d = entscheide(
            &p,
            &LfVertragslage::default(),
            &mako_pruefung::ZuordnungsLage {
                bilanzkreis: Some("11XBK-EEG-----1".to_owned()),
                fehler: None,
            },
        );
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "A01");
        assert!(a.zustimmung);
        assert_eq!(a.ebd.as_deref(), Some("E_0604"));
        assert_eq!(a.bemerkung.as_deref(), Some("11XBK-EEG-----1"));
    }

    /// The producer and the consumer must agree, and only a round trip
    /// through the real builder proves it. Building the event by hand in the
    /// test is what let the two drift: `serde_json` writes an absent `Option`
    /// as `null`, and `Value::Null.is_some()` is `true`.
    ///
    /// A Kündigung to a **fixed** date (`SG4 DTM+93`) must arrive as
    /// [`Terminart::Fix`], because that is the only branch `E_0614`
    /// Prüfschritt 60 lets the LFA refuse for Vertragsbindung.
    #[test]
    fn a_fixed_date_survives_the_round_trip_from_the_producer() {
        let vorgang = LfVorgangsdaten {
            transaktionsgrund: Some("E03".to_owned()),
            transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
            vorgangsnummer: Some("NNV1234".to_owned()),
            uet_lieferanmeldung: None,
            naechstmoeglicher_termin: None,
        };
        let payload = vorgang
            .process_initiated(
                mako_engine::types::Pruefidentifikator::new(55_016).expect("valid PID"),
                &mako_engine::types::MaLo::new("51238696012"),
                &mako_engine::types::MarktpartnerCode::new("9900357000004"),
                &mako_engine::types::MarktpartnerCode::new(OWN),
                "20260901",
                &serde_json::Value::Null,
            )
            .payload
            .clone();

        let p = LfAnfragePayload::parse(&event(55_016, payload)).expect("parses");
        assert_eq!(p.anfrage.terminart, Terminart::Fix);
        assert_eq!(p.anfrage.vorgangsnummer.as_deref(), Some("NNV1234"));
        assert_eq!(p.anfrage.transaktionsgrund.as_deref(), Some("E03"));
        assert_eq!(
            p.anfrage.lokationsart,
            Some(Lokationsart::VerbrauchendeMalo)
        );
    }

    /// And the Gas twin, built by the *other* workflow crate, must parse the
    /// same way — one contract, two Festlegungen. A Gas payload missing the
    /// Ergänzung escalates every 44016 at Prüfschritt 10.
    #[test]
    fn the_gas_producer_publishes_the_same_contract() {
        let vorgang = LfVorgangsdaten {
            transaktionsgrund: Some("E03".to_owned()),
            transaktionsgrund_ergaenzung: Some("ZW4".to_owned()),
            vorgangsnummer: Some("GASV1".to_owned()),
            uet_lieferanmeldung: None,
            naechstmoeglicher_termin: Some("20261231".to_owned()),
        };
        let payload = vorgang
            .process_initiated(
                mako_engine::types::Pruefidentifikator::new(44_016).expect("valid PID"),
                &mako_engine::types::MaLo::new("51238696012"),
                &mako_engine::types::MarktpartnerCode::new("9900357000004"),
                &mako_engine::types::MarktpartnerCode::new(OWN),
                "20261231",
                &serde_json::json!({ "bilanzierungsmethode": "SLP" }),
            )
            .payload
            .clone();

        let p = LfAnfragePayload::parse(&event(44_016, payload)).expect("parses");
        assert_eq!(p.anfrage.terminart, Terminart::Naechstmoeglich);
        assert_eq!(
            p.anfrage.lokationsart,
            Some(Lokationsart::VerbrauchendeMalo)
        );
        assert_eq!(p.anfrage.vorgangsnummer.as_deref(), Some("GASV1"));
    }

    /// `SG4 DTM+471` is the ordinary LFW24 Kündigung, and `E_0614` confirms it
    /// even inside a Vertragsbindung — the LFA answers with the date it
    /// determined.
    #[test]
    fn a_kuendigung_zum_naechstmoeglichen_termin_is_confirmed() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let payload = LfAnfragePayload::parse(&event(
            55_016,
            serde_json::json!({
                "transaktionsgrund_ergaenzung": "ZW4",
                "naechstmoeglicher_termin": "20260901",
            }),
        ))
        .expect("parses");
        assert_eq!(payload.anfrage.terminart, Terminart::Naechstmoeglich);

        let lage = apply_vertrag(
            lage_from_versorgung(&payload.anfrage, Some(&vs), OWN),
            &serde_json::json!({
                "vertrag": { "vertragsende": serde_json::Value::Null },
                "naechstmoeglicher_kuendigungstermin": "2026-12-31",
            }),
            payload.anfrage.termin,
        );

        let d = entscheide(&payload, &lage, &Default::default());
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "A09");
        assert!(a.zustimmung, "refusing this blocks every LFW24 switch");
        assert_eq!(a.termin, Some(date!(2026 - 12 - 31)));
    }

    /// The ordinary Stilllegung decides its own Vorlauffrist instead of
    /// escalating: both dates are in the message.
    #[test]
    fn a_verbrauchende_abmeldung_evaluates_its_own_vorlauffrist() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let payload = LfAnfragePayload::parse(&event(
            55_007,
            serde_json::json!({
                "transaktionsgrund": "Z33",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20261201",
            }),
        ))
        .expect("parses");
        let lage = lage_from_versorgung(&payload.anfrage, Some(&vs), OWN);
        assert_eq!(lage.vorlauffrist_eingehalten, Bekannt::Ja);

        // Announced the day before the Zuordnungsende: far too late.
        let late = LfAnfragePayload::parse(&event(
            55_007,
            serde_json::json!({
                "transaktionsgrund": "Z33",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20260821",
            }),
        ))
        .expect("parses");
        let lage = lage_from_versorgung(&late.anfrage, Some(&vs), OWN);
        assert_eq!(lage.vorlauffrist_eingehalten, Bekannt::Nein);
        assert_eq!(
            entscheide(&late, &lage, &Default::default())
                .as_antwort()
                .expect("answer")
                .code,
            "A03"
        );
    }

    /// An erzeugende Marktlokation might be an EEG one, whose window is a month
    /// rather than a day. `marktd` does not record it, so the check stays
    /// unknown instead of applying the looser window.
    #[test]
    fn an_erzeugende_abmeldung_keeps_escalating_its_vorlauffrist() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let payload = LfAnfragePayload::parse(&event(
            55_007,
            serde_json::json!({
                "transaktionsgrund": "Z33",
                "transaktionsgrund_ergaenzung": "ZW3",
                "lieferende": "20261201",
            }),
        ))
        .expect("parses");
        let lage = lage_from_versorgung(&payload.anfrage, Some(&vs), OWN);
        assert_eq!(lage.vorlauffrist_eingehalten, Bekannt::Unbekannt);
    }

    /// A supplier that does not hold the MaLo cannot state a contract position
    /// on it: the walk escalates rather than agreeing.
    #[test]
    fn a_malo_we_do_not_supply_escalates() {
        let vs = make_vs(LieferStatus::Beliefert, Some("9900999000001"));
        let lage = lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN);
        assert!(!lage.beliefert);
        assert_eq!(lage.zuordnung_am_folgetag, Bekannt::Nein);
    }

    /// An unknown MaLo leaves every fact unknown — and `Default` must not be a
    /// set of convenient `false`s.
    #[test]
    fn an_unknown_malo_leaves_the_facts_unknown() {
        let lage = lage_from_versorgung(&beendigung_anfrage(), None, OWN);
        assert_eq!(lage.vertragsbindung_am_folgetag, Bekannt::Unbekannt);
        assert_eq!(lage.kunde_identisch, Bekannt::Unbekannt);
        assert_eq!(lage.zuordnung_am_folgetag, Bekannt::Unbekannt);
    }

    /// Without contract facts a Beendigung der Zuordnung escalates. Agreeing
    /// would release a customer the supplier may still hold under contract.
    #[test]
    fn without_contract_facts_a_beendigung_escalates_rather_than_agreeing() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let lage = lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN);
        let payload = LfAnfragePayload::parse(&event(
            55_010,
            serde_json::json!({
                "transaktionsgrund": "E03",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20260901",
            }),
        ))
        .expect("parses");

        let d = entscheide(&payload, &lage, &Default::default());
        assert!(
            d.ist_eskalation(),
            "a supplier with no contract data must not agree to release a customer: {d:?}"
        );
    }

    /// With the contract on file and a Vertragsbindung, the answer is `A35`.
    #[test]
    fn a_running_contract_answers_a35() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let termin = Some(date!(2026 - 09 - 01));
        let lage = apply_vertrag(
            lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN),
            &serde_json::json!({ "vertrag": { "vertragsende": "2027-12-31" } }),
            termin,
        );
        let payload = LfAnfragePayload::parse(&event(
            55_010,
            serde_json::json!({
                "transaktionsgrund": "E03",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20260901",
            }),
        ))
        .expect("parses");

        let d = entscheide(&payload, &lage, &Default::default());
        let a = d.as_antwort().expect("answer");
        assert_eq!(a.code, "A35");
        assert!(!a.zustimmung);
        assert_eq!(a.ebd.as_deref(), Some("E_0624"));
    }

    /// A contract ending before the requested date releases the MaLo — `A36`.
    #[test]
    fn a_contract_ending_first_answers_a36() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let termin = Some(date!(2026 - 09 - 01));
        let lage = apply_vertrag(
            lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN),
            &serde_json::json!({ "vertrag": { "vertragsende": "2026-08-31" } }),
            termin,
        );
        let payload = LfAnfragePayload::parse(&event(
            55_010,
            serde_json::json!({
                "transaktionsgrund": "E03",
                "transaktionsgrund_ergaenzung": "ZW4",
                "lieferende": "20260901",
            }),
        ))
        .expect("parses");

        let d = entscheide(&payload, &lage, &Default::default());
        assert_eq!(d.as_antwort().expect("answer").code, "A36");
        assert!(d.ist_zustimmung());
    }

    /// A Kündigung to a date before the next admissible one is a
    /// Vertragsbindung even when the contract has no stored end date.
    #[test]
    fn notice_before_the_next_admissible_date_is_a_vertragsbindung() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let termin = Some(date!(2026 - 09 - 01));
        let lage = apply_vertrag(
            lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN),
            &serde_json::json!({
                "vertrag": { "vertragsende": serde_json::Value::Null },
                "naechstmoeglicher_kuendigungstermin": "2026-12-31",
            }),
            termin,
        );
        assert_eq!(lage.vertragsbindung_am_folgetag, Bekannt::Ja);
        // …and the date lands in its own field. Writing it into `vertragsende`
        // made `E_0614` answer `A05` („bereits in der Zukunft beendet") for
        // every contract that was merely still running.
        assert_eq!(lage.vertragsende, None);
        assert_eq!(
            lage.naechstmoeglicher_kuendigungstermin,
            Some(date!(2026 - 12 - 31))
        );
    }

    /// The Gas Kündigung answers from its own Codeliste — `Z12`, not `A06` — and
    /// `Z12` must state the **next admissible** Kündigungszeitpunkt, never the
    /// requested one: it exists precisely to say the contract is still bound at
    /// the date the LFN asked for.
    #[test]
    fn the_gas_kuendigung_uses_gas_codes() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let termin = Some(date!(2026 - 09 - 01));
        let lage = apply_vertrag(
            lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN),
            &serde_json::json!({
                "vertrag": { "vertragsende": "2027-12-31" },
                "naechstmoeglicher_kuendigungstermin": "2027-12-31",
            }),
            termin,
        );
        let payload = LfAnfragePayload::parse(&event(
            44_016,
            serde_json::json!({ "termin": "20260901", "transaktionsgrund_ergaenzung": "ZW4" }),
        ))
        .expect("parses");

        let a = entscheide(&payload, &lage, &Default::default())
            .as_antwort()
            .cloned()
            .expect("answer");
        assert_eq!(a.code, "Z12");
        assert_eq!(a.termin, Some(date!(2027 - 12 - 31)));
        assert!(a.ebd.is_none(), "the Gas MIG names no Codeliste in DE 1131");
    }

    /// Without that date there is nothing to put in the Muss segment, and
    /// echoing the requested one would tell the LFN it may terminate to the very
    /// date `Z12` is refusing.
    #[test]
    fn a_gas_vertragsbindung_without_a_next_date_escalates() {
        let vs = make_vs(LieferStatus::Beliefert, Some(OWN));
        let lage = apply_vertrag(
            lage_from_versorgung(&beendigung_anfrage(), Some(&vs), OWN),
            &serde_json::json!({ "vertrag": { "vertragsende": "2027-12-31" } }),
            Some(date!(2026 - 09 - 01)),
        );
        let payload = LfAnfragePayload::parse(&event(
            44_016,
            serde_json::json!({ "termin": "20260901", "transaktionsgrund_ergaenzung": "ZW4" }),
        ))
        .expect("parses");
        assert!(entscheide(&payload, &lage, &Default::default()).ist_eskalation());
    }

    /// The queue window is the business Frist, not the 45-minute APERAK clock.
    #[test]
    fn the_queue_window_is_the_business_frist() {
        let p = LfAnfragePayload::parse(&event(55_007, serde_json::json!({}))).expect("parses");
        assert!(p.window.is_regulatory);
        assert!(p.window.expires_at < p.window.deadline);
        assert!(p.window.deadline > datetime!(2026-08-20 09:00 UTC));
    }
}
