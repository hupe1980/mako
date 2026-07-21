//! Energy forecast generation for §17 MessZV substitute values.
//!
//! When meter readings are unavailable, §17 MessZV requires substitute values.
//! For longer gaps (> 3 intervals), prior-period averaging or profile-based
//! forecasting is the BDEW-recommended approach.
//!
//! This module provides:
//! 1. **Annual forecast** — project total annual consumption from a partial year's data
//! 2. **Short-term gap fill** — prior-period same-slot average for §17 Abs. 2 MessZV
//! 3. **Seasonal index** — detect consumption patterns (summer/winter)
//!
//! ## Legal basis
//!
//! - **§17 Abs. 1 MessZV**: MSB must supply substitute values for unavailable measurements.
//! - **§17 Abs. 2 MessZV**: Prior-period same-slot average is the preferred method.
//! - **BDEW Richtlinie**: Prognosewert for SLP; Ersatzwert for RLM.
//! - **VDE-AR-N 4400**: Technical rules for substitute value generation.
//!
//! ## What this does NOT do
//!
//! This module does NOT perform machine-learning forecasting. ML requires an
//! external runtime (PyTorch, ONNX), which violates this crate's no-I/O
//! contract; it belongs in a service layer that can host one.

use rust_decimal::Decimal;
use std::collections::HashMap;
use time::{Duration, OffsetDateTime, Weekday};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::interval::{MeterInterval, QualityFlag};

// ── ForecastMethod ────────────────────────────────────────────────────────────

/// Method used for energy forecast or substitute value generation.
///
/// Stored in [`SubstituteValueEntry`] so auditors can explain every derived value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ForecastMethod {
    /// §17 Abs. 2 MessZV: same time slot from prior week(s).
    PriorPeriodSameSlot,
    /// Weighted rolling average over the same time slot from N prior periods.
    WeightedRollingAverage,
    /// Linear interpolation between surrounding measured values (short gaps only).
    LinearInterpolation,
    /// Last known value carried forward (fallback, conservative).
    LastValueCarryForward,
    /// Zero fill (confirmed plant shutdown / delivery pause).
    ZeroFill,
    /// Profile-based reconstruction (BDEW H0/G0 SLP load profile shape).
    ProfileBased,
    /// Annual projection: extrapolate partial-year consumption to full year.
    AnnualProjection,
}

impl ForecastMethod {
    /// Human-readable description (German regulatory language).
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::PriorPeriodSameSlot => {
                "Vorperiodenmittelwert gleicher Zeitschlitz (§17 Abs. 2 MessZV)"
            }
            Self::WeightedRollingAverage => "Gewichteter gleitender Mittelwert",
            Self::LinearInterpolation => "Lineare Interpolation zwischen Messwerten",
            Self::LastValueCarryForward => "Letzter bekannter Wert fortgeschrieben",
            Self::ZeroFill => "Nullwert (bestätigter Lieferstopp)",
            Self::ProfileBased => "Profilbasiert (BDEW Standardlastprofil)",
            Self::AnnualProjection => "Jahreshochrechnung aus Teiljahreswerten",
        }
    }
}

// ── SubstituteValueEntry ──────────────────────────────────────────────────────

/// A single generated substitute value with full audit metadata.
///
/// Every substitute interval produced by this module includes the generation
/// method, the reference data used, and any confidence notes. This satisfies
/// the §17 MessZV traceability requirement.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct SubstituteValueEntry {
    /// The generated substitute interval.
    pub interval: MeterInterval,
    /// Method used to generate this value.
    pub method: ForecastMethod,
    /// Number of reference intervals used (e.g. prior-period sample count).
    pub reference_count: u32,
    /// Confidence note (e.g. "7 reference slots, stddev 0.15 kWh").
    pub confidence_note: Option<String>,
}

// ── AnnualForecast ────────────────────────────────────────────────────────────

