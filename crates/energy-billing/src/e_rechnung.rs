//! Whether an invoice may still go out as a *sonstige Rechnung* — on paper or
//! in an electronic format that is not § 14 Abs. 1 Satz 6 UStG.
//!
//! § 14 Abs. 2 Satz 2 Nr. 1 UStG makes the E-Rechnung mandatory „für eine
//! Leistung an einen anderen Unternehmer für dessen Unternehmen". § 27 Abs. 38
//! UStG (read on gesetze-im-internet.de, 2026-10-05) grants three graces from
//! it, each keyed on when the Umsatz was **ausgeführt**:
//!
//! | Nr. | Umsatz ausgeführt | Allowed until | Condition |
//! |---|---|---|---|
//! | 1 | after 31.12.2024, before 01.01.2027 | übermittelt by 31.12.2026 | — |
//! | 2 | after 31.12.2026, before 01.01.2028 | übermittelt by 31.12.2027 | the issuer's Gesamtumsatz (§ 19 Abs. 2) im vorangegangenen Kalenderjahr „nicht mehr als 800 000 Euro" |
//! | 3 | after 31.12.2026, before 01.01.2028 | ausgestellt by 31.12.2027 | EDI under Art. 2 Empfehlung 94/820/EG |
//!
//! A supply executed before 01.01.2025 is outside the new § 14 altogether:
//! changes to the UStG reach „Umsätze …, die ab dem Inkrafttreten der
//! maßgeblichen Änderungsvorschrift ausgeführt werden" (§ 27 Abs. 1 Satz 1).
//!
//! This module answers Nr. 1 and Nr. 2 — the paper and non-conforming
//! electronic path. Nr. 3 is the EDI issuers' question and tests a different
//! date.

use rust_decimal::Decimal;
use rust_decimal::dec;
use time::Date;
use time::macros::date;

/// § 27 Abs. 38 Nr. 2 UStG — „nicht mehr als 800 000 Euro".
pub const NR2_GESAMTUMSATZ_GRENZE_EUR: Decimal = dec!(800000);

/// What the caller knows about one invoice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonstigeRechnungFall {
    /// When the Umsatz was ausgeführt — for a supply period, its last day.
    pub ausgefuehrt: Date,
    /// When the document would be übermittelt.
    pub uebermittelt: Date,
    /// Whether the Leistungsempfänger is a Unternehmer receiving for his
    /// Unternehmen (§ 14 Abs. 2 Satz 2 Nr. 1 UStG); `None` when not recorded.
    pub empfaenger_unternehmer: Option<bool>,
    /// The issuer's Gesamtumsatz (§ 19 Abs. 2 UStG) in the calendar year before
    /// the supply; `None` when not recorded.
    pub gesamtumsatz_vorjahr_eur: Option<Decimal>,
}

/// Why a sonstige Rechnung is admissible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zulaessig {
    /// The recipient is no Unternehmer receiving for his Unternehmen.
    KeinB2b,
    /// The supply was executed before 01.01.2025 (§ 27 Abs. 1 Satz 1 UStG).
    UmsatzVor2025,
    /// § 27 Abs. 38 Nr. 1 UStG.
    Nr1,
    /// § 27 Abs. 38 Nr. 2 UStG.
    Nr2,
}

impl Zulaessig {
    /// The provision, as a refusal or a document names it.
    #[must_use]
    pub const fn grundlage(self) -> &'static str {
        match self {
            Self::KeinB2b => {
                "§ 14 Abs. 2 Satz 2 Nr. 1 UStG greift nicht (kein Unternehmer-Empfänger)"
            }
            Self::UmsatzVor2025 => "§ 27 Abs. 1 Satz 1 UStG (Umsatz vor dem 01.01.2025)",
            Self::Nr1 => "§ 27 Abs. 38 Nr. 1 UStG",
            Self::Nr2 => "§ 27 Abs. 38 Nr. 2 UStG",
        }
    }
}

/// Why a sonstige Rechnung is not admissible.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unzulaessig {
    /// Whether the recipient is an Unternehmer is not recorded.
    #[error(
        "ob der Leistungsempfänger Unternehmer für dessen Unternehmen ist \
         (§ 14 Abs. 2 Satz 2 Nr. 1 UStG), ist nicht erfasst — die Papierrechnung \
         wird nicht unterstellt"
    )]
    EmpfaengerUnbekannt,
    /// A 2027 supply and the issuer's 2026 Gesamtumsatz is not recorded.
    #[error(
        "der Gesamtumsatz {jahr} des Rechnungsausstellers ist nicht erfasst — \
         § 27 Abs. 38 Nr. 2 UStG wird nicht unterstellt"
    )]
    GesamtumsatzUnbekannt {
        /// The calendar year whose figure is missing.
        jahr: i32,
    },
    /// A 2027 supply by an issuer above 800 000 € in 2026.
    #[error(
        "Gesamtumsatz {jahr} von {eur} EUR übersteigt 800 000 EUR — § 27 Abs. 38 Nr. 2 \
         UStG lässt die sonstige Rechnung nicht zu; eine E-Rechnung nach § 14 Abs. 1 \
         Satz 6 UStG ist auszustellen"
    )]
    Nr2UeberGrenze {
        /// The prior calendar year.
        jahr: i32,
        /// The recorded Gesamtumsatz.
        eur: Decimal,
    },
    /// No grace reaches this supply and transmission date.
    #[error(
        "für einen am {ausgefuehrt} ausgeführten und am {uebermittelt} übermittelten Umsatz \
         an einen Unternehmer gewährt § 27 Abs. 38 Nr. 1/2 UStG keine Übergangsfrist — eine \
         E-Rechnung nach § 14 Abs. 1 Satz 6 UStG ist auszustellen"
    )]
    KeineUebergangsfrist {
        /// The supply date.
        ausgefuehrt: Date,
        /// The transmission date.
        uebermittelt: Date,
    },
}

