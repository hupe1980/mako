//! Inputs and outputs of the Messstellenbetreiber's WiM decisions.

use serde::{Deserialize, Serialize};
use time::Date;

use crate::antwort::{AntwortDetail, RejectReason};
use crate::codes::{AntwortCode, Cluster};

/// The outcome of a WiM MSB check.
///
/// The three variants exist for the same reason they do on the NB side: an
/// unfounded Ablehnung is a binding statement to the market, so a Prüfschritt
/// the caller's records cannot answer escalates rather than resolving to a
/// plausible code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MsbEntscheidung {
    /// Every applicable Prüfschritt passed; carries the Zustimmungscode the
    /// Bestätigung must state.
    ///
    /// `SG4 STS+E01` is Muss on every WiM Antwortnachricht, so an acceptance
    /// carries a code — `E15`, or `Z01` when the answer moves the date.
    Accept(AntwortDetail),
    /// A deterministic Prüfschritt failed.
    Reject(RejectReason),
    /// The decision needs a human.
    Escalate {
        /// What the operator has to establish.
        reason: String,
    },
}

impl MsbEntscheidung {
    /// Build an `Accept` from a published Zustimmungscode.
    ///
    /// # Panics
    ///
    /// In debug builds, when `code` is an Ablehnung.
    #[must_use]
    pub fn accept(tree: &'static str, code: &'static AntwortCode) -> Self {
        debug_assert_eq!(
            code.cluster,
            Cluster::Zustimmung,
            "{} is an Ablehnungscode and cannot carry a Bestätigung",
            code.code
        );
        Self::Accept(AntwortDetail::new(tree, code))
    }

    /// The Antwortcode this decision puts on the wire, for either cluster.
    #[must_use]
    pub fn antwortcode(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => Some(&a.antwortcode),
            Self::Reject(r) => Some(&r.antwort.antwortcode),
            Self::Escalate { .. } => None,
        }
    }

    /// The EBD the Antwortcode belongs to.
    #[must_use]
    pub fn ebd(&self) -> Option<&str> {
        match self {
            Self::Accept(a) => Some(&a.tree),
            Self::Reject(r) => Some(&r.antwort.tree),
            Self::Escalate { .. } => None,
        }
    }

    /// The date the answer confirms, when it differs from the requested one.
    ///
    /// `Z01`, `Z12` and `Z14` all assert a Terminänderung, and an answer that
    /// asserts one without naming the date is incomplete.
    #[must_use]
    pub const fn abweichender_termin(&self) -> Option<Date> {
        match self {
            Self::Accept(a) => a.abweichender_termin,
            Self::Reject(r) => r.antwort.abweichender_termin,
            Self::Escalate { .. } => None,
        }
    }
}

/// Whether an Anmeldung MSB sets the Messstellenbetrieb up for the first time.
///
/// It decides the Mindestvorlaufzeit — 7 Werktage instead of 15 (WiM Teil 1
/// Kap. 2.3.2 Nr. 1) — and it is carried in the message rather than derivable
/// from the NB's records, because a Messlokation being commissioned is not in
/// them yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Einrichtungsart {
    /// „Erstmalige Einrichtung" — 7 Werktage.
    ErstmaligeEinrichtung,
    /// „Wiederinbetriebnahme" — 15 Werktage.
    Wiederinbetriebnahme,
    /// „Bereits bestehender Messstellenbetrieb an dieser Messlokation" — the
    /// ordinary MSB-Wechsel, 15 Werktage.
    BestehenderMessstellenbetrieb,
}

impl Einrichtungsart {
    /// `true` for the case that takes the shortened Vorlauffrist.
    #[must_use]
    pub const fn ist_erstmalig(self) -> bool {
        matches!(self, Self::ErstmaligeEinrichtung)
    }
}

/// Which Sparte a WiM MSB-Wechsel Anfrage arrived in.
///
/// The Prüfschritte are identical — AWH WiM Gas 2.0 restates WiM Strom Teil 1
/// verbatim — but the alphabets are not: a Strom answer resolves against
/// `E_0200`/`E_0201`/`E_0202`/`E_0203` and a Gas one against
/// `E_2000`/`E_2002`/`E_2005`/`E_2004`. Naming the wrong tree yields a code the
/// counterparty's Codeliste does not contain.
///
/// Local to this crate: `mako-pruefung` is a pure library and takes no
/// dependency on the engine's type catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sparte {
    /// WiM Strom Teil 1 (BK6-22-024 Anlage 2a) — UTILMD 55xxx.
    #[default]
    Strom,
    /// AWH WiM Gas 2.0 — UTILMD 44xxx.
    Gas,
}

