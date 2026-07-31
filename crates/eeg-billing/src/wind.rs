//! Wind onshore reference yield model — §36h EEG 2023 i.V.m. Anlage 2.
//!
//! Onshore wind tariffs are location-corrected by a Korrekturfaktor based on the
//! Gütefaktor (ratio of a plant's Standortertrag to the Referenzertrag).
//! Without this correction, low-wind-site plants are under-compensated
//! and high-wind-site plants are over-compensated.
//!
//! ## Legal basis
//!
//! §36h Abs. 1 EEG 2023: "Der Netzbetreiber berechnet den anzulegenden Wert
//! aufgrund des Zuschlagswerts für den Referenzstandort nach Anlage 2 Nummer 4
//! … mit dem Korrekturfaktor des Gütefaktors, der nach Anlage 2 Nummer 2 und 7
//! ermittelt worden ist." The Gütefaktor is defined in §36h Abs. 1 Satz 5 as the
//! ratio of Standortertrag to Referenzertrag (Anlage 2 Nummer 2), in percent.
//! (§36k EEG 2021/2023 is "Finanzielle Beteiligung von Kommunen" — unrelated.)

use rust_decimal::Decimal;
use rust_decimal::dec;
use time::Date;

// ── WindStandort ──────────────────────────────────────────────────────────────

/// Wind onshore site quality and §36h correction data.
///
/// Certified by a BNetzA-accredited Windgutachter. Must be provided for all
/// wind onshore plants under EEG 2017/2021/2023 using Direktvermarktung.
///
/// ## Bestandsschutz
///
/// Plants commissioned before 01.01.2017 (EEG ≤2012, §100 Abs. 1 Satz 4 EEG 2017) do not
/// have a §36h Korrekturfaktor. Do not populate this struct for those plants.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct WindStandort {
    /// Gütegrad: ratio of actual site yield to reference yield, as a fraction.
    ///
    /// Examples:
    /// - `1.03` = 103% of reference yield (slightly above reference)
    /// - `0.85` = 85% of reference yield (below-reference = higher AW)
    /// - `1.50` = 150% of reference yield (excellent site = lower AW)
    ///
    /// Valid range: 0.70 – 2.00 (outside this range: contact BNetzA).
    pub guetegrad: Decimal,

    /// Pre-certified Korrekturfaktor from BNetzA §36h tables.
    ///
    /// Computed from `guetegrad` by the wind energy assessor using the BNetzA
    /// Korrekturfaktortabelle. Typical range: 0.70 – 1.30.
    ///
    /// The billing engine uses this to adjust the statutory AW:
    /// `effective_aw = base_aw × korrekturfaktor`
    pub korrekturfaktor: Decimal,

    /// Whether the **Grundvergütungsperiode** is currently active.
    ///
    /// Plants with Gütegrad < 100% receive a higher "Grundvergütung" rate
    /// for the first N full calendar years after commissioning.
    /// After that, the regular corrected AW applies.
    ///
    /// This flag is `true` during the Grundvergütungsperiode.
    pub grundverguetungsperiode_aktiv: bool,

    /// Site quality classification per BNetzA §36h table.
    pub standortklasse: WindStandortklasse,
}

impl WindStandort {
    /// Compute the effective Anzulegender Wert by applying the Korrekturfaktor.
    ///
    /// ```rust
    /// use eeg_billing::wind::{WindStandort, WindStandortklasse};
    /// use rust_decimal::dec;
    ///
    /// let standort = WindStandort {
    ///     guetegrad: dec!(0.95),
    ///     korrekturfaktor: dec!(1.06),
    ///     grundverguetungsperiode_aktiv: true,
    ///     standortklasse: WindStandortklasse::BelowReference,
    /// };
    /// let base_aw = dec!(7.0);
    /// let effective_aw = standort.effective_aw(base_aw);
    /// assert_eq!(effective_aw.round_dp(2), dec!(7.42)); // 7.0 × 1.06
    /// ```
    #[must_use]
    pub fn effective_aw(&self, base_aw_ct_kwh: Decimal) -> Decimal {
        (base_aw_ct_kwh * self.korrekturfaktor).round_dp(5)
    }

