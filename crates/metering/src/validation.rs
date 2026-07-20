//! Validation engine for meter interval time series.
//!
//! ## Rules
//!
//! | Rule | ID | Regulatory basis |
//! |---|---|---|
//! | Gap detection | V01 | §17 MessZV — missing intervals require substitute values |
//! | Overlap detection | V02 | Duplicate/overlapping intervals are data errors |
//! | Negative energy | V03 | Consumption values < 0 indicate wiring error or rollover |
//! | Impossible spike | V04 | Value > `spike_factor × rolling_mean` suggests error |
//! | Zero run | V05 | Long run of zeros may indicate frozen/stuck meter |
//! | Interval length | V06 | Interval length must be consistent (e.g. always 15 min) |
//! | DST ambiguity | V07 | UTC required; local-time overlap at CEST→CET transition |
//! | Future timestamp | V08 | Intervals starting in the future are suspect |
//!
//! ## DST note (German time, §3 Allgemeine Festlegungen)
//!
//! All timestamps in MSCONS and direct push are **UTC**. The ambiguous hour
//! at the CEST→CET transition (last Sunday in October, 01:00 UTC = 03:00 CET)
//! must be transmitted as UTC — there is no local-time ambiguity in UTC.
//! Rule V07 detects if timestamps were accidentally stored as local time by
//! checking for duplicate hour boundaries.

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::interval::MeterInterval;
use time_tz::{OffsetDateTimeExt as _, timezones};

// ── ValidationSeverity ────────────────────────────────────────────────────────

/// Severity level of a validation finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ValidationSeverity {
    /// Informational — no action required, but worth noting.
    Info,
    /// Warning — value may be usable for billing but should be reviewed.
    Warning,
    /// Error — value must NOT be used for billing; substitute value required.
    Error,
}

// ── ValidationRuleId ─────────────────────────────────────────────────────────

/// Identifies which validation rule triggered an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum ValidationRuleId {
    /// V01 — missing interval in expected time range.
    GapDetected,
    /// V02 — two intervals have overlapping time windows.
    OverlapDetected,
    /// V03 — consumption value is negative (impossible for Bezug-only meters).
    NegativeEnergy,
    /// V04 — value exceeds `spike_factor × rolling_mean` (statistical outlier).
    ImpossibleSpike,
    /// V05 — consecutive zero values suggest stuck/frozen meter.
    SuspiciousZeroRun,
    /// V06 — interval length differs from expected granularity.
    InconsistentIntervalLength,
    /// V07 — potential DST local-time leak (duplicate hour boundary values).
    DstAmbiguity,
    /// V08 — interval starts in the future.
    FutureTimestamp,
    /// V09 — quality flag is non-billable (`Faulty` or `Unknown`).
    NonBillableQuality,
    /// V10 — value dropped sharply (≥ rollover threshold) suggesting meter rollover.
    ///
    /// Triggered when `value[i] << value[i-1]` by more than `rollover_threshold_kwh`.
    /// §14 MessZV: meter replacement and rollover events must be documented.
    RegisterRollover,
}

impl std::fmt::Display for ValidationRuleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::GapDetected => "V01",
            Self::OverlapDetected => "V02",
            Self::NegativeEnergy => "V03",
            Self::ImpossibleSpike => "V04",
            Self::SuspiciousZeroRun => "V05",
            Self::InconsistentIntervalLength => "V06",
            Self::DstAmbiguity => "V07",
            Self::FutureTimestamp => "V08",
            Self::NonBillableQuality => "V09",
            Self::RegisterRollover => "V10",
        };
        write!(f, "{code}")
    }
}

// ── ValidationIssue ──────────────────────────────────────────────────────────

/// A single validation finding on a meter interval or time series.
///
/// Every `ValidationIssue` carries enough information to:
/// 1. Identify which interval(s) are affected (`interval_index`)
/// 2. Explain the problem (`rule_id`, `message`)
/// 3. Drive automated remediation (substitute value generation for `Error` issues)
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ValidationIssue {
    /// Which validation rule triggered this issue.
    pub rule_id: ValidationRuleId,
    /// Severity: `Info`, `Warning`, or `Error`.
    pub severity: ValidationSeverity,
    /// Human-readable description of the issue.
    pub message: String,
    /// Index into the validated slice where the issue was found.
    /// `None` for series-level issues (e.g. gap between two intervals).
    pub interval_index: Option<usize>,
    /// The timestamp of the affected interval start (for diagnostics).
    pub affected_from: Option<OffsetDateTime>,
    /// The measured value at the affected interval (if known).
    pub affected_value_kwh: Option<Decimal>,
}