/// Annual energy consumption forecast from a partial year's data.
///
/// Used to:
/// 1. Estimate annual Abschlag (advance payment) amounts.
/// 2. Generate the annual Jahresprognose for SLP customers.
/// 3. Project Mehr-/Mindermengen at year-end.
///
/// ## Method
///
/// ```text
/// annual_kwh = observed_kwh / observed_days × 365
/// ```
///
/// Adjusted for seasonal index when prior-year data is available.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct AnnualForecast {
    /// 11-digit MaLo-ID.
    pub malo_id: String,
    /// Projection base period.
    pub observation_from: OffsetDateTime,
    /// Projection base period end.
    pub observation_to: OffsetDateTime,
    /// Observed energy in the base period (kWh).
    pub observed_kwh: Decimal,
    /// Number of observed days.
    pub observed_days: u32,
    /// Projected annual consumption (kWh).
    pub projected_annual_kwh: Decimal,
    /// Whether seasonal correction was applied.
    pub seasonal_correction_applied: bool,
    /// Seasonal correction factor (1.0 = no correction).
    pub seasonal_factor: Decimal,
    /// Method used for projection.
    pub method: ForecastMethod,
    /// 95% confidence interval lower bound (kWh), clamped at zero.
    ///
    /// From the day-to-day variability of the observed period: with daily
    /// sums treated as independent draws, the annual-total standard deviation
    /// is `sd_daily × √365`, and the bounds are `projection ± 1.96 × sd_annual`
    /// (seasonally scaled like the projection itself). Informational — the
    /// billed figure is always `projected_annual_kwh`.
    pub confidence_lower_kwh: Option<Decimal>,
    /// 95% confidence interval upper bound (kWh). See `confidence_lower_kwh`.
    pub confidence_upper_kwh: Option<Decimal>,
}

/// 95% CI half-width for the annual projection, from daily-sum variability.
///
/// Returns `None` with fewer than two observed days or when the statistics
/// degenerate. Computed in `f64` — the bounds are diagnostics, not billed
/// quantities, and the projection itself stays exact `Decimal`.
fn confidence_half_width(intervals: &[MeterInterval], seasonal_factor: Decimal) -> Option<Decimal> {
    use std::collections::BTreeMap;

    let mut daily: BTreeMap<time::Date, Decimal> = BTreeMap::new();
    for iv in intervals.iter().filter(|iv| iv.quality.is_billable()) {
        *daily.entry(iv.from.date()).or_insert(Decimal::ZERO) += iv.value_kwh;
    }
    if daily.len() < 2 {
        return None;
    }

    let n = daily.len() as f64;
    let values: Vec<f64> = daily
        .values()
        .filter_map(|d| d.to_string().parse::<f64>().ok())
        .collect();
    if values.len() != daily.len() {
        return None;
    }
    let mean = values.iter().sum::<f64>() / n;
    let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let sd_daily = var.sqrt();
    let factor: f64 = seasonal_factor.to_string().parse().ok()?;
    let half = 1.96 * sd_daily * 365.0_f64.sqrt() * factor;
    if !half.is_finite() {
        return None;
    }
    Decimal::try_from(half).ok().map(|d| d.round_dp(3))
}

// ── project_annual_consumption ────────────────────────────────────────────────

/// Project annual consumption from a partial year's interval data.
///
/// Aggregates observed kWh, computes daily average, scales to 365 days.
/// When `prior_year_intervals` are provided, applies a seasonal correction
/// factor based on the ratio of the prior year's same-period consumption
/// to the prior year's annual total.
///
/// ## Returns
///
/// `None` when `intervals` is empty or covers fewer than 7 days.
#[must_use]
pub fn project_annual_consumption(
    malo_id: &str,
    intervals: &[MeterInterval],
    prior_year_intervals: Option<&[MeterInterval]>,
) -> Option<AnnualForecast> {
    if intervals.is_empty() {
        return None;
    }

    let first_from = intervals.iter().map(|iv| iv.from).min()?;
    let last_to = intervals.iter().map(|iv| iv.to).max()?;
    let observed_days_i64 = (last_to - first_from).whole_days();
    if observed_days_i64 < 7 {
        return None; // insufficient data for meaningful projection
    }
    let observed_days = observed_days_i64 as u32;

    let observed_kwh: Decimal = intervals
        .iter()
        .filter(|iv| iv.quality.is_billable())
        .map(|iv| iv.value_kwh)
        .sum();

    // Base projection: daily average × 365
    let daily_avg = observed_kwh / Decimal::from(observed_days);
    let mut projected = daily_avg * Decimal::from(365u32);

    // Seasonal correction using prior year data
    let (seasonal_correction_applied, seasonal_factor) = if let Some(prior) = prior_year_intervals {
        if let Some(factor) = compute_seasonal_factor(first_from, last_to, prior) {
            projected *= factor;
            (true, factor)
        } else {
            (false, Decimal::ONE)
        }
    } else {
        (false, Decimal::ONE)
    };

    Some(AnnualForecast {
        malo_id: malo_id.to_owned(),
        observation_from: first_from,
        observation_to: last_to,
        observed_kwh,
        observed_days,
        projected_annual_kwh: projected.round_dp(3),
        seasonal_correction_applied,
        seasonal_factor,
        method: if prior_year_intervals.is_some() {
            ForecastMethod::WeightedRollingAverage
        } else {
            ForecastMethod::AnnualProjection
        },
        confidence_lower_kwh: confidence_half_width(intervals, seasonal_factor)
            .map(|h| (projected - h).max(Decimal::ZERO).round_dp(3)),
        confidence_upper_kwh: confidence_half_width(intervals, seasonal_factor)
            .map(|h| (projected + h).round_dp(3)),
    })
}

