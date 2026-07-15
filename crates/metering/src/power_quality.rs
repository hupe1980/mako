//! Power quality data types for SMGW and RLM advanced metering.
//!
//! ## Scope
//!
//! German smart meters (iMSys / SMGW per BSI TR-03109) transmit power quality
//! data alongside energy measurements. This module provides typed structures
//! for storing and analysing power quality intervals.
//!
//! ## Regulatory basis
//!
//! - **BSI TR-03109-1** (Smart Meter Gateway) — power quality measurement requirements
//! - **DIN EN 50160** — voltage characteristics in public distribution networks
//! - **§10a MessZV** — smart meter data management obligations
//! - **MsbG §29** — Messstellenbetreiber power quality reporting
//!
//! ## OBIS codes for power quality
//!
//! | OBIS code | Quantity | Unit |
//! |---|---|---|
//! | `1-0:12.7.0*255` | Voltage L1 (avg) | V |
//! | `1-0:52.7.0*255` | Voltage L2 (avg) | V |
//! | `1-0:72.7.0*255` | Voltage L3 (avg) | V |
//! | `1-0:11.7.0*255` | Current L1 (avg) | A |
//! | `1-0:14.7.0*255` | Frequency | Hz |
//! | `1-0:13.7.0*255` | Power factor (cos φ) | — |

use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A power quality measurement interval.
///
/// Captures the average electrical parameters during one measurement interval.
/// All values are averages over the interval duration unless noted.
///
/// ## Relationship to energy intervals
///
/// `PowerQualityInterval` shares the same `(from, to)` window as `MeterInterval`.
/// They are stored separately — energy intervals go into `meter_reads`, power
/// quality data into a dedicated `power_quality_reads` table (planned).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct PowerQualityInterval {
    /// Interval start (UTC).
    pub from: OffsetDateTime,
    /// Interval end (UTC).
    pub to: OffsetDateTime,

    // ── Voltage (Spannung) ─────────────────────────────────────────────────
    /// L1 phase voltage in Volt (average over interval). `None` when not measured.
    pub voltage_l1_v: Option<Decimal>,
    /// L2 phase voltage in Volt. `None` for single-phase meters.
    pub voltage_l2_v: Option<Decimal>,
    /// L3 phase voltage in Volt. `None` for single-phase meters.
    pub voltage_l3_v: Option<Decimal>,

    // ── Current (Strom) ────────────────────────────────────────────────────
    /// L1 phase current in Ampere (average over interval).
    pub current_l1_a: Option<Decimal>,
    /// L2 phase current in Ampere.
    pub current_l2_a: Option<Decimal>,
    /// L3 phase current in Ampere.
    pub current_l3_a: Option<Decimal>,

    // ── Frequency ──────────────────────────────────────────────────────────
    /// Grid frequency in Hz (average over interval). Nominal: 50.00 Hz.
    ///
    /// DIN EN 50160: normal range 49.5–50.5 Hz (99.5% of time).
    pub frequency_hz: Option<Decimal>,

    // ── Power factor ───────────────────────────────────────────────────────
    /// Power factor (cos φ, dimensionless, 0.0–1.0).
    ///
    /// Industrial customers with low power factor (<0.9) may incur reactive
    /// energy surcharges per their NNE tariff.
    pub power_factor: Option<Decimal>,

    // ── Harmonic distortion ────────────────────────────────────────────────
    /// Total harmonic distortion of voltage (%), if measured by SMGW.
    ///
    /// DIN EN 50160: THD_U should be < 8% under normal conditions.
    pub thd_voltage_pct: Option<Decimal>,
    /// Total harmonic distortion of current (%).
    pub thd_current_pct: Option<Decimal>,
}

impl PowerQualityInterval {
    /// `true` when the voltage deviates more than `threshold_pct` from `nominal_v`.
    ///
    /// Per DIN EN 50160: ±10% of nominal is the standard tolerance (230V ± 23V).
    ///
    /// ## Example
    ///
    /// ```rust
    /// use metering::power_quality::PowerQualityInterval;
    /// use rust_decimal_macros::dec;
    /// use time::macros::datetime;
    ///
    /// let iv = PowerQualityInterval {
    ///     from: datetime!(2026-06-01 0:00 UTC),
    ///     to:   datetime!(2026-06-01 0:15 UTC),
    ///     voltage_l1_v: Some(dec!(245.5)),
    ///     ..Default::default()
    /// };
    /// // 245.5V vs 230V nominal → deviation = 6.7%, within ±10%
    /// assert!(!iv.voltage_out_of_range(dec!(230), dec!(10)));
    /// ```
    #[must_use]
    pub fn voltage_out_of_range(&self, nominal_v: Decimal, threshold_pct: Decimal) -> bool {
        let check = |v: Option<Decimal>| {
            v.map(|voltage| {
                let deviation = ((voltage - nominal_v) / nominal_v * Decimal::from(100)).abs();
                deviation > threshold_pct
            })
            .unwrap_or(false)
        };
        check(self.voltage_l1_v) || check(self.voltage_l2_v) || check(self.voltage_l3_v)
    }

