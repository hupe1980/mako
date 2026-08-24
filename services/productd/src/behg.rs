//! BEHG / nEHS certificate prices — what the statute actually fixes.
//!
//! The CO₂ component of every Gas and Wärme invoice is derived from the price
//! the supplier paid for its nEHS certificates (CO2KostAufG § 3 passes through
//! the actual cost), so a mistyped price here mis-bills every gas customer at
//! once and is not visible on any one invoice. § 10 BEHG fixes enough about
//! that price to catch the mistake at import.
//!
//! # The three phases, and which price belongs to which
//!
//! | Phase | Period | Price | § 10 BEHG |
//! |---|---|---|---|
//! | **Einführungsphase** (Festpreis) | 2021–2025 | 25 / 30 / 30 / 45 / **55** EUR/t | Abs. 2 |
//! | **Versteigerung** | 2026 | auction clearing inside **55–65** EUR/t | Abs. 1, Abs. 2 |
//! | **Nachkauf** (Mehrmengen) | after the 2026 auctions | **68** EUR/t | auction terms |
//!
//! The 68 EUR/t figure is the **Nachkauf** price for supplementary purchases
//! once the auctioned volume no longer covers demand, not a "Verkaufsphase"
//! price — the Verkaufsphase ended at 55 EUR/t in 2025.
//!
//! From 2027 § 10 Abs. 2 BEHG sets no figures of its own (it defers to the
//! decision under § 24 Abs. 2 Nr. 2), so nothing is asserted about those years.

use rust_decimal::Decimal;
use time::Date;

/// Where a price point came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quelle {
    /// EEX auction clearing price (2026: Wednesdays, 55–65 EUR/t corridor).
    Auktion,
    /// Einführungsphase fixed price, 2021–2025 (§ 10 Abs. 2 BEHG).
    Verkaufsphase,
    /// Supplementary purchase at the Mehrmengenpreis once the auctioned volume
    /// is exhausted (2026: 68 EUR/t).
    Nachkauf,
    /// Operator entry — validated for plausibility only.
    Manual,
}

impl Quelle {
    /// Parse the wire value. `None` for anything the column does not allow.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "auktion" => Self::Auktion,
            "verkaufsphase" => Self::Verkaufsphase,
            "nachkauf" => Self::Nachkauf,
            "manual" => Self::Manual,
            _ => None?,
        })
    }

    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Auktion => "auktion",
            Self::Verkaufsphase => "verkaufsphase",
            Self::Nachkauf => "nachkauf",
            Self::Manual => "manual",
        }
    }
}

/// The § 10 Abs. 2 BEHG Festpreis for a year of the Einführungsphase.
#[must_use]
pub fn festpreis(jahr: i32) -> Option<Decimal> {
    let eur = match jahr {
        2021 => 25,
        // 2022 and 2023 are both 30: the 2023 step was deferred by the
        // Zweites Gesetz zur Änderung des BEHG, so the sequence is not the
        // 25/30/35/45/55 ladder the original draft had.
        2022 | 2023 => 30,
        2024 => 45,
        2025 => 55,
        _ => return None,
    };
    Some(Decimal::from(eur))
}

/// The § 10 Abs. 2 BEHG auction price corridor for a year, as `(min, max)`.
#[must_use]
pub fn auktionskorridor(jahr: i32) -> Option<(Decimal, Decimal)> {
    // Only 2026 has statutory figures. Later years follow the decision under
    // § 24 Abs. 2 Nr. 2 BEHG, which is not law yet.
    (jahr == 2026).then(|| (Decimal::from(55), Decimal::from(65)))
}

/// The Mehrmengenpreis for supplementary purchases in a year.
#[must_use]
pub fn nachkaufpreis(jahr: i32) -> Option<Decimal> {
    (jahr == 2026).then(|| Decimal::from(68))
}

/// Why a price point was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Beanstandung {
    pub grund: String,
    pub rechtsgrundlage: &'static str,
}