/// An inbound Anmeldung MSB (UTILMD 55042), as the NB reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnmeldungMsb {
    /// The Messlokation the MSBN wants assigned to it.
    pub melo_id: String,
    /// MP-ID of the anmeldender MSB.
    pub msbn_mp_id: String,
    /// `SG4 DTM+76` — the requested Zuordnungsbeginn.
    pub gewuenschter_zuordnungsbeginn: Date,
    /// Sparte of the interchange — it picks the Entscheidungsbaum.
    pub sparte: Sparte,
    /// Which of the three cases the message declares.
    pub einrichtungsart: Einrichtungsart,
    /// Whether the message carries the Versicherung über die Beauftragung
    /// durch den Anschlussnutzer, or — for a gMSB taking a Messlokation over
    /// because of the iMS rollout — the Versicherung that it does so.
    ///
    /// WiM Teil 1 Kap. 2.3.2 Nr. 1 Ziff. 2 makes one of the two mandatory and
    /// Nr. 2 Ziff. 1 makes checking it the NB's first duty.
    pub versicherung_liegt_vor: bool,
    /// Whether the NB's own records show the Messlokation.
    ///
    /// `None` when the lookup could not be performed — a transport failure is
    /// not evidence of absence, and `ZC9` on a Messlokation that exists refuses
    /// a lawful § 5 MsbG registration.
    pub melo_bekannt: Option<bool>,
    /// Whether a Vertrag nach § 9 Abs. 1 Nr. 3 MsbG with the MSBN exists.
    ///
    /// The third check of Kap. 2.3.2 Nr. 2. `None` escalates.
    pub msb_rahmenvertrag: Option<bool>,
}

/// An inbound Kündigung MSB (UTILMD 55039), as the **MSBA** reads it.
///
/// Note what is absent: there is no grid registry here and no Netzbetreiber.
/// The Kündigung runs on the contract layer between two MSB (WiM Teil 1
/// Kap. 2.1.3), so every Prüfschritt is a question about the MSBA's own
/// Messstellenbetriebsvertrag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KuendigungMsb {
    /// The Messlokation whose contract is being terminated.
    pub melo_id: String,
    /// MP-ID of the kündigender MSB, acting for the Anschlussnutzer.
    pub msbn_mp_id: String,
    /// Sparte of the interchange — it picks the Entscheidungsbaum.
    pub sparte: Sparte,
    /// What the Kündigung asks for.
    pub kuendigungstermin: Kuendigungstermin,
    /// The MSBA's own contract position at that Messlokation.
    pub vertragslage: Vertragslage,
}

/// „Ein beliebiger in der Zukunft liegender Kündigungstermin (auch
/// untermonatlich)" — as a fixed date, or as „zum nächstmöglichen Zeitpunkt".
///
/// The two are answered differently (WiM Teil 1 Kap. 2.2.1): a fixed date the
/// contract cannot honour is **refused** with the nächstmöglicher Termin named
/// in the Ablehnung, while „nächstmöglich" is **confirmed** with that same date
/// stated. Collapsing them into one `Option<Date>` loses which of the two the
/// MSBN asked for and therefore which answer it is owed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Kuendigungstermin {
    /// `SG4 DTM+93` — „Ende zum" a fixed date, 00:00 Uhr.
    Fix(Date),
    /// `SG4 DTM+471` — „Ende zum nächstmöglichen Termin".
    Naechstmoeglich,
}

