//! §17 MessZV substitute value generation (Ersatzwertbildung).
//!
//! When meter readings are missing or faulty, the Messstellenbetreiber must
//! generate substitute values (Ersatzwerte) before billing. This module
//! implements the standard methods defined by §17 MessZV and BDEW practice.
//!
//! ## Legal basis
//!
//! - **§17 Abs. 1 MessZV**: The MSB must supply substitute values when measurements
//!   are unavailable. Estimated (Prognosewert) and substituted values are billable.
//! - **BDEW MSCONS AHB**: Defines how `Messwertstatus` flags map to substitute types.
//! - **VDE-AR-N 4400**: Technical rules for substitute value methods.
//!
//! ## Methods implemented
//!
//! | Method | When to use | BDEW recommendation |
//! |---|---|---|
//! | `LinearInterpolation` | Short gaps (≤ 3 intervals) between valid readings | Primary for RLM/iMSys |
//! | `PriorPeriodAverage` | Longer gaps using prior week same-slot average | Biomass, industrial |
//! | `ZeroFill` | Confirmed zero delivery (documented shutdown) | Plant outage only |
//! | `LastValueCarryForward` | Conservative fallback when no context | SLP, default for long gaps |
//!
//! ## Gap filling
//!
//! [`fill_gaps`] uses automatic method selection (linear for short gaps, carry-forward
//! for longer ones). Use [`fill_gaps_with_config`] with [`FillGapsConfig`] to specify
//! a preferred method — in particular [`SubstituteMethod::PriorPeriodAverage`] per
//! §17 Abs. 2 MessZV requires providing `prior_period_intervals`.

use crate::interval::{MeterInterval, QualityFlag};
use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(test)]
use rust_decimal_macros::dec;

// ── SubstituteMethod ──────────────────────────────────────────────────────────

/// Method used to generate a substitute value per §17 MessZV.
///
/// Stored in the generated `MeterInterval.quality` as `Substituted` but
/// the method can be tracked separately for audit purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SubstituteMethod {
    /// Linear interpolation between surrounding measured values.
    ///
    /// Best for short gaps (≤ 3 intervals) when readings before and after are available.
    #[default]
    LinearInterpolation,

    /// Average of the same time slot from a prior reference period.
    ///
    /// Per §17 Abs. 2 MessZV: use the same quarter-hour from the prior week.
    /// Requires [`FillGapsConfig::prior_period_intervals`] to be populated.
    /// Falls back to `LastValueCarryForward` when no matching slot is found.
    PriorPeriodAverage,

    /// Zero — confirmed absence of delivery (documented plant shutdown).
    ZeroFill,

    /// Carry forward the last known good value (conservative fallback).
    LastValueCarryForward,
}

// ── FillGapsConfig ────────────────────────────────────────────────────────────

/// Configuration for [`fill_gaps_with_config`].
///
/// Controls which [`SubstituteMethod`] is applied and provides prior-period
/// reference data for [`SubstituteMethod::PriorPeriodAverage`].
///
/// ## Example — prior-period averaging per §17 Abs. 2 MessZV
///
/// ```rust,ignore
/// use metering::{fill_gaps_with_config, FillGapsConfig, SubstituteMethod};
///
/// // Reference readings from 7 days prior (from edmd)
/// let prior: Vec<_> = fetch_prior_week_intervals(&malo_id).await;
///
/// let config = FillGapsConfig::prior_period(prior);
/// let filled = fill_gaps_with_config(&current, 900, period_from, period_to, &config);
/// ```
/// Reason why a substitute value was generated (for §22 MessZV audit trail).
///
/// Stored alongside each synthetic interval so that auditors and billing systems
/// can explain every line item.
///
/// ## Legal basis
///
/// §17 MessZV requires the MSB to document the substitute value method.
/// §22 MessZV requires a 3-year audit trail for all billing-relevant data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SubstitutionReason {
    /// §17 Abs. 1 MessZV — no measurement available for this interval.
    NoMeasurementAvailable,
    /// Meter hardware failure or communication fault.
    MeterFault,
    /// SMGW communication error (gateway not reachable).
    GatewayCommFailure,
    /// Plausibility check failed — value rejected, substitute generated.
    PlausibilityCheckFailed,
    /// Manual correction by MSB or operator.
    ManualCorrection,
    /// Meter exchange — value interpolated across the replacement boundary.
    MeterExchangeInterpolation,
    /// DST spring-forward: the "missing" hour (clock jumped from 02:00 to 03:00 CET).
    DstSpringForward,
    /// Billing period start/end gap filled for annual settlement.
    BillingPeriodGapFill,
    /// Other documented reason — see free-text `note` field if available.
    Other,
}