/// Check a dated price point against what the statute fixes for its year.
///
/// A price the law pins is checked exactly; a price the law only bounds is
/// checked against the bound; a year the law says nothing about is accepted.
/// `Manual` is exempt from the phase rules but still has to be a plausible
/// certificate price — the point is to catch a decimal slip, not to second-
/// guess an operator who knows why the number is unusual.
///
/// # Errors
///
/// Returns the reason the value cannot be right for that date.
pub fn pruefe(datum: Date, eur_per_t: Decimal, quelle: Quelle) -> Result<(), Beanstandung> {
    if eur_per_t <= Decimal::ZERO {
        return Err(Beanstandung {
            grund: format!("{eur_per_t} EUR/t ist kein gültiger Zertifikatspreis"),
            rechtsgrundlage: "§ 10 BEHG",
        });
    }
    let jahr = datum.year();

    match quelle {
        Quelle::Verkaufsphase => {
            let Some(erwartet) = festpreis(jahr) else {
                return Err(Beanstandung {
                    grund: format!(
                        "{jahr} gehört nicht zur Einführungsphase (2021–2025); \
                         ab 2026 wird versteigert"
                    ),
                    rechtsgrundlage: "§ 10 Abs. 1, 2 BEHG",
                });
            };
            if eur_per_t != erwartet {
                return Err(Beanstandung {
                    grund: format!(
                        "der Festpreis für {jahr} beträgt {erwartet} EUR/t, angegeben: {eur_per_t}"
                    ),
                    rechtsgrundlage: "§ 10 Abs. 2 BEHG",
                });
            }
        }
        Quelle::Auktion => {
            if let Some((min, max)) = auktionskorridor(jahr)
                && (eur_per_t < min || eur_per_t > max)
            {
                return Err(Beanstandung {
                    grund: format!(
                        "der Zuschlagspreis {eur_per_t} EUR/t liegt außerhalb des \
                         Preiskorridors {min}–{max} EUR/t für {jahr}"
                    ),
                    rechtsgrundlage: "§ 10 Abs. 2 BEHG",
                });
            }
            if festpreis(jahr).is_some() {
                return Err(Beanstandung {
                    grund: format!(
                        "{jahr} liegt in der Einführungsphase — es wurde nicht versteigert"
                    ),
                    rechtsgrundlage: "§ 10 Abs. 1 BEHG",
                });
            }
        }
        Quelle::Nachkauf => {
            if let Some(erwartet) = nachkaufpreis(jahr)
                && eur_per_t != erwartet
            {
                return Err(Beanstandung {
                    grund: format!(
                        "der Mehrmengenpreis für {jahr} beträgt {erwartet} EUR/t, \
                         angegeben: {eur_per_t}"
                    ),
                    rechtsgrundlage: "§ 10 BEHG (Versteigerungsbedingungen)",
                });
            }
        }
        Quelle::Manual => {
            // A certificate price two orders of magnitude off is a typo, not a
            // market. The window is deliberately wide.
            if eur_per_t < Decimal::from(5) || eur_per_t > Decimal::from(500) {
                return Err(Beanstandung {
                    grund: format!(
                        "{eur_per_t} EUR/t liegt außerhalb jedes plausiblen \
                         Zertifikatspreises (5–500 EUR/t)"
                    ),
                    rechtsgrundlage: "Plausibilitätsprüfung",
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    #[test]
    fn the_einfuehrungsphase_ladder_matches_the_statute() {
        assert_eq!(festpreis(2021), Some(dec!(25)));
        assert_eq!(festpreis(2022), Some(dec!(30)));
        // Deferred — 2023 stayed at 30, it did not step to 35.
        assert_eq!(festpreis(2023), Some(dec!(30)));
        assert_eq!(festpreis(2024), Some(dec!(45)));
        assert_eq!(festpreis(2025), Some(dec!(55)));
        assert_eq!(festpreis(2026), None, "2026 is auctioned, not fixed");
    }

    #[test]
    fn a_wrong_festpreis_is_refused_with_the_right_one() {
        let err = pruefe(date!(2025 - 03 - 01), dec!(45), Quelle::Verkaufsphase).unwrap_err();
        assert!(err.grund.contains("55 EUR/t"), "{}", err.grund);
        assert_eq!(err.rechtsgrundlage, "§ 10 Abs. 2 BEHG");
    }

    #[test]
    fn the_2026_corridor_is_enforced_in_both_directions() {
        assert!(pruefe(date!(2026 - 07 - 01), dec!(55), Quelle::Auktion).is_ok());
        assert!(pruefe(date!(2026 - 07 - 01), dec!(63.50), Quelle::Auktion).is_ok());
        assert!(pruefe(date!(2026 - 07 - 01), dec!(65), Quelle::Auktion).is_ok());
        assert!(pruefe(date!(2026 - 07 - 01), dec!(54.99), Quelle::Auktion).is_err());
        assert!(pruefe(date!(2026 - 07 - 01), dec!(65.01), Quelle::Auktion).is_err());
    }

    #[test]
    fn a_decimal_slip_in_an_auction_price_is_caught() {
        // 6.35 instead of 63.50 would have under-billed the CO₂ component of
        // every gas invoice by a factor of ten, invisibly.
        let err = pruefe(date!(2026 - 07 - 08), dec!(6.35), Quelle::Auktion).unwrap_err();
        assert!(err.grund.contains("Preiskorridors"), "{}", err.grund);
    }

    #[test]
    fn an_auction_price_dated_into_the_einfuehrungsphase_is_refused() {
        assert!(pruefe(date!(2025 - 07 - 01), dec!(55), Quelle::Auktion).is_err());
    }

    #[test]
    fn the_nachkauf_price_is_the_mehrmengenpreis_not_the_corridor() {
        assert!(pruefe(date!(2026 - 11 - 10), dec!(68), Quelle::Nachkauf).is_ok());
        let err = pruefe(date!(2026 - 11 - 10), dec!(65), Quelle::Nachkauf).unwrap_err();
        assert!(err.grund.contains("68 EUR/t"), "{}", err.grund);
    }

    #[test]
    fn years_the_statute_does_not_fix_are_accepted() {
        // § 10 Abs. 2 BEHG sets no figures from 2027; asserting one would be
        // inventing law.
        assert!(pruefe(date!(2027 - 03 - 01), dec!(72), Quelle::Auktion).is_ok());
    }

    #[test]
    fn a_manual_entry_is_free_but_still_has_to_be_a_price() {
        assert!(pruefe(date!(2027 - 03 - 01), dec!(80), Quelle::Manual).is_ok());
        assert!(pruefe(date!(2027 - 03 - 01), dec!(0.63), Quelle::Manual).is_err());
        assert!(pruefe(date!(2027 - 03 - 01), dec!(-5), Quelle::Manual).is_err());
    }
}
