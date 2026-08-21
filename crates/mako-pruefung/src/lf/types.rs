//! Inputs and outputs of the LF answer decision.
//!
//! Everything is `Clone + Debug + Serialize + Deserialize` so a caller can log
//! the exact inputs beside the decision — which is what a § 20 EnWG audit and a
//! BDEW clarification both ask for.

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::codes::AntwortCode;

// ── Request ───────────────────────────────────────────────────────────────────

/// What kind of object the Vorgang is about.
///
/// Read from `SG4 STS+7` DE 9013 element 3, the Transaktionsgrundergänzung.
/// Every LF-answered EBD opens by branching on it, so it is not optional
/// context: `E_0609` splits at Prüfschritt 10 and `E_0624` at Prüfschritt 10,
/// and the two halves use **different code ranges**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Lokationsart {
    /// `ZW4` — verbrauchende Marktlokation.
    VerbrauchendeMalo,
    /// `ZW3` — erzeugende Marktlokation.
    ErzeugendeMalo,
    /// `ZW5` — Tranche.
    Tranche,
    /// `ZAP` — ruhende Marktlokation (§ 20 Abs. 1d EnWG / § 10c EEG).
    RuhendeMalo,
}

impl Lokationsart {
    /// Parse the `STS+7` DE 9013 element 3 code.
    #[must_use]
    pub fn from_ergaenzung(code: &str) -> Option<Self> {
        match code {
            "ZW4" => Some(Self::VerbrauchendeMalo),
            "ZW3" => Some(Self::ErzeugendeMalo),
            "ZW5" => Some(Self::Tranche),
            "ZAP" => Some(Self::RuhendeMalo),
            _ => None,
        }
    }

    /// `true` for the branch `E_0609` and `E_0624` call „verbrauchende
    /// Marktlokation" — the one whose codes start at `A01` / `A30`.
    #[must_use]
    pub const fn ist_verbrauchend(self) -> bool {
        matches!(self, Self::VerbrauchendeMalo | Self::RuhendeMalo)
    }
}

/// An NB- or LFN-initiated request the supplier must answer.
///
/// Parsed at the transport boundary; no CloudEvent JSON reaches the decision
/// functions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfAnfrage {
    /// BDEW Prüfidentifikator of the **inbound** message (`55007`, `55010`,
    /// `55016`, `44007`, `44010`, `44016`).
    pub pid: u32,
    /// mako process UUID (the CloudEvent `subject`).
    pub process_id: Uuid,
    /// Marktlokations-ID from `SG5 LOC+Z16`.
    pub malo_id: String,
    /// `IDE+24` DE 7402 — echoed back on the answer.
    pub vorgangsnummer: Option<String>,
    /// MP-ID of the sender (the NB, or the LFN on a Kündigung).
    pub absender_mp_id: String,
    /// MP-ID the message is addressed to; must be one of our own.
    pub empfaenger_mp_id: String,
    /// `STS+7` DE 9013 element 3 — which object the Vorgang is about.
    pub lokationsart: Lokationsart,
    /// `STS+7` DE 9013 element 2 — the Transaktionsgrund (`E01`, `E03`, `Z33`, …).
    pub transaktionsgrund: Option<String>,
    /// `DTM+93` — the Zuordnungsende / Kündigungstermin the sender proposes.
    pub termin: Option<Date>,
    /// `DTM+154` — ÜT der Lieferanmeldung des LFN.
    ///
    /// Only `55010` carries it, and `E_0624` Prüfschritt 5 measures its own
    /// Frist from it.
    pub uet_lieferanmeldung: Option<Date>,
    /// When the message reached us — the „Nachrichteneingang" several
    /// Prüfschritte compare against.
    pub eingang: OffsetDateTime,
}

impl LfAnfrage {
    /// `true` when the named Transaktionsgrund matches.
    #[must_use]
    pub fn grund_ist(&self, code: &str) -> bool {
        self.transaktionsgrund.as_deref() == Some(code)
    }
}

// ── Supplier-side facts ───────────────────────────────────────────────────────

/// A fact the EBD asks about that the supplier's own systems may not know.
///
/// The EBDs ask questions like „Liegen dem LF Informationen darüber vor, dass
/// die Marktlokation nicht stillgelegt wird?" — a question whose honest answer
/// is sometimes „we have no record either way". Collapsing that into `false`
/// silently commits the supplier to a position; [`Unbekannt`] routes it to an
/// operator instead.
///
/// [`Unbekannt`]: Bekannt::Unbekannt
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Bekannt {
    /// The supplier's records say yes.
    Ja,
    /// The supplier's records say no.
    Nein,
    /// The supplier has no record — escalate rather than guess.
    Unbekannt,
}

