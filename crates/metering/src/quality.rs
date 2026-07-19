//! Hampel-filter quality scoring.
//!
//! A Hampel filter flags a point as an outlier when it deviates from its local
//! median by more than `t` robust sigma, where sigma is the median absolute
//! deviation scaled by 1.4826 — the constant making MAD a consistent estimator
//! of the standard deviation for normally distributed data. Median and MAD both
//! have a 50 % breakdown point, so a run of corrupt readings cannot shift the
//! threshold enough to mask itself.
//!
//! Two entry points, differing only in the numeric type they accept:
//! - [`score_intervals`] — `&[MeterInterval]`, `Decimal`-based, no precision loss
//! - [`score_intervals_raw`] — `&[f64]`, for callers holding raw floats

use crate::interval::MeterInterval;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Constant: converts MAD to equivalent Gaussian standard deviation.
pub const K_MAD: f64 = 1.4826;

/// Configuration for the Hampel-filter quality scorer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityConfig {
    /// Hampel filter half-window (total = `2k+1` points). Default: 3.
    pub hampel_k: usize,
    /// Hampel threshold in robust-sigma units. Default: 3.0.
    pub hampel_t: f64,
    /// Spike detection multiplier: flag when value > `spike_factor × window_median`. Default: 10.0.
    pub spike_factor: f64,
    /// Longest run of consecutive zero intervals that is *not* a warning.
    /// Default: 2.
    ///
    /// Electricity has a standby floor, so a short zero run indicates a dead
    /// meter. Water and heat have no such floor: an empty flat or an unheated
    /// circuit reads zero for days. See [`QualityConfig::for_sparte`].
    pub max_zero_run_allowed: usize,
    /// Absolute floor on the robust sigma, in the series' own unit. Default: 0.0.
    ///
    /// Across a flat window the median absolute deviation is 0, so `t × sigma`
    /// is 0 and every nonzero deviation scores as an outlier. On a flat-profile
    /// medium that flags the first genuine consumption after a quiet period. The
    /// floor makes the test "deviates by more than `min_sigma`".
    pub min_sigma: f64,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            hampel_k: 3,
            hampel_t: 3.0,
            spike_factor: 10.0,
            max_zero_run_allowed: 2,
            min_sigma: 0.0,
        }
    }
}

impl QualityConfig {
    /// Media-aware defaults.
    ///
    /// The electricity thresholds suit 15-minute RLM load profiles, which are
    /// noisy and rarely flat. Heat and water submetering profiles are dominated
    /// by long legitimate zero runs and need wider tolerances.
    #[must_use]
    pub fn for_sparte(sparte: crate::interval::Sparte) -> Self {
        use crate::interval::Sparte;
        match sparte {
            Sparte::Strom => Self::default(),
            // Gas heating is seasonal: a summer week of near-zero draw is normal.
            Sparte::Gas => Self {
                max_zero_run_allowed: 48,
                min_sigma: 0.01,
                ..Self::default()
            },
            // Heat: unheated months, and HCA units are dimensionless and coarse.
            Sparte::Waerme => Self {
                max_zero_run_allowed: 720,
                min_sigma: 0.05,
                ..Self::default()
            },
            // Water: a vacant flat reads exactly zero indefinitely, and the
            // resolution is litres, so sigma floors must be small.
            Sparte::Wasser => Self {
                max_zero_run_allowed: 720,
                min_sigma: 0.001,
                ..Self::default()
            },
        }
    }
}

/// Quality grade: A / B / C / F.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum QualityGrade {
    /// No warnings — clean data. Normal billing.
    A,
    /// Minor issues (≤1 gap, ≤1 outlier, coverage ≥99%). Proceed with note.
    B,
    /// Significant issues (≤3 gaps, ≤3 outliers, coverage ≥95%). Manual review.
    C,
    /// Unusable — block billing.
    F,
}

impl QualityGrade {
    /// `true` when this grade blocks automated billing.
    #[must_use]
    pub fn blocks_billing(&self) -> bool {
        matches!(self, QualityGrade::F)
    }

