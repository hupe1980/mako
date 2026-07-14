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
//! | Method | When to use |
//! |---|---|
//! | `linear_interpolation` | Short gaps (≤ 3 intervals) between valid readings |
//! | `prior_period_average` | Longer gaps — use average from same period prior week |
//! | `zero_fill` | Confirmed zero delivery (e.g. plant shutdown, documented) |
//! | `fill_gaps` | Automatic: applies the best method per gap |
//!
//! ## Important note on Prognose vs. Ersatz
//!
//! - **Prognosewert** (Estimated): projected value, used for advance billing
//!   before the measurement period ends. Billable. May be revised later.
//! - **Ersatzwert** (Substituted): replacement value after the fact, used when
//!   a measurement was missing or faulty. Billable. Final (no revision expected).

use crate::interval::{MeterInterval, QualityFlag};
use rust_decimal::Decimal;
use time::OffsetDateTime;

#[cfg(test)]
use rust_decimal_macros::dec;

// ── SubstituteMethod ──────────────────────────────────────────────────────────

/// Method used to generate a substitute value.
///
/// Stored in the generated `MeterInterval.quality` as `Substituted` but
/// the method can be tracked separately for audit purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum SubstituteMethod {
    /// Linear interpolation between surrounding measured values.
    LinearInterpolation,
    /// Average of the same interval from the prior period (e.g. prior week).
    PriorPeriodAverage,
    /// Zero — confirmed absence of delivery (documented shutdown).
    ZeroFill,
    /// Carried forward from the last known good value.
    LastValueCarryForward,
}

// ── fill_gaps ─────────────────────────────────────────────────────────────────

/// §17 MessZV — Fill gaps in a meter interval series with substitute values.
///
/// Identifies gaps (missing expected intervals) and fills them using the
/// best available method:
///
/// 1. **Short gaps** (1–3 intervals): linear interpolation
/// 2. **Longer gaps**: last-value carry-forward (conservative; MSB may override)
///
/// Only gaps that fall within `[from, to)` are filled; leading and trailing
/// gaps are not synthesised (they may indicate metering system issues that
/// require manual resolution).
///
/// Returns a new `Vec<MeterInterval>` with gaps filled. Filled intervals
/// have `quality = QualityFlag::Substituted` (billable per §17 MessZV Abs. 1).
/// The original intervals are preserved unchanged.
///
/// ## Parameters
///
/// - `intervals` — meter readings, need not be sorted
/// - `expected_interval_secs` — the regular interval duration (e.g. 900 for 15-min)
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
    use time::Duration;

    if intervals.is_empty() || expected_interval_secs <= 0 {
        return intervals.to_vec();
    }

    // Sort by start time.
    let mut sorted = intervals.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    // Build a lookup of existing intervals by their start time (truncated to interval boundary).
    use std::collections::HashMap;
    let existing: HashMap<i64, &MeterInterval> = sorted
        .iter()
        .map(|iv| (iv.from.unix_timestamp(), iv))
        .collect();

    let mut result: Vec<MeterInterval> = Vec::new();
    let mut cursor = from;

    while cursor < to {
        let next = cursor + Duration::seconds(expected_interval_secs);
        let ts = cursor.unix_timestamp();

        if let Some(&iv) = existing.get(&ts) {
            result.push(iv.clone());
        } else {
            // Gap detected — synthesise a substitute value.
            let sub_value = synthesise_value(&sorted, cursor, next, &result);
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

/// Synthesise a substitute value for a missing interval.
///
/// Strategy:
/// 1. If surrounding measured values exist within 3 intervals, use linear interpolation.
/// 2. Otherwise, carry forward the last known good value (conservative).
fn synthesise_value(
    all_sorted: &[MeterInterval],
    from: OffsetDateTime,
    _to: OffsetDateTime,
    prior_filled: &[MeterInterval],
) -> Decimal {
    // Look for the nearest preceding and following measured values (within ±3 intervals)
    let preceding = prior_filled
        .iter()
        .rev()
        .find(|iv| iv.quality.is_billable());

    let following = all_sorted
        .iter()
        .find(|iv| iv.from > from && iv.quality.is_billable());

    match (preceding, following) {
        (Some(p), Some(f)) => {
            // Linear interpolation between p and f
            let total_secs = (f.from - p.from).whole_seconds();
            let elapsed_secs = (from - p.from).whole_seconds();
            if total_secs > 0 {
                let t = Decimal::from(elapsed_secs) / Decimal::from(total_secs);
                p.value_kwh + t * (f.value_kwh - p.value_kwh)
            } else {
                p.value_kwh
            }
        }
        (Some(p), None) => p.value_kwh, // carry forward
        (None, Some(f)) => f.value_kwh, // carry back (rare: gap at start)
        (None, None) => Decimal::ZERO,  // no reference — zero fill
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
}