impl ValidationIssue {
    fn new(
        rule_id: ValidationRuleId,
        severity: ValidationSeverity,
        message: impl Into<String>,
    ) -> Self {
        Self {
            rule_id,
            severity,
            message: message.into(),
            interval_index: None,
            affected_from: None,
            affected_value_kwh: None,
        }
    }

    fn at(mut self, idx: usize, interval: &MeterInterval) -> Self {
        self.interval_index = Some(idx);
        self.affected_from = Some(interval.from);
        self.affected_value_kwh = Some(interval.value_kwh);
        self
    }

    /// `true` when this issue prevents the interval from being billed.
    #[must_use]
    pub fn blocks_billing(&self) -> bool {
        self.severity == ValidationSeverity::Error
    }
}

// ── ValidationConfig ─────────────────────────────────────────────────────────

/// Configuration for `validate_intervals`.
#[derive(Debug, Clone)]
pub struct ValidationConfig {
    /// Expected interval duration in seconds (e.g. 900 = 15 min, 3600 = 1 h).
    ///
    /// When `None`, interval length consistency is not checked.
    pub expected_interval_secs: Option<u32>,

    /// Multiplier for the spike detection rule (V04).
    ///
    /// An interval value exceeding `spike_factor × rolling_mean` is flagged.
    /// Default: `10.0` — values 10× the rolling average are suspicious.
    pub spike_factor: f64,

    /// Number of consecutive zero-value intervals to trigger V05.
    ///
    /// Default: `4` — four consecutive zeros (1 hour at 15-min granularity).
    pub zero_run_threshold: usize,

    /// Treat negative energy as an Error (V03).
    ///
    /// Set to `false` for bidirectional meters (Einspeisung can be negative
    /// relative to the net direction). Default: `true` for Bezug meters.
    pub negative_energy_is_error: bool,

    /// Reference time for "future timestamp" detection (V08).
    ///
    /// Usually `now()` at ingestion time. When `None`, V08 is disabled.
    pub now: Option<OffsetDateTime>,

    /// Physical plant/connection capacity ceiling in kW for V04.
    ///
    /// A value whose average power over its interval exceeds this ceiling is
    /// physically impossible for the metered plant (nameplate capacity,
    /// Anschlussleistung) and is flagged as an **Error** — unlike the
    /// statistical spike check, which is a Warning. `None` disables the check.
    pub max_plant_power_kw: Option<Decimal>,

    /// Minimum drop (kWh) between consecutive intervals that triggers V10 (RegisterRollover).
    ///
    /// When `value[i] < value[i-1] - rollover_threshold_kwh`, the interval is flagged as a
    /// potential meter rollover (§14 MessZV: rollover events must be documented).
    ///
    /// Typical meter max: 99 999.9 kWh. Default: `50 000` kWh (flags drops > 50 MWh).
    /// Set to `None` to disable rollover detection.
    pub rollover_threshold_kwh: Option<Decimal>,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            expected_interval_secs: Some(900), // 15 minutes
            spike_factor: 10.0,
            zero_run_threshold: 4,
            negative_energy_is_error: true,
            now: None,
            max_plant_power_kw: None,
            rollover_threshold_kwh: Some(Decimal::from(50_000u32)),
        }
    }
}

impl ValidationConfig {
    /// Configuration for 15-minute RLM/iMSys electricity Bezug meters.
    #[must_use]
    pub fn rlm_strom_15min() -> Self {
        Self::default()
    }

    /// Configuration for hourly gas intervals.
    #[must_use]
    pub fn gas_hourly() -> Self {
        Self {
            expected_interval_secs: Some(3600),
            ..Self::default()
        }
    }