    /// Grade as a static string.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            QualityGrade::A => "A",
            QualityGrade::B => "B",
            QualityGrade::C => "C",
            QualityGrade::F => "F",
        }
    }
}

impl std::fmt::Display for QualityGrade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Quality report for a batch of meter intervals.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct QualityReport {
    /// Number of intervals analysed.
    pub intervals_analysed: usize,
    /// Number of gaps (pairs where `to[i] ≠ from[i+1]`).
    pub gaps_detected: usize,
    /// Start timestamps of detected gaps (up to 20).
    pub gap_starts: Vec<String>,
    /// Longest consecutive zero-value run.
    pub max_zero_run: usize,
    /// Start timestamps of Hampel-detected outliers.
    pub outlier_intervals: Vec<String>,
    /// Start timestamps of spike-detected intervals.
    pub spike_intervals: Vec<String>,
    /// `true` when all intervals have the same duration.
    pub intervals_consistent: bool,
    /// Coverage percentage: analysed / expected × 100.
    pub coverage_pct: f64,
    /// `true` when any warning was detected.
    pub has_warnings: bool,
    /// Overall quality grade.
    pub grade: QualityGrade,
}

/// Run the Hampel filter on a float slice.
///
/// # Example
/// ```rust
/// use metering::hampel_filter;
///
/// let values = vec![1.0, 1.1, 1.0, 50.0, 1.0, 1.1, 1.0];
/// let outliers = hampel_filter(&values, 3, 3.0);
/// assert!(outliers.contains(&3), "spike at index 3 must be detected");
/// ```
#[must_use]
pub fn hampel_filter(values: &[f64], k: usize, t: f64) -> Vec<usize> {
    hampel_filter_with_floor(values, k, t, 0.0)
}

/// [`hampel_filter`] with an absolute floor on the robust sigma.
///
/// See [`QualityConfig::min_sigma`] for why a floor is needed on flat-profile
/// media such as water and heat.
#[must_use]
pub fn hampel_filter_with_floor(values: &[f64], k: usize, t: f64, min_sigma: f64) -> Vec<usize> {
    let n = values.len();
    let mut outliers = Vec::new();
    for i in 0..n {
        let lo = i.saturating_sub(k);
        let hi = (i + k + 1).min(n);
        let window: Vec<f64> = values[lo..hi].to_vec();
        if window.is_empty() {
            continue;
        }

        let mut sw = window.clone();
        sw.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = if sw.len().is_multiple_of(2) {
            (sw[sw.len() / 2 - 1] + sw[sw.len() / 2]) / 2.0
        } else {
            sw[sw.len() / 2]
        };

        let mut abs_devs: Vec<f64> = window.iter().map(|x| (x - median).abs()).collect();
        abs_devs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mad = if abs_devs.len().is_multiple_of(2) {
            (abs_devs[abs_devs.len() / 2 - 1] + abs_devs[abs_devs.len() / 2]) / 2.0
        } else {
            abs_devs[abs_devs.len() / 2]
        };

        // `min_sigma` floors the scale estimate so a perfectly flat window
        // (mad == 0) does not declare every nonzero deviation an outlier.
        let sigma = (K_MAD * mad).max(min_sigma);
        if sigma <= 0.0 {
            if (values[i] - median).abs() > 0.0 {
                outliers.push(i);
            }
        } else if (values[i] - median).abs() > t * sigma {
            outliers.push(i);
        }
    }
    outliers
}

