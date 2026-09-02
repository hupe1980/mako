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
//!
//! The Korrekturfaktor **Stützwerte live in §36h Abs. 1 Satz 2 itself**, not in
//! Anlage 2 Nummer 7 — Anlage 2 Nummer 7 only defines how the Standortertrag
//! that feeds the Gütefaktor is measured. See [`KORREKTURFAKTOR_STUETZWERTE`].

use crate::rounding::RoundMoney;
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
    /// Gütefaktor: ratio of Standortertrag to Referenzertrag, as a fraction.
    ///
    /// §36h Abs. 1 Satz 5 states it in percent; this field carries the same
    /// quantity as a fraction (`1.03` = 103 %).
    ///
    /// Examples:
    /// - `1.03` = 103 % of reference yield (slightly above reference)
    /// - `0.85` = 85 % of reference yield (below-reference = higher AW)
    /// - `1.50` = 150 % of reference yield (excellent site = lower AW)
    pub guetefaktor: Decimal,

    /// Korrekturfaktor from the §36h Abs. 1 Satz 2 Stützwerte.
    ///
    /// Derive it with [`korrekturfaktor_fuer_guetefaktor`] or take the value
    /// certified in the Windgutachten. Range: 0.79 – 1.55.
    ///
    /// The billing engine uses this to adjust the statutory AW:
    /// `effective_aw = base_aw × korrekturfaktor`
    pub korrekturfaktor: Decimal,

    /// **§36h Abs. 1 Satz 2 Halbsatz 2** — whether the plant is in the Südregion.
    ///
    /// A Gütefaktor below 60 % may only be applied to Südregion plants; for all
    /// others the Korrekturfaktor is capped at the 60 % Stützwert (1.55 vs 1.42,
    /// §36h Abs. 1 Satz 4 Nr. 2 and 3).
    pub suedregion: bool,

    /// Site quality classification derived from the Gütefaktor.
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
    ///     guetefaktor: dec!(0.95),
    ///     korrekturfaktor: dec!(1.035),
    ///     suedregion: false,
    ///     standortklasse: WindStandortklasse::BelowReference,
    /// };
    /// let base_aw = dec!(7.0);
    /// assert_eq!(standort.effective_aw(base_aw), dec!(7.245)); // 7.0 × 1.035
    /// ```
    #[must_use]
    pub fn effective_aw(&self, base_aw_ct_kwh: Decimal) -> Decimal {
        (base_aw_ct_kwh * self.korrekturfaktor).round_kfm(5)
    }

    /// Construct from the certified Gütefaktor via the §36h Abs. 1 Satz 2 table.
    ///
    /// `suedregion` selects between the §36h Abs. 1 Satz 4 Nr. 2 and Nr. 3 floors
    /// below the tabled range — see [`korrekturfaktor_fuer_guetefaktor`].
    #[must_use]
    pub fn from_guetefaktor(guetefaktor: Decimal, suedregion: bool) -> Self {
        Self {
            guetefaktor,
            korrekturfaktor: korrekturfaktor_fuer_guetefaktor(guetefaktor, suedregion),
            suedregion,
            standortklasse: WindStandortklasse::from_guetefaktor(guetefaktor),
        }
    }
}

// ── §36h Abs. 1 Satz 2 EEG 2023 — Korrekturfaktor Stützwerte ─────────────────

/// **§36h Abs. 1 Satz 2 EEG 2023** — the Gütefaktor → Korrekturfaktor Stützwerte.
///
/// | Gütefaktor | 50 % | 60 % | 70 % | 80 % | 90 % | 100 % | 110 % | 120 % | 130 % | 140 % | 150 % |
/// |---|---|---|---|---|---|---|---|---|---|---|---|
/// | Korrekturfaktor | 1,55 | 1,42 | 1,29 | 1,16 | 1,07 | 1 | 0,94 | 0,89 | 0,85 | 0,81 | 0,79 |
///
/// Gütefaktor is stored here as a fraction, so `100 %` is `1.00` and the
/// reference site's Korrekturfaktor is exactly `1` — an exactly-reference site
/// receives the unmodified Zuschlagswert.
pub const KORREKTURFAKTOR_STUETZWERTE: [(Decimal, Decimal); 11] = [
    (dec!(0.50), dec!(1.55)),
    (dec!(0.60), dec!(1.42)),
    (dec!(0.70), dec!(1.29)),
    (dec!(0.80), dec!(1.16)),
    (dec!(0.90), dec!(1.07)),
    (dec!(1.00), dec!(1.00)),
    (dec!(1.10), dec!(0.94)),
    (dec!(1.20), dec!(0.89)),
    (dec!(1.30), dec!(0.85)),
    (dec!(1.40), dec!(0.81)),
    (dec!(1.50), dec!(0.79)),
];

