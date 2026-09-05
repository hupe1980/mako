//! `mako-nbw` — **Netzbetreiberwechsel**: the § 46 EnWG handover of every
//! Lokation in a grid area from the outgoing to the incoming Netzbetreiber.
//!
//! A NB-Wechsel is triggered by any event that changes the **MP-ID of the
//! Netzbetreiber at a Lokation** — a Konzessionsübergang, a Netzverkauf, an
//! Ausgründung einer Tochtergesellschaft. It is a bulk migration measured in
//! months across every Marktlokation of a Netzgebiet, not an event-driven
//! per-message workflow, and that is what this crate models: the identity of
//! the migration ([`PaketId`]), the instant it lands on ([`Aenderungszeitpunkt`])
//! and the ordered calendar of what must have been sent by when
//! ([`Fristenkalender`]).
//!
//! # NB-Wechsel has no message family of its own
//!
//! This is the thing to know before reading anything else. There is **no NBW
//! Prüfidentifikator, no NBW AHB and no NBW EDIFACT message**. The BDEW
//! Anwendungshilfe sequences Use-Cases that already exist in GPKE, MaBiS and
//! WiM, and adds two exchanges that are explicitly *not* standardised:
//!
//! | Kapitel | What actually goes on the wire |
//! |---|---|
//! | 6.1, 7.2 | „Übermittlung von Informationen" (GPKE Teil 4) |
//! | 6.2, 9.1 | a Liste der Lokationen — **NON-EDIFACT** |
//! | 6.3, 6.4 | Lokationsbündelstruktur, DB and ergänzende Daten NBA → NBN |
//! | 7.1, 7.5 | „Stammdatenänderung vom NB verantwortlich (ausgehend)" (GPKE Teil 4) |
//! | 7.3 | Profildefinitionen and normierte Profile (MaBiS); Preisblatt (GPKE Teil 2) |
//! | 7.4 | an Informationsschreiben in Textform — **NON-EDIFACT** |
//! | 7.5 | Abrechnungsdaten Netznutzung/Bilanzkreis, Stammdaten zur Bilanzkreistreue (GPKE Teil 2/4), Berechnungsformel (WiM Strom Teil 2) |
//! | 8 | „Stammdatenänderung vom LF bzw. MSB verantwortlich (ausgehend)" (GPKE Teil 4) |
//! | 9.2 | „Ende Messstellenbetrieb" (WiM Strom Teil 1) |
//!
//! [`Meilenstein::prozessquelle`] carries that mapping per row. A caller that
//! wants to know which PID to send looks the Use-Case up in the crate that owns
//! it; what this crate owns is *when*, *by whom* and *in which order*.
//!
//! # The three things it models
//!
//! ## [`PaketId`] — the identity of one handover
//!
//! The NBA applies for it at the Energie Codes & Services GmbH at least
//! [`paket::PAKET_ID_VORLAUF_MONATE`] months before the planned
//! Änderungszeitpunkt, and it is then attached to every affected Marktlokation.
//! Kap. 3 gives three cases, and the third is a refusal: when NBN and NBA are
//! the same Marktpartner **no Paket-ID is created at all** and the whole
//! Prozessbeschreibung does not apply ([`paket::PaketAntragFehler::NbnIdentischMitNba`]).
//!
//! ## [`Aenderungszeitpunkt`] — the instant both sides pivot on
//!
//! The NBA's Zuordnungsende and the NBN's Zuordnungsbeginn are the *same*
//! instant, it is the same instant for every Lokation of one Paket-ID, and it
//! is admissible only in the future, on a **Monatserster**, respecting the lead
//! times. The constructor refuses everything else.
//!
//! ## [`Fristenkalender`] — the Prozess- und Fristenübersicht
//!
//! One [`Meilenstein`] per published row, each carrying its Kapitel, its
//! Prozess, the parties, the lead time, the chapters that must be done first,
//! whether the communication is auf Lokationsebene and whether it rides an
//! EDI@Energy format. [`Fristenkalender::plan`] turns an Änderungszeitpunkt
//! into dated milestones in dependency order, and **refuses** a table whose
//! prerequisite falls after the milestone depending on it.
//!
//! ```rust
//! use mako_nbw::{Aenderungszeitpunkt, Fristenkalender, Sparte};
//! use time::{Date, Month};
//!
//! let heute = Date::from_calendar_date(2026, Month::January, 15).unwrap();
//! let az = Aenderungszeitpunkt::neu(
//!     Date::from_calendar_date(2027, Month::January, 1).unwrap(),
//!     heute,
//!     Sparte::Strom,
//! )
//! .expect("ein Monatserster mit 6 Monaten Vorlauf");
//!
//! let plan = Fristenkalender::strom().plan(az).expect("die Tabelle ist konsistent");
//! // Kap. 6.1 is due four months before — 2026-09-01.
//! assert_eq!(
//!     plan[0].faellig,
//!     Some(Date::from_calendar_date(2026, Month::September, 1).unwrap())
//! );
//! ```
//!
//! # Sparte
//!
//! Strom and Gas are published as two separate Anwendungshilfen with different
//! chapter numbering, different Fristen and a different set of Rollen, so
//! [`Fristenkalender`] is keyed on [`Sparte`] and every [`Meilenstein`] states
//! which one it belongs to. The differences that change behaviour:
//!
//! | | Strom | Gas |
//! |---|---|---|
//! | Paket-ID | Kap. 3 — required, applied for at ECS | **does not exist** |
//! | Meilensteine | 21 rows, Kap. 5 | 7 rows, Kap. 4.1.2 |
//! | Ordering column | „vorab durchzuführen (Kapitel-Nr.)" | none — the UC Vorbedingungen carry it |
//! | Lokationsebene column | present | none |
//! | Übergang um Mitternacht | not stated | Gastag: deliveries until 06:00 stay with the NBA |
//! | Rollen | LF, MSB, NB, BKV, BIKO, ÜNB, RB, EIV | LF, MSB, NB, BKV, MGV |
//!
//! # Sources
//!
//! - BDEW-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Strom",
//!   Version 1.2 (30.10.2025) — applicable from 01.08.2025 for a NB-Wechsel to
//!   01.01.2026; a Paket-ID may be applied for before that date.
//! - BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte
//!   Gas", Version 1.0 (26.06.2026).
//! - BDEW-Anwendungshilfe „Rollenmodell für die Marktkommunikation im deutschen
//!   Energiemarkt", Version 2.1 — the Rollen, Gebiete and Objekte both
//!   Anwendungshilfen build on.
//! - § 46 EnWG — the Konzessionsverfahren whose Abs. 3 Bekanntgabe starts the
//!   clock on a Konzessionsübergang.