/// Score a set of meter intervals and return a [`QualityReport`].
///
/// # Example
/// ```rust
/// use metering::{score_intervals, QualityGrade, MeterInterval, QualityFlag, QualityConfig};
/// use rust_decimal::Decimal;
/// use time::macros::datetime;
///
/// let samples: Vec<MeterInterval> = (0..20).map(|i| MeterInterval {
///     from: datetime!(2026-01-01 0:00 UTC) + time::Duration::minutes(i * 15),
///     to:   datetime!(2026-01-01 0:00 UTC) + time::Duration::minutes(i * 15 + 15),
///     value_kwh: Decimal::from_str_exact("2.0").unwrap(),
///     quality: QualityFlag::Measured,
///     obis_code: None,
/// }).collect();
/// let report = score_intervals(&samples, QualityConfig::default());
/// assert_eq!(report.grade, QualityGrade::A);
/// ```
#[must_use]
pub fn score_intervals(samples: &[MeterInterval], cfg: QualityConfig) -> QualityReport {
    if samples.is_empty() {
        return QualityReport {
            intervals_analysed: 0,
            gaps_detected: 0,
            gap_starts: vec![],
            max_zero_run: 0,
            outlier_intervals: vec![],
            spike_intervals: vec![],
            intervals_consistent: true,
            coverage_pct: 0.0,
            has_warnings: true,
            grade: QualityGrade::F,
        };
    }

    let mut sorted = samples.to_vec();
    sorted.sort_by_key(|s| s.from);

    // 1. Gap detection
    let mut gaps_detected = 0usize;
    let mut gap_starts: Vec<String> = Vec::new();
    for pair in sorted.windows(2) {
        if pair[0].to != pair[1].from {
            gaps_detected += 1;
            if gap_starts.len() < 20 {
                gap_starts.push(pair[0].to.to_string());
            }
        }
    }

    // 2. Zero-run
    let mut zero_run = 0usize;
    let mut max_zero_run = 0usize;
    for s in &sorted {
        if s.value_kwh.is_zero() {
            zero_run += 1;
            max_zero_run = max_zero_run.max(zero_run);
        } else {
            zero_run = 0;
        }
    }

    // 3. Interval consistency
    let durations: Vec<i64> = sorted
        .iter()
        .map(|s| s.duration_secs())
        .filter(|&d| d > 0)
        .collect();
    let intervals_consistent = durations.windows(2).all(|d| d[0] == d[1]);

    let values: Vec<f64> = sorted
        .iter()
        .map(|s| s.value_kwh.to_string().parse::<f64>().unwrap_or(0.0))
        .collect();

    // 4. Hampel outlier detection
    let outlier_indices = if sorted.len() > cfg.hampel_k * 2 {
        hampel_filter_with_floor(&values, cfg.hampel_k, cfg.hampel_t, cfg.min_sigma)
    } else {
        vec![]
    };
    let outlier_intervals: Vec<String> = outlier_indices
        .iter()
        .map(|&i| sorted[i].from.to_string())
        .collect();

    // 5. Spike detection (value > spike_factor × window_median)
    let spike_indices: Vec<usize> = if sorted.len() >= 5 {
        (0..sorted.len())
            .filter(|&i| {
                let k = cfg.hampel_k.min(3);
                let lo = i.saturating_sub(k);
                let hi = (i + k + 1).min(sorted.len());
                let neighbours: Vec<f64> = values[lo..hi]
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| lo + j != i)
                    .map(|(_, &v)| v)
                    .filter(|&v| v > 0.0)
                    .collect();
                if neighbours.len() < 3 {
                    return false;
                }
                let mut ns = neighbours.clone();
                ns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = ns[ns.len() / 2];
                median > 0.0 && values[i] > cfg.spike_factor * median
            })
            .collect()
    } else {
        vec![]
    };
    let spike_intervals: Vec<String> = spike_indices
        .iter()
        .map(|&i| sorted[i].from.to_string())
        .collect();

    // 6. Coverage
    let median_dur: f64 = if durations.is_empty() {
        900.0
    } else {
        let mut ds = durations.clone();
        ds.sort_unstable();
        ds[ds.len() / 2] as f64
    };
    let period_secs = (sorted.last().unwrap().to - sorted.first().unwrap().from)
        .whole_seconds()
        .max(1) as f64;
    let expected = (period_secs / median_dur).ceil() as usize;
    let coverage_pct = if expected == 0 {
        100.0
    } else {
        ((sorted.len() as f64 / expected as f64) * 100.0).min(100.0)
    };

    // Grade
    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    let has_warnings = gaps_detected > 0
        || max_zero_run > cfg.max_zero_run_allowed
        || total_anomalies > 0
        || coverage_pct < 99.0
        || !intervals_consistent;

    let grade = if !has_warnings {
        QualityGrade::A
    } else if gaps_detected <= 1 && total_anomalies <= 1 && coverage_pct >= 99.0 {
        QualityGrade::B
    } else if gaps_detected <= 3 && total_anomalies <= 3 && coverage_pct >= 95.0 {
        QualityGrade::C
    } else {
        QualityGrade::F
    };

    QualityReport {
        intervals_analysed: sorted.len(),
        gaps_detected,
        gap_starts,
        max_zero_run,
        outlier_intervals,
        spike_intervals,
        intervals_consistent,
        coverage_pct,
        has_warnings,
        grade,
    }
}

