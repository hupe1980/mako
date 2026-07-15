//! Fuzz target: `fuzz_validation_engine`
//!
//! Verifies that the metering validation engine (V01–V10) never panics on
//! arbitrary interval sequences, regardless of timestamps, values, or quality flags.
//!
//! ## What this catches
//!
//! - Overflow/underflow in gap duration arithmetic
//! - Panic in rolling-mean spike detection with extreme values
//! - DST boundary edge cases in V07
//! - Register rollover detection with NaN-like Decimal inputs
//!
//! ## Run locally
//!
//! ```text
//! cargo +nightly fuzz run fuzz_validation_engine
//! ```

#![no_main]

use libfuzzer_sys::{Corpus, fuzz_target};
use metering::{MeterInterval, QualityFlag, ValidationConfig, validate_intervals};
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
        data.get(2..6).and_then(|b| b.try_into().ok()).unwrap_or([0u8; 4])
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

    // Build a sequence of intervals starting from a fuzz-controlled base time
    let base_ts = OffsetDateTime::UNIX_EPOCH
        .saturating_add(time::Duration::seconds(base_seconds.abs() % (50 * 365 * 86400)));

    let intervals: Vec<MeterInterval> = (0..interval_count)
        .filter_map(|i| {
            let from = base_ts.checked_add(time::Duration::minutes(i as i64 * 15))?;
            let to = from.checked_add(time::Duration::minutes(15))?;
            // Use fuzz bytes for value; default to 0 if not enough bytes
            let val_bytes = data.get(6 + i * 4..6 + i * 4 + 4)
                .and_then(|b| b.try_into().ok())
                .map(u32::from_le_bytes)
                .unwrap_or(0);
            let value_kwh = Decimal::new(val_bytes as i64, 3); // 0.000 .. 4294967.295
            Some(MeterInterval { from, to, value_kwh, quality, obis_code: None })
        })
        .collect();

    if intervals.is_empty() {
        return Corpus::Reject;
    }

    // Run validation — must never panic regardless of input
    let config = ValidationConfig {
        now: Some(OffsetDateTime::now_utc()),
        ..ValidationConfig::default()
    };
    let result = validate_intervals(&intervals, &config);

    // Basic sanity: issue count must be consistent
    assert_eq!(
        result.issues.len(),
        result.by_severity(metering::ValidationSeverity::Error).count()
            + result.by_severity(metering::ValidationSeverity::Warning).count()
            + result.by_severity(metering::ValidationSeverity::Info).count(),
        "issue count inconsistency"
    );

    Corpus::Keep
});