impl Bekannt {
    /// Build from an `Option<bool>`; `None` becomes [`Bekannt::Unbekannt`].
    #[must_use]
    pub fn from_option(value: Option<bool>) -> Self {
        match value {
            Some(true) => Self::Ja,
            Some(false) => Self::Nein,
            None => Self::Unbekannt,
        }
    }
}

/// Where the Vollmacht stands on an inbound Kündigung (`E_0614` Prüfschritte
/// 90–110 / 600–620).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Vollmacht {
    /// The LFA did not ask for one — the ordinary case.
    NichtAngefordert,
    /// Requested and still outstanding. `E_0614` explicitly parks the process
    /// here („wartet an diesem Prüfschritt") rather than rejecting.
    AngefordertAusstehend,
    /// Received and accepted.
    Wirksam,
    /// Received and rejected as ineffective.
    Unwirksam,
}

/// What the supplier's own records say about this Marktlokation.
///
/// Assembled by the caller from the supply state (`marktd`) and the contract
/// (`vertragd`). Every field the EBDs interrogate is here; a field the caller
/// cannot fill stays `Unbekannt` and the decision escalates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfVertragslage {
    /// `true` when this LF supplies the MaLo today (Beliefert, Grund- or
    /// Ersatzversorgung with our own MP-ID).
    pub beliefert: bool,
    /// `true` when a Zuordnung still exists on the **day after** the requested
    /// Termin — the question `E_0624` Prüfschritt 20 asks.
    ///
    /// A customer in the Ersatz-/Grundversorgung counts as `true`, per the
    /// EBD's own Hinweis.
    pub zuordnung_am_folgetag: Bekannt,
    /// A Zuordnungsende the NB has already confirmed, if any.
    pub bestaetigtes_zuordnungsende: Option<Date>,
    /// The date the supply contract ends, if it is already terminated.
    pub vertragsende: Option<Date>,
    /// `true` when the Vertragsverhältnis still stands on the day after the
    /// requested Termin (`E_0624` Prüfschritte 90 / 220, `E_0614` 70 / 580).
    pub vertragsbindung_am_folgetag: Bekannt,
    /// `true` when the customer named in the request is the same person the
    /// LFA has on file (`E_0624` Prüfschritt 50).
    pub kunde_identisch: Bekannt,
    /// `true` when the LFA knows the customer did **not** move out
    /// (`E_0624` Prüfschritt 60).
    pub kunde_nicht_ausgezogen: Bekannt,
    /// `true` when this LF is also the Grundversorger for the Netzgebiet
    /// (`E_0624` Prüfschritt 70).
    pub ist_grundversorger: bool,
    /// `true` when the MaLo is in the Ersatzversorgung on the day after the
    /// requested Termin (`E_0624` Prüfschritt 80).
    pub in_ersatzversorgung_am_folgetag: Bekannt,
    /// `true` when the LF holds information that the MaLo is **not** being
    /// decommissioned (`E_0609` Prüfschritte 60 / 540).
    pub keine_stilllegung: Bekannt,
    /// `true` when the Zeitreihentyp changed to one that would need a
    /// Zuordnungsermächtigung (`E_0609` Prüfschritte 90 / 570).
    ///
    /// The EBD's Hinweis: when no ZRT change happened at all, this is `Ja`.
    pub zrt_wechsel_mit_ermaechtigung: Bekannt,
    /// `true` when the BKV deactivated the Zuordnungsermächtigung to the
    /// transmitted Lieferende (`E_0609` Prüfschritte 100 / 580).
    pub zuordnungsermaechtigung_deaktiviert: Bekannt,
    /// The Vorlauffrist window the AHB sets for this process, already
    /// evaluated by the caller (`E_0609` Prüfschritte 40 / 520).
    pub vorlauffrist_eingehalten: Bekannt,
    /// Where the Vollmacht stands (`E_0614` only).
    pub vollmacht: Vollmacht,
    /// `true` when the LFA already sent an Abmeldung to this Termin that the NB
    /// has not answered yet (`E_0624` Prüfschritte 30 / 210 „Zustimmung zum in
    /// der bereits versendeten Abmeldung genannten Termin").
    pub eigene_abmeldung_offen: bool,
}