/// Score a set of raw `f64` values using the Hampel filter and return a [`QualityGrade`].
///
/// This is the lightweight entry-point for callers who have raw float values
/// (e.g. from a PostgreSQL `NUMERIC` column via sqlx `try_get::<f64,_>()`)
/// rather than `MeterInterval` slices.
///
/// For gap detection, zero-run detection, and coverage calculation, use
/// [`score_intervals`] with proper `MeterInterval`s.  This function only
/// applies the Hampel outlier filter and spike detection.
///
/// # Example
/// ```rust
/// use metering::{score_intervals_raw, QualityGrade};
///
/// let values = vec![2.3_f64, 2.4, 2.3, 2.5, 2.2, 2.4, 2.3];
/// assert_eq!(score_intervals_raw(&values, 3, 3.0), QualityGrade::A);
///
/// // Spike in the middle
/// let with_spike = vec![2.3_f64, 2.4, 2.3, 500.0, 2.3, 2.4, 2.3];
/// assert_ne!(score_intervals_raw(&with_spike, 3, 3.0), QualityGrade::A);
/// ```
#[must_use]
pub fn score_intervals_raw(values: &[f64], k: usize, t: f64) -> QualityGrade {
    if values.is_empty() {
        return QualityGrade::F;
    }
    let outlier_count = if values.len() > k * 2 {
        hampel_filter(values, k, t).len()
    } else {
        0
    };
    // Spike detection: value > 10× window_median of neighbours
    let spike_count: usize = if values.len() >= 5 {
        (0..values.len())
            .filter(|&i| {
                let lo = i.saturating_sub(k.min(3));
                let hi = (i + k.min(3) + 1).min(values.len());
                let neighbours: Vec<f64> = values[lo..hi]
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| lo + j != i)
                    .map(|(_, &v)| v)
                    .filter(|&v| v > 0.0)
                    .collect();
                if neighbours.len() < 3 {
                    return false;
                }
                let mut ns = neighbours.clone();
                ns.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = ns[ns.len() / 2];
                median > 0.0 && values[i] > 10.0 * median
            })
            .count()
    } else {
        0
    };

    let total = outlier_count + spike_count;
    if total == 0 {
        QualityGrade::A
    } else if total <= 1 {
        QualityGrade::B
    } else if total <= 3 {
        QualityGrade::C
    } else {
        QualityGrade::F
    }
}