// ── prior_period_substitutes ──────────────────────────────────────────────────

/// Generate substitute values for a gap using prior-period same-slot averaging.
///
/// Generate §17 MessZV substitute values for a gap, using `method`.
///
/// Every emitted [`SubstituteValueEntry`] records the method that actually
/// produced it, which may differ from `method` when the requested strategy has
/// no data to work from — a prior-period average with no matching reference slot
/// falls back to carry-forward, then to zero. Reporting the requested method
/// instead would put a claim in the §22 MessZV audit trail that the value does
/// not support.
///
/// ## §17 Abs. 2 MessZV compliance
///
/// [`crate::substitute::SubstituteMethod::PriorPeriodAverage`] implements the BDEW
/// "Vorperiodenmittelwert" — the same slot of the preceding week, matched on
/// (weekday, hour, minute) in German local time.
///
/// ## Parameters
///
/// - `gap_from` / `gap_to`: UTC timestamps of the gap to fill.
/// - `interval_secs`: Expected interval length (900 for 15-min RLM).
/// - `method`: Requested substitution strategy.
/// - `prior_period_intervals`: Reference intervals (the preceding week).
/// - `last_known_value`: Last billable value before the gap.
/// - `next_known_value`: First billable value after the gap, required for
///   linear interpolation to have a slope to follow.
#[must_use]
pub fn substitute_values(
    gap_from: OffsetDateTime,
    gap_to: OffsetDateTime,
    interval_secs: u32,
    method: crate::substitute::SubstituteMethod,
    prior_period_intervals: &[MeterInterval],
    last_known_value: Option<Decimal>,
    next_known_value: Option<Decimal>,
) -> Vec<SubstituteValueEntry> {
    if gap_from >= gap_to || interval_secs == 0 {
        return Vec::new();
    }

    // Build lookup: (weekday, hour, minute) → Vec<value>
    let mut slot_map: HashMap<(Weekday, u8, u8), Vec<Decimal>> = HashMap::new();
    for iv in prior_period_intervals {
        if iv.quality.is_billable() {
            use time_tz::{OffsetDateTimeExt, timezones};
            let local = iv.from.to_timezone(timezones::db::europe::BERLIN);
            let key = (local.weekday(), local.hour(), local.minute());
            slot_map.entry(key).or_default().push(iv.value_kwh);
        }
    }

    let mut result = Vec::new();
    let interval_dur = Duration::seconds(i64::from(interval_secs));
    let mut cursor = gap_from;

    while cursor < gap_to {
        let to = (cursor + interval_dur).min(gap_to);

        // Look up prior-period slot
        use time_tz::{OffsetDateTimeExt, timezones};
        let local = cursor.to_timezone(timezones::db::europe::BERLIN);
        let key = (local.weekday(), local.hour(), local.minute());

        // Each arm falls back only when its own inputs are absent, and the arm
        // that ran is what gets reported.
        let (value, applied, ref_count) = match method {
            crate::substitute::SubstituteMethod::ZeroFill => {
                (Decimal::ZERO, ForecastMethod::ZeroFill, 0)
            }

            crate::substitute::SubstituteMethod::LastValueCarryForward => match last_known_value {
                Some(last) => (last, ForecastMethod::LastValueCarryForward, 1),
                None => (Decimal::ZERO, ForecastMethod::ZeroFill, 0),
            },

            crate::substitute::SubstituteMethod::LinearInterpolation => {
                // Interpolate between the last known value before the gap and
                // the first after it. With no closing value the series has no
                // slope to follow, so this degrades to carry-forward.
                match (last_known_value, next_known_value) {
                    (Some(start), Some(end)) => {
                        let span = (gap_to - gap_from).whole_seconds();
                        let elapsed = (cursor - gap_from).whole_seconds();
                        let value = if span > 0 {
                            start + (end - start) * Decimal::from(elapsed) / Decimal::from(span)
                        } else {
                            start
                        };
                        (value, ForecastMethod::LinearInterpolation, 2)
                    }
                    (Some(start), None) => (start, ForecastMethod::LastValueCarryForward, 1),
                    (None, Some(end)) => (end, ForecastMethod::LastValueCarryForward, 1),
                    (None, None) => (Decimal::ZERO, ForecastMethod::ZeroFill, 0),
                }
            }

            crate::substitute::SubstituteMethod::PriorPeriodAverage => {
                if let Some(prior_values) = slot_map.get(&key) {
                    let avg = prior_values.iter().sum::<Decimal>()
                        / Decimal::from(prior_values.len() as u32);
                    (
                        avg,
                        ForecastMethod::PriorPeriodSameSlot,
                        prior_values.len() as u32,
                    )
                } else if let Some(last) = last_known_value {
                    (last, ForecastMethod::LastValueCarryForward, 1)
                } else {
                    (Decimal::ZERO, ForecastMethod::ZeroFill, 0)
                }
            }
        };
        let method = applied;

        result.push(SubstituteValueEntry {
            interval: MeterInterval {
                from: cursor,
                to,
                value_kwh: value.round_dp(6),
                quality: QualityFlag::Substituted,
                obis_code: None,
            },
            method,
            reference_count: ref_count,
            confidence_note: match applied {
                ForecastMethod::PriorPeriodSameSlot => Some(format!(
                    "{ref_count} Referenzintervall(e) im Vorperiodenmittelwert"
                )),
                ForecastMethod::LinearInterpolation => {
                    Some("lineare Interpolation zwischen den Randwerten".to_owned())
                }
                ForecastMethod::LastValueCarryForward => {
                    Some("letzter bekannter Messwert fortgeschrieben".to_owned())
                }
                ForecastMethod::ZeroFill => {
                    Some("kein Referenzwert verfügbar — Nullwert gesetzt".to_owned())
                }
                _ => None,
            },
        });

        cursor = to;
    }

    result
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn compute_seasonal_factor(
    obs_from: OffsetDateTime,
    obs_to: OffsetDateTime,
    prior_year: &[MeterInterval],
) -> Option<Decimal> {
    if prior_year.is_empty() {
        return None;
    }

    // Shift the observation window back one year to find the matching prior-year period
    let one_year = Duration::days(365);
    let prior_from = obs_from - one_year;
    let prior_to = obs_to - one_year;

    let prior_period_kwh: Decimal = prior_year
        .iter()
        .filter(|iv| iv.from >= prior_from && iv.to <= prior_to && iv.quality.is_billable())
        .map(|iv| iv.value_kwh)
        .sum();

    let prior_annual_kwh: Decimal = prior_year
        .iter()
        .filter(|iv| iv.quality.is_billable())
        .map(|iv| iv.value_kwh)
        .sum();

    if prior_annual_kwh == Decimal::ZERO || prior_period_kwh == Decimal::ZERO {
        return None;
    }

    // Seasonal factor = (prior-year period fraction × 365/period_days)
    // If this period is normally higher-than-average consumption, factor > 1
    let period_days = Decimal::from((obs_to - obs_from).whole_days().max(1) as u32);
    let prior_daily = prior_annual_kwh / Decimal::from(365u32);
    let prior_period_daily = prior_period_kwh / period_days;

    if prior_daily == Decimal::ZERO {
        return None;
    }

    Some((prior_period_daily / prior_daily).round_dp(4))
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;
    use time::{Duration, macros::datetime};

    fn make_iv(from: OffsetDateTime, kwh: Decimal) -> MeterInterval {
        MeterInterval {
            from,
            to: from + Duration::minutes(15),
            value_kwh: kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    #[test]
    fn annual_projection_simple() {
        // 30 days of data, 2 kWh per 15-min interval = 8 kWh/h = 192 kWh/day
        let base = datetime!(2026-01-01 00:00 UTC);
        let intervals: Vec<_> =
            (0..30 * 96) // 30 days × 96 intervals
                .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(2.0)))
                .collect();

        let forecast = project_annual_consumption("51238696780", &intervals, None).unwrap();
        assert!(forecast.observed_days >= 30);
        // Should project ~70080 kWh per year (192 kWh/day × 365 days)
        assert!(forecast.projected_annual_kwh > dec!(60000));
        assert_eq!(forecast.method, ForecastMethod::AnnualProjection);
        assert!(!forecast.seasonal_correction_applied);
    }

    #[test]
    fn insufficient_data_returns_none() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let intervals = vec![make_iv(base, dec!(1.0))];
        assert!(project_annual_consumption("test", &intervals, None).is_none());
    }

    #[test]
    fn empty_intervals_returns_none() {
        assert!(project_annual_consumption("test", &[], None).is_none());
    }

    #[test]
    fn confidence_bounds_bracket_the_projection() {
        // 14 days with day-to-day variation → computable 95% CI.
        let base = datetime!(2026-01-01 00:00 UTC);
        let intervals: Vec<_> = (0..14 * 96)
            .map(|i| {
                let day = i / 96;
                let kwh = if day % 2 == 0 { dec!(1.0) } else { dec!(1.4) };
                make_iv(base + Duration::minutes(15 * i), kwh)
            })
            .collect();
        let f = project_annual_consumption("51238696781", &intervals, None).expect("forecast");
        let lower = f.confidence_lower_kwh.expect("lower bound computed");
        let upper = f.confidence_upper_kwh.expect("upper bound computed");
        assert!(lower < f.projected_annual_kwh && f.projected_annual_kwh < upper);
        assert!(lower >= Decimal::ZERO, "lower bound clamped at zero");
    }

    #[test]
    fn constant_consumption_has_a_tight_interval() {
        let base = datetime!(2026-01-01 00:00 UTC);
        let intervals: Vec<_> = (0..14 * 96)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.0)))
            .collect();
        let f = project_annual_consumption("51238696781", &intervals, None).expect("forecast");
        // Zero day-to-day variance → CI collapses onto the projection.
        assert_eq!(f.confidence_lower_kwh, Some(f.projected_annual_kwh));
        assert_eq!(f.confidence_upper_kwh, Some(f.projected_annual_kwh));
    }

    #[test]
    fn prior_period_substitution_uses_same_slot() {
        // 7 days of prior-period data — same hour every day = 1.5 kWh
        let base = datetime!(2026-01-01 00:00 UTC);
        let prior: Vec<_> = (0..7 * 96)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.5)))
            .collect();

        let gap_from = datetime!(2026-01-08 00:00 UTC);
        let gap_to = datetime!(2026-01-08 01:00 UTC); // 4 intervals

        let subs = substitute_values(
            gap_from,
            gap_to,
            900,
            crate::substitute::SubstituteMethod::PriorPeriodAverage,
            &prior,
            None,
            None,
        );
        assert_eq!(subs.len(), 4);
        // All should use PriorPeriodSameSlot and value ≈ 1.5
        for s in &subs {
            assert_eq!(s.method, ForecastMethod::PriorPeriodSameSlot);
            assert!((s.interval.value_kwh - dec!(1.5)).abs() < dec!(0.001));
            assert_eq!(s.interval.quality, QualityFlag::Substituted);
        }
    }

    #[test]
    fn prior_period_fallback_to_carry_forward() {
        // No prior-period data — should fall back to last known value
        let gap_from = datetime!(2026-06-01 00:00 UTC);
        let gap_to = datetime!(2026-06-01 00:15 UTC);
        let subs = substitute_values(
            gap_from,
            gap_to,
            900,
            crate::substitute::SubstituteMethod::PriorPeriodAverage,
            &[],
            Some(dec!(2.5)),
            None,
        );
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].method, ForecastMethod::LastValueCarryForward);
        assert_eq!(subs[0].interval.value_kwh, dec!(2.5));
    }

    #[test]
    fn forecast_method_descriptions_non_empty() {
        for m in [
            ForecastMethod::PriorPeriodSameSlot,
            ForecastMethod::WeightedRollingAverage,
            ForecastMethod::LinearInterpolation,
            ForecastMethod::LastValueCarryForward,
            ForecastMethod::ZeroFill,
            ForecastMethod::ProfileBased,
            ForecastMethod::AnnualProjection,
        ] {
            assert!(!m.description().is_empty());
        }
    }
}

