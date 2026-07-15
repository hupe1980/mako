//! Property-based tests for the metering validation engine using `proptest`.
//!
//! These tests verify that validation invariants hold for any combination
//! of randomised interval values and configurations.

use metering::{
    MeterInterval, QualityFlag, ValidationConfig, ValidationRuleId, validate_intervals,
};
use proptest::prelude::*;
use rust_decimal::Decimal;
use time::macros::datetime;

// ── Strategies ────────────────────────────────────────────────────────────────

fn arb_decimal_kwh() -> impl Strategy<Value = Decimal> {
    (0u64..100_000).prop_map(|n| Decimal::new(n as i64, 3)) // 0.000 .. 100.000
}

fn arb_quality() -> impl Strategy<Value = QualityFlag> {
    prop_oneof![
        Just(QualityFlag::Measured),
        Just(QualityFlag::Estimated),
        Just(QualityFlag::Substituted),
        Just(QualityFlag::Calculated),
        Just(QualityFlag::Faulty),
        Just(QualityFlag::Unknown),
    ]
}

/// Build a contiguous sequence of N 15-min intervals starting at 2026-01-01T00:00Z.
fn make_intervals(kwhs: Vec<Decimal>, quality: QualityFlag) -> Vec<MeterInterval> {
    let base = datetime!(2026-01-01 0:00 UTC);
    kwhs.into_iter()
        .enumerate()
        .map(|(i, kwh)| MeterInterval {
            from: base + time::Duration::minutes(i as i64 * 15),
            to: base + time::Duration::minutes((i + 1) as i64 * 15),
            value_kwh: kwh,
            quality,
            obis_code: None,
        })
        .collect()
}

// ── Properties ────────────────────────────────────────────────────────────────

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// A clean series of measured, non-negative intervals produces no errors.
    #[test]
    fn clean_measured_series_has_no_errors(
        kwhs in prop::collection::vec(arb_decimal_kwh(), 1..48),
    ) {
        let intervals = make_intervals(kwhs, QualityFlag::Measured);
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        prop_assert!(
            !result.has_errors(),
            "clean measured series should have no errors: {:?}",
            result.issues
        );
    }

    /// Every faulty interval generates exactly one V09 non-billable error.
    #[test]
    fn faulty_intervals_each_trigger_v09(
        count in 1usize..=10,
    ) {
        let kwhs = vec![Decimal::new(25, 1); count]; // 2.5 kWh each
        let intervals = make_intervals(kwhs, QualityFlag::Faulty);
        let result = validate_intervals(&intervals, &ValidationConfig::default());

        let v09_count = result.issues.iter()
            .filter(|i| i.rule_id == ValidationRuleId::NonBillableQuality)
            .count();

        prop_assert_eq!(
            v09_count, count,
            "each faulty interval must trigger exactly one V09 issue"
        );
        prop_assert!(result.has_errors(), "faulty quality is always an error");
    }

    /// Negative energy always triggers V03 for non-bidirectional configs.
    #[test]
    fn negative_energy_triggers_v03(
        magnitude in 0.001f64..100.0,
    ) {
        let neg = -Decimal::try_from(magnitude).unwrap_or(Decimal::new(-1, 3));
        let base = datetime!(2026-01-01 0:00 UTC);
        let intervals = vec![MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: neg,
            quality: QualityFlag::Measured,
            obis_code: None,
        }];
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        let v03 = result.issues.iter().any(|i| i.rule_id == ValidationRuleId::NegativeEnergy);
        prop_assert!(v03, "negative energy must trigger V03");
    }

    /// Bidirectional config never triggers V03 regardless of negative values.
    #[test]
    fn bidirectional_config_never_v03(
        magnitude in 0.001f64..100.0,
    ) {
        let neg = -Decimal::try_from(magnitude).unwrap_or(Decimal::new(-1, 3));
        let base = datetime!(2026-01-01 0:00 UTC);
        let intervals = vec![MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: neg,
            quality: QualityFlag::Measured,
            obis_code: None,
        }];
        let config = ValidationConfig::bidirectional();
        let result = validate_intervals(&intervals, &config);
        let v03 = result.issues.iter().any(|i| i.rule_id == ValidationRuleId::NegativeEnergy);
        prop_assert!(!v03, "bidirectional config must never trigger V03");
    }

    /// Contiguous intervals with no gaps produce no V01 gap errors.
    #[test]
    fn contiguous_series_has_no_gaps(
        kwhs in prop::collection::vec(arb_decimal_kwh(), 2..=96),
    ) {
        let intervals = make_intervals(kwhs, QualityFlag::Measured);
        let result = validate_intervals(&intervals, &ValidationConfig::default());
        let gap_issues: Vec<_> = result.issues.iter()
            .filter(|i| i.rule_id == ValidationRuleId::GapDetected)
            .collect();
        prop_assert!(
            gap_issues.is_empty(),
            "contiguous series must have no V01 gaps: {:?}", gap_issues
        );
    }

    /// Validation is deterministic: same input always produces same output.
    #[test]
    fn validation_is_deterministic(
        kwhs in prop::collection::vec(arb_decimal_kwh(), 1..=48),
        quality in arb_quality(),
    ) {
        let intervals = make_intervals(kwhs, quality);
        let config = ValidationConfig::default();
        let r1 = validate_intervals(&intervals, &config);
        let r2 = validate_intervals(&intervals, &config);
        prop_assert_eq!(
            r1.issues.len(), r2.issues.len(),
            "same input must produce same issue count"
        );
    }
}