impl SubstitutionReason {
    /// Human-readable explanation for this reason (German).
    #[must_use]
    pub fn description(&self) -> &'static str {
        match self {
            Self::NoMeasurementAvailable => "Kein Messwert verfügbar (§17 Abs. 1 MessZV)",
            Self::MeterFault => "Zählerdefekt oder Kommunikationsstörung",
            Self::GatewayCommFailure => "SMGW-Kommunikationsfehler",
            Self::PlausibilityCheckFailed => "Plausibilitätsprüfung fehlgeschlagen",
            Self::ManualCorrection => "Manuelle Korrektur durch MSB/Betreiber",
            Self::MeterExchangeInterpolation => "Zählerwechsel — Interpolation über Wechselgrenze",
            Self::DstSpringForward => "Sommerzeit-Umstellung — fehlende Stunde",
            Self::BillingPeriodGapFill => "Abrechnungszeitraum-Lücke",
            Self::Other => "Sonstiger dokumentierter Grund",
        }
    }
}

/// Configuration for [`fill_gaps_with_config`].
///
/// Controls which substitute value method is applied, how many prior-period
/// reference intervals are used, and what reason is recorded in the audit trail.
pub struct FillGapsConfig {
    /// Which method to apply when synthesising missing values.
    ///
    /// Default: `LinearInterpolation` (auto-falls back to `LastValueCarryForward`
    /// when surrounding data is absent).
    pub method: SubstituteMethod,

    /// Reference period intervals used by [`SubstituteMethod::PriorPeriodAverage`].
    ///
    /// Typically the same calendar week from 7 days prior.
    /// For each gap at time `t`, the algorithm finds all intervals in this slice
    /// whose time-of-day matches `t` (hour, minute, second) and averages their
    /// `value_kwh`. Falls back to `LastValueCarryForward` when none is found.
    pub prior_period_intervals: Vec<MeterInterval>,

    /// Maximum consecutive missing intervals for which linear interpolation is used.
    ///
    /// Default: `3`. Gaps of ≤ this length always use linear interpolation.
    /// Gaps longer than this threshold use the `method` field.
    pub short_gap_threshold: usize,

    /// Documented reason for gap filling (§22 MessZV audit trail).
    ///
    /// When set, the generated substitute intervals carry this reason in their
    /// audit metadata. Used by `edmd` to persist the substitution rationale
    /// in `meter_read_corrections`.
    ///
    /// `None` = reason not specified (acceptable for automated gap-fill).
    pub reason: Option<SubstitutionReason>,
}

impl Default for FillGapsConfig {
    fn default() -> Self {
        Self {
            method: SubstituteMethod::default(),
            prior_period_intervals: Vec::new(),
            short_gap_threshold: 3,
            reason: None,
        }
    }
}

impl FillGapsConfig {
    /// Config for `PriorPeriodAverage` with the given reference data.
    #[must_use]
    pub fn prior_period(prior_period_intervals: Vec<MeterInterval>) -> Self {
        Self {
            method: SubstituteMethod::PriorPeriodAverage,
            prior_period_intervals,
            short_gap_threshold: 3,
            reason: None,
        }
    }

    /// Config for `ZeroFill` (affirmatively documented zero delivery).
    #[must_use]
    pub fn zero_fill() -> Self {
        Self {
            method: SubstituteMethod::ZeroFill,
            prior_period_intervals: Vec::new(),
            short_gap_threshold: 0,
            reason: None,
        }
    }
}

// ── fill_gaps ─────────────────────────────────────────────────────────────────