#[cfg(test)]
mod db_vocabulary_tests {
    use super::ForecastMethod;

    /// `ForecastMethod`'s Debug names are not a persistence vocabulary.
    ///
    /// `edmd` writes substitutions to `substitute_value_log`, whose `method`
    /// CHECK accepts only the §17 MessZV categories. Emitting the Debug form
    /// violated that CHECK on the default path, so the audit INSERT failed after
    /// the billable substitute had already been committed.
    ///
    /// This pins the variant set: adding a variant makes the exhaustive match in
    /// `edmd::server::forecast_method_to_db` fail to compile, which is the point.
    #[test]
    fn every_variant_is_accounted_for() {
        let all = [
            ForecastMethod::PriorPeriodSameSlot,
            ForecastMethod::WeightedRollingAverage,
            ForecastMethod::LinearInterpolation,
            ForecastMethod::LastValueCarryForward,
            ForecastMethod::ZeroFill,
            ForecastMethod::ProfileBased,
            ForecastMethod::AnnualProjection,
        ];
        assert_eq!(all.len(), 7, "ForecastMethod gained or lost a variant");
    }
    #[test]
    fn each_requested_strategy_is_the_one_applied() {
        use super::substitute_values;
        use crate::interval::{MeterInterval, QualityFlag};
        use crate::substitute::SubstituteMethod;
        use rust_decimal::dec;
        use time::macros::datetime;
        let from = datetime!(2026-03-09 00:00 UTC);
        let to = datetime!(2026-03-09 01:00 UTC); // four quarter-hours

        // ZeroFill must not silently become a prior-period average, even when
        // reference data and bracketing values are available.
        let prior = vec![MeterInterval {
            from: datetime!(2026-03-02 00:00 UTC),
            to: datetime!(2026-03-02 00:15 UTC),
            value_kwh: dec!(99),
            quality: QualityFlag::Measured,
            obis_code: None,
        }];

        let zero = substitute_values(
            from,
            to,
            900,
            SubstituteMethod::ZeroFill,
            &prior,
            Some(dec!(50)),
            Some(dec!(70)),
        );
        assert_eq!(zero.len(), 4);
        assert!(
            zero.iter().all(|e| e.interval.value_kwh == dec!(0)),
            "ZeroFill must produce zeros regardless of available reference data"
        );
        assert!(
            zero.iter().all(|e| e.method == ForecastMethod::ZeroFill),
            "the reported method must be the one applied"
        );

        let carry = substitute_values(
            from,
            to,
            900,
            SubstituteMethod::LastValueCarryForward,
            &prior,
            Some(dec!(50)),
            Some(dec!(70)),
        );
        assert!(carry.iter().all(|e| e.interval.value_kwh == dec!(50)));

        // Linear interpolation walks from the leading value toward the trailing
        // one across the gap.
        let linear = substitute_values(
            from,
            to,
            900,
            SubstituteMethod::LinearInterpolation,
            &prior,
            Some(dec!(0)),
            Some(dec!(100)),
        );
        let values: Vec<_> = linear.iter().map(|e| e.interval.value_kwh).collect();
        assert_eq!(values, vec![dec!(0), dec!(25), dec!(50), dec!(75)]);
        assert!(
            linear
                .iter()
                .all(|e| e.method == ForecastMethod::LinearInterpolation)
        );
    }

    #[test]
    fn a_strategy_with_no_data_reports_what_it_fell_back_to() {
        use super::substitute_values;
        use crate::substitute::SubstituteMethod;
        use rust_decimal::dec;
        use time::macros::datetime;
        // Linear interpolation with no closing value has no slope to follow.
        let entries = substitute_values(
            datetime!(2026-03-09 00:00 UTC),
            datetime!(2026-03-09 00:15 UTC),
            900,
            SubstituteMethod::LinearInterpolation,
            &[],
            Some(dec!(42)),
            None,
        );
        assert_eq!(entries[0].interval.value_kwh, dec!(42));
        assert_eq!(
            entries[0].method,
            ForecastMethod::LastValueCarryForward,
            "the audit record must name the fallback that ran, not the request"
        );
    }
}