/// **§36h Abs. 1 Satz 2–4 EEG 2023** — the Korrekturfaktor for a Gütefaktor.
///
/// Linear interpolation between the neighbouring [`KORREKTURFAKTOR_STUETZWERTE`]
/// (Satz 3). Outside the tabled range (Satz 4):
///
/// - above 150 %: `0.79`
/// - Südregion, below 50 %: `1.55`
/// - all other plants, below 60 %: `1.42` — a Gütefaktor under 60 % may only be
///   applied to Südregion plants (Satz 2 Halbsatz 2)
///
/// There is **no eligibility floor** in §36h: a poor site yields a *higher*
/// Korrekturfaktor, never a zero one.
///
/// ```rust
/// use eeg_billing::wind::korrekturfaktor_fuer_guetefaktor;
/// use rust_decimal::dec;
///
/// // §36h Abs. 1 Satz 2: the reference site is exactly 1, not an approximation.
/// assert_eq!(korrekturfaktor_fuer_guetefaktor(dec!(1.00), false), dec!(1.00));
/// // Interpolated midway between the 90 % (1.07) and 100 % (1.00) Stützwerte.
/// assert_eq!(korrekturfaktor_fuer_guetefaktor(dec!(0.95), false), dec!(1.035));
/// ```
#[must_use]
pub fn korrekturfaktor_fuer_guetefaktor(guetefaktor: Decimal, suedregion: bool) -> Decimal {
    let (lowest_gf, lowest_kf) = KORREKTURFAKTOR_STUETZWERTE[0];
    let (highest_gf, highest_kf) =
        KORREKTURFAKTOR_STUETZWERTE[KORREKTURFAKTOR_STUETZWERTE.len() - 1];

    // Satz 4 Nr. 3: non-Südregion plants never go below the 60 % Stützwert.
    let floor_gf = if suedregion { lowest_gf } else { dec!(0.60) };
    if guetefaktor <= floor_gf {
        return if suedregion { lowest_kf } else { dec!(1.42) };
    }
    if guetefaktor >= highest_gf {
        return highest_kf; // Satz 4 Nr. 1
    }

    // Satz 3: linear interpolation between the neighbouring Stützwerte.
    let upper = KORREKTURFAKTOR_STUETZWERTE
        .iter()
        .position(|(gf, _)| *gf >= guetefaktor)
        .unwrap_or(KORREKTURFAKTOR_STUETZWERTE.len() - 1);
    let (gf_lo, kf_lo) = KORREKTURFAKTOR_STUETZWERTE[upper - 1];
    let (gf_hi, kf_hi) = KORREKTURFAKTOR_STUETZWERTE[upper];
    (kf_lo + (kf_hi - kf_lo) * (guetefaktor - gf_lo) / (gf_hi - gf_lo)).round_kfm(5)
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
    ///
    /// Carried for the §36h Abs. 2 Satz 2 reconciliation test, which compares
    /// Gütefaktoren — not Korrekturfaktoren.
    pub guetefaktor: Decimal,
    /// The Korrekturfaktor certified for this re-evaluation.
    ///
    /// Supplied rather than derived: §36h Abs. 3 Nr. 2 makes the adjusted claim
    /// conditional on the operator *proving* the new Gütefaktor by Gutachten, and
    /// that Gutachten's factor is what the Netzbetreiber settles on. Build it with
    /// [`korrekturfaktor_fuer_guetefaktor`] when no certified value is at hand.
    pub korrekturfaktor: Decimal,
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
        .map_or(initial_korrekturfaktor, |r| r.korrekturfaktor)
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

/// Site quality classification based on the Gütefaktor.
///
/// Descriptive only — §36h sets no eligibility threshold, so no class here means
/// "not funded". A weak site is *better* paid, via a Korrekturfaktor above 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum WindStandortklasse {
    /// Gütefaktor ≥ 150 %: excellent site (Korrekturfaktor floored at 0.79).
    Excellent,
    /// 110 % ≤ Gütefaktor < 150 %: above-reference site.
    AboveReference,
    /// 90 % ≤ Gütefaktor < 110 %: reference site (Korrekturfaktor ≈ 1.0).
    Reference,
    /// 60 % ≤ Gütefaktor < 90 %: below-reference site.
    BelowReference,
    /// Gütefaktor < 60 %: only applicable to Südregion plants (§36h Abs. 1 Satz 2).
    Suedregion,
}