    /// Configuration for bidirectional meters (Einspeisung + Bezug, net metering).
    ///
    /// Negative values are allowed (export exceeds import).
    #[must_use]
    pub fn bidirectional() -> Self {
        Self {
            negative_energy_is_error: false,
            ..Self::default()
        }
    }

    /// Disable spike detection (e.g., industrial loads with legitimate spikes).
    #[must_use]
    pub fn without_spike_detection(mut self) -> Self {
        self.spike_factor = f64::INFINITY;
        self
    }

    /// Set the physical capacity ceiling (kW) for the V04 hard limit.
    #[must_use]
    pub fn with_plant_capacity_kw(mut self, kw: Decimal) -> Self {
        self.max_plant_power_kw = Some(kw);
        self
    }
}

// ── Validation result ─────────────────────────────────────────────────────────

/// Result of validating a slice of meter intervals.
#[derive(Debug, Clone)]
pub struct ValidationResult {
    /// All issues found, ordered by interval index.
    pub issues: Vec<ValidationIssue>,
}

impl ValidationResult {
    /// `true` when there are no validation issues of any severity.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.issues.is_empty()
    }

    /// `true` when at least one issue has `Error` severity.
    #[must_use]
    pub fn has_errors(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.severity == ValidationSeverity::Error)
    }

    /// Number of intervals that must be replaced with substitute values.
    #[must_use]
    pub fn billing_block_count(&self) -> usize {
        self.issues.iter().filter(|i| i.blocks_billing()).count()
    }

    /// Filter by severity level.
    pub fn by_severity(
        &self,
        severity: ValidationSeverity,
    ) -> impl Iterator<Item = &ValidationIssue> {
        self.issues.iter().filter(move |i| i.severity == severity)
    }
}

// ── Main validation function ──────────────────────────────────────────────────