/// Whether the invoice may go out as a sonstige Rechnung.
///
/// # Errors
///
/// [`Unzulaessig`] — naming the provision, or the missing fact; a missing fact
/// never admits the grace.
pub fn sonstige_rechnung_zulaessig(f: SonstigeRechnungFall) -> Result<Zulaessig, Unzulaessig> {
    match f.empfaenger_unternehmer {
        None => return Err(Unzulaessig::EmpfaengerUnbekannt),
        Some(false) => return Ok(Zulaessig::KeinB2b),
        Some(true) => {}
    }
    if f.ausgefuehrt < date!(2025 - 01 - 01) {
        return Ok(Zulaessig::UmsatzVor2025);
    }
    if f.ausgefuehrt <= date!(2026 - 12 - 31) && f.uebermittelt <= date!(2026 - 12 - 31) {
        return Ok(Zulaessig::Nr1);
    }
    if (date!(2027 - 01 - 01)..=date!(2027 - 12 - 31)).contains(&f.ausgefuehrt)
        && f.uebermittelt <= date!(2027 - 12 - 31)
    {
        let jahr = 2026;
        return match f.gesamtumsatz_vorjahr_eur {
            None => Err(Unzulaessig::GesamtumsatzUnbekannt { jahr }),
            Some(eur) if eur <= NR2_GESAMTUMSATZ_GRENZE_EUR => Ok(Zulaessig::Nr2),
            Some(eur) => Err(Unzulaessig::Nr2UeberGrenze { jahr, eur }),
        };
    }
    Err(Unzulaessig::KeineUebergangsfrist {
        ausgefuehrt: f.ausgefuehrt,
        uebermittelt: f.uebermittelt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fall(
        ausgefuehrt: Date,
        uebermittelt: Date,
        umsatz: Option<Decimal>,
    ) -> SonstigeRechnungFall {
        SonstigeRechnungFall {
            ausgefuehrt,
            uebermittelt,
            empfaenger_unternehmer: Some(true),
            gesamtumsatz_vorjahr_eur: umsatz,
        }
    }

    /// The boundary table. „nicht mehr als 800 000 Euro" keeps exactly
    /// 800 000 € inside the grace; one cent above is out. A `<` in place of
    /// `<=` fails the first row.
    #[test]
    fn the_nr2_threshold_keeps_exactly_800000_inside() {
        let (a, u) = (date!(2027 - 06 - 30), date!(2027 - 07 - 15));
        assert_eq!(
            sonstige_rechnung_zulaessig(fall(a, u, Some(dec!(800000)))),
            Ok(Zulaessig::Nr2)
        );
        assert!(matches!(
            sonstige_rechnung_zulaessig(fall(a, u, Some(dec!(800000.01)))),
            Err(Unzulaessig::Nr2UeberGrenze { .. })
        ));
        assert_eq!(
            sonstige_rechnung_zulaessig(fall(a, u, None)),
            Err(Unzulaessig::GesamtumsatzUnbekannt { jahr: 2026 })
        );
    }

    /// From 2028 no grace reaches paper, whatever the turnover.
    #[test]
    fn a_2028_document_has_no_grace() {
        assert!(matches!(
            sonstige_rechnung_zulaessig(fall(
                date!(2028 - 01 - 31),
                date!(2028 - 02 - 05),
                Some(dec!(1000))
            )),
            Err(Unzulaessig::KeineUebergangsfrist { .. })
        ));
        // A 2027 supply transmitted in 2028 is past Nr. 2's „bis zum 31.
        // Dezember 2027 … übermittelt".
        assert!(matches!(
            sonstige_rechnung_zulaessig(fall(
                date!(2027 - 12 - 31),
                date!(2028 - 01 - 01),
                Some(dec!(1000))
            )),
            Err(Unzulaessig::KeineUebergangsfrist { .. })
        ));
    }

    /// Nr. 1 needs both the supply and the transmission inside 2025–2026: a
    /// December-2026 supply invoiced in January 2027 falls between Nr. 1
    /// (transmitted too late) and Nr. 2 (executed too early).
    #[test]
    fn nr1_is_keyed_on_supply_and_transmission() {
        assert_eq!(
            sonstige_rechnung_zulaessig(fall(date!(2026 - 12 - 31), date!(2026 - 12 - 31), None)),
            Ok(Zulaessig::Nr1)
        );
        assert!(matches!(
            sonstige_rechnung_zulaessig(fall(
                date!(2026 - 12 - 31),
                date!(2027 - 01 - 10),
                Some(dec!(1000))
            )),
            Err(Unzulaessig::KeineUebergangsfrist { .. })
        ));
    }

    /// No Unternehmer recipient, no duty; an unrecorded recipient refuses.
    #[test]
    fn the_recipient_decides_whether_the_duty_applies() {
        let base = fall(date!(2028 - 01 - 31), date!(2028 - 02 - 05), None);
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                empfaenger_unternehmer: Some(false),
                ..base
            }),
            Ok(Zulaessig::KeinB2b)
        );
        assert_eq!(
            sonstige_rechnung_zulaessig(SonstigeRechnungFall {
                empfaenger_unternehmer: None,
                ..base
            }),
            Err(Unzulaessig::EmpfaengerUnbekannt)
        );
    }

    #[test]
    fn a_supply_before_2025_is_outside_the_new_section_14() {
        assert_eq!(
            sonstige_rechnung_zulaessig(fall(date!(2024 - 12 - 31), date!(2028 - 03 - 01), None)),
            Ok(Zulaessig::UmsatzVor2025)
        );
    }
}
