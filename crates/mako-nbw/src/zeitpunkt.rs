//! The **Änderungszeitpunkt** — the one instant a NB-Wechsel turns on.
//!
//! „Das Zuordnungsende eines NBA und der Zuordnungsbeginn eines NBN zu einer
//! Lokation fallen bei einer vom NB-Wechsel betroffenen Lokation auf denselben
//! Zeitpunkt" (Strom Kap. 2.2, Gas Kap. 2.2). Three properties follow, and this
//! module enforces all three at construction:
//!
//! 1. **It is one instant, not two.** There is no gap and no overlap between the
//!    NBA's Zuordnungsende and the NBN's Zuordnungsbeginn, so a model with two
//!    dates can express a state the market cannot have.
//! 2. **It is the same for every Lokation** of one Paket-ID (Strom Kap. 2.2) or
//!    of one handover (Gas Kap. 2.2) — it belongs to the Paket, never to a
//!    Marktlokation.
//! 3. **It is admissible „nur in die Zukunft zum Monatsersten unter Einhaltung
//!    der … Vorlauffristen"** (Strom Kap. 2.2, Gas Kap. 2.2 and Kap. 3.1
//!    Rahmenbedingung 5).
//!
//! # What „Vorlauffristen" binds to
//!
//! The Anwendungshilfen state the rule and leave the number to the process
//! tables, so the minimum lead is *derived* rather than quoted:
//!
//! | Sparte | Binding lead | Where it comes from |
//! |---|---|---|
//! | Strom | 6 Monate | Kap. 3 — the Paket-ID is applied for „spätestens 6 Monate vor dem geplanten Änderungszeitpunkt", and Kap. 4 Rahmenbedingung 3 makes the Paket-ID a precondition of everything else |
//! | Gas | 4 Monate | Kap. 4.1.2 — the earliest published row, „Übergabe der Kontaktdaten der DB … spätestens 4 Monate vor dem Änderungszeitpunkt" |
//!
//! [`Sparte::mindestvorlauf_monate`] carries them.

use serde::Serialize;
use time::{Date, Month, Time};

use crate::{Sparte, monate_verschieben, monate_zurueck};

/// Months before the planned Änderungszeitpunkt by which the NBA must apply for
/// the Paket-ID at the Energie Codes & Services GmbH (Strom Kap. 3).
pub const PAKET_ID_VORLAUF_MONATE: u32 = 6;

/// Months before the Änderungszeitpunkt by which the NBN must be reported to the
/// Energie Codes & Services GmbH when it was not yet known at application time
/// (Strom Kap. 3).
///
/// „unverzüglich nach Festlegung des NBN … jedoch im Sinne der nachfolgenden
/// Prozessbeschreibung spätestens 4 Monate vor dem Änderungszeitpunkt". The
/// same act replaces the „geplanter Änderungszeitpunkt" with the
/// Änderungszeitpunkt, and it is owed **even when NBA and NBN are identical** —
/// the published list then shows that no NB-Wechsel will take place.
pub const NBN_MELDUNG_VORLAUF_MONATE: u32 = 4;

/// Months the Strom Anwendungshilfe relates to the Bekanntgabe des Ablaufs von
/// Verträgen nach § 46 Abs. 3 EnWG (Kap. 3).
///
/// Kap. 3 states it as a parenthetical Hinweis to the 6-month Paket-ID lead:
/// „(Hinweis: Bei Konzessionsübergängen entspricht dies 18 Monate nach der
/// Bekanntgabe des Ablaufs von Verträgen nach § 46 Absatz 3 des
/// Energiewirtschaftsgesetzes)". **What „dies" refers back to is not resolved
/// by the text** — it reads equally as the Antragsfrist and as the geplanter
/// Änderungszeitpunkt, and the two differ by the 6 months of
/// [`PAKET_ID_VORLAUF_MONATE`]. This crate therefore publishes the figure and
/// derives no date from it; a caller that needs one must decide the reading and
/// own that decision.
pub const KONZESSION_BEKANNTGABE_MONATE: u32 = 18;

