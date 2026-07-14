//! Wind onshore reference yield model — §36k EEG 2023.
//!
//! Onshore wind tariffs are location-corrected by a BNetzA-certified
//! Korrekturfaktor based on the ratio of local to reference yield.
//! Without this correction, low-wind-site plants are under-compensated
//! and high-wind-site plants are over-compensated.
//!
//! ## Legal basis
//!
//! §36k EEG 2023: "Der Wert, den der Netzbetreiber an den
//! Anlagenbetreiber zahlt, ist von einem Umrechnungsfaktor abhängig."
//!
//! The Korrekturfaktor is computed from BNetzA tables based on the plant's
//! Gütegrad (ratio of modeled annual yield to reference yield at 100% site).

use rust_decimal::Decimal;
use rust_decimal_macros::dec;

// ── WindStandort ──────────────────────────────────────────────────────────────

/// Wind onshore site quality and §36k correction data.
///
/// Certified by a BNetzA-accredited Windgutachter. Must be provided for all
/// wind onshore plants under EEG 2017/2021/2023 using Direktvermarktung.
///
/// ## Bestandsschutz
///
/// Plants commissioned before 01.01.2017 (EEG ≤2012, §100 Abs. 1 Satz 4 EEG 2017) do not
/// have a §36k Korrekturfaktor. Do not populate this struct for those plants.
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

    /// Pre-certified Korrekturfaktor from BNetzA §36k tables.
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

    /// Site quality classification per BNetzA §36k table.
    pub standortklasse: WindStandortklasse,
}

impl WindStandort {
    /// Compute the effective Anzulegender Wert by applying the Korrekturfaktor.
    ///
    /// ```rust
    /// use eeg_billing::wind::{WindStandort, WindStandortklasse};
    /// use rust_decimal_macros::dec;
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

    /// Construct from Gütegrad using the §36k approximate formula.
    ///
    /// This is a simplified approximation of the BNetzA §36k correction table.
    /// For production billing, always use the certified Korrekturfaktor from
    /// the wind energy assessor's report (Windgutachten).
    ///
    /// ## Approximation formula
    ///
    /// Based on §36k Abs. 2 EEG 2023 correction curve:
    /// - Gütegrad < 0.80: not eligible for EEG support
    /// - 0.80 ≤ Gütegrad < 1.00: Korrekturfaktor = (1.25 − 0.25 × Gütegrad)
    /// - 1.00 ≤ Gütegrad ≤ 1.50: Korrekturfaktor = (0.90 − 0.10 × Gütegrad + 0.05)
    /// - Gütegrad > 1.50: Korrekturfaktor = 0.70 (floor)
    ///
    /// **Important**: Use certified values from §36k table in production.
    #[must_use]
    pub fn approximate_from_guetegrad(guetegrad: Decimal) -> Self {
        let korrekturfaktor = if guetegrad < dec!(0.80) {
            dec!(0.0) // not eligible
        } else if guetegrad < Decimal::ONE {
            // §36k Abs. 2 interpolation for below-reference sites
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
