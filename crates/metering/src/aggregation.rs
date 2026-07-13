//! Billing period aggregation: `arbeitsmenge_kwh`, `spitzenleistung_kw`, HT/NT split.
//!
//! ## Legal basis
//!
//! - **§2 Nr. 17 MessZV**: Spitzenleistung = höchste Viertelstundenleistung im Abrechnungszeitraum.
//! - **§3 MessZV**: RLM = registrierende Lastgangmessung (15-min intervals).
//! - **§4 MessZV**: SLP = Standardlastprofil (daily or monthly totals).
//! - **GPKE BK6-22-024 §3**: MMM billing requires arbeitsmenge_kwh + spitzenleistung_kw.
//!
//! ## Spitzenleistung (peak demand)
//!
//! For RLM (15-min metering), peak demand in kW is:
//! ```text
//! spitzenleistung_kw = max(interval_kwh × 4)   for all 15-min intervals
//! ```
//! Generalised: `demand_kw = kwh / duration_h`
//!
//! For SLP, `spitzenleistung_kw` is `None` — SLP billing uses arbeitsmenge only.
//!
//! ## HT/NT (high/low tariff)
//!
//! A simplified model based on standard German Zweitarif definitions.
//! Full implementation requires the applicable Zaehlzeitdefinition per §14a EnWG.

use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::interval::MeterInterval;

/// Configuration for billing period aggregation.
#[derive(Debug, Clone)]
pub struct AggregationConfig {
    /// Include Spitzenleistung (peak demand) calculation.
    /// Only meaningful for RLM (15-min) intervals.
    pub include_spitzenleistung: bool,
    /// Include HT/NT split.
    /// Requires OBIS codes 1-0:1.8.1 (HT) and 1-0:1.8.2 (NT) or a `HtNtRule`.
    pub include_ht_nt: bool,
    /// HT hours: hours of the day (local, ignoring DST) classified as Hochtarif.
    /// Default: 06–22 weekdays (simple approximation; use Zaehlzeitdefinition for precision).
    pub ht_hours: HtHours,
}

impl AggregationConfig {
    /// RLM Strom configuration: Spitzenleistung enabled, HT/NT disabled.
    #[must_use]
    pub fn rlm_strom() -> Self {
        Self {
            include_spitzenleistung: true,
            include_ht_nt: false,
            ht_hours: HtHours::default(),
        }
    }

    /// SLP Strom configuration: no Spitzenleistung, no HT/NT.
    #[must_use]
    pub fn slp_strom() -> Self {
        Self {
            include_spitzenleistung: false,
            include_ht_nt: false,
            ht_hours: HtHours::default(),
        }
    }

    /// RLM Zweitarif (HT/NT) configuration.
    #[must_use]
    pub fn rlm_zweitarif() -> Self {
        Self {
            include_spitzenleistung: true,
            include_ht_nt: true,
            ht_hours: HtHours::default(),
        }
    }

    /// Gas configuration: no Spitzenleistung, no HT/NT.
    #[must_use]
    pub fn gas() -> Self {
        Self {
            include_spitzenleistung: false,
            include_ht_nt: false,
            ht_hours: HtHours::default(),
        }
    }
}

/// Hours classified as Hochtarif (HT) in a simplified Zweitarif model.
/// Default: 06:00–22:00 (UTC+1, approximated as UTC for simplicity).
#[derive(Debug, Clone)]
pub struct HtHours {
    /// Start hour (inclusive, 0–23 UTC).
    pub start: u8,
    /// End hour (exclusive, 0–23 UTC).
    pub end: u8,
    /// Include weekends as HT? Default: false (weekends are NT).
    pub include_weekends: bool,
}

impl Default for HtHours {
    fn default() -> Self {
        Self {
            start: 6,
            end: 22,
            include_weekends: false,
        }
    }
}

impl HtHours {
    /// `true` when `ts` falls in the HT window.
    #[must_use]
    pub fn is_ht(&self, ts: OffsetDateTime) -> bool {
        use time::Weekday;
        let weekday = ts.weekday();
        if !self.include_weekends && matches!(weekday, Weekday::Saturday | Weekday::Sunday) {
            return false;
        }
        let h = ts.hour();
        h >= self.start && h < self.end
    }
}