/// The MSBA's contract position at the Messlokation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "lage", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Vertragslage {
    /// A live Messstellenbetriebsvertrag, terminable at `naechstmoeglich`.
    Laufend {
        /// The earliest date the contract's own Kündigungsfrist allows.
        naechstmoeglich: Date,
    },
    /// Already effectively terminated, ending at `vertragsende`.
    ///
    /// Kap. 2.2.3 tabulates all four constellations against this date.
    BereitsGekuendigt {
        /// The Vertragsende already in force.
        vertragsende: Date,
        /// The earliest date the contract situation would still allow, when
        /// the MSBA is willing to accept an even earlier end.
        ///
        /// `None` means the existing Vertragsende cannot be brought forward.
        frueher_moeglich: Option<Date>,
    },
    /// The contract already ended — nothing is left to terminate.
    Beendet,
    /// The MSBA is not the Messstellenbetreiber at this Messlokation.
    KeineZuordnung,
    /// The MSBA cannot decide from its own records.
    ///
    /// Escalates. „Unbekannt" is never `KeineZuordnung`: answering `ZC9`
    /// because a lookup failed refuses a lawful Kündigung.
    Unbekannt,
}

/// An inbound Ende Messstellenbetrieb (UTILMD 55051), as the NB reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbmeldungMsb {
    /// The Messlokation being released.
    pub melo_id: String,
    /// MP-ID of the abmeldender MSB.
    pub msba_mp_id: String,
    /// `SG4 DTM+76` — the requested Zuordnungsende.
    pub gewuenschtes_zuordnungsende: Date,
    /// Sparte of the interchange — it picks the Entscheidungsbaum.
    pub sparte: Sparte,
    /// Why the MSB is deregistering.
    pub grund: Abmeldegrund,
    /// Whether the NB's records show this MSB assigned to the Messlokation.
    ///
    /// `None` escalates.
    pub zuordnung_besteht: Option<bool>,
}

/// The Abmeldegrund of WiM Teil 1 Kap. 2.4.2 Nr. 1 Ziff. 1.
///
/// It decides which Vorlauffrist applies: an Außerbetriebnahme is reported
/// „unverzüglich nach Außerbetriebnahme" with the Zuordnungsende fixed to the
/// Folgetag of the Geräteausbau, so the 20-Werktage rule does not apply to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Abmeldegrund {
    /// Ende aufgrund Anschlussnutzerwechsel.
    AnschlussnutzerWechsel,
    /// Beendigung des MSB-Vertrags.
    VertragsEnde,
    /// Außerbetriebnahme (Stilllegung) der Messlokation.
    Ausserbetriebnahme,
}

impl Abmeldegrund {
    /// `true` when the 20-Werktage Mindestvorlauffrist applies.
    ///
    /// Kap. 2.4.2 Nr. 1: the Außerbetriebnahme is reported after the fact, so
    /// measuring a lead time against it manufactures a rejection on every
    /// Stilllegung.
    #[must_use]
    pub const fn hat_mindestvorlauffrist(self) -> bool {
        !matches!(self, Self::Ausserbetriebnahme)
    }

    /// The maximum Weiterverpflichtungszeitraum this Abmeldegrund permits.
    ///
    /// WiM Teil 1 Kap. 2.4.2 Nr. 4: „längstens drei Monate" on an
    /// Anschlussnutzerwechsel, „längstens einen Monat" in every other case.
    #[must_use]
    pub const fn max_weiterverpflichtung_monate(self) -> i64 {
        match self {
            Self::AnschlussnutzerWechsel => 3,
            Self::VertragsEnde | Self::Ausserbetriebnahme => 1,
        }
    }
}

/// An inbound Weiterverpflichtung (ORDERS 17002), as the **MSBA** reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeiterverpflichtungAuftrag {
    /// The Messlokation the NB wants kept in operation.
    pub melo_id: String,
    /// The Zuordnungsende the NB confirmed on the Abmeldung.
    pub bestaetigtes_zuordnungsende: Date,
    /// The date up to which the NB now wants the MSBA to continue.
    pub verschobenes_zuordnungsende: Date,
    /// Sparte of the interchange — it picks the Entscheidungsbaum.
    pub sparte: Sparte,
    /// The Abmeldegrund of the Ende Messstellenbetrieb this follows — it caps
    /// the Weiterverpflichtungszeitraum at three months or one.
    pub grund: Abmeldegrund,
    /// Whether the NB has already exhausted the maximum with an earlier
    /// Weiterverpflichtung on this Messlokation.
    ///
    /// `Z22` is available only on such a *further* ORDERS (EBD 4.3 `S_0062`);
    /// on a first one an overshoot is answered with `Z14` and the corrected
    /// date, not refused.
    pub bereits_ausgeschoepft: bool,
}