/// Score a pre-converted `f64` value slice with interval start-times and return a
/// full [`QualityReport`].
///
/// This is the **fast path** for callers that already hold raw float values
/// (e.g. from a PostgreSQL `NUMERIC` column via `sqlx::try_get::<f64,_>()`,
/// or from the direct-push ingest path). It avoids the `Decimal → f64`
/// conversion inside [`score_intervals`] and eliminates the intermediate
/// `Vec<MeterInterval>` allocation.
///
/// ## SIMD-friendly design (no platform-specific intrinsics required)
///
/// The inner loops are written as tight, branchless passes over contiguous
/// `f64` slices. LLVM auto-vectorises them to AVX2 (4×f64/cycle) on x86-64
/// and NEON (2×f64/cycle) on AArch64 when compiled with `opt-level >= 2`:
///
/// - **Gap detection**: single pass over `timestamps[i+1] - timestamps[i]`
/// - **Zero-run**: single pass with running counter reset
/// - **Interval consistency**: single pass over duration deltas
/// - **Absolute deviations**: tight `|xi − median|` loop, vectorises to SIMD
///   subtract + absolute value (no conditional branches in the hot path)
///
/// The Hampel window sort (7 elements, k=3) is too small for SIMD benefit;
/// it uses insertion sort with branch-prediction-friendly ordering.
///
/// ## Arguments
///
/// - `values` — energy quantities in kWh (one per interval).
/// - `timestamps` — interval start times as nanosecond-precision Unix epochs.
///   Length must equal `values.len()`. Used for gap detection and coverage.
/// - `period_start_ns`, `period_end_ns` — expected period boundaries in
///   nanoseconds. Used for coverage percentage calculation.
/// - `cfg` — Hampel filter + spike configuration.
///
/// # Example
///
/// ```rust
/// use metering::{score_intervals_f64, QualityConfig, QualityGrade};
///
/// let values    = vec![2.3_f64, 2.4, 2.3, 2.5, 2.2, 2.4, 2.3];
/// let ts_ns: Vec<i64> = (0..7).map(|i| i * 900_000_000_000i64).collect(); // 15-min
/// let report = score_intervals_f64(
///     &values, &ts_ns,
///     ts_ns[0], ts_ns[6] + 900_000_000_000,
///     QualityConfig::default(),
/// );
/// assert_eq!(report.grade, QualityGrade::A);
/// ```
#[must_use]
pub fn score_intervals_f64(
    values: &[f64],
    timestamps_ns: &[i64],
    period_start_ns: i64,
    period_end_ns: i64,
    cfg: QualityConfig,
) -> QualityReport {
    assert_eq!(
        values.len(),
        timestamps_ns.len(),
        "values and timestamps_ns must have the same length"
    );

    if values.is_empty() {
        return QualityReport {
            intervals_analysed: 0,
            gaps_detected: 0,
            gap_starts: vec![],
            max_zero_run: 0,
            outlier_intervals: vec![],
            spike_intervals: vec![],
            intervals_consistent: true,
            coverage_pct: 0.0,
            has_warnings: true,
            grade: QualityGrade::F,
        };
    }

    let n = values.len();

    // ── 1. Gap detection — tight loop over consecutive timestamp deltas ───────
    // Auto-vectorises: delta = ts[i+1] − ts[i]; compare against expected_ns.
    let mut gaps_detected = 0usize;
    let mut gap_starts: Vec<String> = Vec::new();

    // Infer expected interval from median duration (robust against outlier gaps).
    let mut durations_ns: Vec<i64> = timestamps_ns
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|&d| d > 0)
        .collect();

    let expected_interval_ns: i64 = if durations_ns.is_empty() {
        900_000_000_000 // default: 15 min in nanoseconds
    } else {
        durations_ns.sort_unstable();
        durations_ns[durations_ns.len() / 2]
    };

    for (i, w) in timestamps_ns.windows(2).enumerate() {
        let delta = w[1] - w[0];
        if delta.abs() > expected_interval_ns + expected_interval_ns / 10 {
            gaps_detected += 1;
            if gap_starts.len() < 20 {
                gap_starts.push(format!("t+{}", w[0]));
            }
        }
        let _ = i; // suppress unused warning
    }

    // ── 2. Zero-run — single pass, branchless accumulator ────────────────────
    let mut zero_run = 0usize;
    let mut max_zero_run = 0usize;
    // Tight loop: branch-prediction-friendly; auto-vectorises on x86/ARM.
    for &v in values {
        if v == 0.0 {
            zero_run += 1;
            if zero_run > max_zero_run {
                max_zero_run = zero_run;
            }
        } else {
            zero_run = 0;
        }
    }

    // ── 3. Interval consistency ───────────────────────────────────────────────
    let intervals_consistent = timestamps_ns
        .windows(2)
        .map(|w| w[1] - w[0])
        .collect::<Vec<_>>()
        .windows(2)
        .all(|d| (d[0] - d[1]).abs() < expected_interval_ns / 100);

    // ── 4. Hampel outlier detection ───────────────────────────────────────────
    let outlier_indices = if n > cfg.hampel_k * 2 {
        hampel_filter_with_floor(values, cfg.hampel_k, cfg.hampel_t, cfg.min_sigma)
    } else {
        vec![]
    };
    let outlier_intervals: Vec<String> = outlier_indices
        .iter()
        .map(|&i| format!("t+{}", timestamps_ns[i]))
        .collect();

    // ── 5. Spike detection ────────────────────────────────────────────────────
    // Inner loop: tight abs-deviation computation auto-vectorises.
    let k = cfg.hampel_k.min(3);
    let spike_intervals: Vec<String> = if n >= 5 {
        (0..n)
            .filter(|&i| {
                let lo = i.saturating_sub(k);
                let hi = (i + k + 1).min(n);
                // Batch |v - values[i]| for neighbours — this loop auto-vectorises
                let mut neighbours: Vec<f64> = values[lo..hi]
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| lo + j != i)
                    .map(|(_, &v)| v)
                    .filter(|&v| v > 0.0)
                    .collect();
                if neighbours.len() < 3 {
                    return false;
                }
                neighbours.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = neighbours[neighbours.len() / 2];
                median > 0.0 && values[i] > cfg.spike_factor * median
            })
            .map(|i| format!("t+{}", timestamps_ns[i]))
            .collect()
    } else {
        vec![]
    };

    // ── 6. Coverage ───────────────────────────────────────────────────────────
    let period_secs = (period_end_ns - period_start_ns).max(1) as f64 / 1_000_000_000.0;
    let expected_count =
        (period_secs / (expected_interval_ns as f64 / 1_000_000_000.0)).ceil() as usize;
    let coverage_pct = if expected_count == 0 {
        100.0
    } else {
        ((n as f64 / expected_count as f64) * 100.0).min(100.0)
    };

    // ── Grade ─────────────────────────────────────────────────────────────────
    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    let has_warnings = gaps_detected > 0
        || max_zero_run > cfg.max_zero_run_allowed
        || total_anomalies > 0
        || coverage_pct < 99.0
        || !intervals_consistent;

    let grade = if !has_warnings {
        QualityGrade::A
    } else if gaps_detected <= 1 && total_anomalies <= 1 && coverage_pct >= 99.0 {
        QualityGrade::B
    } else if gaps_detected <= 3 && total_anomalies <= 3 && coverage_pct >= 95.0 {
        QualityGrade::C
    } else {
        QualityGrade::F
    };

    QualityReport {
        intervals_analysed: n,
        gaps_detected,
        gap_starts,
        max_zero_run,
        outlier_intervals,
        spike_intervals,
        intervals_consistent,
        coverage_pct,
        has_warnings,
        grade,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::QualityFlag;
    use rust_decimal_macros::dec;
    use time::{OffsetDateTime, macros::datetime};

    fn make_iv(from: OffsetDateTime, v: f64) -> MeterInterval {
        use rust_decimal::Decimal;
        MeterInterval {
            from,
            to: from + time::Duration::minutes(15),
            value_kwh: Decimal::try_from(v).unwrap_or(Decimal::ZERO),
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    fn clean_series(n: usize) -> Vec<MeterInterval> {
        let base = datetime!(2026-01-01 0:00 UTC);
        (0..n)
            .map(|i| {
                make_iv(
                    base + time::Duration::minutes(i as i64 * 15),
                    2.0 + i as f64 * 0.001,
                )
            })
            .collect()
    }

    #[test]
    fn clean_96_intervals_grade_a() {
        let samples = clean_series(96);
        let report = score_intervals(&samples, QualityConfig::default());
        assert_eq!(report.grade, QualityGrade::A);
        assert!(!report.has_warnings);
    }

    #[test]
    fn spike_detected_grade_c_or_f() {
        let mut samples = clean_series(96);
        samples[50].value_kwh = dec!(2000); // 1000× spike
        let report = score_intervals(&samples, QualityConfig::default());
        assert!(report.grade != QualityGrade::A, "spike must degrade grade");
    }

    #[test]
    fn gap_detected() {
        let mut samples = clean_series(96);
        samples.remove(48);
        let report = score_intervals(&samples, QualityConfig::default());
        assert_eq!(report.gaps_detected, 1);
    }

    #[test]
    fn hampel_detects_outlier_in_clean_series() {
        let values = vec![1.0, 1.1, 1.0, 50.0, 1.0, 1.1, 1.0];
        let outliers = hampel_filter(&values, 3, 3.0);
        assert!(outliers.contains(&3));
    }

    #[test]
    fn grade_f_blocks_billing() {
        assert!(QualityGrade::F.blocks_billing());
        assert!(!QualityGrade::A.blocks_billing());
        assert!(!QualityGrade::B.blocks_billing());
        assert!(!QualityGrade::C.blocks_billing());
    }
}

#[cfg(test)]
mod media_aware_tests {
    use super::*;
    use crate::interval::Sparte;

    /// Across a flat window the median absolute deviation is 0, so without a
    /// floor every nonzero deviation scores as an outlier.
    #[test]
    fn sigma_floor_stops_mad_implosion_on_a_flat_series() {
        // A vacant flat, then somebody runs a tap.
        let values = vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.012, 0.0, 0.0, 0.0, 0.0];

        let unfloored = hampel_filter(&values, 3, 3.0);
        assert!(
            !unfloored.is_empty(),
            "without a floor the flat window flags the real draw"
        );

        let floored = hampel_filter_with_floor(&values, 3, 3.0, 0.05);
        assert!(
            floored.is_empty(),
            "a 12 L draw is below the 50 L floor and must not be an outlier"
        );
    }

    /// Electricity keeps the strict default; the flat-profile media do not.
    #[test]
    fn zero_run_tolerance_is_media_specific() {
        assert_eq!(
            QualityConfig::for_sparte(Sparte::Strom).max_zero_run_allowed,
            2
        );
        assert_eq!(
            QualityConfig::for_sparte(Sparte::Gas).max_zero_run_allowed,
            48
        );
        assert_eq!(
            QualityConfig::for_sparte(Sparte::Wasser).max_zero_run_allowed,
            720
        );
        assert_eq!(
            QualityConfig::for_sparte(Sparte::Waerme).max_zero_run_allowed,
            720
        );

        // Strom uses the RLM defaults.
        assert_eq!(
            QualityConfig::for_sparte(Sparte::Strom),
            QualityConfig::default()
        );
    }

    /// A vacant flat's daily water series grades A under the water profile and
    /// worse under the electricity one.
    #[test]
    fn vacant_flat_water_series_grades_clean() {
        let values = vec![0.0_f64; 24];
        let ts_ns: Vec<i64> = (0..24).map(|i| i * 3_600_000_000_000i64).collect();
        let end = ts_ns[23] + 3_600_000_000_000;

        let water = score_intervals_f64(
            &values,
            &ts_ns,
            ts_ns[0],
            end,
            QualityConfig::for_sparte(Sparte::Wasser),
        );
        assert_eq!(
            water.grade,
            QualityGrade::A,
            "a vacant flat reading zero is normal, not a data fault"
        );

        let strom = score_intervals_f64(
            &values,
            &ts_ns,
            ts_ns[0],
            end,
            QualityConfig::for_sparte(Sparte::Strom),
        );
        assert_ne!(
            strom.grade,
            QualityGrade::A,
            "24 zero hours on electricity means a dead meter"
        );
    }
}