impl Default for LfVertragslage {
    fn default() -> Self {
        Self {
            beliefert: false,
            zuordnung_am_folgetag: Bekannt::Unbekannt,
            bestaetigtes_zuordnungsende: None,
            vertragsende: None,
            vertragsbindung_am_folgetag: Bekannt::Unbekannt,
            kunde_identisch: Bekannt::Unbekannt,
            kunde_nicht_ausgezogen: Bekannt::Unbekannt,
            ist_grundversorger: false,
            in_ersatzversorgung_am_folgetag: Bekannt::Unbekannt,
            keine_stilllegung: Bekannt::Unbekannt,
            zrt_wechsel_mit_ermaechtigung: Bekannt::Unbekannt,
            zuordnungsermaechtigung_deaktiviert: Bekannt::Unbekannt,
            vorlauffrist_eingehalten: Bekannt::Unbekannt,
            vollmacht: Vollmacht::NichtAngefordert,
            eigene_abmeldung_offen: false,
        }
    }
}

// ── Decision ──────────────────────────────────────────────────────────────────

/// What the supplier answers, or why it cannot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "entscheidung", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LfEntscheidung {
    /// Send this Antwortcode. The code's [`Cluster`] decides which of the two
    /// answer PIDs carries it — the caller never chooses that separately.
    ///
    /// [`Cluster`]: crate::codes::Cluster
    Antwort(LfAntwort),
    /// The tree reached a question the supplier's records cannot answer.
    ///
    /// Queue it for an operator with the Frist attached; do **not** invent a
    /// code. An answer to the market is a binding statement about a contract.
    Eskalation {
        /// What the operator needs to decide, in the EBD's own terms.
        grund: String,
        /// The Prüfschritt the tree stopped at.
        pruefschritt: u16,
    },
}

/// A resolved answer: which code, with what it must be accompanied by.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LfAntwort {
    /// DE 9013 — the Antwortcode.
    pub code: String,
    /// DE 1131 — the EBD it comes from, absent on the Gas Codelisten.
    pub ebd: Option<String>,
    /// `true` when the code sits in the Zustimmungs-Cluster.
    pub zustimmung: bool,
    /// The BDEW wording, carried into the audit log and the operator queue.
    pub bedeutung: String,
    /// `FTX+ACB` Erläuterung — populated whenever the code requires one.
    pub bemerkung: Option<String>,
    /// The date the answer states.
    ///
    /// `A34` requires the LFA's own Lieferendedatum; the Gas `Z01` „Zustimmung
    /// mit Terminänderung" requires the alternative date. Otherwise the
    /// requested Termin is echoed.
    pub termin: Option<Date>,
    /// The Prüfschritt that produced the code — the audit trail the BDEW asks
    /// for in a clarification.
    pub pruefschritt: u16,
}

impl LfEntscheidung {
    /// Build an [`LfEntscheidung::Antwort`] from a catalogue entry.
    pub(crate) fn antwort(
        code: &'static AntwortCode,
        pruefschritt: u16,
        termin: Option<Date>,
        bemerkung: Option<String>,
    ) -> Self {
        Self::Antwort(LfAntwort {
            code: code.code.to_owned(),
            ebd: code.ebd.map(ToOwned::to_owned),
            // The LF trees are all on the Zustimmung/Ablehnung axis; a code
            // that is not would have no answer PID to select here.
            zustimmung: code.ist_zustimmung().unwrap_or(false),
            bedeutung: code.bedeutung.to_owned(),
            // A code the BDEW says must be explained gets a default explanation
            // rather than silently going out bare.
            bemerkung: bemerkung
                .or_else(|| code.braucht_bemerkung.then(|| code.bedeutung.to_owned())),
            termin,
            pruefschritt,
        })
    }

    pub(crate) fn eskalation(pruefschritt: u16, grund: impl Into<String>) -> Self {
        Self::Eskalation {
            grund: grund.into(),
            pruefschritt,
        }
    }

    /// The resolved answer, when the tree produced one.
    #[must_use]
    pub fn as_antwort(&self) -> Option<&LfAntwort> {
        match self {
            Self::Antwort(a) => Some(a),
            Self::Eskalation { .. } => None,
        }
    }

    /// `true` when the tree produced a Zustimmung.
    #[must_use]
    pub fn ist_zustimmung(&self) -> bool {
        self.as_antwort().is_some_and(|a| a.zustimmung)
    }

    /// `true` when the tree needs an operator.
    #[must_use]
    pub fn ist_eskalation(&self) -> bool {
        matches!(self, Self::Eskalation { .. })
    }
}