/// Gas: the clock time until which a delivery on the Änderungszeitpunkt is
/// still assigned to the NBA (Gas Kap. 3.1 Rahmenbedingung 5, Fußnote 1).
///
/// „Auch in der Sparte Gas findet der NB-Wechsel zum Monatsersten statt.
/// Lieferungen an diesem Tag bis 6:00 Uhr werden jedoch noch vom NBA
/// zugeordnet. Der Gastag findet somit Anwendung." The Strom Anwendungshilfe
/// states no clock time at all.
pub const GAS_UEBERGANG_UHRZEIT: Time = time::macros::time!(6:00);

impl Sparte {
    /// The minimum lead an Änderungszeitpunkt must leave, in whole months.
    ///
    /// Neither Anwendungshilfe states this as a single number: both say the
    /// Änderungszeitpunkt is admissible „unter Einhaltung der in den
    /// nachfolgenden Prozessen beschriebenen Vorlauffristen" and leave the
    /// figure to the tables. It is the longest lead any obligation in that
    /// Sparte carries — Strom's Paket-ID (Kap. 3), Gas's Übergabe der
    /// Kontaktdaten der DB (Kap. 4.1.2).
    #[must_use]
    pub const fn mindestvorlauf_monate(self) -> u32 {
        match self {
            Self::Strom => PAKET_ID_VORLAUF_MONATE,
            Self::Gas => NBN_MELDUNG_VORLAUF_MONATE,
        }
    }

    /// The Fundstelle behind [`Self::mindestvorlauf_monate`].
    #[must_use]
    pub const fn mindestvorlauf_fundstelle(self) -> &'static str {
        match self {
            Self::Strom => {
                "Kap. 3 — die Paket-ID beantragt der NBA „spätestens 6 Monate vor dem geplanten \
                 Änderungszeitpunkt\"; Kap. 4 Rahmenbedingung 3 setzt sie für alle weiteren \
                 Prozesse voraus"
            }
            Self::Gas => {
                "Kap. 4.1.2 — „Übergabe der Kontaktdaten der DB … spätestens 4 Monate vor dem \
                 Änderungszeitpunkt\", die früheste veröffentlichte Frist"
            }
        }
    }
}

/// A validated Änderungszeitpunkt: a future Monatserster that leaves the
/// Sparte's minimum lead.
///
/// Constructed only through [`Aenderungszeitpunkt::neu`], including from JSON —
/// deriving `Deserialize` would let a payload build a value the constructor
/// refuses, and every downstream Frist is measured from this date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct Aenderungszeitpunkt {
    datum: Date,
    sparte: Sparte,
}

impl Aenderungszeitpunkt {
    /// Build one, checking the three rules of Kap. 2.2 in the order they are
    /// stated: Monatserster, in der Zukunft, Vorlauffrist eingehalten.
    ///
    /// `heute` is the business date the check runs against — a Berlin date, not
    /// a wall clock. This crate holds no clock, so the caller supplies it.
    ///
    /// # Errors
    ///
    /// [`AenderungszeitpunktFehler`] naming which of the three rules failed.
    pub fn neu(
        datum: Date,
        heute: Date,
        sparte: Sparte,
    ) -> Result<Self, AenderungszeitpunktFehler> {
        if datum.day() != 1 {
            return Err(AenderungszeitpunktFehler::KeinMonatserster { datum });
        }
        if datum <= heute {
            return Err(AenderungszeitpunktFehler::NichtInDerZukunft { datum, heute });
        }
        let monate = sparte.mindestvorlauf_monate();
        let fruehestmoeglich = fruehester_monatserster(heute, monate);
        if datum < fruehestmoeglich {
            return Err(AenderungszeitpunktFehler::VorlaufUnterschritten {
                datum,
                heute,
                sparte,
                monate,
                fruehestmoeglich,
            });
        }
        Ok(Self { datum, sparte })
    }

    /// The date itself.
    #[must_use]
    pub const fn datum(self) -> Date {
        self.datum
    }