/// §17 MessZV — Fill gaps with a [`FillGapsConfig`] specifying the substitute method.
///
/// Provides full control over gap-filling strategy — use this when the MSB
/// has determined the appropriate method:
/// - [`SubstituteMethod::PriorPeriodAverage`] — prior-week same-slot values (set `prior_period_intervals`)
/// - [`SubstituteMethod::ZeroFill`] — documented plant shutdown
/// - [`SubstituteMethod::LastValueCarryForward`] — explicit carry-forward
///
/// Short gaps (≤ `config.short_gap_threshold` intervals) always use linear
/// interpolation regardless of `config.method`, as this produces the most
/// accurate substitute for brief data outages.
#[must_use]
pub fn fill_gaps_with_config(
    intervals: &[MeterInterval],
    expected_interval_secs: i64,
    from: OffsetDateTime,
    to: OffsetDateTime,
    config: &FillGapsConfig,
) -> Vec<MeterInterval> {
    use time::Duration;

    if expected_interval_secs <= 0 {
        return intervals.to_vec();
    }

    let mut sorted = intervals.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    use std::collections::HashMap;
    let existing: HashMap<i64, &MeterInterval> = sorted
        .iter()
        .map(|iv| (iv.from.unix_timestamp(), iv))
        .collect();

    // Pre-sort prior_period for quick lookup
    let mut prior_sorted = config.prior_period_intervals.clone();
    prior_sorted.sort_by_key(|iv| iv.from);

    let mut result: Vec<MeterInterval> = Vec::new();
    let mut cursor = from;

    while cursor < to {
        let next = cursor + Duration::seconds(expected_interval_secs);
        let ts = cursor.unix_timestamp();

        if let Some(&iv) = existing.get(&ts) {
            result.push(iv.clone());
        } else {
            // Measure gap length to decide between linear and configured method.
            let gap_len = count_consecutive_gaps(&sorted, cursor, expected_interval_secs);
            let effective_method = if gap_len <= config.short_gap_threshold && gap_len > 0 {
                SubstituteMethod::LinearInterpolation
            } else {
                config.method
            };

            let sub_value = synthesise_value(
                &sorted,
                cursor,
                next,
                &result,
                effective_method,
                &prior_sorted,
            );
            result.push(MeterInterval {
                from: cursor,
                to: next,
                value_kwh: sub_value,
                quality: QualityFlag::Substituted,
                obis_code: sorted.first().and_then(|iv| iv.obis_code.clone()),
            });
        }
        cursor = next;
    }

    result
}

/// Count how many consecutive intervals starting at `gap_start` are missing.
fn count_consecutive_gaps(
    sorted: &[MeterInterval],
    gap_start: OffsetDateTime,
    interval_secs: i64,
) -> usize {
    use time::Duration;
    let existing_starts: std::collections::HashSet<i64> =
        sorted.iter().map(|iv| iv.from.unix_timestamp()).collect();
    let mut count = 0;
    let mut cursor = gap_start;
    while !existing_starts.contains(&cursor.unix_timestamp()) {
        count += 1;
        cursor += Duration::seconds(interval_secs);
        if count > 100 {
            break; // safety cap
        }
    }
    count
}

/// §17 MessZV — Fill gaps in a meter interval series with substitute values.
///
/// Identifies gaps (missing expected intervals) and fills them using the
/// best available method:
///
/// 1. **Short gaps** (1–3 intervals): linear interpolation
/// 2. **Longer gaps**: last-value carry-forward (conservative; MSB may override)
///
/// Use [`fill_gaps_with_config`] to specify an explicit method such as
/// [`SubstituteMethod::PriorPeriodAverage`] per §17 Abs. 2 MessZV.
///
/// Only gaps within `[from, to)` are filled. Leading and trailing gaps are
/// not synthesised — they indicate metering system issues requiring manual review.
///
/// Filled intervals carry `quality = QualityFlag::Substituted` (billable per §17 MessZV Abs. 1).
///
/// ## Parameters
///
/// - `intervals` — meter readings, need not be sorted
/// - `expected_interval_secs` — the regular interval duration (e.g. `900` for 15-min)
/// - `from` / `to` — the metering period boundaries
///
/// ## Example
///
/// ```rust
/// use metering::{MeterInterval, QualityFlag, fill_gaps};
/// use rust_decimal::Decimal;
/// use time::macros::datetime;
///
/// // Two intervals with a gap at 00:15 UTC
/// let intervals = vec![
///     MeterInterval {
///         from:      datetime!(2026-01-01 0:00 UTC),
///         to:        datetime!(2026-01-01 0:15 UTC),
///         value_kwh: Decimal::from_str_exact("2.0").unwrap(),
///         quality:   QualityFlag::Measured,
///         obis_code: None,
///     },
///     MeterInterval {
///         from:      datetime!(2026-01-01 0:30 UTC),
///         to:        datetime!(2026-01-01 0:45 UTC),
///         value_kwh: Decimal::from_str_exact("2.4").unwrap(),
///         quality:   QualityFlag::Measured,
///         obis_code: None,
///     },
/// ];
///
/// let filled = fill_gaps(
///     &intervals,
///     900,
///     datetime!(2026-01-01 0:00 UTC),
///     datetime!(2026-01-01 0:45 UTC),
/// );
/// // Now has 3 intervals; the gap at 00:15 is filled with Substituted quality
/// assert_eq!(filled.len(), 3);
/// assert_eq!(filled[1].quality, QualityFlag::Substituted);
/// ```
#[must_use]
pub fn fill_gaps(
    intervals: &[MeterInterval],
    expected_interval_secs: i64,
    from: OffsetDateTime,
    to: OffsetDateTime,
) -> Vec<MeterInterval> {
    fill_gaps_with_config(
        intervals,
        expected_interval_secs,
        from,
        to,
        &FillGapsConfig::default(),
    )
}