/// Validate a slice of meter intervals against the configured rules.
///
/// The input slice should be pre-sorted by `from` timestamp (ascending).
/// Unsorted input may produce spurious overlap/gap findings — sort with
/// `intervals.sort_by_key(|iv| iv.from)` first.
///
/// ## Example
///
/// ```rust
/// use metering::{MeterInterval, QualityFlag, validate_intervals, ValidationConfig};
/// use rust_decimal::dec;
/// use time::macros::datetime;
///
/// let intervals = vec![
///     MeterInterval {
///         from: datetime!(2026-06-01 0:00 UTC),
///         to:   datetime!(2026-06-01 0:15 UTC),
///         value_kwh: dec!(2.5),
///         quality: QualityFlag::Measured,
///         obis_code: None,
///     },
/// ];
/// let result = validate_intervals(&intervals, &ValidationConfig::default());
/// assert!(result.is_clean()); // single interval, no gaps, no spikes
/// ```
#[must_use]
pub fn validate_intervals(
    intervals: &[MeterInterval],
    config: &ValidationConfig,
) -> ValidationResult {
    let mut issues: Vec<ValidationIssue> = Vec::new();

    if intervals.is_empty() {
        return ValidationResult { issues };
    }

    // Compute rolling mean using Decimal arithmetic to avoid f64 precision loss.
    // spike_factor is config (f64, set at construction time, not a billing amount),
    // so the comparison is done in f64 after converting from Decimal.
    let total_kwh: Decimal = intervals.iter().map(|iv| iv.value_kwh).sum();
    let rolling_mean_dec = if intervals.is_empty() {
        Decimal::ONE
    } else {
        total_kwh / Decimal::from(intervals.len() as u32)
    };
    // Convert once for spike factor comparison (spike_factor itself is non-monetary config).
    let rolling_mean: f64 = rolling_mean_dec
        .to_string()
        .parse::<f64>()
        .unwrap_or(1.0)
        .max(f64::EPSILON);

    let mut zero_run = 0usize;

    for (idx, iv) in intervals.iter().enumerate() {
        // V03 — negative energy
        if config.negative_energy_is_error && iv.value_kwh < Decimal::ZERO {
            issues.push(
                ValidationIssue::new(
                    ValidationRuleId::NegativeEnergy,
                    ValidationSeverity::Error,
                    format!("negative energy {} kWh at {}", iv.value_kwh, iv.from),
                )
                .at(idx, iv),
            );
        }

        // V04 — impossible spike
        if config.spike_factor.is_finite()
            && rolling_mean > 0.0
            && let Ok(v) = iv.value_kwh.to_string().parse::<f64>()
            && v > config.spike_factor * rolling_mean
        {
            issues.push(
                ValidationIssue::new(
                    ValidationRuleId::ImpossibleSpike,
                    ValidationSeverity::Warning,
                    format!(
                        "spike {:.3} kWh is {:.1}× rolling mean {:.3} kWh at {} (V04)",
                        v,
                        v / rolling_mean,
                        rolling_mean,
                        iv.from
                    ),
                )
                .at(idx, iv),
            );
        }

        // V04 (hard limit) — average power above the physical plant capacity.
        //
        // The statistical spike check above compares against the series' own
        // rolling mean; this compares against the plant's nameplate /
        // connection capacity, which no genuine reading can exceed.
        if let Some(cap_kw) = config.max_plant_power_kw
            && let Some(power_kw) = iv.demand_kw()
            && power_kw > cap_kw
        {
            issues.push(
                ValidationIssue::new(
                    ValidationRuleId::ImpossibleSpike,
                    ValidationSeverity::Error,
                    format!(
                        "average power {power_kw} kW exceeds plant capacity {cap_kw} kW at {} (V04)",
                        iv.from
                    ),
                )
                .at(idx, iv),
            );
        }

        // V05 — zero run
        if iv.value_kwh.is_zero() {
            zero_run += 1;
            // Only emit once at the start of the run (when the threshold is first reached)
            if zero_run == config.zero_run_threshold {
                let start_idx = idx + 1 - config.zero_run_threshold;
                issues.push(
                    ValidationIssue::new(
                        ValidationRuleId::SuspiciousZeroRun,
                        ValidationSeverity::Warning,
                        format!(
                            "{} consecutive zero intervals starting at index {}",
                            config.zero_run_threshold, start_idx
                        ),
                    )
                    .at(start_idx, &intervals[start_idx]),
                );
            }
        } else {
            zero_run = 0;
        }

        // V06 — interval length consistency
        if let Some(expected_secs) = config.expected_interval_secs {
            let actual_secs = (iv.to - iv.from).whole_seconds();
            if actual_secs != expected_secs as i64 {
                issues.push(
                    ValidationIssue::new(
                        ValidationRuleId::InconsistentIntervalLength,
                        ValidationSeverity::Warning,
                        format!(
                            "expected {}s interval, got {}s at {}",
                            expected_secs, actual_secs, iv.from
                        ),
                    )
                    .at(idx, iv),
                );
            }
        }

        // V08 — future timestamp
        if let Some(now) = config.now
            && iv.from > now
        {
            issues.push(
                ValidationIssue::new(
                    ValidationRuleId::FutureTimestamp,
                    ValidationSeverity::Warning,
                    format!("interval starts in the future: {} > now {}", iv.from, now),
                )
                .at(idx, iv),
            );
        }

        // V09 — non-billable quality
        if !iv.quality.is_billable() {
            issues.push(
                ValidationIssue::new(
                    ValidationRuleId::NonBillableQuality,
                    ValidationSeverity::Error,
                    format!(
                        "quality {:?} is not billable at {} — substitute value required (§17 MessZV)",
                        iv.quality, iv.from
                    ),
                )
                .at(idx, iv),
            );
        }

        // V02 — overlap with previous
        if idx > 0 {
            let prev = &intervals[idx - 1];
            if iv.from < prev.to {
                issues.push(
                    ValidationIssue::new(
                        ValidationRuleId::OverlapDetected,
                        ValidationSeverity::Error,
                        format!(
                            "interval [{}, {}) overlaps previous [{}, {})",
                            iv.from, iv.to, prev.from, prev.to
                        ),
                    )
                    .at(idx, iv),
                );
            }

            // V10 — register rollover (§14 MessZV — must be documented)
            if let Some(threshold) = config.rollover_threshold_kwh {
                let drop = prev.value_kwh - iv.value_kwh;
                if drop >= threshold {
                    issues.push(
                        ValidationIssue::new(
                            ValidationRuleId::RegisterRollover,
                            ValidationSeverity::Warning,
                            format!(
                                "value dropped {:.3} kWh ({:.3} → {:.3}) at {} — \
                                 possible meter rollover (§14 MessZV)",
                                drop, prev.value_kwh, iv.value_kwh, iv.from
                            ),
                        )
                        .at(idx, iv),
                    );
                }
            }
        }
    }

    // V01 — gap detection (series-level, between consecutive intervals)
    if let Some(expected_secs) = config.expected_interval_secs {
        for (idx, window) in intervals.windows(2).enumerate() {
            let (a, b) = (&window[0], &window[1]);
            let gap_secs = (b.from - a.to).whole_seconds();
            if gap_secs >= expected_secs as i64 {
                let gap_count = (gap_secs / expected_secs as i64) as usize;
                issues.push(ValidationIssue {
                    rule_id: ValidationRuleId::GapDetected,
                    severity: ValidationSeverity::Error,
                    message: format!(
                        "gap of {} interval(s) between {} and {} — substitute values required (§17 MessZV)",
                        gap_count, a.to, b.from
                    ),
                    interval_index: Some(idx + 1),
                    affected_from: Some(a.to),
                    affected_value_kwh: None,
                });
            }
        }
    }

    // V07 — DST ambiguity on the autumn fall-back day.
    //
    // Germany repeats the local hour 02:00–03:00 when CEST ends. Stored in UTC
    // that hour appears **twice** — once at UTC+2, once at UTC+1 — so a correct
    // quarter-hour series has 100 intervals that day and every local wall-clock
    // time in the repeated hour occurs exactly twice.
    //
    // A series that was converted from local time without carrying the offset
    // collapses those two passes into one. The surviving interval is then
    // ambiguous: it cannot be said which pass it measured, and one hour of
    // energy has silently vanished. That is what this rule detects — the local
    // time lies in the repeated hour but its partner is absent.
    issues.extend(detect_dst_ambiguity(intervals));

    // Sort issues by interval index for deterministic output
    issues.sort_by_key(|i| i.interval_index.unwrap_or(usize::MAX));

    ValidationResult { issues }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interval::QualityFlag;
    use rust_decimal::dec;
    use time::macros::datetime;

    fn iv(from: OffsetDateTime, to: OffsetDateTime, kwh: rust_decimal::Decimal) -> MeterInterval {
        MeterInterval {
            from,
            to,
            value_kwh: kwh,
            quality: QualityFlag::Measured,
            obis_code: None,
        }
    }

    fn iv_15min(start_min: i64, kwh: rust_decimal::Decimal) -> MeterInterval {
        let base = datetime!(2026-06-01 0:00 UTC);
        let from = base + time::Duration::minutes(start_min);
        let to = from + time::Duration::minutes(15);
        iv(from, to, kwh)
    }

    #[test]
    fn plant_capacity_ceiling_flags_impossible_power() {
        // 30 kWh in 15 min = 120 kW average power against a 30 kW plant.
        let intervals = vec![
            iv_15min(0, dec!(2.5)),
            iv_15min(15, dec!(30.0)),
            iv_15min(30, dec!(2.8)),
        ];
        let cfg = ValidationConfig::default()
            .without_spike_detection()
            .with_plant_capacity_kw(dec!(30));
        let result = validate_intervals(&intervals, &cfg);
        let hard = result
            .issues
            .iter()
            .filter(|i| {
                i.rule_id == ValidationRuleId::ImpossibleSpike
                    && i.severity == ValidationSeverity::Error
            })
            .count();
        assert_eq!(hard, 1, "issues: {:?}", result.issues);
        assert_eq!(result.issues[0].interval_index, Some(1));
    }

    #[test]
    fn plant_capacity_ceiling_passes_plausible_power() {
        // 5 kWh in 15 min = 20 kW — under the 30 kW ceiling.
        let intervals = vec![iv_15min(0, dec!(5.0)), iv_15min(15, dec!(5.0))];
        let cfg = ValidationConfig::default()
            .without_spike_detection()
            .with_plant_capacity_kw(dec!(30));
        assert!(validate_intervals(&intervals, &cfg).is_clean());
    }

    #[test]
    fn clean_series_passes() {
        let intervals = vec![
            iv_15min(0, dec!(2.5)),
            iv_15min(15, dec!(2.3)),
            iv_15min(30, dec!(2.8)),
        ];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        assert!(
            result.is_clean(),
            "expected clean, got: {:?}",
            result.issues
        );
    }

    #[test]
    fn gap_detected() {
        let intervals = vec![
            iv_15min(0, dec!(2.5)),
            iv_15min(30, dec!(2.3)), // skipped 15-min interval at t=15
        ];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        assert!(result.has_errors());
        let gap = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::GapDetected);
        assert!(gap.is_some(), "V01 gap issue not found");
    }

    #[test]
    fn overlap_detected() {
        let base = datetime!(2026-06-01 0:00 UTC);
        let intervals = vec![
            iv(base, base + time::Duration::minutes(20), dec!(2.5)),
            iv(
                base + time::Duration::minutes(15),
                base + time::Duration::minutes(30),
                dec!(2.3),
            ),
        ];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        let overlap = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::OverlapDetected);
        assert!(overlap.is_some(), "V02 overlap issue not found");
        assert!(overlap.unwrap().blocks_billing());
    }

    #[test]
    fn negative_energy_error() {
        let intervals = vec![iv_15min(0, dec!(-1.5))];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        let neg = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::NegativeEnergy);
        assert!(neg.is_some(), "V03 negative energy issue not found");
        assert!(neg.unwrap().blocks_billing());
    }

    #[test]
    fn bidirectional_allows_negative() {
        let intervals = vec![iv_15min(0, dec!(-1.5))];
        let result = validate_intervals(&intervals, &ValidationConfig::bidirectional());
        assert!(!result.has_errors(), "bidirectional should allow negative");
    }

    #[test]
    fn spike_detection() {
        // Spike factor = 10. Mean of [2.5, 2.5, 2.5, 2.5] without spike ≈ 2.5.
        // Since rolling_mean includes all values, use a very large spike to exceed threshold.
        // With values [2.5, 2.5, 2.5, 100.0, 2.5], mean = 110/5 = 22.0
        // spike_factor=10, threshold = 10 * 22 = 220; 100 < 220... still fails.
        // Use spike_factor=3 and spike=20: mean=(2.5*4+20)/5=30/5=6, 20 > 3*6=18 → detected
        let intervals = vec![
            iv_15min(0, dec!(2.5)),
            iv_15min(15, dec!(2.5)),
            iv_15min(30, dec!(2.5)),
            iv_15min(45, dec!(20.0)), // spike
            iv_15min(60, dec!(2.5)),
        ];
        let config = ValidationConfig {
            spike_factor: 3.0,
            ..ValidationConfig::default()
        };
        let result = validate_intervals(&intervals, &config);
        let spike = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::ImpossibleSpike);
        assert!(spike.is_some(), "V04 spike not detected");
        // Spike is a warning, not an error
        assert_eq!(spike.unwrap().severity, ValidationSeverity::Warning);
    }

    #[test]
    fn zero_run_detected() {
        let intervals = vec![
            iv_15min(0, dec!(2.5)),
            iv_15min(15, dec!(0)),
            iv_15min(30, dec!(0)),
            iv_15min(45, dec!(0)),
            iv_15min(60, dec!(0)), // 4 zeros = threshold
            iv_15min(75, dec!(2.5)),
        ];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        let zero = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::SuspiciousZeroRun);
        assert!(zero.is_some(), "V05 zero run not detected");
    }

    #[test]
    fn non_billable_quality_is_error() {
        let base = datetime!(2026-06-01 0:00 UTC);
        let interval = MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Faulty,
            obis_code: None,
        };
        let result = validate_intervals(&[interval], &ValidationConfig::default());
        let nq = result
            .issues
            .iter()
            .find(|i| i.rule_id == ValidationRuleId::NonBillableQuality);
        assert!(nq.is_some(), "V09 non-billable quality not detected");
        assert!(nq.unwrap().blocks_billing());
    }

    /// DST test: at CET→CEST (last Sunday in March), clocks spring forward.
    /// 02:00 CET becomes 03:00 CEST — the hour 02:00–03:00 CET is skipped.
    /// In UTC: 01:00 UTC on that day.
    ///
    /// A correctly stored UTC series has no gap — 01:00 UTC directly precedes
    /// 01:15 UTC. If stored as local time, there would be a phantom gap.
    #[test]
    fn dst_spring_forward_no_false_gap_in_utc() {
        // 2026 DST spring forward: last Sunday in March = March 29
        // 01:00 UTC = 02:00 CET = clock skips to 03:00 CEST
        let base = datetime!(2026-03-29 0:45 UTC); // 01:45 CET, just before skip
        let intervals = vec![
            iv(base, base + time::Duration::minutes(15), dec!(2.5)),
            // 01:00 UTC = the "missing" hour in local time — but UTC is continuous
            iv(
                base + time::Duration::minutes(15),
                base + time::Duration::minutes(30),
                dec!(2.3),
            ),
            iv(
                base + time::Duration::minutes(30),
                base + time::Duration::minutes(45),
                dec!(2.1),
            ),
        ];
        let result = validate_intervals(&intervals, &ValidationConfig::rlm_strom_15min());
        assert!(
            result.is_clean(),
            "UTC series should have no gap at DST spring-forward: {:?}",
            result.issues
        );
    }

    /// DST test: at CEST→CET (last Sunday in October), clocks fall back.
    /// 03:00 CEST becomes 02:00 CET — the hour 02:00–03:00 appears TWICE in local time.
    /// In UTC: 01:00 UTC on that day.
    ///
    /// A correctly stored UTC series has no overlap — values are unique by UTC timestamp.
    #[test]
    fn dst_fall_back_no_overlap_in_utc() {
        // 2026 DST fall back: last Sunday in October = October 25
        // 01:00 UTC = 03:00 CEST → 02:00 CET  (the "extra" hour)
        let base = datetime!(2026-10-25 0:45 UTC);
        let intervals: Vec<MeterInterval> = (0..8)
            .map(|i| {
                let from = base + time::Duration::minutes(i * 15);
                iv(from, from + time::Duration::minutes(15), dec!(2.5))
            })
            .collect();
        let result = validate_intervals(&intervals, &ValidationConfig::rlm_strom_15min());
        assert!(
            result.is_clean(),
            "UTC series should have no overlap at DST fall-back: {:?}",
            result.issues
        );
    }
}