/// HT/NT energy split.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HtNtSplit {
    /// High-tariff energy in kWh.
    pub ht_kwh: Decimal,
    /// Low-tariff energy in kWh.
    pub nt_kwh: Decimal,
}

/// Result of a billing period aggregation.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BillingPeriod {
    /// Total energy in kWh (Arbeitsmenge, sum of all intervals).
    pub arbeitsmenge_kwh: Decimal,

    /// Peak demand in kW (§2 Nr. 17 MessZV Spitzenleistung).
    ///
    /// `None` for SLP or when `config.include_spitzenleistung = false`.
    /// For RLM: `max(interval_kwh / duration_h)` across all intervals.
    pub spitzenleistung_kw: Option<Decimal>,

    /// HT/NT split (only when `config.include_ht_nt = true`).
    pub ht_nt: Option<HtNtSplit>,

    /// Number of intervals used.
    pub interval_count: usize,

    /// Coverage: `interval_count / expected_count × 100 %`.
    /// Expected is derived from the period length ÷ median interval length.
    pub coverage_pct: f64,
}

/// Aggregate meter intervals into a [`BillingPeriod`].
///
/// Intervals are sorted by `from` internally.
/// Only intervals where `quality.is_billable()` contribute to the result.
///
/// # Example
/// ```rust
/// use metering::{MeterInterval, QualityFlag, aggregate, AggregationConfig};
/// use rust_decimal::Decimal;
/// use time::macros::datetime;
///
/// let iv = MeterInterval {
///     from: datetime!(2026-06-01 0:00 UTC),
///     to:   datetime!(2026-06-01 0:15 UTC),
///     value_kwh: Decimal::from_str_exact("2.5").unwrap(),
///     quality: QualityFlag::Measured,
///     obis_code: None,
/// };
/// let period = aggregate(&[iv], AggregationConfig::rlm_strom());
/// assert_eq!(period.arbeitsmenge_kwh, Decimal::from_str_exact("2.5").unwrap());
/// assert_eq!(period.spitzenleistung_kw, Some(Decimal::from(10u32))); // 2.5 × 4 = 10 kW
/// ```
#[must_use]
pub fn aggregate(intervals: &[MeterInterval], config: AggregationConfig) -> BillingPeriod {
    if intervals.is_empty() {
        return BillingPeriod {
            arbeitsmenge_kwh: Decimal::ZERO,
            spitzenleistung_kw: None,
            ht_nt: None,
            interval_count: 0,
            coverage_pct: 0.0,
        };
    }

    let mut sorted = intervals.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    // Only billable intervals contribute to the sum
    let billable: Vec<&MeterInterval> = sorted
        .iter()
        .filter(|iv| iv.quality.is_billable())
        .collect();

    let arbeitsmenge_kwh: Decimal = billable.iter().map(|iv| iv.value_kwh).sum();

    // Spitzenleistung: max instantaneous demand over all billable intervals
    let spitzenleistung_kw = if config.include_spitzenleistung && !billable.is_empty() {
        billable
            .iter()
            .filter_map(|iv| iv.demand_kw())
            .reduce(Decimal::max)
    } else {
        None
    };

    // HT/NT split
    let ht_nt = if config.include_ht_nt && !billable.is_empty() {
        let mut ht = Decimal::ZERO;
        let mut nt = Decimal::ZERO;
        for iv in &billable {
            if config.ht_hours.is_ht(iv.from) {
                ht += iv.value_kwh;
            } else {
                nt += iv.value_kwh;
            }
        }
        Some(HtNtSplit {
            ht_kwh: ht,
            nt_kwh: nt,
        })
    } else {
        None
    };

    // Coverage
    let durations: Vec<i64> = sorted
        .iter()
        .map(|iv| iv.duration_secs())
        .filter(|&d| d > 0)
        .collect();
    let median_dur = if durations.is_empty() {
        900i64
    } else {
        let mut ds = durations.clone();
        ds.sort_unstable();
        ds[ds.len() / 2]
    };
    let period_secs = (sorted.last().unwrap().to - sorted.first().unwrap().from)
        .whole_seconds()
        .max(1);
    let expected_f64 = if median_dur > 0 {
        period_secs as f64 / median_dur as f64
    } else {
        0.0
    };
    let coverage_pct = if expected_f64 <= 0.0 {
        100.0_f64
    } else {
        ((sorted.len() as f64 / expected_f64) * 100.0).min(100.0)
    };

    BillingPeriod {
        arbeitsmenge_kwh,
        spitzenleistung_kw,
        ht_nt,
        interval_count: sorted.len(),
        coverage_pct,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::QualityFlag;
    use rust_decimal_macros::dec;
    use time::macros::datetime;

    fn iv(from: OffsetDateTime, kwh: Decimal) -> MeterInterval {
        MeterInterval {
            from,
            to: from + time::Duration::minutes(15),
            value_kwh: kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    #[test]
    fn spitzenleistung_max_15min() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let intervals = vec![
            iv(base, dec!(2.5)),                               // 10 kW
            iv(base + time::Duration::minutes(15), dec!(5.0)), // 20 kW — peak
            iv(base + time::Duration::minutes(30), dec!(1.0)), // 4 kW
        ];
        let period = aggregate(&intervals, AggregationConfig::rlm_strom());
        assert_eq!(period.spitzenleistung_kw, Some(dec!(20)));
    }

    #[test]
    fn arbeitsmenge_sum_only_billable() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let mut intervals = vec![
            iv(base, dec!(2.0)),
            iv(base + time::Duration::minutes(15), dec!(3.0)),
        ];
        // Mark second interval as estimated — should NOT contribute
        intervals[1].quality = QualityFlag::Estimated;

        let period = aggregate(&intervals, AggregationConfig::rlm_strom());
        // Only first interval (2.0 kWh) contributes
        assert_eq!(period.arbeitsmenge_kwh, dec!(2.0));
    }

    #[test]
    fn slp_no_spitzenleistung() {
        let base = datetime!(2026-01-01 0:00 UTC);
        let intervals = vec![iv(base, dec!(24.0))]; // daily SLP read
        let period = aggregate(&intervals, AggregationConfig::slp_strom());
        assert_eq!(period.spitzenleistung_kw, None);
        assert_eq!(period.arbeitsmenge_kwh, dec!(24.0));
    }

    #[test]
    fn ht_nt_split() {
        // Weekday 07:00 = HT, 23:00 = NT
        let ht_time = datetime!(2026-01-05 7:00 UTC); // Monday
        let nt_time = datetime!(2026-01-05 23:00 UTC); // Monday night
        let intervals = vec![
            iv(ht_time, dec!(4.0)), // HT
            iv(nt_time, dec!(1.0)), // NT
        ];
        let period = aggregate(&intervals, AggregationConfig::rlm_zweitarif());
        let ht_nt = period.ht_nt.unwrap();
        assert_eq!(ht_nt.ht_kwh, dec!(4.0));
        assert_eq!(ht_nt.nt_kwh, dec!(1.0));
    }

    #[test]
    fn empty_intervals() {
        let period = aggregate(&[], AggregationConfig::rlm_strom());
        assert_eq!(period.arbeitsmenge_kwh, Decimal::ZERO);
        assert_eq!(period.spitzenleistung_kw, None);
        assert_eq!(period.interval_count, 0);
    }

    #[test]
    fn spitzenleistung_mess_zv_definition() {
        // §2 Nr. 17 MessZV: Spitzenleistung = höchste Viertelstundenleistung
        // 3.5 kWh in 15 min = 14 kW
        // 1.0 kWh in 15 min = 4 kW
        // Peak = 14 kW
        let base = datetime!(2026-06-01 10:00 UTC);
        let intervals = vec![
            iv(base, dec!(3.5)),
            iv(base + time::Duration::minutes(15), dec!(1.0)),
        ];
        let period = aggregate(&intervals, AggregationConfig::rlm_strom());
        assert_eq!(period.spitzenleistung_kw, Some(dec!(14)));
    }
}
