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
//! This module does NOT perform machine-learning forecasting. ML requires an external
//! runtime (PyTorch, ONNX) which violates the no-I/O contract of this crate.
//! For ML-based forecasting, see the `agentd` AI agents.

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
    /// Confidence interval lower bound (kWh), if computable.
    pub confidence_lower_kwh: Option<Decimal>,
    /// Confidence interval upper bound (kWh), if computable.
    pub confidence_upper_kwh: Option<Decimal>,
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
        confidence_lower_kwh: None,
        confidence_upper_kwh: None,
    })
}

// ── prior_period_substitutes ──────────────────────────────────────────────────

/// Generate substitute values for a gap using prior-period same-slot averaging.
///
/// For each missing interval in `[gap_from, gap_to)`:
/// 1. Find the corresponding time slot in `prior_period_intervals` (same weekday, same hour).
/// 2. Average all matching prior-period values.
/// 3. Emit a `SubstituteValueEntry` with `Substituted` quality.
///
/// Falls back to `LastValueCarryForward` when no prior-period slot is found.
///
/// ## §17 Abs. 2 MessZV compliance
///
/// This function implements the BDEW-recommended "Vorperiodenmittelwert"
/// (prior-period average, same time slot). The BDEW recommends using the
/// prior week (7 days) as the reference period.
///
/// ## Parameters
///
/// - `gap_from` / `gap_to`: UTC timestamps of the gap to fill.
/// - `interval_secs`: Expected interval length (900 for 15-min RLM).
/// - `prior_period_intervals`: Reference intervals (e.g. prior 7 days).
/// - `last_known_value`: Fallback value for carry-forward.
#[must_use]
pub fn prior_period_substitutes(
    gap_from: OffsetDateTime,
    gap_to: OffsetDateTime,
    interval_secs: u32,
    prior_period_intervals: &[MeterInterval],
    last_known_value: Option<Decimal>,
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

        let (value, method, ref_count) = if let Some(prior_values) = slot_map.get(&key) {
            let avg =
                prior_values.iter().sum::<Decimal>() / Decimal::from(prior_values.len() as u32);
            (
                avg,
                ForecastMethod::PriorPeriodSameSlot,
                prior_values.len() as u32,
            )
        } else if let Some(last) = last_known_value {
            (last, ForecastMethod::LastValueCarryForward, 1)
        } else {
            (Decimal::ZERO, ForecastMethod::ZeroFill, 0)
        };

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
            confidence_note: if ref_count > 0 {
                Some(format!(
                    "{ref_count} Referenzintervall(e) im Vorperiodenmittelwert"
                ))
            } else {
                None
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
    use rust_decimal_macros::dec;
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
    fn prior_period_substitution_uses_same_slot() {
        // 7 days of prior-period data — same hour every day = 1.5 kWh
        let base = datetime!(2026-01-01 00:00 UTC);
        let prior: Vec<_> = (0..7 * 96)
            .map(|i| make_iv(base + Duration::minutes(15 * i), dec!(1.5)))
            .collect();

        let gap_from = datetime!(2026-01-08 00:00 UTC);
        let gap_to = datetime!(2026-01-08 01:00 UTC); // 4 intervals

        let subs = prior_period_substitutes(gap_from, gap_to, 900, &prior, None);
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
        let subs = prior_period_substitutes(gap_from, gap_to, 900, &[], Some(dec!(2.5)));
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