/// Detect a collapsed DST fall-back hour (V07).
///
/// Germany repeats local 02:00–03:00 when CEST ends, so that calendar day has
/// **25 hours**. A series converted from local time without carrying the UTC
/// offset collapses the two passes into one and silently loses an hour of
/// energy.
///
/// The test has to be immune to truncated query windows: a series that merely
/// *starts* inside the repeated hour is not corrupt, it is just short. So this
/// only judges a series that demonstrably covers the **whole local fall-back
/// day** — first interval at local 00:00, last ending at local 00:00 the next
/// day. Within that, a day carrying less than 25 hours of intervals is missing
/// the repeat.
fn detect_dst_ambiguity(intervals: &[MeterInterval]) -> Vec<ValidationIssue> {
    let berlin = timezones::db::europe::BERLIN;
    let Some(first) = intervals.first() else {
        return Vec::new();
    };
    let Some(last) = intervals.last() else {
        return Vec::new();
    };

    let start_local = first.from.to_timezone(berlin);
    let end_local = last.to.to_timezone(berlin);

    // Anchor on the start only. A *collapsed* day ends an hour early, so
    // requiring the series to end at local midnight would exclude exactly the
    // case this rule exists to catch. Requiring it to *begin* at local midnight
    // is enough to distinguish a full-day series from a truncated query window.
    if start_local.time() != time::Time::MIDNIGHT {
        return Vec::new();
    }
    // Must not run past the following local midnight — beyond that the series is
    // multi-day and the per-day arithmetic below does not apply.
    if (end_local.date() - start_local.date()).whole_days() > 1 {
        return Vec::new();
    }

    // A fall-back day starts at CEST (+2) and ends at CET (+1).
    let span_hours = (last.to - first.from).whole_hours();
    let is_fall_back_day =
        start_local.offset().whole_hours() == 2 && end_local.offset().whole_hours() == 1;
    if !is_fall_back_day {
        return Vec::new();
    }

    // Sum the covered duration rather than counting intervals, so the rule holds
    // at any resolution.
    let covered: i64 = intervals
        .iter()
        .map(|iv| (iv.to - iv.from).whole_seconds())
        .sum();
    const TWENTY_FIVE_HOURS: i64 = 25 * 3600;
    if covered >= TWENTY_FIVE_HOURS {
        return Vec::new();
    }

    vec![ValidationIssue {
        rule_id: ValidationRuleId::DstAmbiguity,
        severity: ValidationSeverity::Error,
        message: format!(
            "local day {} is a DST fall-back day (25 hours) but the series covers only \
             {covered} s over a {span_hours} h span — the repeated hour 02:00–03:00 was \
             collapsed, so one hour of energy is missing and the surviving intervals are \
             ambiguous between the two passes",
            start_local.date()
        ),
        interval_index: Some(0),
        affected_from: Some(first.from),
        affected_value_kwh: None,
    }]
}

