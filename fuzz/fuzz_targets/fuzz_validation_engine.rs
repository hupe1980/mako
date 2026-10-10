//! Fuzz target: `fuzz_validation_engine`
//!
//! Verifies that the metering validation engine never panics on arbitrary
//! interval sequences, regardless of timestamps, values, or quality flags.
//!
//! ## What this catches
//!
//! - Overflow/underflow in gap and run arithmetic
//! - Panic in the Hampel outlier test with extreme values
//! - DST boundary edge cases in the day's slot grid
//! - A `Report` whose findings do not partition across the three severities —
//!   a caller that triages by severity would silently drop a finding that
//!   blocks billing — or whose grade disagrees with its findings
//!
//! ## Run locally
//!
//! ```text
//! cargo +nightly fuzz run fuzz_validation_engine
//! ```

#![no_main]
#![deny(unsafe_code)]

use libfuzzer_sys::{Corpus, fuzz_target};
use metering::vee::validation::{Grade, Rules, Severity, validate};
use metering::{DayBoundary, MeterInterval, QualityFlag, Resolution, Series};
use rust_decimal::Decimal;
use time::OffsetDateTime;

fuzz_target!(|data: &[u8]| -> Corpus {
    if data.len() < 3 {
        return Corpus::Reject;
    }

    // Use first bytes to control interval count and quality
    let interval_count = (data[0] as usize % 48) + 1; // 1–48 intervals
    let quality_byte = data[1];
    let base_seconds = i64::from(u32::from_le_bytes(
        data.get(2..6)
            .and_then(|b| b.try_into().ok())
            .unwrap_or([0u8; 4]),
    ));

    let quality = match quality_byte % 8 {
        0 => QualityFlag::Measured,
        1 => QualityFlag::Estimated,
        2 => QualityFlag::Substituted,
        3 => QualityFlag::Calculated,
        4 => QualityFlag::Corrected,
        5 => QualityFlag::Preliminary,
        6 => QualityFlag::Faulty,
        _ => QualityFlag::Unknown,
    };

    // A quarter-hour-aligned base time under fuzz control.
    let base_ts = OffsetDateTime::UNIX_EPOCH.saturating_add(time::Duration::minutes(
        15 * ((base_seconds.abs() % (50 * 365 * 86400)) / 900),
    ));

    let intervals: Vec<MeterInterval> = (0..interval_count)
        .filter_map(|i| {
            let from = base_ts.checked_add(time::Duration::minutes(i as i64 * 15))?;
            // Use fuzz bytes for value; default to 0 if not enough bytes
            let val_bytes = data
                .get(6 + i * 4..6 + i * 4 + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
                .unwrap_or(0);
            let value = Decimal::new(i64::from(val_bytes), 3); // 0.000 .. 4294967.295
            MeterInterval::quarter_hour(from, value, quality).ok()
        })
        .collect();

    let Ok(series) = Series::new(Resolution::QUARTER_HOUR, DayBoundary::Strom, intervals) else {
        return Corpus::Reject;
    };
    let Some(day) = DayBoundary::Strom
        .day_of(base_ts)
        .and_then(|d| DayBoundary::Strom.day(d))
    else {
        return Corpus::Reject;
    };

    // Run validation — must never panic regardless of input
    let report = validate(&series, &Rules::strom(day, OffsetDateTime::now_utc(), None));

    // Every finding carries exactly one of the three severities: the
    // per-severity counts must add up to the whole finding set.
    let per_severity: usize = Severity::ALL
        .iter()
        .map(|s| {
            report
                .findings
                .iter()
                .filter(|f| f.severity() == *s)
                .count()
        })
        .sum();
    assert_eq!(
        report.findings.len(),
        per_severity,
        "finding count inconsistency"
    );
    assert_eq!(
        report.grade(),
        Grade::of(&report.findings),
        "the grade is a function of the findings"
    );

    Corpus::Keep
});
