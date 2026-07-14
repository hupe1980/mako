//! §23a EEG 2023 — quarterly solar PV feed-in tariff degression.
//!
//! Solar PV rates in Germany decrease quarterly under §23a EEG 2023 (§49 EEG 2021).
//! The degression percentage per quarter is published annually by BNetzA based on
//! the previous year's total new solar PV capacity (installed GW).
//!
//! ## Annual degression schedule (§23a Abs. 2 EEG 2023)
//!
//! | Previous year PV added | Quarterly degression rate |
//! |---|---|
//! | ≤ 9 GW | **0.00 %** — no degression |
//! | > 9 GW ≤ 12 GW | **0.25 %** per quarter |
//! | > 12 GW ≤ 13 GW | **0.50 %** per quarter |
//! | > 13 GW ≤ 14 GW | **1.00 %** per quarter |
//! | > 14 GW ≤ 15 GW | **1.40 %** per quarter |
//! | > 15 GW | **1.50 %** per quarter (maximum) |
//!
//! The "start rate" (Anfangssatz) for a given calendar year is the rate published
//! by BNetzA in Q1 of that year. From Q2 onward the rate is degresssed quarterly.
//!
//! ## Accelerator (§23a Abs. 4 EEG 2023)
//!
//! If rolling 12-month installed capacity exceeds 10 GW, BNetzA may publish an
//! **accelerated degression** notice increasing the quarterly rate by an additional
//! 1.0 % step. This is rare (has not triggered as of 2026) and handled by using a
//! custom `DegressionTier::Custom(rate)` variant.
//!
//! ## How to use in production
//!
//! 1. Load the BNetzA-published quarterly degression percentages into
//!    `einsd`'s `eeg_verguetungssaetze` DB table (keyed by technology + quarter).
//! 2. Use `einsd`'s `lookup_verguetungssatz` endpoint to get the exact rate for
//!    a commissioning date — it already stores the quarterly-degresssed net rate.
//! 3. Use this module's [`apply_degression`] + [`solar_ueberschuss_rate_for_quarter`]
//!    helpers for:
//!    - **On-demand verification**: re-derive expected rates from reference rates
//!    - **Rate forecasting**: compute expected future rates for new plants
//!    - **Gap-filling**: fill missing quarters in the DB table
//!
//! This module provides the **formula** — not the BNetzA data.
//! BNetzA publishes actual degression percentages here:
//! <https://www.bundesnetzagentur.de/DE/Fachthemen/ElektrizitaetundGas/ErneuerbareEnergien/Solarenergie/start.html>

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::Date;

// ── DegressionTier ────────────────────────────────────────────────────────────

/// §23a Abs. 2 EEG 2023 — quarterly degression tier.
///
/// Determined by the previous calendar year's total new solar PV capacity
/// installed in Germany (as published by BNetzA in Q1 of the following year).
///
/// ## Historical note
///
/// Germany installed ~14.3 GW in 2023 and ~16.5 GW in 2024, placing both years
/// in the `High` / `Maximum` tier. The actual BNetzA quarterly bulletins
/// are authoritative; store them in `einsd`'s `eeg_verguetungssaetze` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum DegressionTier {
    /// ≤ 9 GW added: **0.00 %** per quarter — no degression.
    None,
    /// > 9 GW, ≤ 12 GW: **0.25 %** per quarter.
    Low,
    /// > 12 GW, ≤ 13 GW: **0.50 %** per quarter.
    Medium,
    /// > 13 GW, ≤ 14 GW: **1.00 %** per quarter (standard).
    Standard,
    /// > 14 GW, ≤ 15 GW: **1.40 %** per quarter.
    High,
    /// > 15 GW: **1.50 %** per quarter (maximum under §23a Abs. 2).
    Maximum,
}