    /// Construct from Gütegrad using the §36h approximate formula.
    ///
    /// This is a simplified approximation of the BNetzA §36h correction table.
    /// For production billing, always use the certified Korrekturfaktor from
    /// the wind energy assessor's report (Windgutachten).
    ///
    /// ## Approximation formula
    ///
    /// Based on the §36h Abs. 1 / Anlage 2 Nr. 7 EEG 2023 correction curve:
    /// - Gütegrad < 0.80: not eligible for EEG support
    /// - 0.80 ≤ Gütegrad < 1.00: Korrekturfaktor = (1.25 − 0.25 × Gütegrad)
    /// - 1.00 ≤ Gütegrad ≤ 1.50: Korrekturfaktor = (0.90 − 0.10 × Gütegrad + 0.05)
    /// - Gütegrad > 1.50: Korrekturfaktor = 0.70 (floor)
    ///
    /// **Important**: Use certified values from §36h table in production.
    #[must_use]
    pub fn approximate_from_guetegrad(guetegrad: Decimal) -> Self {
        let korrekturfaktor = if guetegrad < dec!(0.80) {
            dec!(0.0) // not eligible
        } else if guetegrad < Decimal::ONE {
            // §36h Abs. 1 / Anlage 2 Nr. 7 interpolation for below-reference sites
            (dec!(1.25) - dec!(0.25) * guetegrad).round_dp(4)
        } else if guetegrad <= dec!(1.50) {
            // Above-reference sites
            (dec!(0.95) - dec!(0.10) * (guetegrad - Decimal::ONE)).round_dp(4)
        } else {
            dec!(0.70) // floor
        };

        let standortklasse = WindStandortklasse::from_guetegrad(guetegrad);
        let grundverguetungsperiode_aktiv = guetegrad < Decimal::ONE;

        Self {
            guetegrad,
            korrekturfaktor,
            grundverguetungsperiode_aktiv,
            standortklasse,
        }
    }
}

// ── §36h Abs. 2 EEG 2023 — Standortgüte re-evaluation (year 6/11/16) ─────────

/// §36h Abs. 2 EEG 2023 — one Standortgüte re-evaluation of the Gütefaktor.
///
/// The anzulegender Wert is re-adjusted **with effect from the start of the 6th,
/// 11th, and 16th** year after commissioning (§36h Abs. 2 Satz 1), based on the
/// measured Standortertrag of the preceding five years. Each re-evaluation
/// supersedes the previous Korrekturfaktor from its effective year onward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GuetefaktorReeval {
    /// Year after commissioning from which the adjusted AW takes effect: 6, 11 or 16.
    pub wirksam_ab_jahr: u8,
    /// Gütefaktor recomputed from the measured 5-year Standortertrag (ratio, e.g. 1.05).
    pub guetefaktor: Decimal,
}

/// §36h Abs. 2 EEG 2023 — the Korrekturfaktor in effect for `billing_date`.
///
/// `initial_korrekturfaktor` (from commissioning) applies until the start of the
/// 6th year; each re-evaluation supersedes it from its `wirksam_ab_jahr`. Returns
/// the factor of the re-evaluation with the latest effective year already reached,
/// or `initial_korrekturfaktor` when none apply yet.
#[must_use]
pub fn korrekturfaktor_fuer_periode(
    inbetriebnahme: Date,
    billing_date: Date,
    initial_korrekturfaktor: Decimal,
    reevals: &[GuetefaktorReeval],
) -> Decimal {
    reevals
        .iter()
        .filter(|r| jahr_erreicht(inbetriebnahme, billing_date, r.wirksam_ab_jahr))
        .max_by_key(|r| r.wirksam_ab_jahr)
        .map_or(initial_korrekturfaktor, |r| {
            WindStandort::approximate_from_guetegrad(r.guetefaktor).korrekturfaktor
        })
}

/// `true` when `billing_date` is at or after the start of the plant's `n`-th
/// operating year — year `n` begins `n − 1` years after commissioning.
fn jahr_erreicht(inbetriebnahme: Date, billing_date: Date, n: u8) -> bool {
    let target_year = inbetriebnahme.year() + i32::from(n) - 1;
    match inbetriebnahme.replace_year(target_year) {
        Ok(start) => billing_date >= start,
        // Feb-29 commissioning → non-leap target year: the year boundary suffices.
        Err(_) => billing_date.year() >= target_year,
    }
}

/// §36h Abs. 2 Satz 2 EEG 2023 — whether a re-evaluation triggers reconciliation
/// of the reviewed five-year period.
///
/// Over- or under-payments in the period must be settled when the recomputed
/// Gütefaktor deviates by **more than 2 percentage points** from the last one
/// (repayment claims bear interest at EURIBOR-12M + 1 pp, §36h Abs. 2 Satz 3).
#[must_use]
pub fn reevaluation_requires_reconciliation(
    previous_guetefaktor: Decimal,
    new_guetefaktor: Decimal,
) -> bool {
    (new_guetefaktor - previous_guetefaktor).abs() > dec!(0.02)
}

// ── WindStandortklasse ────────────────────────────────────────────────────────