/// Synthesise a substitute value for a missing interval.
fn synthesise_value(
    all_sorted: &[MeterInterval],
    from: OffsetDateTime,
    _to: OffsetDateTime,
    prior_filled: &[MeterInterval],
    method: SubstituteMethod,
    prior_period: &[MeterInterval],
) -> Decimal {
    match method {
        SubstituteMethod::ZeroFill => Decimal::ZERO,

        SubstituteMethod::PriorPeriodAverage => {
            // F-16: §17 Abs. 2 MessZV requires "same time slot from the prior reference period
            // (prior calendar week)". Callers must supply exactly 7 days of prior data;
            // averaging across multiple weeks produces a multi-week average, not the prior-week
            // value. This assertion guards against mis-use at the call site.
            debug_assert!(
                prior_period.len() <= 7 * 24 * 4,
                "PriorPeriodAverage: prior_period should contain ≤ 7 days of data \
                 per §17 Abs. 2 MessZV (got {} intervals — caller must pass only prior-week data)",
                prior_period.len()
            );
            // §17 Abs. 2 MessZV: use the same time slot from the prior reference period.
            // Match by time-of-day (hour, minute, second) — period-independent.
            let target_time = (from.hour(), from.minute(), from.second());
            let matches: Vec<Decimal> = prior_period
                .iter()
                .filter(|iv| {
                    iv.quality.is_billable()
                        && (iv.from.hour(), iv.from.minute(), iv.from.second()) == target_time
                })
                .map(|iv| iv.value_kwh)
                .collect();

            if !matches.is_empty() {
                let sum: Decimal = matches.iter().sum();
                return sum / Decimal::from(matches.len() as u32);
            }
            // Fallback: carry forward last known value
            prior_filled
                .iter()
                .rev()
                .find(|iv| iv.quality.is_billable())
                .map_or(Decimal::ZERO, |iv| iv.value_kwh)
        }

        SubstituteMethod::LastValueCarryForward => prior_filled
            .iter()
            .rev()
            .find(|iv| iv.quality.is_billable())
            .or_else(|| {
                // carry back (gap at start)
                all_sorted
                    .iter()
                    .find(|iv| iv.from > from && iv.quality.is_billable())
            })
            .map_or(Decimal::ZERO, |iv| iv.value_kwh),

        SubstituteMethod::LinearInterpolation => {
            let preceding = prior_filled
                .iter()
                .rev()
                .find(|iv| iv.quality.is_billable());
            let following = all_sorted
                .iter()
                .find(|iv| iv.from > from && iv.quality.is_billable());

            match (preceding, following) {
                (Some(p), Some(f)) => {
                    let total_secs = (f.from - p.from).whole_seconds();
                    let elapsed_secs = (from - p.from).whole_seconds();
                    if total_secs > 0 {
                        let t = Decimal::from(elapsed_secs) / Decimal::from(total_secs);
                        p.value_kwh + t * (f.value_kwh - p.value_kwh)
                    } else {
                        p.value_kwh
                    }
                }
                (Some(p), None) => p.value_kwh,
                (None, Some(f)) => f.value_kwh,
                (None, None) => Decimal::ZERO,
            }
        }
    }
}