impl DegressionTier {
    /// Return the quarterly degression rate in percent (e.g. `1.00` for 1 %).
    ///
    /// ```rust
    /// use eeg_billing::degression::DegressionTier;
    /// use rust_decimal_macros::dec;
    ///
    /// assert_eq!(DegressionTier::Standard.rate_pct_per_quarter(), dec!(1.00));
    /// assert_eq!(DegressionTier::None.rate_pct_per_quarter(),     dec!(0.00));
    /// ```
    #[must_use]
    pub fn rate_pct_per_quarter(self) -> Decimal {
        match self {
            Self::None => dec!(0.00),
            Self::Low => dec!(0.25),
            Self::Medium => dec!(0.50),
            Self::Standard => dec!(1.00),
            Self::High => dec!(1.40),
            Self::Maximum => dec!(1.50),
        }
    }

    /// Derive the degression tier from the previous year's installed GW.
    ///
    /// Source: §23a Abs. 2 EEG 2023 table.
    ///
    /// ```rust
    /// use eeg_billing::degression::DegressionTier;
    /// use rust_decimal_macros::dec;
    ///
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(8.5)),  DegressionTier::None);
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(11.0)), DegressionTier::Low);
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(12.5)), DegressionTier::Medium);
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(13.5)), DegressionTier::Standard);
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(14.5)), DegressionTier::High);
    /// assert_eq!(DegressionTier::from_gw_expansion(dec!(16.0)), DegressionTier::Maximum);
    /// ```
    #[must_use]
    pub fn from_gw_expansion(gw_added_prev_year: Decimal) -> Self {
        if gw_added_prev_year <= dec!(9) {
            Self::None
        } else if gw_added_prev_year <= dec!(12) {
            Self::Low
        } else if gw_added_prev_year <= dec!(13) {
            Self::Medium
        } else if gw_added_prev_year <= dec!(14) {
            Self::Standard
        } else if gw_added_prev_year <= dec!(15) {
            Self::High
        } else {
            Self::Maximum
        }
    }
}

// ── Quarter ───────────────────────────────────────────────────────────────────

/// A calendar quarter in `(year, quarter 1–4)` representation.
///
/// Q1 = January–March, Q2 = April–June, Q3 = July–September, Q4 = October–December.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Quarter {
    /// Calendar year (e.g. 2024).
    pub year: i32,
    /// Quarter within the year: 1–4.
    pub quarter: u8,
}

impl Quarter {
    /// Derive the quarter containing the given date.
    ///
    /// ```rust
    /// use eeg_billing::degression::Quarter;
    /// use time::macros::date;
    ///
    /// assert_eq!(Quarter::from_date(date!(2024-05-01)), Quarter { year: 2024, quarter: 2 });
    /// assert_eq!(Quarter::from_date(date!(2024-01-15)), Quarter { year: 2024, quarter: 1 });
    /// assert_eq!(Quarter::from_date(date!(2024-10-30)), Quarter { year: 2024, quarter: 4 });
    /// assert_eq!(Quarter::from_date(date!(2025-07-01)), Quarter { year: 2025, quarter: 3 });
    /// ```
    #[must_use]
    pub fn from_date(d: Date) -> Self {
        let month = d.month() as u8;
        Self {
            year: d.year(),
            quarter: (month - 1) / 3 + 1,
        }
    }

    /// Number of quarters elapsed since `base` (positive = self is later).
    ///
    /// May be negative when `self` is earlier than `base`.
    ///
    /// ```rust
    /// use eeg_billing::degression::Quarter;
    ///
    /// let q1_2024 = Quarter { year: 2024, quarter: 1 };
    /// let q3_2025 = Quarter { year: 2025, quarter: 3 };
    /// assert_eq!(q3_2025.quarters_since(q1_2024), 6);
    /// assert_eq!(q1_2024.quarters_since(q3_2025), -6);
    /// ```
    #[must_use]
    pub fn quarters_since(self, base: Quarter) -> i32 {
        (self.year - base.year) * 4 + (self.quarter as i32 - base.quarter as i32)
    }
}

