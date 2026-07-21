//! Time-series resampling — down-sample high-resolution intervals to coarser buckets.
//!
//! ## Use cases
//!
//! | Use case | Target resolution |
//! |---|---|
//! | API summaries (client dashboards) | Hourly or daily |
//! | MMM billing (GPKE BK6-22-024 §3) | Monthly totals |
//! | Mehr-/Mindermengensaldo (§ 13 StromNZV) | Monthly |
//! | MABIS Summenzeitreihe | Monthly |
//! | SLP compatibility (daily totals) | Daily |
//!
//! ## Invariants
//!
//! - `resample()` only **aggregates** — it never interpolates.
//! - Partial buckets (missing intervals) are flagged via `has_missing_data`.
//! - Peak demand (`peak_kw`) per bucket = maximum `kWh / interval_h` across contributors.
//! - Bucket quality = worst [`QualityFlag`] among contributing intervals.
//!
//! ## DST handling
//!
//! Hourly and daily bucket boundaries are computed in **UTC**. For display in
//! German local time (CET/CEST), callers should convert `from`/`to` using
//! `time-tz::BERLIN`. This module is deliberately UTC-only to stay pure.
//!
//! ## Regulatory basis
//!
//! - **§ 2 MsbG**: RLM = 15-min interval metering.
//! - **GPKE BK6-22-024 §3**: MMM billing uses monthly arbeitsmenge totals.
//! - **§ 13 StromNZV**: Mehr-/Mindermengen use calendar-month totals.

use std::collections::BTreeMap;

use rust_decimal::Decimal;
use time::{Date, Duration, OffsetDateTime};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::interval::{MeterInterval, QualityFlag};
use crate::resolution::IntervalResolution;

// ── ResampledBucket ───────────────────────────────────────────────────────────

/// A resampled bucket: one or more source intervals aggregated into a coarser window.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ResampledBucket {
    /// Bucket start (UTC, inclusive).
    pub from: OffsetDateTime,
    /// Bucket end (UTC, exclusive).
    pub to: OffsetDateTime,
    /// Sum of all `value_kwh` from contributing intervals.
    pub total_kwh: Decimal,
    /// Peak demand in kW across contributing intervals.
    ///
    /// Computed as `max(interval.value_kwh / interval_duration_h)`.
    /// `None` only when no intervals contributed (should not normally occur).
    pub peak_kw: Option<Decimal>,
    /// Number of intervals that contributed to this bucket.
    pub interval_count: u32,
    /// Expected number of intervals for full coverage at the source resolution.
    ///
    /// When `interval_count < expected_count`, the bucket has missing data.
    pub expected_count: u32,
    /// Worst quality flag among all contributing intervals.
    pub quality: QualityFlag,
    /// `true` when some source intervals are missing (gap in the time series).
    pub has_missing_data: bool,
}

impl ResampledBucket {
    /// Coverage percentage (0.0–100.0).
    #[must_use]
    pub fn coverage_pct(&self) -> f64 {
        if self.expected_count == 0 {
            100.0
        } else {
            f64::from(self.interval_count) / f64::from(self.expected_count) * 100.0
        }
    }

    /// `true` when this bucket has complete, uninterrupted coverage.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        !self.has_missing_data && self.interval_count >= self.expected_count
    }
}

// ── ResampleConfig ────────────────────────────────────────────────────────────

/// Configuration for [`resample`].
#[derive(Debug, Clone)]
pub struct ResampleConfig {
    /// Target resolution to down-sample to.
    pub target_resolution: IntervalResolution,
    /// Duration of each source interval in seconds.
    ///
    /// Used to calculate `expected_count` per bucket. Default: `900` (15 min).
    pub source_interval_seconds: u32,
}

impl ResampleConfig {
    /// Standard: resample 15-min RLM data to hourly buckets.
    #[must_use]
    pub fn to_hourly() -> Self {
        Self {
            target_resolution: IntervalResolution::Hour,
            source_interval_seconds: 900,
        }
    }

    /// Standard: resample 15-min RLM data to daily buckets.
    #[must_use]
    pub fn to_daily() -> Self {
        Self {
            target_resolution: IntervalResolution::Day,
            source_interval_seconds: 900,
        }
    }

    /// Monthly totals — used for MMM billing and Mehr-/Mindermengensaldo (§ 13 StromNZV).
    #[must_use]
    pub fn to_monthly() -> Self {
        Self {
            target_resolution: IntervalResolution::Month,
            source_interval_seconds: 900,
        }
    }