    /// The Sparte whose Anwendungshilfe validated it.
    #[must_use]
    pub const fn sparte(self) -> Sparte {
        self.sparte
    }

    /// `n` calendar months before the Änderungszeitpunkt.
    ///
    /// Every lead in both Prozess- und Fristenübersichten is „n Monate vor dem
    /// Änderungszeitpunkt", so this is the arithmetic the whole calendar rests
    /// on.
    #[must_use]
    pub fn monate_vorher(self, n: u32) -> Date {
        monate_zurueck(self.datum, n)
    }

    /// The latest date the NBA may apply for the Paket-ID (Strom Kap. 3).
    ///
    /// `None` in Gas: that Anwendungshilfe knows no Paket-ID.
    #[must_use]
    pub fn spaetester_paket_id_antrag(self) -> Option<Date> {
        self.sparte
            .hat_paket_id()
            .then(|| self.monate_vorher(PAKET_ID_VORLAUF_MONATE))
    }

    /// The latest date the NBN must be reported to the Energie Codes & Services
    /// GmbH when it was unknown at application time (Strom Kap. 3).
    ///
    /// `None` in Gas, for the same reason as
    /// [`Self::spaetester_paket_id_antrag`].
    #[must_use]
    pub fn spaeteste_nbn_meldung(self) -> Option<Date> {
        self.sparte
            .hat_paket_id()
            .then(|| self.monate_vorher(NBN_MELDUNG_VORLAUF_MONATE))
    }

    /// The clock time on the Änderungszeitpunkt until which a delivery is still
    /// the NBA's, where the Anwendungshilfe states one.
    ///
    /// `Some(06:00)` in Gas — the Gastag (Kap. 3.1 Fußnote 1). `None` in Strom:
    /// Kap. 2.2 gives a date and no time of day, and picking midnight would be
    /// this crate's invention rather than the Festlegung's.
    #[must_use]
    pub const fn uebergang_uhrzeit(self) -> Option<Time> {
        match self.sparte {
            Sparte::Strom => None,
            Sparte::Gas => Some(GAS_UEBERGANG_UHRZEIT),
        }
    }
}

impl std::fmt::Display for Aenderungszeitpunkt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:04}-{:02}-01",
            self.datum.year(),
            u8::from(self.datum.month())
        )
    }
}

/// The first Monatserster that is at least `monate` calendar months after
/// `heute`.
fn fruehester_monatserster(heute: Date, monate: u32) -> Date {
    let verschoben = monate_verschieben(heute, i32::try_from(monate).unwrap_or(i32::MAX));
    if verschoben.day() == 1 {
        verschoben
    } else {
        naechster_monatserster(verschoben)
    }
}

/// The Monatserster strictly after `datum`.
fn naechster_monatserster(datum: Date) -> Date {
    let (jahr, monat) = if datum.month() == Month::December {
        (datum.year() + 1, Month::January)
    } else {
        (datum.year(), datum.month().next())
    };
    Date::from_calendar_date(jahr, monat, 1).unwrap_or(datum)
}