impl std::fmt::Display for Quarter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Q{}/{}", self.quarter, self.year)
    }
}

// ── Reference quarters ────────────────────────────────────────────────────────

/// Reference quarter for **Solarpaket I** rates: **Q2 2024** (valid from 01.05.2024).
///
/// Rates commissioned from this quarter onward use the Solarpaket I reference rates
/// as their degression starting point.
pub const SOLARPAKET_I_REFERENCE_QUARTER: Quarter = Quarter {
    year: 2024,
    quarter: 2,
};

// ── Core degression formula ────────────────────────────────────────────────────

/// Apply quarterly compound degression to a reference rate.
///
/// Formula: `effective_rate_ct = reference_rate_ct × (1 − rate_pct/100)^n`
///
/// Result is rounded to **2 decimal places** (matching BDEW AHB published-rate
/// precision and the format used in `einsd`'s `eeg_verguetungssaetze` table).
///
/// ## Parameters
///
/// - `reference_rate_ct`: Starting rate in **ct/kWh** (e.g. `8.51`).
/// - `quarters_elapsed`: Quarters after the reference quarter (must be ≥ 0).
///   When 0 or negative, the reference rate is returned unchanged.
/// - `tier`: Degression tier from §23a Abs. 2 EEG 2023.
///
/// # Example
///
/// ```rust
/// use eeg_billing::degression::{apply_degression, DegressionTier};
/// use rust_decimal_macros::dec;
///
/// // Solarpaket I reference 8.51 ct/kWh, 4 quarters at Standard (1 %) tier
/// // 8.51 × 0.99^4 = 8.51 × 0.96059601 ≈ 8.17 ct/kWh
/// let rate = apply_degression(dec!(8.51), 4, DegressionTier::Standard);
/// assert_eq!(rate, dec!(8.17));
///
/// // No degression → rate unchanged
/// let unchanged = apply_degression(dec!(8.51), 4, DegressionTier::None);
/// assert_eq!(unchanged, dec!(8.51));
/// ```
#[must_use]
pub fn apply_degression(
    reference_rate_ct: Decimal,
    quarters_elapsed: i32,
    tier: DegressionTier,
) -> Decimal {
    if quarters_elapsed <= 0 {
        return reference_rate_ct.round_dp(2);
    }
    let factor_per_quarter = Decimal::ONE - tier.rate_pct_per_quarter() / dec!(100);
    // Compute factor_per_quarter^n using a simple loop (no `maths` feature needed).
    let n = quarters_elapsed as u32;
    let factor = (0..n).fold(Decimal::ONE, |acc, _| acc * factor_per_quarter);
    (reference_rate_ct * factor).round_dp(2)
}

// ── Solar PV rate helpers ─────────────────────────────────────────────────────

/// Solarpaket I (Q2 2024) reference rates for **Überschusseinspeisung** in ct/kWh.
///
/// Returns `None` for capacity brackets outside the table (> 1 MWp are tendering-mandatory).
#[must_use]
fn solarpaket_i_ueberschuss_base(leistung_kwp: Decimal) -> Option<Decimal> {
    if leistung_kwp <= dec!(10) {
        Some(dec!(8.51))
    } else if leistung_kwp <= dec!(40) {
        Some(dec!(7.43))
    } else if leistung_kwp <= dec!(1000) {
        Some(dec!(7.64)) // ≤ 1 MWp
    } else {
        None // > 1 MWp: Ausschreibungspflicht — no statutory Einspeisevergütung
    }
}