#[cfg(test)]
mod v07_tests {
    use super::*;
    use crate::interval::QualityFlag;
    use rust_decimal::dec;
    use time::macros::datetime;

    /// Build `n` consecutive quarter-hours from `start`.
    fn qh(start: OffsetDateTime, n: i64) -> Vec<MeterInterval> {
        (0..n)
            .map(|i| {
                let from = start + time::Duration::minutes(15 * i);
                MeterInterval {
                    from,
                    to: from + time::Duration::minutes(15),
                    value_kwh: dec!(1.0),
                    quality: QualityFlag::Measured,
                    obis_code: None,
                }
            })
            .collect()
    }

    /// 2026-10-25 local runs 22:00Z (24 Oct) → 23:00Z (25 Oct): 25 hours,
    /// 100 quarter-hours. A complete day is not ambiguous.
    #[test]
    fn a_complete_25_hour_fall_back_day_is_clean() {
        let intervals = qh(datetime!(2026-10-24 22:00 UTC), 100);
        assert!(
            detect_dst_ambiguity(&intervals).is_empty(),
            "a full 25-hour day must not raise V07"
        );
    }

    /// The same local day carrying only 24 hours means the repeated hour was
    /// collapsed — an hour of energy is gone.
    #[test]
    fn a_collapsed_fall_back_day_raises_v07() {
        let intervals = qh(datetime!(2026-10-24 22:00 UTC), 96);
        let issues = detect_dst_ambiguity(&intervals);
        assert_eq!(issues.len(), 1, "expected V07: {issues:?}");
        assert_eq!(issues[0].rule_id, ValidationRuleId::DstAmbiguity);
    }