#![deny(unsafe_code)]
#![deny(missing_docs)]
#![warn(clippy::pedantic, clippy::must_use_candidate)]
#![allow(clippy::doc_markdown)] // German MaKo terms produce many false positives

pub mod kalender;
pub mod meilenstein;
pub mod paket;
pub mod zeitpunkt;

pub use kalender::{Fristenkalender, GeplanterMeilenstein, KalenderFehler, PlanFehler};
pub use meilenstein::{
    Beteiligte, Lokationsebene, Meilenstein, Rolle, Uebertragung, Vorbedingung, VorbedingungArt,
    Vorlauf,
};
pub use paket::{Paket, PaketAntrag, PaketAntragFehler, PaketId, PaketIdFehler, PaketStatus};
pub use zeitpunkt::{Aenderungszeitpunkt, AenderungszeitpunktFehler};

use serde::{Deserialize, Serialize};

/// The commodity a NB-Wechsel runs in.
///
/// Strom and Gas are two separate Anwendungshilfen, not two dialects of one:
/// they number their chapters differently, publish different Fristen and name
/// different Rollen. Everything in this crate that quotes a chapter is keyed on
/// this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sparte {
    /// BDEW-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Strom",
    /// Version 1.2 (30.10.2025).
    Strom,
    /// BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel
    /// Sparte Gas", Version 1.0 (26.06.2026).
    Gas,
}