/// Solarpaket I (Q2 2024) reference rates for **Volleinspeisung** in ct/kWh.
///
/// Returns `None` for capacity brackets outside the table (> 1 MWp).
#[must_use]
fn solarpaket_i_volleinspeisung_base(leistung_kwp: Decimal) -> Option<Decimal> {
    if leistung_kwp <= dec!(10) {
        Some(dec!(13.31)) // 8.51 + 4.80
    } else if leistung_kwp <= dec!(40) {
        Some(dec!(11.23)) // 7.43 + 3.80
    } else if leistung_kwp <= dec!(100) {
        Some(dec!(12.74)) // 7.64 + 5.10
    } else if leistung_kwp <= dec!(400) {
        Some(dec!(10.84)) // 7.64 + 3.20
    } else if leistung_kwp <= dec!(1000) {
        Some(dec!(9.54)) // 7.64 + 1.90
    } else {
        None // > 1 MWp: Ausschreibungspflicht
    }
}

/// Compute the effective **Überschusseinspeisung** rate (gross AW, before §53 deduction)
/// for a solar PV plant commissioned in the given quarter.
///
/// Uses Solarpaket I Q2 2024 as the reference point.
/// Returns `None` for:
/// - Plants commissioned before Q2 2024 (use historical `einsd` DB lookup)
/// - Plants > 1 MWp (Ausschreibungspflicht — no statutory feed-in rate)
///
/// The returned rate is the **gross AW**. Subtract the §53 EEG deduction
/// (`rates::sect53_deduction(ErzeugungsArt::SolarAufdach)` = 0.4 ct/kWh)
/// before storing as `verguetungssatz_ct` in the settlement engine.
///
/// # Example
///
/// ```rust
/// use eeg_billing::degression::{solar_ueberschuss_rate_for_quarter, DegressionTier, Quarter};
/// use rust_decimal_macros::dec;
///
/// // Reference quarter Q2 2024, ≤10 kWp, no degression
/// let rate = solar_ueberschuss_rate_for_quarter(
///     Quarter { year: 2024, quarter: 2 },
///     dec!(9),
///     DegressionTier::None,
/// );
/// assert_eq!(rate, Some(dec!(8.51)));
///
/// // Q4 2024 (2 quarters after reference), Standard (1 %) tier
/// // 8.51 × 0.99^2 ≈ 8.34 ct/kWh
/// let rate2 = solar_ueberschuss_rate_for_quarter(
///     Quarter { year: 2024, quarter: 4 },
///     dec!(9),
///     DegressionTier::Standard,
/// );
/// assert_eq!(rate2, Some(dec!(8.34)));
/// ```
#[must_use]
pub fn solar_ueberschuss_rate_for_quarter(
    commissioning_quarter: Quarter,
    leistung_kwp: Decimal,
    tier: DegressionTier,
) -> Option<Decimal> {
    let n = commissioning_quarter.quarters_since(SOLARPAKET_I_REFERENCE_QUARTER);
    if n < 0 {
        return None; // Before Solarpaket I — use historical DB lookup
    }
    let base = solarpaket_i_ueberschuss_base(leistung_kwp)?;
    Some(apply_degression(base, n, tier))
}

/// Compute the effective **Volleinspeisung** rate (gross AW, before §53 deduction)
/// for a solar PV plant commissioned in the given quarter.
///
/// Uses Solarpaket I Q2 2024 as the reference point.
/// Returns `None` for plants commissioned before Q2 2024 or > 1 MWp.
///
/// # Example
///
/// ```rust
/// use eeg_billing::degression::{solar_volleinspeisung_rate_for_quarter, DegressionTier, Quarter};
/// use rust_decimal_macros::dec;
///
/// // Reference quarter Q2 2024, ≤10 kWp, no degression
/// let rate = solar_volleinspeisung_rate_for_quarter(
///     Quarter { year: 2024, quarter: 2 },
///     dec!(9),
///     DegressionTier::None,
/// );
/// assert_eq!(rate, Some(dec!(13.31)));
/// ```
#[must_use]
pub fn solar_volleinspeisung_rate_for_quarter(
    commissioning_quarter: Quarter,
    leistung_kwp: Decimal,
    tier: DegressionTier,
) -> Option<Decimal> {
    let n = commissioning_quarter.quarters_since(SOLARPAKET_I_REFERENCE_QUARTER);
    if n < 0 {
        return None;
    }
    let base = solarpaket_i_volleinspeisung_base(leistung_kwp)?;
    Some(apply_degression(base, n, tier))
}