    /// Annual totals — used for Jahresabrechnung.
    #[must_use]
    pub fn to_yearly() -> Self {
        Self {
            target_resolution: IntervalResolution::Year,
            source_interval_seconds: 900,
        }
    }
}

// ── resample ──────────────────────────────────────────────────────────────────

/// Down-sample a slice of meter intervals to the target resolution.
///
/// Input intervals do **not** need to be contiguous — gaps reduce `interval_count`
/// relative to `expected_count` and set `has_missing_data = true`.
///
/// Output is sorted ascending by `from`. Empty input returns an empty vec.
///
/// ## Panics
///
/// Does not panic.
#[must_use]
pub fn resample(intervals: &[MeterInterval], config: &ResampleConfig) -> Vec<ResampledBucket> {
    if intervals.is_empty() {
        return Vec::new();
    }

    let src_secs = config.source_interval_seconds;

    // BTreeMap: bucket_start_unix → ResampledBucket (sorted automatically)
    let mut buckets: BTreeMap<i64, ResampledBucket> = BTreeMap::new();

    for iv in intervals {
        let bucket_start = bucket_start_for(iv.from, &config.target_resolution);
        let bucket_end = bucket_end_for(bucket_start, &config.target_resolution);

        let entry = buckets
            .entry(bucket_start.unix_timestamp())
            .or_insert_with(|| {
                let expected = expected_count(bucket_start, bucket_end, src_secs);
                ResampledBucket {
                    from: bucket_start,
                    to: bucket_end,
                    total_kwh: Decimal::ZERO,
                    peak_kw: None,
                    interval_count: 0,
                    expected_count: expected,
                    quality: QualityFlag::Measured,
                    has_missing_data: false,
                }
            });

        entry.total_kwh += iv.value_kwh;
        entry.interval_count += 1;

        // Peak demand = energy / duration_h
        let duration_secs = (iv.to - iv.from).whole_seconds().max(1);
        let duration_h = Decimal::from(duration_secs) / Decimal::from(3_600u32);
        if duration_h > Decimal::ZERO {
            let kw = iv.value_kwh / duration_h;
            entry.peak_kw = Some(entry.peak_kw.map_or(kw, |prev| prev.max(kw)));
        }

        // Quality: keep worst
        if quality_rank(iv.quality) > quality_rank(entry.quality) {
            entry.quality = iv.quality;
        }
    }

    buckets
        .into_values()
        .map(|mut b| {
            if b.interval_count < b.expected_count {
                b.has_missing_data = true;
            }
            b
        })
        .collect()
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn quality_rank(q: QualityFlag) -> u8 {
    match q {
        QualityFlag::Faulty | QualityFlag::Unknown => 5,
        QualityFlag::Preliminary => 4,
        QualityFlag::Estimated => 3,
        QualityFlag::Corrected | QualityFlag::Substituted => 2,
        QualityFlag::Calculated => 1,
        QualityFlag::Measured => 0,
    }
}

fn bucket_start_for(ts: OffsetDateTime, res: &IntervalResolution) -> OffsetDateTime {
    match res {
        IntervalResolution::QuarterHour => {
            let excess_min = i64::from(ts.minute() % 15);
            (ts - Duration::minutes(excess_min))
                .replace_second(0)
                .unwrap_or(ts)
        }
        IntervalResolution::HalfHour => {
            let excess_min = i64::from(ts.minute() % 30);
            (ts - Duration::minutes(excess_min))
                .replace_second(0)
                .unwrap_or(ts)
        }
        IntervalResolution::Hour => ts
            .replace_minute(0)
            .and_then(|t| t.replace_second(0))
            .and_then(|t| t.replace_nanosecond(0))
            .unwrap_or(ts),
        IntervalResolution::Day => OffsetDateTime::new_utc(ts.date(), time::Time::MIDNIGHT),
        IntervalResolution::Month => {
            let d = Date::from_calendar_date(ts.year(), ts.month(), 1).unwrap_or(ts.date());
            OffsetDateTime::new_utc(d, time::Time::MIDNIGHT)
        }
        IntervalResolution::Year => {
            let d =
                Date::from_calendar_date(ts.year(), time::Month::January, 1).unwrap_or(ts.date());
            OffsetDateTime::new_utc(d, time::Time::MIDNIGHT)
        }
        IntervalResolution::Custom(secs) => {
            let unix = ts.unix_timestamp();
            let s = i64::from(*secs);
            if s == 0 {
                return ts;
            }
            let snapped = (unix / s) * s;
            OffsetDateTime::from_unix_timestamp(snapped).unwrap_or(ts)
        }
    }
}

fn bucket_end_for(bucket_start: OffsetDateTime, res: &IntervalResolution) -> OffsetDateTime {
    match res {
        IntervalResolution::QuarterHour => bucket_start + Duration::minutes(15),
        IntervalResolution::HalfHour => bucket_start + Duration::minutes(30),
        IntervalResolution::Hour => bucket_start + Duration::hours(1),
        IntervalResolution::Day => bucket_start + Duration::days(1),
        IntervalResolution::Month => {
            let (ny, nm) = if bucket_start.month() == time::Month::December {
                (bucket_start.year() + 1, time::Month::January)
            } else {
                (bucket_start.year(), bucket_start.month().next())
            };
            Date::from_calendar_date(ny, nm, 1)
                .map(|d| OffsetDateTime::new_utc(d, time::Time::MIDNIGHT))
                .unwrap_or_else(|_| bucket_start + Duration::days(31))
        }
        IntervalResolution::Year => {
            Date::from_calendar_date(bucket_start.year() + 1, time::Month::January, 1)
                .map(|d| OffsetDateTime::new_utc(d, time::Time::MIDNIGHT))
                .unwrap_or_else(|_| bucket_start + Duration::days(365))
        }
        IntervalResolution::Custom(secs) => bucket_start + Duration::seconds(i64::from(*secs)),
    }
}

fn expected_count(start: OffsetDateTime, end: OffsetDateTime, source_secs: u32) -> u32 {
    let duration = (end - start).whole_seconds().max(0) as u64;
    let src = u64::from(source_secs);
    if src == 0 {
        return 0;
    }
    (duration / src) as u32
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn make_iv(from: OffsetDateTime, value_kwh: Decimal) -> MeterInterval {
        MeterInterval {
            from,
            to: from + Duration::minutes(15),
            value_kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    #[test]
    fn four_quarters_sum_to_one_hour() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let ivs: Vec<_> = (0..4)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(2.5)))
            .collect();
        let result = resample(&ivs, &ResampleConfig::to_hourly());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total_kwh, dec!(10.0));
        assert_eq!(result[0].interval_count, 4);
        assert_eq!(result[0].expected_count, 4);
        assert!(result[0].is_complete());
    }

    #[test]
    fn gap_in_bucket_sets_has_missing_data() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let ivs: Vec<_> =
            (0..3) // only 3 of 4 expected
                .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.0)))
                .collect();
        let result = resample(&ivs, &ResampleConfig::to_hourly());
        assert_eq!(result.len(), 1);
        assert!(result[0].has_missing_data);
        assert!(!result[0].is_complete());
    }

    #[test]
    fn daily_aggregation_96_intervals() {
        let base = datetime!(2026-03-15 00:00 UTC);
        let ivs: Vec<_> = (0..96)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.0)))
            .collect();
        let result = resample(&ivs, &ResampleConfig::to_daily());
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].total_kwh, dec!(96.0));
        assert!(result[0].is_complete());
    }

    #[test]
    fn two_months_produce_two_buckets() {
        // Start at 23:30 UTC on Jan 31 — first 2 intervals in Jan, next 2 in Feb
        let base = datetime!(2026-01-31 23:30 UTC);
        let ivs: Vec<_> = (0..4)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.0)))
            .collect();
        let result = resample(&ivs, &ResampleConfig::to_monthly());
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn peak_kw_is_maximum_across_intervals() {
        let base = datetime!(2026-06-01 10:00 UTC);
        let ivs = vec![
            make_iv(base, dec!(5.0)),                         // 5 kWh / 0.25 h = 20 kW
            make_iv(base + Duration::minutes(15), dec!(2.5)), // 2.5 / 0.25 = 10 kW
        ];
        let result = resample(&ivs, &ResampleConfig::to_hourly());
        assert_eq!(result[0].peak_kw, Some(dec!(20.0)));
    }

    #[test]
    fn worst_quality_propagates() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let mut ivs: Vec<_> = (0..4)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.0)))
            .collect();
        ivs[2].quality = QualityFlag::Estimated;
        let result = resample(&ivs, &ResampleConfig::to_hourly());
        assert_eq!(result[0].quality, QualityFlag::Estimated);
    }

    #[test]
    fn empty_input_returns_empty() {
        assert!(resample(&[], &ResampleConfig::to_hourly()).is_empty());
    }

    #[test]
    fn coverage_pct_partial_bucket() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let ivs = vec![make_iv(base, dec!(1.0))]; // 1 of 4
        let result = resample(&ivs, &ResampleConfig::to_hourly());
        assert!((result[0].coverage_pct() - 25.0).abs() < 0.01);
    }
}