impl Sparte {
    /// The Anwendungshilfe this Sparte's chapter references point at.
    #[must_use]
    pub const fn anwendungshilfe(self) -> &'static str {
        match self {
            Self::Strom => {
                "BDEW-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Strom\", V1.2 \
                 (30.10.2025)"
            }
            Self::Gas => {
                "BDEW/VKU/GEODE-Anwendungshilfe „Marktprozesse Netzbetreiberwechsel Sparte Gas\", \
                 V1.0 (26.06.2026)"
            }
        }
    }

    /// Whether a Paket-ID exists in this Sparte.
    ///
    /// Strom Kap. 3 defines it and Kap. 4 Rahmenbedingung 3 makes it a
    /// precondition of every other process. The Gas Anwendungshilfe does not
    /// mention a Paket-ID anywhere: NBA and NBN agree the affected Lokationen
    /// bilaterally (Gas Kap. 3.1 Rahmenbedingung 3).
    #[must_use]
    pub const fn hat_paket_id(self) -> bool {
        matches!(self, Self::Strom)
    }
}

impl std::fmt::Display for Sparte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Strom => "Strom",
            Self::Gas => "Gas",
        })
    }
}

/// Shift a date `n` calendar months, clamping to the target month's last day.
///
/// § 188 Abs. 3 BGB: a Frist stated in months that would land on a day the
/// target month does not have ends on its last day. An Änderungszeitpunkt is
/// always a Monatserster so the clamp never bites there, but the earliest
/// admissible Änderungszeitpunkt is computed from an arbitrary `heute`.
pub(crate) fn monate_verschieben(datum: time::Date, delta: i32) -> time::Date {
    let total = i32::from(u8::from(datum.month())) - 1 + delta;
    let jahr = datum.year() + total.div_euclid(12);
    let monat = time::Month::try_from(u8::try_from(total.rem_euclid(12) + 1).unwrap_or(1))
        .unwrap_or(time::Month::January);
    let letzter = time::util::days_in_month(monat, jahr);
    time::Date::from_calendar_date(jahr, monat, datum.day().min(letzter)).unwrap_or(datum)
}

/// Shift a date `n` calendar months back.
pub(crate) fn monate_zurueck(datum: time::Date, n: u32) -> time::Date {
    monate_verschieben(datum, -i32::try_from(n).unwrap_or(i32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::{Date, Month};

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    /// Kap. 3 (Strom) creates a Paket-ID; the Gas Anwendungshilfe has no such
    /// concept, and treating it as if it did would gate every Gas milestone on
    /// an artefact that is never issued.
    #[test]
    fn only_strom_has_a_paket_id() {
        assert!(Sparte::Strom.hat_paket_id());
        assert!(!Sparte::Gas.hat_paket_id());
    }

    /// § 188 Abs. 3 BGB — a month shift onto a day the target month lacks ends
    /// on its last day.
    #[test]
    fn month_arithmetic_clamps_to_the_last_day() {
        assert_eq!(
            monate_zurueck(d(2026, Month::March, 31), 1),
            d(2026, Month::February, 28)
        );
        assert_eq!(
            monate_zurueck(d(2027, Month::January, 1), 6),
            d(2026, Month::July, 1)
        );
        assert_eq!(
            monate_verschieben(d(2026, Month::November, 15), 4),
            d(2027, Month::March, 15)
        );
    }

    #[test]
    fn sparte_round_trips_through_json() {
        assert_eq!(serde_json::to_string(&Sparte::Gas).unwrap(), "\"GAS\"");
        let back: Sparte = serde_json::from_str("\"STROM\"").unwrap();
        assert_eq!(back, Sparte::Strom);
    }
}