/// Site quality classification based on Gütegrad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum WindStandortklasse {
    /// Gütegrad ≥ 150%: excellent site (reduced AW, Korrekturfaktor ≤ 0.70).
    Excellent,
    /// 110% ≤ Gütegrad < 150%: above-reference site.
    AboveReference,
    /// 90% ≤ Gütegrad < 110%: reference site (Korrekturfaktor ≈ 1.0).
    Reference,
    /// 80% ≤ Gütegrad < 90%: below-reference site (Grundvergütungsperiode applies).
    BelowReference,
    /// Gütegrad < 80%: marginal site (not eligible for EEG support).
    Marginal,
}

impl WindStandortklasse {
    /// Derive the Standortklasse from a Gütegrad value.
    #[must_use]
    pub fn from_guetegrad(guetegrad: Decimal) -> Self {
        if guetegrad >= dec!(1.50) {
            Self::Excellent
        } else if guetegrad >= dec!(1.10) {
            Self::AboveReference
        } else if guetegrad >= dec!(0.90) {
            Self::Reference
        } else if guetegrad >= dec!(0.80) {
            Self::BelowReference
        } else {
            Self::Marginal
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn sect36h_abs2_korrekturfaktor_steps_at_year_6_and_11() {
        let ibn = date!(2024 - 07 - 01);
        let initial = dec!(1.06);
        let reevals = [
            GuetefaktorReeval {
                wirksam_ab_jahr: 6,
                guetefaktor: dec!(0.90),
            },
            GuetefaktorReeval {
                wirksam_ab_jahr: 11,
                guetefaktor: dec!(0.95),
            },
        ];
        // Before year 6 → the initial factor.
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2028 - 12 - 01), initial, &reevals),
            initial
        );
        // Year 6 starts 2029-07-01 → the first re-evaluation applies.
        let kf_y6 = WindStandort::approximate_from_guetegrad(dec!(0.90)).korrekturfaktor;
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2029 - 07 - 01), initial, &reevals),
            kf_y6
        );
        // Year 11 starts 2034-07-01 → the second supersedes it.
        let kf_y11 = WindStandort::approximate_from_guetegrad(dec!(0.95)).korrekturfaktor;
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2034 - 08 - 01), initial, &reevals),
            kf_y11
        );
    }

    #[test]
    fn sect36h_abs2_reconciliation_threshold_is_two_points() {
        // ≤ 2 pp deviation → no reconciliation.
        assert!(!reevaluation_requires_reconciliation(
            dec!(1.00),
            dec!(1.02)
        ));
        // > 2 pp → reconciliation of the reviewed period.
        assert!(reevaluation_requires_reconciliation(
            dec!(1.00),
            dec!(1.021)
        ));
        assert!(reevaluation_requires_reconciliation(dec!(1.05), dec!(1.00)));
    }

    #[test]
    fn standortklasse_from_guetegrad() {
        assert_eq!(
            WindStandortklasse::from_guetegrad(dec!(1.60)),
            WindStandortklasse::Excellent
        );
        assert_eq!(
            WindStandortklasse::from_guetegrad(dec!(1.20)),
            WindStandortklasse::AboveReference
        );
        assert_eq!(
            WindStandortklasse::from_guetegrad(dec!(1.00)),
            WindStandortklasse::Reference
        );
        assert_eq!(
            WindStandortklasse::from_guetegrad(dec!(0.85)),
            WindStandortklasse::BelowReference
        );
        assert_eq!(
            WindStandortklasse::from_guetegrad(dec!(0.70)),
            WindStandortklasse::Marginal
        );
    }

    #[test]
    fn effective_aw_applies_korrekturfaktor() {
        let standort = WindStandort {
            guetegrad: dec!(0.85),
            korrekturfaktor: dec!(1.08),
            grundverguetungsperiode_aktiv: true,
            standortklasse: WindStandortklasse::BelowReference,
        };
        let effective = standort.effective_aw(dec!(7.35));
        // 7.35 × 1.08 = 7.938 (5dp)
        assert_eq!(effective, dec!(7.938));
    }

    #[test]
    fn reference_site_korrekturfaktor_near_one() {
        let standort = WindStandort::approximate_from_guetegrad(dec!(1.00));
        // At Gütegrad = 1.0, korrekturfaktor should be ≈ 0.95
        assert!(standort.korrekturfaktor > dec!(0.90) && standort.korrekturfaktor < dec!(1.10));
        assert!(!standort.grundverguetungsperiode_aktiv);
    }

    #[test]
    fn below_reference_triggers_grundverguetungsperiode() {
        let standort = WindStandort::approximate_from_guetegrad(dec!(0.85));
        assert!(standort.grundverguetungsperiode_aktiv);
        assert!(
            standort.korrekturfaktor > Decimal::ONE,
            "below-reference => higher AW"
        );
    }
}