/// Why a proposed Änderungszeitpunkt is not admissible.
///
/// Each variant is one of the three rules of Kap. 2.2, and they are checked in
/// that order so the message names the first thing wrong rather than the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AenderungszeitpunktFehler {
    /// „nur … zum Monatsersten" (Strom Kap. 2.2, Gas Kap. 2.2 / 3.1 Nr. 5).
    #[error(
        "{datum} ist kein Monatserster — der Änderungszeitpunkt ist „nur in die Zukunft zum \
         Monatsersten … zulässig\" (Kap. 2.2). Er ist für alle Lokationen einer Paket-ID \
         derselbe Zeitpunkt, sodass ein Datum mitten im Monat die Zuordnung eines ganzen \
         Netzgebiets gegen den Abrechnungsmonat verschiebt"
    )]
    KeinMonatserster {
        /// The offered date.
        datum: Date,
    },
    /// „nur in die Zukunft" (Strom Kap. 2.2, Gas Kap. 2.2).
    #[error(
        "{datum} liegt nicht in der Zukunft (heute: {heute}) — ein Änderungszeitpunkt ist „nur \
         in die Zukunft … zulässig\" (Kap. 2.2)"
    )]
    NichtInDerZukunft {
        /// The offered date.
        datum: Date,
        /// The business date it was checked against.
        heute: Date,
    },
    /// „unter Einhaltung der … Vorlauffristen" (Strom Kap. 2.2, Gas Kap. 2.2).
    #[error(
        "{datum} lässt von heute ({heute}) aus weniger als {monate} Monate Vorlauf — Sparte \
         {sparte}: {fundstelle}. Frühestmöglicher Änderungszeitpunkt: {fruehestmoeglich}",
        fundstelle = sparte.mindestvorlauf_fundstelle()
    )]
    VorlaufUnterschritten {
        /// The offered date.
        datum: Date,
        /// The business date it was checked against.
        heute: Date,
        /// The Sparte whose lead was applied.
        sparte: Sparte,
        /// The required lead in whole months.
        monate: u32,
        /// The first Monatserster that would satisfy it.
        fruehestmoeglich: Date,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Month;

    fn d(y: i32, m: Month, day: u8) -> Date {
        Date::from_calendar_date(y, m, day).expect("valid date")
    }

    /// Kap. 2.2 — „nur in die Zukunft zum Monatsersten". The Änderungszeitpunkt
    /// is the same instant for every Lokation of one Paket-ID, so a mid-month
    /// date moves a whole Netzgebiet against the Abrechnungsmonat.
    #[test]
    fn only_a_monatserster_is_admissible() {
        let heute = d(2026, Month::January, 15);
        assert!(Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute, Sparte::Strom).is_ok());
        let err = Aenderungszeitpunkt::neu(d(2027, Month::January, 2), heute, Sparte::Strom)
            .expect_err("kein Monatserster");
        assert!(matches!(
            err,
            AenderungszeitpunktFehler::KeinMonatserster { .. }
        ));
        assert!(err.to_string().contains("Monatsersten"), "{err}");
    }

    /// Kap. 2.2 — „nur in die Zukunft". Today itself is not in the future.
    #[test]
    fn the_aenderungszeitpunkt_must_lie_in_the_future() {
        let heute = d(2026, Month::February, 1);
        assert!(matches!(
            Aenderungszeitpunkt::neu(d(2026, Month::February, 1), heute, Sparte::Strom),
            Err(AenderungszeitpunktFehler::NichtInDerZukunft { .. })
        ));
        assert!(matches!(
            Aenderungszeitpunkt::neu(d(2025, Month::December, 1), heute, Sparte::Strom),
            Err(AenderungszeitpunktFehler::NichtInDerZukunft { .. })
        ));
    }

    /// Strom Kap. 3 — the Paket-ID is applied for „spätestens 6 Monate vor dem
    /// geplanten Änderungszeitpunkt", and Kap. 4 Rahmenbedingung 3 makes it a
    /// precondition of every other process. An Änderungszeitpunkt closer than
    /// that cannot be reached at all.
    #[test]
    fn strom_needs_six_months_of_lead() {
        assert_eq!(Sparte::Strom.mindestvorlauf_monate(), 6);
        let heute = d(2026, Month::January, 15);
        // heute + 6 Monate = 2026-07-15 → the first admissible Monatserster is
        // 2026-08-01.
        assert!(Aenderungszeitpunkt::neu(d(2026, Month::August, 1), heute, Sparte::Strom).is_ok());
        let err = Aenderungszeitpunkt::neu(d(2026, Month::July, 1), heute, Sparte::Strom)
            .expect_err("nur knapp 6 Monate");
        match err {
            AenderungszeitpunktFehler::VorlaufUnterschritten {
                monate,
                fruehestmoeglich,
                ..
            } => {
                assert_eq!(monate, PAKET_ID_VORLAUF_MONATE);
                assert_eq!(fruehestmoeglich, d(2026, Month::August, 1));
            }
            other => panic!("erwartet VorlaufUnterschritten, war {other:?}"),
        }
    }

    /// The 6-month lead falls on a Monatserster exactly when `heute` is one, and
    /// then that very date is admissible — rounding up unconditionally would
    /// cost a month of planning that Kap. 3 grants.
    #[test]
    fn a_lead_landing_on_a_monatserster_is_not_rounded_up() {
        let heute = d(2026, Month::January, 1);
        assert!(Aenderungszeitpunkt::neu(d(2026, Month::July, 1), heute, Sparte::Strom).is_ok());
    }

    /// Gas Kap. 4.1.2 — the earliest published row is „Übergabe der Kontaktdaten
    /// der DB … spätestens 4 Monate vor dem Änderungszeitpunkt". Applying the
    /// Strom figure to Gas would refuse two admissible months.
    #[test]
    fn gas_needs_four_months_of_lead() {
        assert_eq!(Sparte::Gas.mindestvorlauf_monate(), 4);
        let heute = d(2026, Month::January, 15);
        assert!(Aenderungszeitpunkt::neu(d(2026, Month::June, 1), heute, Sparte::Gas).is_ok());
        assert!(Aenderungszeitpunkt::neu(d(2026, Month::June, 1), heute, Sparte::Strom).is_err());
        assert!(matches!(
            Aenderungszeitpunkt::neu(d(2026, Month::May, 1), heute, Sparte::Gas),
            Err(AenderungszeitpunktFehler::VorlaufUnterschritten { .. })
        ));
    }

    /// Strom Kap. 3 — the Antrag is due 6 months before, the NBN report 4 months
    /// before. Gas has neither: it knows no Paket-ID.
    #[test]
    fn the_paket_id_dates_exist_only_in_strom() {
        let heute = d(2026, Month::January, 15);
        let az =
            Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute, Sparte::Strom).unwrap();
        assert_eq!(
            az.spaetester_paket_id_antrag(),
            Some(d(2026, Month::July, 1))
        );
        assert_eq!(
            az.spaeteste_nbn_meldung(),
            Some(d(2026, Month::September, 1))
        );

        let gas = Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute, Sparte::Gas).unwrap();
        assert_eq!(gas.spaetester_paket_id_antrag(), None);
        assert_eq!(gas.spaeteste_nbn_meldung(), None);
    }

    /// Gas Kap. 3.1 Fußnote 1 — the Gastag: deliveries until 06:00 on the
    /// Änderungszeitpunkt are still assigned to the NBA. The Strom
    /// Anwendungshilfe states no clock time, so none is offered.
    #[test]
    fn gas_hands_over_at_the_gastag_boundary() {
        let heute = d(2026, Month::January, 15);
        let gas = Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute, Sparte::Gas).unwrap();
        assert_eq!(gas.uebergang_uhrzeit(), Some(time::macros::time!(6:00)));
        let strom =
            Aenderungszeitpunkt::neu(d(2027, Month::January, 1), heute, Sparte::Strom).unwrap();
        assert_eq!(strom.uebergang_uhrzeit(), None);
    }

    /// § 188 Abs. 3 BGB carries into the lead computation: 31.08. + 6 Monate is
    /// 28.02., and the next Monatserster is 01.03.
    #[test]
    fn the_lead_clamps_short_months_before_rounding_up() {
        assert_eq!(
            fruehester_monatserster(d(2025, Month::August, 31), 6),
            d(2026, Month::March, 1)
        );
        assert_eq!(
            fruehester_monatserster(d(2026, Month::August, 1), 4),
            d(2026, Month::December, 1)
        );
        // Year rollover out of December.
        assert_eq!(
            fruehester_monatserster(d(2026, Month::June, 15), 6),
            d(2027, Month::January, 1)
        );
    }

    /// Kap. 3 states the § 46 Abs. 3 EnWG figure but not what it anchors, so the
    /// crate publishes the number and derives no date from it.
    #[test]
    fn the_paragraph_46_figure_is_published_without_a_derived_date() {
        assert_eq!(KONZESSION_BEKANNTGABE_MONATE, 18);
    }
}