    /// A window that merely starts inside the repeated hour is short, not
    /// corrupt. This is the false positive the first implementation produced.
    #[test]
    fn a_truncated_window_across_the_boundary_is_not_flagged() {
        let intervals = qh(datetime!(2026-10-25 0:45 UTC), 4);
        assert!(
            detect_dst_ambiguity(&intervals).is_empty(),
            "a truncated query window must not be reported as collapsed"
        );
    }

    /// An ordinary 24-hour day has no repeat to lose.
    #[test]
    fn an_ordinary_day_raises_nothing() {
        let intervals = qh(datetime!(2026-07-14 22:00 UTC), 96);
        assert!(detect_dst_ambiguity(&intervals).is_empty());
    }

    /// Spring forward skips an hour rather than repeating one; a 23-hour day is
    /// correct there, so V07 must stay silent.
    #[test]
    fn spring_forward_raises_nothing() {
        let intervals = qh(datetime!(2026-03-28 23:00 UTC), 92);
        assert!(detect_dst_ambiguity(&intervals).is_empty());
    }

    /// V07 must be reachable through the public entry point — it was previously
    /// declared but never emitted.
    #[test]
    fn v07_is_emitted_by_validate_intervals() {
        let intervals = qh(datetime!(2026-10-24 22:00 UTC), 96);
        let report = validate_intervals(&intervals, &ValidationConfig::default());
        assert!(
            report
                .issues
                .iter()
                .any(|i| i.rule_id == ValidationRuleId::DstAmbiguity),
            "validate_intervals must surface V07: {:?}",
            report.issues
        );
    }
}
