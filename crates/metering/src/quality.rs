//! Hampel-filter quality scoring (M7).
//!
//! Extracted from `edmd/src/server.rs` where it was embedded inline.
//! The `meter-quality` crate has been folded into this module.
//!
//! Two entry points:
//! - [`score_intervals`] — accepts `&[MeterInterval]` (Decimal-based, full edmd integration)
//! - [`score_intervals_raw`] — accepts `&[f64]` for callers with raw float data (e.g. from DB NUMERIC)

use crate::interval::MeterInterval;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Constant: converts MAD to equivalent Gaussian standard deviation.
pub const K_MAD: f64 = 1.4826;

/// Configuration for the Hampel-filter quality scorer.
#[derive(Debug, Clone)]
pub struct QualityConfig {
    /// Hampel filter half-window (total = `2k+1` points). Default: 3.
    pub hampel_k: usize,
    /// Hampel threshold in robust-sigma units. Default: 3.0.
    pub hampel_t: f64,
    /// Spike detection multiplier: flag when value > `spike_factor × window_median`. Default: 10.0.
    pub spike_factor: f64,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self {
            hampel_k: 3,
            hampel_t: 3.0,
            spike_factor: 10.0,
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
        let median = if sw.len() % 2 == 0 {
            (sw[sw.len() / 2 - 1] + sw[sw.len() / 2]) / 2.0
        } else {
            sw[sw.len() / 2]
        };

        let mut abs_devs: Vec<f64> = window.iter().map(|x| (x - median).abs()).collect();
        abs_devs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mad = if abs_devs.len() % 2 == 0 {
            (abs_devs[abs_devs.len() / 2 - 1] + abs_devs[abs_devs.len() / 2]) / 2.0
        } else {
            abs_devs[abs_devs.len() / 2]
        };

        let sigma = K_MAD * mad;
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
    let outlier_indices = if sorted.len() >= cfg.hampel_k * 2 + 1 {
        hampel_filter(&values, cfg.hampel_k, cfg.hampel_t)
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
        || max_zero_run > 2
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
    let outlier_count = if values.len() >= k * 2 + 1 {
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