/// Compute the effective rate for a solar PV plant commissioned on a specific date.
///
/// Convenience wrapper around [`solar_ueberschuss_rate_for_quarter`] and
/// [`solar_volleinspeisung_rate_for_quarter`] using a full `Date` input.
///
/// ## Parameters
///
/// - `commissioning_date`: Plant commissioning date.
/// - `leistung_kwp`: Installed capacity.
/// - `volleinspeisung`: `true` = Volleinspeisung (full feed-in) rate; `false` = Überschuss.
/// - `tier`: Degression tier for the commissioning year (from BNetzA annual bulletin).
///
/// Returns `None` for plants commissioned before Solarpaket I or outside the table.
///
/// # Example
///
/// ```rust
/// use eeg_billing::degression::{solar_rate_at_commissioning, DegressionTier};
/// use rust_decimal_macros::dec;
/// use time::macros::date;
///
/// // 9 kWp plant, commissioned 2025-03-01 (Q1 2025 = 3 quarters after Q2 2024)
/// // at Standard (1 %) tier: 8.51 × 0.99^3 ≈ 8.26 ct/kWh
/// let rate = solar_rate_at_commissioning(date!(2025-03-01), dec!(9), false, DegressionTier::Standard);
/// assert_eq!(rate, Some(dec!(8.26)));
/// ```
#[must_use]
pub fn solar_rate_at_commissioning(
    commissioning_date: Date,
    leistung_kwp: Decimal,
    volleinspeisung: bool,
    tier: DegressionTier,
) -> Option<Decimal> {
    let q = Quarter::from_date(commissioning_date);
    if volleinspeisung {
        solar_volleinspeisung_rate_for_quarter(q, leistung_kwp, tier)
    } else {
        solar_ueberschuss_rate_for_quarter(q, leistung_kwp, tier)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn quarter_from_date_all_quarters() {
        assert_eq!(
            Quarter::from_date(date!(2024 - 01 - 15)),
            Quarter {
                year: 2024,
                quarter: 1
            }
        );
        assert_eq!(
            Quarter::from_date(date!(2024 - 05 - 01)),
            Quarter {
                year: 2024,
                quarter: 2
            }
        );
        assert_eq!(
            Quarter::from_date(date!(2024 - 09 - 30)),
            Quarter {
                year: 2024,
                quarter: 3
            }
        );
        assert_eq!(
            Quarter::from_date(date!(2024 - 10 - 01)),
            Quarter {
                year: 2024,
                quarter: 4
            }
        );
        assert_eq!(
            Quarter::from_date(date!(2024 - 12 - 31)),
            Quarter {
                year: 2024,
                quarter: 4
            }
        );
    }

    #[test]
    fn quarters_since_same_year() {
        let q1 = Quarter {
            year: 2024,
            quarter: 1,
        };
        let q4 = Quarter {
            year: 2024,
            quarter: 4,
        };
        assert_eq!(q4.quarters_since(q1), 3);
        assert_eq!(q1.quarters_since(q4), -3);
    }

    #[test]
    fn quarters_since_cross_year() {
        let start = Quarter {
            year: 2024,
            quarter: 1,
        };
        let end = Quarter {
            year: 2025,
            quarter: 3,
        };
        assert_eq!(end.quarters_since(start), 6);
    }

    #[test]
    fn no_degression_keeps_rate() {
        assert_eq!(
            apply_degression(dec!(8.51), 8, DegressionTier::None),
            dec!(8.51)
        );
        assert_eq!(
            apply_degression(dec!(8.51), 0, DegressionTier::Standard),
            dec!(8.51)
        );
    }

    #[test]
    fn standard_1pct_4_quarters() {
        // 8.51 × 0.99^4 = 8.51 × 0.96059601 ≈ 8.17495 → round to 8.17
        let rate = apply_degression(dec!(8.51), 4, DegressionTier::Standard);
        assert_eq!(rate, dec!(8.17));
    }

    #[test]
    fn low_025pct_degression() {
        // 8.51 × 0.9975^4 = 8.51 × 0.990037… ≈ 8.42 ct
        let rate = apply_degression(dec!(8.51), 4, DegressionTier::Low);
        assert!(rate > dec!(8.40) && rate <= dec!(8.51));
    }

    #[test]
    fn maximum_15pct_degression() {
        // 8.51 × 0.985^8 = 8.51 × 0.8872… ≈ 7.55
        let rate = apply_degression(dec!(8.51), 8, DegressionTier::Maximum);
        assert!(rate > dec!(7.40) && rate < dec!(8.51));
    }

    #[test]
    fn solar_ueberschuss_reference_quarter_unchanged() {
        let rate = solar_ueberschuss_rate_for_quarter(
            SOLARPAKET_I_REFERENCE_QUARTER,
            dec!(9),
            DegressionTier::None,
        );
        assert_eq!(rate, Some(dec!(8.51)));
    }

    #[test]
    fn solar_ueberschuss_2_quarters_standard() {
        // Q4 2024: 2 quarters after Q2 2024, 1% tier → 8.51 × 0.99^2 ≈ 8.34
        let rate = solar_ueberschuss_rate_for_quarter(
            Quarter {
                year: 2024,
                quarter: 4,
            },
            dec!(9),
            DegressionTier::Standard,
        );
        assert_eq!(rate, Some(dec!(8.34)));
    }

    #[test]
    fn solar_ueberschuss_before_solarpaket_returns_none() {
        let rate = solar_ueberschuss_rate_for_quarter(
            Quarter {
                year: 2024,
                quarter: 1,
            },
            dec!(9),
            DegressionTier::Standard,
        );
        assert!(rate.is_none());
    }

    #[test]
    fn solar_volleinspeisung_reference_quarter() {
        let rate = solar_volleinspeisung_rate_for_quarter(
            SOLARPAKET_I_REFERENCE_QUARTER,
            dec!(9),
            DegressionTier::None,
        );
        assert_eq!(rate, Some(dec!(13.31)));
    }

    #[test]
    fn solar_rate_at_commissioning_date() {
        // 2025-03-01 = Q1 2025 = 3 quarters after Q2 2024
        // 8.51 × 0.99^3 = 8.51 × 0.970299 ≈ 8.257 → 8.26
        let rate = solar_rate_at_commissioning(
            date!(2025 - 03 - 01),
            dec!(9),
            false,
            DegressionTier::Standard,
        );
        assert_eq!(rate, Some(dec!(8.26)));
    }

    #[test]
    fn tier_from_gw_expansion_all_brackets() {
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(8.5)),
            DegressionTier::None
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(9.0)),
            DegressionTier::None
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(9.1)),
            DegressionTier::Low
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(12.0)),
            DegressionTier::Low
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(12.5)),
            DegressionTier::Medium
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(13.5)),
            DegressionTier::Standard
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(14.5)),
            DegressionTier::High
        );
        assert_eq!(
            DegressionTier::from_gw_expansion(dec!(16.0)),
            DegressionTier::Maximum
        );
    }

    #[test]
    fn large_plant_above_1mwp_returns_none() {
        // > 1 MWp solar: Ausschreibungspflicht, no statutory feed-in rate
        let rate = solar_ueberschuss_rate_for_quarter(
            SOLARPAKET_I_REFERENCE_QUARTER,
            dec!(1500), // 1.5 MWp
            DegressionTier::None,
        );
        assert!(rate.is_none());
    }

    #[test]
    fn quarter_display() {
        let q = Quarter {
            year: 2024,
            quarter: 2,
        };
        assert_eq!(format!("{q}"), "Q2/2024");
    }
}