    /// `true` when frequency deviates more than `threshold_hz` from 50.0 Hz.
    #[must_use]
    pub fn frequency_out_of_range(&self, threshold_hz: Decimal) -> bool {
        self.frequency_hz
            .map(|f| (f - Decimal::from(50u32)).abs() > threshold_hz)
            .unwrap_or(false)
    }

    /// `true` when power factor is below `min_pf` (inductive load, cos φ too low).
    ///
    /// Industrial/commercial customers with cos φ < 0.9 typically pay reactive
    /// energy surcharges per their NNE tariff. RLM customers should maintain ≥ 0.9.
    ///
    /// DIN EN 50160 does not mandate a specific cos φ, but NNE tariffs do.
    #[must_use]
    pub fn power_factor_below_threshold(&self, min_pf: Decimal) -> bool {
        self.power_factor.map(|pf| pf < min_pf).unwrap_or(false)
    }

    /// `true` when THD of voltage exceeds `max_thd_pct` percent.
    ///
    /// DIN EN 50160: THD_U should remain below 8% under normal network conditions.
    #[must_use]
    pub fn voltage_thd_exceeded(&self, max_thd_pct: Decimal) -> bool {
        self.thd_voltage_pct
            .map(|thd| thd > max_thd_pct)
            .unwrap_or(false)
    }

    /// `true` when any power quality parameter is outside its normal range.
    ///
    /// Convenience method applying DIN EN 50160 nominal thresholds:
    /// - Voltage: ±10% of 230V nominal
    /// - Frequency: ±0.5 Hz
    /// - Power factor: < 0.9 (if measured)
    /// - THD voltage: > 8% (if measured)
    #[must_use]
    pub fn has_quality_issue(&self) -> bool {
        self.voltage_out_of_range(Decimal::from(230u32), Decimal::from(10u32))
            || self.frequency_out_of_range(rust_decimal::Decimal::new(5, 1)) // 0.5 Hz
            || self.power_factor_below_threshold(rust_decimal::Decimal::new(9, 1)) // 0.9
            || self.voltage_thd_exceeded(Decimal::from(8u32))
    }
}

impl Default for PowerQualityInterval {
    fn default() -> Self {
        use time::OffsetDateTime;
        Self {
            from: OffsetDateTime::UNIX_EPOCH,
            to: OffsetDateTime::UNIX_EPOCH,
            voltage_l1_v: None,
            voltage_l2_v: None,
            voltage_l3_v: None,
            current_l1_a: None,
            current_l2_a: None,
            current_l3_a: None,
            frequency_hz: None,
            power_factor: None,
            thd_voltage_pct: None,
            thd_current_pct: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::macros::datetime;

    fn pq(v_l1: Option<Decimal>, freq: Option<Decimal>) -> PowerQualityInterval {
        PowerQualityInterval {
            from: datetime!(2026-01-01 0:00 UTC),
            to: datetime!(2026-01-01 0:15 UTC),
            voltage_l1_v: v_l1,
            frequency_hz: freq,
            ..Default::default()
        }
    }

    #[test]
    fn nominal_voltage_in_range() {
        let iv = pq(Some(dec!(230)), None);
        assert!(!iv.voltage_out_of_range(dec!(230), dec!(10)));
    }

    #[test]
    fn overvoltage_detected() {
        let iv = pq(Some(dec!(254)), None); // 254V > 230 * 1.10 = 253V
        assert!(iv.voltage_out_of_range(dec!(230), dec!(10)));
    }

    #[test]
    fn nominal_frequency_in_range() {
        let iv = pq(None, Some(dec!(50.0)));
        assert!(!iv.frequency_out_of_range(dec!(0.5)));
    }

    #[test]
    fn frequency_deviation_detected() {
        let iv = pq(None, Some(dec!(49.0)));
        assert!(iv.frequency_out_of_range(dec!(0.5)));
    }

    #[test]
    fn no_measurement_returns_false() {
        let iv = pq(None, None);
        assert!(!iv.voltage_out_of_range(dec!(230), dec!(10)));
        assert!(!iv.frequency_out_of_range(dec!(0.5)));
    }
}