impl WindStandortklasse {
    /// Derive the Standortklasse from a Gütefaktor value.
    #[must_use]
    pub fn from_guetefaktor(guetefaktor: Decimal) -> Self {
        if guetefaktor >= dec!(1.50) {
            Self::Excellent
        } else if guetefaktor >= dec!(1.10) {
            Self::AboveReference
        } else if guetefaktor >= dec!(0.90) {
            Self::Reference
        } else if guetefaktor >= dec!(0.60) {
            Self::BelowReference
        } else {
            Self::Suedregion
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    /// The eleven §36h Abs. 1 Satz 2 Stützwerte, as published.
    #[test]
    fn sect36h_abs1_satz2_stuetzwerte() {
        for (gf, kf) in [
            (dec!(0.50), dec!(1.55)),
            (dec!(0.60), dec!(1.42)),
            (dec!(0.70), dec!(1.29)),
            (dec!(0.80), dec!(1.16)),
            (dec!(0.90), dec!(1.07)),
            (dec!(1.00), dec!(1.00)),
            (dec!(1.10), dec!(0.94)),
            (dec!(1.20), dec!(0.89)),
            (dec!(1.30), dec!(0.85)),
            (dec!(1.40), dec!(0.81)),
            (dec!(1.50), dec!(0.79)),
        ] {
            assert_eq!(
                korrekturfaktor_fuer_guetefaktor(gf, true),
                kf,
                "Gütefaktor {gf} (Südregion)"
            );
            // Only the sub-60 % Stützwert is Südregion-only.
            if gf >= dec!(0.60) {
                assert_eq!(korrekturfaktor_fuer_guetefaktor(gf, false), kf);
            }
        }
    }

    /// §36h Abs. 1 Satz 2: an exactly-reference site keeps its full Zuschlagswert.
    #[test]
    fn sect36h_reference_site_is_exactly_one() {
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(Decimal::ONE, false),
            dec!(1)
        );
        assert_eq!(
            WindStandort::from_guetefaktor(Decimal::ONE, false).effective_aw(dec!(7.35)),
            dec!(7.35)
        );
    }

    /// §36h Abs. 1 Satz 3 — linear interpolation between neighbouring Stützwerte.
    #[test]
    fn sect36h_abs1_satz3_interpolates_linearly() {
        // Halfway 90 %→100 %: (1.07 + 1.00) / 2.
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(0.95), false),
            dec!(1.035)
        );
        // A fifth of the way 100 %→110 %: 1.00 − 0.2 × 0.06.
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(1.02), false),
            dec!(0.988)
        );
    }

    /// §36h Abs. 1 Satz 4 — the three out-of-range rules, and no eligibility floor.
    #[test]
    fn sect36h_abs1_satz4_out_of_range_rules() {
        // Nr. 1: above 150 % → 0.79.
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(1.80), false),
            dec!(0.79)
        );
        // Nr. 2: Südregion below 50 % → 1.55.
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(0.40), true),
            dec!(1.55)
        );
        // Nr. 3: all other plants below 60 % → 1.42, never zero.
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(0.55), false),
            dec!(1.42)
        );
        assert_eq!(
            korrekturfaktor_fuer_guetefaktor(dec!(0.10), false),
            dec!(1.42)
        );
    }

    #[test]
    fn sect36h_abs2_korrekturfaktor_steps_at_year_6_and_11() {
        let ibn = date!(2024 - 07 - 01);
        let initial = dec!(1.06);
        let reevals = [
            GuetefaktorReeval {
                wirksam_ab_jahr: 6,
                guetefaktor: dec!(0.90),
                korrekturfaktor: dec!(1.07),
            },
            GuetefaktorReeval {
                wirksam_ab_jahr: 11,
                guetefaktor: dec!(0.95),
                korrekturfaktor: dec!(1.035),
            },
        ];
        // Before year 6 → the initial factor.
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2028 - 12 - 01), initial, &reevals),
            initial
        );
        // Year 6 starts 2029-07-01 → the certified factor of the first re-evaluation.
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2029 - 07 - 01), initial, &reevals),
            dec!(1.07)
        );
        // Year 11 starts 2034-07-01 → the second supersedes it.
        assert_eq!(
            korrekturfaktor_fuer_periode(ibn, date!(2034 - 08 - 01), initial, &reevals),
            dec!(1.035)
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
    fn standortklasse_from_guetefaktor() {
        assert_eq!(
            WindStandortklasse::from_guetefaktor(dec!(1.60)),
            WindStandortklasse::Excellent
        );
        assert_eq!(
            WindStandortklasse::from_guetefaktor(dec!(1.20)),
            WindStandortklasse::AboveReference
        );
        assert_eq!(
            WindStandortklasse::from_guetefaktor(dec!(1.00)),
            WindStandortklasse::Reference
        );
        assert_eq!(
            WindStandortklasse::from_guetefaktor(dec!(0.85)),
            WindStandortklasse::BelowReference
        );
        assert_eq!(
            WindStandortklasse::from_guetefaktor(dec!(0.55)),
            WindStandortklasse::Suedregion
        );
    }

    #[test]
    fn effective_aw_applies_korrekturfaktor() {
        let standort = WindStandort {
            guetefaktor: dec!(0.85),
            korrekturfaktor: dec!(1.115),
            suedregion: false,
            standortklasse: WindStandortklasse::BelowReference,
        };
        // 7.35 × 1.115 = 8.19525
        assert_eq!(standort.effective_aw(dec!(7.35)), dec!(8.19525));
    }

    #[test]
    fn below_reference_site_raises_the_aw() {
        let standort = WindStandort::from_guetefaktor(dec!(0.85), false);
        // Halfway 80 %→90 %: (1.16 + 1.07) / 2 = 1.115.
        assert_eq!(standort.korrekturfaktor, dec!(1.115));
        assert!(standort.korrekturfaktor > Decimal::ONE);
    }
}