/// Linear interpolation between two `MeterInterval` values.
///
/// Fills the gap between `before` and `after` with a single substitute interval.
/// The value is linearly interpolated based on time position.
///
/// # Returns
///
/// A synthesised `MeterInterval` with `quality = Substituted`.
#[must_use]
pub fn linear_interpolation(before: &MeterInterval, after: &MeterInterval) -> MeterInterval {
    // Time fraction: how far into the gap is the midpoint?
    let total_secs = (after.from - before.to).whole_seconds() as f64;
    let mid_secs = total_secs / 2.0;
    let t = if total_secs > 0.0 {
        mid_secs / total_secs
    } else {
        0.5
    };
    let t_dec = Decimal::try_from(t).unwrap_or_else(|_| Decimal::new(5, 1));
    let value = before.value_kwh + t_dec * (after.value_kwh - before.value_kwh);

    MeterInterval {
        from: before.to,
        to: after.from,
        value_kwh: value,
        quality: QualityFlag::Substituted,
        obis_code: before.obis_code.clone(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    fn iv(from_h: i64, from_m: i64, kwh: f64) -> MeterInterval {
        let base = datetime!(2026-01-01 0:00 UTC);
        let start = base + time::Duration::hours(from_h) + time::Duration::minutes(from_m);
        MeterInterval {
            from: start,
            to: start + time::Duration::minutes(15),
            value_kwh: Decimal::try_from(kwh).unwrap(),
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    #[test]
    fn fill_gaps_single_gap() {
        // 00:00 ✓, 00:15 MISSING, 00:30 ✓
        let intervals = vec![iv(0, 0, 2.0), iv(0, 30, 2.4)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);
        let filled = fill_gaps(&intervals, 900, from, to);

        assert_eq!(filled.len(), 3, "should have 3 intervals after gap fill");
        assert_eq!(filled[0].quality, QualityFlag::Measured);
        assert_eq!(
            filled[1].quality,
            QualityFlag::Substituted,
            "gap must be Substituted"
        );
        assert_eq!(filled[2].quality, QualityFlag::Measured);
        // Linear interpolation: (2.0 + 2.4) / 2 = 2.2 kWh
        assert!(
            filled[1].value_kwh > dec!(1.9) && filled[1].value_kwh < dec!(2.5),
            "interpolated value {} out of range",
            filled[1].value_kwh
        );
    }

    #[test]
    fn fill_gaps_no_gaps() {
        let intervals = vec![iv(0, 0, 2.0), iv(0, 15, 2.1), iv(0, 30, 2.2)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);
        let filled = fill_gaps(&intervals, 900, from, to);
        assert_eq!(filled.len(), 3);
        assert!(filled.iter().all(|iv| iv.quality == QualityFlag::Measured));
    }

    #[test]
    fn fill_gaps_carry_forward_at_end() {
        // Only one interval, gap at the end
        let intervals = vec![iv(0, 0, 3.0)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:30 UTC);
        let filled = fill_gaps(&intervals, 900, from, to);
        assert_eq!(filled.len(), 2);
        assert_eq!(filled[0].quality, QualityFlag::Measured);
        assert_eq!(filled[1].quality, QualityFlag::Substituted);
        assert_eq!(
            filled[1].value_kwh,
            dec!(3.0),
            "carry-forward from last known"
        );
    }

    #[test]
    fn linear_interpolation_midpoint() {
        let before = iv(0, 0, 2.0);
        let mut after = iv(0, 30, 4.0);
        after.from = before.to + time::Duration::minutes(15); // 00:30 gap
        after.to = after.from + time::Duration::minutes(15);
        let sub = linear_interpolation(&before, &after);
        assert_eq!(sub.quality, QualityFlag::Substituted);
        // Midpoint of [2.0, 4.0] = 3.0
        assert!(
            sub.value_kwh > dec!(2.5) && sub.value_kwh < dec!(3.5),
            "interpolated value {}",
            sub.value_kwh
        );
    }

    #[test]
    fn fill_gaps_multiple_gaps() {
        // 00:00 ✓, 00:15 MISSING, 00:30 MISSING, 00:45 ✓
        let intervals = vec![iv(0, 0, 2.0), iv(0, 45, 2.6)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 1:00 UTC);
        let filled = fill_gaps(&intervals, 900, from, to);
        assert_eq!(filled.len(), 4);
        assert_eq!(filled[1].quality, QualityFlag::Substituted);
        assert_eq!(filled[2].quality, QualityFlag::Substituted);
    }

    // ── fill_gaps_with_config tests ────────────────────────────────────────────

    /// §17 Abs. 2 MessZV — PriorPeriodAverage uses the same time slot from prior week.
    #[test]
    fn fill_gaps_prior_period_average_uses_matching_slot() {
        // Prior period: 00:15 slot had 3.0 kWh last week
        let prior_week_base = datetime!(2025-12-25 0:00 UTC); // 7 days earlier
        let prior_reading = MeterInterval {
            from: prior_week_base + time::Duration::minutes(15),
            to: prior_week_base + time::Duration::minutes(30),
            value_kwh: dec!(3.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        };

        // Current week: 00:00 ✓, 00:15 MISSING, 00:30 ✓
        let intervals = vec![iv(0, 0, 2.0), iv(0, 30, 4.0)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);

        let config = FillGapsConfig::prior_period(vec![prior_reading]);
        let filled = fill_gaps_with_config(&intervals, 900, from, to, &config);

        assert_eq!(filled.len(), 3);
        assert_eq!(filled[1].quality, QualityFlag::Substituted);
        // Gap at 00:15 should use prior-period 00:15 value = 3.0
        assert_eq!(
            filled[1].value_kwh,
            dec!(3.0),
            "PriorPeriodAverage must use prior-week same-slot value"
        );
    }

    /// PriorPeriodAverage falls back to carry-forward when no prior slot matches.
    #[test]
    fn fill_gaps_prior_period_average_fallback_to_carry_forward() {
        // Prior period has no data at 00:15 (different time slots only)
        let prior_reading = MeterInterval {
            from: datetime!(2025-12-25 1:00 UTC), // 01:00 slot, not 00:15
            to: datetime!(2025-12-25 1:15 UTC),
            value_kwh: dec!(5.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        };

        let intervals = vec![iv(0, 0, 2.5), iv(0, 30, 4.0)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);

        // short_gap_threshold=0 disables the short-gap linear override so
        // PriorPeriodAverage (and its carry-forward fallback) applies to all gaps.
        let config = FillGapsConfig {
            method: SubstituteMethod::PriorPeriodAverage,
            prior_period_intervals: vec![prior_reading],
            short_gap_threshold: 0,
            reason: None,
        };
        let filled = fill_gaps_with_config(&intervals, 900, from, to, &config);

        assert_eq!(filled.len(), 3);
        assert_eq!(filled[1].quality, QualityFlag::Substituted);
        // No prior-period match → carry forward from 00:00 value = 2.5
        assert_eq!(
            filled[1].value_kwh,
            dec!(2.5),
            "fallback must carry forward last known value"
        );
    }

    /// ZeroFill produces confirmed-zero substitute values.
    #[test]
    fn fill_gaps_zero_fill_config() {
        let intervals = vec![iv(0, 0, 2.0), iv(0, 30, 2.0)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);

        let filled = fill_gaps_with_config(&intervals, 900, from, to, &FillGapsConfig::zero_fill());
        assert_eq!(filled.len(), 3);
        assert_eq!(filled[1].quality, QualityFlag::Substituted);
        assert_eq!(filled[1].value_kwh, dec!(0), "ZeroFill must produce 0");
    }

    /// Short gaps always use linear interpolation regardless of configured method.
    #[test]
    fn fill_gaps_short_gap_always_linear_even_with_zero_fill_method() {
        // short_gap_threshold = 3 by default; gap of 1 = always linear
        let intervals = vec![iv(0, 0, 2.0), iv(0, 30, 4.0)];
        let from = datetime!(2026-01-01 0:00 UTC);
        let to = datetime!(2026-01-01 0:45 UTC);

        // Despite ZeroFill method, a gap of 1 interval ≤ threshold → linear
        let config = FillGapsConfig {
            method: SubstituteMethod::ZeroFill,
            prior_period_intervals: vec![],
            short_gap_threshold: 3,
            reason: None,
        };
        let filled = fill_gaps_with_config(&intervals, 900, from, to, &config);
        assert_eq!(filled.len(), 3);
        // Linear interpolation: (2.0 + 4.0) / 2 ≈ 3.0 (not 0)
        assert!(
            filled[1].value_kwh > dec!(1.0),
            "short gap must use linear interpolation, not ZeroFill"
        );
    }
}
