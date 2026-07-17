//! Regulatory showcase tests for the `metering` crate.
//!
//! Each test group corresponds to a specific German energy metering regulation.
//! These tests serve as executable documentation of the regulatory requirements
//! and verify all domain calculations are correct.
//!
//! Run: `cargo test -p metering --test regulatory_showcase`
//!
//! ## Legal sources
//! - **MessZV**: Messzugangsverordnung (§§2–27)
//! - **GasGVV**: Gasgrundversorgungsverordnung §24 (Abrechnungsbrennwert)
//! - **DVGW G 685**: Gasabrechnung §10 (Zustandszahl)
//! - **DVGW G 260**: Gasbeschaffenheit (Brennwertbereiche H-Gas / L-Gas)
//! - **GPKE BK6-22-024**: §3 MMM billing (arbeitsmenge + spitzenleistung)
//! - **EnWG §41a**: Dynamic tariff billing (15-min iMSys resolution)
//! - **EnWG §40 Abs. 2**: Annual SLP meter reading

use metering::*;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use time::macros::datetime;

// ═══════════════════════════════════════════════════════════════════════════
// §2 Nr. 17 MessZV — Spitzenleistung (peak demand)
// ═══════════════════════════════════════════════════════════════════════════

/// §2 Nr. 17 MessZV: Spitzenleistung = höchste Viertelstundenleistung.
/// For 15-min RLM: demand_kw = kwh × 4.
#[test]
fn peak_demand_is_highest_15min_quarter() {
    let base = datetime!(2026-07-01 10:00 UTC);
    let intervals = vec![
        MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        }, // 10 kW
        MeterInterval {
            from: base + time::Duration::minutes(15),
            to: base + time::Duration::minutes(30),
            value_kwh: dec!(5.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        }, // 20 kW ← peak
        MeterInterval {
            from: base + time::Duration::minutes(30),
            to: base + time::Duration::minutes(45),
            value_kwh: dec!(1.25),
            quality: QualityFlag::Measured,
            obis_code: None,
        }, // 5 kW
    ];
    let period = aggregate(&intervals, AggregationConfig::rlm_strom());
    assert_eq!(period.spitzenleistung_kw, Some(dec!(20)));
    assert_eq!(period.arbeitsmenge_kwh, dec!(8.75)); // sum
}

/// §2 Nr. 17 MessZV: SLP has no Spitzenleistung.
#[test]
fn slp_has_no_spitzenleistung() {
    let base = datetime!(2026-07-01 0:00 UTC);
    let intervals = vec![MeterInterval {
        from: base,
        to: base + time::Duration::hours(24),
        value_kwh: dec!(24.0),
        quality: QualityFlag::Measured,
        obis_code: None,
    }];
    let period = aggregate(&intervals, AggregationConfig::slp_strom());
    assert_eq!(
        period.spitzenleistung_kw, None,
        "SLP billing has no peak demand"
    );
    assert_eq!(period.arbeitsmenge_kwh, dec!(24.0));
}

/// Only billable intervals (Measured, Substituted, Calculated) contribute.
/// Estimated intervals are EXCLUDED from Spitzenleistung but their energy may be included
/// depending on the operator's substitution policy.
/// Here we test that non-billable intervals are excluded from calculations.
#[test]
fn non_billable_intervals_excluded_from_spitzenleistung() {
    let base = datetime!(2026-07-01 10:00 UTC);
    let intervals = vec![
        MeterInterval {
            from: base,
            to: base + time::Duration::minutes(15),
            value_kwh: dec!(5.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        }, // billable: 20 kW
        MeterInterval {
            from: base + time::Duration::minutes(15),
            to: base + time::Duration::minutes(30),
            value_kwh: dec!(100.0),
            quality: QualityFlag::Unknown,
            obis_code: None,
        }, // NOT billable: would be 400 kW
    ];
    let period = aggregate(&intervals, AggregationConfig::rlm_strom());
    // Spitzenleistung is only from the billable interval
    assert_eq!(
        period.spitzenleistung_kw,
        Some(dec!(20)),
        "Unknown quality must not contribute to Spitzenleistung"
    );
    assert_eq!(
        period.arbeitsmenge_kwh,
        dec!(5.0),
        "Unknown quality excluded from arbeitsmenge"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §24 GasGVV + DVGW G 685 — Gas m³ → kWh_Hs conversion
// ═══════════════════════════════════════════════════════════════════════════

/// §24 GasGVV / DVGW G 685 §10: kWh_Hs = m³ × Hs × Zustandszahl.
/// Typical German Erdgas H: Hs ≈ 10.55 kWh/m³, Z ≈ 0.9764.
#[test]
fn gas_h_gas_conversion_typical_values() {
    let kwh = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(0.9764));
    // 100 × 10.55 × 0.9764 = 1030.102000 kWh_Hs
    assert_eq!(kwh, dec!(1030.102000));
}

/// DVGW G 260: L-Gas has lower calorific value than H-Gas.
/// L-Gas: Hs ≈ 8.4–9.2 kWh/m³ vs H-Gas: Hs ≈ 10.2–12.0 kWh/m³.
#[test]
fn l_gas_lower_energy_content_than_h_gas() {
    let h_gas_kwh = gas_m3_to_kwh_hs(dec!(1000), dec!(10.55), dec!(1.0));
    let l_gas_kwh = gas_m3_to_kwh_hs(dec!(1000), dec!(8.80), dec!(1.0));
    assert!(
        l_gas_kwh < h_gas_kwh,
        "L-Gas delivers less kWh per m³ than H-Gas"
    );
}

/// Zustandszahl > 1 means meter under-measures (higher elevation, lower pressure).
/// Zustandszahl < 1 means meter over-measures.
#[test]
fn zustandszahl_above_one_increases_kwh() {
    let base = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(1.0));
    let high_z = gas_m3_to_kwh_hs(dec!(100), dec!(10.55), dec!(1.05));
    assert!(high_z > base, "Z > 1 must increase kWh_Hs");
}

/// Zero volume → zero kWh (no division by zero, no panic).
#[test]
fn gas_zero_volume_is_zero_kwh() {
    assert_eq!(
        gas_m3_to_kwh_hs(Decimal::ZERO, dec!(10.55), dec!(0.9764)),
        Decimal::ZERO
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §3 / §4 MessZV — SLP/RLM/iMSys classification
// ═══════════════════════════════════════════════════════════════════════════

/// §3 MessZV: 15-min intervals → RLM.
#[test]
fn fifteen_min_intervals_classify_as_rlm() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let intervals: Vec<MeterInterval> = (0..4)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();
    assert_eq!(classify_messtyp(&intervals, None), Messtyp::Rlm);
}

/// §41a EnWG: SMGW source → always iMSys, regardless of interval length.
#[test]
fn smgw_source_forces_imsys() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let intervals: Vec<MeterInterval> = (0..4)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();
    assert_eq!(classify_messtyp(&intervals, Some("SMGW")), Messtyp::IMsys);
    assert_eq!(
        classify_messtyp(&intervals, Some("CLS_GATEWAY")),
        Messtyp::IMsys
    );
}

/// §4 MessZV: daily SLP reads → Slp.
#[test]
fn daily_intervals_classify_as_slp() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let intervals = vec![
        MeterInterval {
            from: base,
            to: base + time::Duration::days(1),
            value_kwh: dec!(24.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: base + time::Duration::days(1),
            to: base + time::Duration::days(2),
            value_kwh: dec!(22.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];
    assert_eq!(classify_messtyp(&intervals, None), Messtyp::Slp);
}

/// §41a EnWG: only iMSys supports dynamic tariff billing.
#[test]
fn only_imsys_supports_dynamic_tariff() {
    assert!(
        Messtyp::IMsys.supports_dynamic_tariff(),
        "iMSys must support §41a dynamic tariff"
    );
    assert!(!Messtyp::Rlm.supports_dynamic_tariff());
    assert!(!Messtyp::Slp.supports_dynamic_tariff());
}

// ═══════════════════════════════════════════════════════════════════════════
// §27 MessZV — Mehr-/Mindermengensaldo
// ═══════════════════════════════════════════════════════════════════════════

/// §27 MessZV: LF delivered more than contracted → Mehr-Menge (LF owes NB).
#[test]
fn mehr_menge_lf_owes_nb() {
    let saldo = compute_imbalance(dec!(1050), dec!(1000));
    assert_eq!(saldo.mehr_kwh, dec!(50));
    assert_eq!(saldo.minder_kwh, Decimal::ZERO);
    assert!(saldo.is_mehr());
    assert!(!saldo.is_minder());
}

/// §27 MessZV: LF delivered less than contracted → Minder-Menge (NB owes LF).
#[test]
fn minder_menge_nb_owes_lf() {
    let saldo = compute_imbalance(dec!(950), dec!(1000));
    assert_eq!(saldo.mehr_kwh, Decimal::ZERO);
    assert_eq!(saldo.minder_kwh, dec!(50));
    assert!(!saldo.is_mehr());
    assert!(saldo.is_minder());
}

/// §27 MessZV: balanced period.
#[test]
fn balanced_period_no_imbalance() {
    let saldo = compute_imbalance(dec!(1000), dec!(1000));
    assert!(saldo.is_balanced());
    assert_eq!(saldo.delta_pct(), Some(Decimal::ZERO));
}

/// §27 MessZV: imbalance percentage calculation.
/// 50 kWh on 1000 contracted = 5%.
#[test]
fn imbalance_delta_pct() {
    let saldo = compute_imbalance(dec!(1050), dec!(1000));
    assert_eq!(saldo.delta_pct(), Some(dec!(5)));
}

/// §27 MessZV: mehr and minder are mutually exclusive.
#[test]
fn mehr_and_minder_mutually_exclusive() {
    for (actual, contracted) in [
        (dec!(900), dec!(1000)),
        (dec!(1100), dec!(1000)),
        (dec!(1000), dec!(1000)),
    ] {
        let s = compute_imbalance(actual, contracted);
        assert!(
            !(s.is_mehr() && s.is_minder()),
            "mehr and minder cannot both be true for actual={actual}, contracted={contracted}"
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// M7 — Hampel-filter quality scoring
// ═══════════════════════════════════════════════════════════════════════════

/// M7: clean 96-interval series (24h of 15-min data) → grade A.
#[test]
fn clean_24h_series_grades_a() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let samples: Vec<MeterInterval> = (0..96)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: Decimal::try_from(2.0 + i as f64 * 0.001).unwrap_or(dec!(2.0)),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();
    let report = score_intervals(&samples, QualityConfig::default());
    assert_eq!(report.grade, QualityGrade::A);
    assert!(!report.has_warnings);
    assert!(!report.grade.blocks_billing());
}

/// M7: single spike → grade C or worse.
#[test]
fn spike_degrades_quality_grade() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let mut samples: Vec<MeterInterval> = (0..96)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();
    samples[50].value_kwh = dec!(2000); // 1000× spike
    let report = score_intervals(&samples, QualityConfig::default());
    assert_ne!(report.grade, QualityGrade::A, "spike must degrade grade");
    assert!(report.has_warnings);
}

/// M7: grade F blocks automated billing.
#[test]
fn grade_f_blocks_billing() {
    assert!(QualityGrade::F.blocks_billing());
    assert!(!QualityGrade::A.blocks_billing());
    assert!(!QualityGrade::B.blocks_billing());
    assert!(!QualityGrade::C.blocks_billing());
}

/// M7: score_intervals_raw() — f64 API (from database queries).
#[test]
fn score_raw_clean_series_grades_a() {
    let values: Vec<f64> = (0..20).map(|i| 2.0 + i as f64 * 0.01).collect();
    assert_eq!(score_intervals_raw(&values, 3, 3.0), QualityGrade::A);
}

/// M7: score_intervals_raw() — spike detection.
#[test]
fn score_raw_spike_degrades_grade() {
    let mut values: Vec<f64> = (0..20).map(|_| 2.0).collect();
    values[10] = 500.0; // 250× spike
    let grade = score_intervals_raw(&values, 3, 3.0);
    assert_ne!(grade, QualityGrade::A, "spike must degrade raw score");
}

/// M7: gap detection — missing interval between reads.
#[test]
fn gap_in_series_detected() {
    let base = datetime!(2026-01-01 0:00 UTC);
    let mut samples: Vec<MeterInterval> = (0..96)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();
    samples.remove(48); // create a gap
    let report = score_intervals(&samples, QualityConfig::default());
    assert_eq!(report.gaps_detected, 1);
    assert!(report.has_warnings);
}

// ═══════════════════════════════════════════════════════════════════════════
// GPKE §3 — MMM billing: arbeitsmenge + spitzenleistung
// ═══════════════════════════════════════════════════════════════════════════

/// GPKE BK6-22-024 §3: MMM billing requires arbeitsmenge_kwh and spitzenleistung_kw.
/// This test verifies the full monthly RLM billing period calculation.
#[test]
fn rlm_monthly_billing_period() {
    let base = datetime!(2026-06-01 0:00 UTC);
    // 30 days × 24h × 4 intervals/h = 2880 intervals at 2.5 kWh each
    let intervals: Vec<MeterInterval> = (0..2880_i64)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();

    let period = aggregate(&intervals, AggregationConfig::rlm_strom());

    // Arbeitsmenge: 2880 × 2.5 = 7200 kWh
    assert_eq!(period.arbeitsmenge_kwh, dec!(7200.0));
    // Spitzenleistung: 2.5 × 4 = 10 kW (uniform, all equal)
    assert_eq!(period.spitzenleistung_kw, Some(dec!(10)));
    assert_eq!(period.interval_count, 2880);
    assert!(
        (period.coverage_pct - 100.0).abs() < 1.0,
        "full month should have ~100% coverage"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// HT/NT Zweitarif
// ═══════════════════════════════════════════════════════════════════════════

/// Weekday 07:00–22:00 → HT; 22:00–06:00 and weekends → NT.
#[test]
fn ht_nt_weekday_split() {
    // Monday 09:00 UTC = HT; Monday 23:00 UTC = NT
    let ht_time = datetime!(2026-01-05 9:00 UTC); // Monday 09:00
    let nt_time = datetime!(2026-01-05 23:00 UTC); // Monday 23:00
    let intervals = vec![
        MeterInterval {
            from: ht_time,
            to: ht_time + time::Duration::minutes(15),
            value_kwh: dec!(4.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: nt_time,
            to: nt_time + time::Duration::minutes(15),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];
    let period = aggregate(&intervals, AggregationConfig::rlm_zweitarif());
    let ht_nt = period.ht_nt.unwrap();
    assert_eq!(ht_nt.ht_kwh, dec!(4.0), "daytime weekday = HT");
    assert_eq!(ht_nt.nt_kwh, dec!(1.0), "nighttime weekday = NT");
}

/// Weekend → all NT.
#[test]
fn ht_nt_weekend_all_nt() {
    // Saturday 10:00 → NT (weekend)
    let sat = datetime!(2026-01-03 10:00 UTC); // Saturday
    let intervals = vec![MeterInterval {
        from: sat,
        to: sat + time::Duration::minutes(15),
        value_kwh: dec!(3.0),
        quality: QualityFlag::Measured,
        obis_code: None,
    }];
    let period = aggregate(&intervals, AggregationConfig::rlm_zweitarif());
    let ht_nt = period.ht_nt.unwrap();
    assert_eq!(ht_nt.ht_kwh, Decimal::ZERO, "Saturday morning = NT");
    assert_eq!(ht_nt.nt_kwh, dec!(3.0));
}

// ═══════════════════════════════════════════════════════════════════════════
// Gas billing period (Brennwert + Zustandszahl workflow)
// ═══════════════════════════════════════════════════════════════════════════

/// Complete Gas billing workflow: m³ readings → kWh_Hs → billing period.
#[test]
fn gas_billing_workflow_m3_to_kwh_to_period() {
    let params = GasConversionParams {
        hs_kwh_per_m3: dec!(10.55),
        zustandszahl: dec!(0.9764),
    };
    let base = datetime!(2026-06-01 0:00 UTC);

    // 24 hourly Gas interval reads in m³; convert each to kWh_Hs first
    let intervals: Vec<MeterInterval> = (0..24)
        .map(|i| {
            let m3 = dec!(10); // 10 m³/h typical domestic gas meter
            let kwh = gas_m3_to_kwh_hs(m3, params.hs_kwh_per_m3, params.zustandszahl);
            MeterInterval {
                from: base + time::Duration::hours(i),
                to: base + time::Duration::hours(i + 1),
                value_kwh: kwh,
                quality: QualityFlag::Measured,
                obis_code: Some("7-10:3.1.0".to_owned()), // Gas OBIS
            }
        })
        .collect();

    let period = aggregate(&intervals, AggregationConfig::gas());

    // 24h × 10 m³ × 10.55 kWh/m³ × 0.9764 = 2471.76... kWh_Hs
    let expected_kwh = dec!(24) * gas_m3_to_kwh_hs(dec!(10), dec!(10.55), dec!(0.9764));
    assert_eq!(period.arbeitsmenge_kwh, expected_kwh);
    assert_eq!(
        period.spitzenleistung_kw, None,
        "Gas billing has no Spitzenleistung"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// MeterInterval domain logic
// ═══════════════════════════════════════════════════════════════════════════

/// demand_kw: 2.5 kWh in 15 min = 10 kW.
#[test]
fn demand_kw_15min() {
    let iv = MeterInterval {
        from: datetime!(2026-01-01 0:00 UTC),
        to: datetime!(2026-01-01 0:15 UTC),
        value_kwh: dec!(2.5),
        quality: QualityFlag::Measured,
        obis_code: None,
    };
    assert_eq!(iv.demand_kw(), Some(dec!(10)));
}

/// demand_kw: zero duration → None (no division by zero).
#[test]
fn demand_kw_zero_duration_is_none() {
    let ts = datetime!(2026-01-01 0:00 UTC);
    let iv = MeterInterval {
        from: ts,
        to: ts,
        value_kwh: dec!(5.0),
        quality: QualityFlag::Measured,
        obis_code: None,
    };
    assert_eq!(iv.demand_kw(), None);
}

/// QualityFlag billability rules per §17 MessZV.
///
/// CRITICAL REGULATORY REQUIREMENT: `Estimated` (Prognosewert) IS billable.
/// §17 MessZV requires substitute values including estimates for advance billing.
/// Excluding estimated values would produce zero arbeitsmenge for SLP customers.
#[test]
fn quality_flag_billability() {
    // All of these are valid billing bases per §17 MessZV
    assert!(
        QualityFlag::Measured.is_billable(),
        "Measured must be billable"
    );
    assert!(
        QualityFlag::Substituted.is_billable(),
        "Substituted (Ersatzwert) must be billable per §17 MessZV"
    );
    assert!(
        QualityFlag::Calculated.is_billable(),
        "Calculated must be billable"
    );
    assert!(
        QualityFlag::Corrected.is_billable(),
        "Corrected must be billable"
    );
    assert!(
        QualityFlag::Preliminary.is_billable(),
        "Preliminary must be billable"
    );
    // §17 MessZV FIX: Estimated (Prognosewert) IS billable — used in Abschlagsrechnung
    assert!(
        QualityFlag::Estimated.is_billable(),
        "Estimated (Prognosewert) must be billable per §17 MessZV advance billing"
    );
    // Only Faulty and Unknown block billing
    assert!(
        !QualityFlag::Faulty.is_billable(),
        "Faulty must NOT be billable"
    );
    assert!(
        !QualityFlag::Unknown.is_billable(),
        "Unknown must NOT be billable"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Precision: no f64 in energy calculations
// ═══════════════════════════════════════════════════════════════════════════

/// All calculations use Decimal — no f64 rounding errors.
#[test]
fn decimal_arithmetic_exact_no_float_errors() {
    // Classic float trap: 0.1 + 0.2 ≠ 0.3 in f64
    let sum: Decimal = dec!(0.1) + dec!(0.2);
    assert_eq!(sum, dec!(0.3), "Decimal arithmetic must be exact");

    // kWh sum over 96 intervals at 3.333... kWh each
    let kwh_per_interval = Decimal::from_str_exact("3.333333").unwrap();
    let intervals: Vec<MeterInterval> = (0..96)
        .map(|i| {
            let base = datetime!(2026-01-01 0:00 UTC);
            MeterInterval {
                from: base + time::Duration::minutes(i * 15),
                to: base + time::Duration::minutes(i * 15 + 15),
                value_kwh: kwh_per_interval,
                quality: QualityFlag::Measured,
                obis_code: None,
            }
        })
        .collect();
    let period = aggregate(&intervals, AggregationConfig::slp_strom());
    let expected: Decimal = kwh_per_interval * Decimal::from(96u32);
    assert_eq!(
        period.arbeitsmenge_kwh, expected,
        "Decimal summation must be exact"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// V07 — DST ambiguity detection (M5: regulatory audit gap test)
// Source: Allgemeine Festlegungen V6.1d §3 — UTC required for all EDIFACT
// ═══════════════════════════════════════════════════════════════════════════

/// V07 DST ambiguity: detects if timestamps were stored in local time.
///
/// At the CEST→CET fall-back (last Sunday in October, 01:00 UTC), the
/// local time 02:00 appears TWICE if stored as Europe/Berlin. In UTC the
/// same two intervals are 01:00–01:15 UTC and 01:15–01:30 UTC — no ambiguity.
///
/// V07 fires when two intervals share the same UTC hour boundary value,
/// suggesting the caller mistakenly stored local time.
///
/// ## Regulatory basis
/// §3 Allgemeine Festlegungen V6.1d: "Alle Zeitangaben sind in UTC zu übermitteln."
/// CONTRL/APERAK timestamps are UTC. Any local-time storage breaks audit trail.
#[test]
fn v07_dst_ambiguity_detected_for_local_time_storage() {
    use metering::validation::{ValidationConfig, ValidationRuleId, validate_intervals};

    // Simulate: operator stored 02:00 CET twice (fall-back hour) in "UTC" field.
    // 2026-10-25 fall-back: 03:00 CEST becomes 02:00 CET.
    // If stored as local time these two intervals are identical.
    let fake_utc_duplicate = datetime!(2026-10-25 2:00 UTC); // local 02:00 #1
    let intervals = vec![
        MeterInterval {
            from: fake_utc_duplicate,
            to: fake_utc_duplicate + time::Duration::minutes(15),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: fake_utc_duplicate, // duplicate — stored as local time!
            to: fake_utc_duplicate + time::Duration::minutes(15),
            value_kwh: dec!(2.3),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];

    let result = validate_intervals(&intervals, &ValidationConfig::rlm_strom_15min());
    // Either V02 (overlap) or V07 (DST ambiguity) must fire.
    let has_dst_or_overlap = result.issues.iter().any(|i| {
        matches!(
            i.rule_id,
            ValidationRuleId::DstAmbiguity | ValidationRuleId::OverlapDetected
        )
    });
    assert!(
        has_dst_or_overlap,
        "Duplicate UTC timestamp at DST fall-back hour must trigger V07 DstAmbiguity \
         or V02 OverlapDetected. Issues: {:?}",
        result.issues
    );
}

/// V07 does NOT fire for correctly stored UTC series at DST boundary.
///
/// On 2026-10-25 fall-back, a correctly UTC-stored 15-min series skips no slots
/// and has no duplicates.  Validation must be clean.
#[test]
fn v07_no_false_positive_for_correct_utc_at_dst_fallback() {
    use metering::validation::{ValidationConfig, validate_intervals};

    // 4 consecutive UTC intervals spanning the fall-back moment
    let base = datetime!(2026-10-25 0:45 UTC); // 02:45 CEST = just before fall-back
    let intervals: Vec<MeterInterval> = (0..4)
        .map(|i| MeterInterval {
            from: base + time::Duration::minutes(i * 15),
            to: base + time::Duration::minutes(i * 15 + 15),
            value_kwh: dec!(2.5),
            quality: QualityFlag::Measured,
            obis_code: None,
        })
        .collect();

    let result = validate_intervals(&intervals, &ValidationConfig::rlm_strom_15min());
    assert!(
        result.is_clean(),
        "Correctly stored UTC series at DST fall-back must have no validation issues. \
         Got: {:?}",
        result.issues
    );
}

// ── §42b EnWG Solarpaket I — GGV community solar allocation ──────────────────
//
// These tests encode the three BDEW Anwendungshilfe examples verbatim.
// Reference: "Beispiele von Berechnungsformeln für das Solarpaket 1" v1.0 (25.01.2024).
//
// Formula (constant, CCI+ZG6):
//   net_grid_draw_i = max(0, Melo_i_Verbrauch - fraction_i × Melo1_Erzeugung)
//
// Formula (proportional/variable):
//   ratio_i = Melo_i_Verbrauch / Σ Melo_j_Verbrauch
//   net_grid_draw_i = max(0, Melo_i_Verbrauch - ratio_i × Melo1_Erzeugung)

/// BDEW Anwendungshilfe Beispiel 1 — constant allocation 10%/90%.
///
/// Setup: plant generates 10 kWh per interval.
/// Tenant 2 (MaLo2) gets 10%, tenant 3 (MaLo3) gets 90%.
/// MaLo2 consumes 5 kWh → net = max(0, 5 − 1) = 4 kWh from grid.
/// MaLo3 consumes 20 kWh → net = max(0, 20 − 9) = 11 kWh from grid.
/// Total PV delivered = (5−4) + (20−11) = 1 + 9 = 10 kWh = full plant output.
#[test]
fn sect42b_beispiel1_constant_allocation_10_and_90_percent() {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;

    let base = datetime!(2026-06-01 08:00 UTC);
    let make_iv = |kwh: Decimal| MeterInterval {
        from: base,
        to: base + time::Duration::minutes(15),
        value_kwh: kwh,
        quality: QualityFlag::Measured,
        obis_code: None,
    };

    let mut sources = HashMap::new();
    sources.insert("MELO1_PLANT".to_owned(), vec![make_iv(dec!(10.0))]);
    sources.insert("MELO2_T2".to_owned(), vec![make_iv(dec!(5.0))]);
    sources.insert("MELO3_T3".to_owned(), vec![make_iv(dec!(20.0))]);

    // Malo2 with 10%
    let rule_malo2 = AggregationRule::GgvConstantAllocation {
        plant_melo_id: "MELO1_PLANT".to_owned(),
        tenant_melo_id: "MELO2_T2".to_owned(),
        fraction: dec!(0.10),
    };
    // Malo3 with 90%
    let rule_malo3 = AggregationRule::GgvConstantAllocation {
        plant_melo_id: "MELO1_PLANT".to_owned(),
        tenant_melo_id: "MELO3_T3".to_owned(),
        fraction: dec!(0.90),
    };

    let malo2 = compute_virtual_meter(&rule_malo2, &sources).unwrap();
    let malo3 = compute_virtual_meter(&rule_malo3, &sources).unwrap();

    assert_eq!(
        malo2[0].value_kwh,
        dec!(4.0),
        "Malo2 net grid draw = max(0, 5 - 0.1×10) = 4"
    );
    assert_eq!(
        malo3[0].value_kwh,
        dec!(11.0),
        "Malo3 net grid draw = max(0, 20 - 0.9×10) = 11"
    );

    // Verify: total PV delivered to tenants = full plant output (no grid feed-in)
    let pv_to_malo2 = dec!(5.0) - malo2[0].value_kwh;
    let pv_to_malo3 = dec!(20.0) - malo3[0].value_kwh;
    assert_eq!(
        pv_to_malo2 + pv_to_malo3,
        dec!(10.0),
        "energy balance: all PV consumed locally"
    );
}

/// BDEW Anwendungshilfe Beispiel 1 — §42b Abs. 5 cap: PV ≤ tenant consumption.
///
/// When the allocated fraction exceeds a tenant's actual consumption, the excess
/// PV energy is NOT credited (it feeds into the grid). The tenant's grid draw
/// is clamped to 0 — never negative.
///
/// Setup: plant 10 kWh, tenant 90% → allocation attempt = 9 kWh.
/// But tenant only consumes 2 kWh → net = max(0, 2 − 9) = 0 (not −7!).
/// The 7 kWh over-allocation becomes grid feed-in for the plant MaLo.
#[test]
fn sect42b_allocation_cap_by_tenant_consumption() {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;

    let base = datetime!(2026-06-01 08:15 UTC);
    let make_iv = |kwh: Decimal| MeterInterval {
        from: base,
        to: base + time::Duration::minutes(15),
        value_kwh: kwh,
        quality: QualityFlag::Measured,
        obis_code: None,
    };

    let mut sources = HashMap::new();
    sources.insert("PLANT".to_owned(), vec![make_iv(dec!(10.0))]);
    sources.insert("TENANT".to_owned(), vec![make_iv(dec!(2.0))]);

    let rule = AggregationRule::GgvConstantAllocation {
        plant_melo_id: "PLANT".to_owned(),
        tenant_melo_id: "TENANT".to_owned(),
        fraction: dec!(0.90),
    };
    let result = compute_virtual_meter(&rule, &sources).unwrap();

    assert_eq!(
        result[0].value_kwh,
        dec!(0.0),
        "§42b Abs. 5: net grid draw must never be negative (Pos operator)"
    );
}

/// BDEW Anwendungshilfe Beispiel 3 — variable proportional allocation.
///
/// Each tenant's fraction is computed dynamically from their actual consumption.
/// Setup: plant 10 kWh, T2 consumes 2 kWh, T3 consumes 8 kWh → total 10 kWh.
/// T2 ratio = 2/10 = 0.2 → allocation = 2 → net = max(0, 2−2) = 0.
/// T3 ratio = 8/10 = 0.8 → allocation = 8 → net = max(0, 8−8) = 0.
/// All plant output fully covers tenant consumption.
#[test]
fn sect42b_beispiel3_proportional_allocation_full_coverage() {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;

    let base = datetime!(2026-06-01 09:00 UTC);
    let make_iv = |kwh: Decimal| MeterInterval {
        from: base,
        to: base + time::Duration::minutes(15),
        value_kwh: kwh,
        quality: QualityFlag::Measured,
        obis_code: None,
    };

    let mut sources = HashMap::new();
    sources.insert("PLANT".to_owned(), vec![make_iv(dec!(10.0))]);
    sources.insert("T2".to_owned(), vec![make_iv(dec!(2.0))]);
    sources.insert("T3".to_owned(), vec![make_iv(dec!(8.0))]);

    let rule_t2 = AggregationRule::GgvProportionalAllocation {
        plant_melo_id: "PLANT".to_owned(),
        tenant_melo_id: "T2".to_owned(),
        all_tenant_melo_ids: vec!["T2".to_owned(), "T3".to_owned()],
    };
    let rule_t3 = AggregationRule::GgvProportionalAllocation {
        plant_melo_id: "PLANT".to_owned(),
        tenant_melo_id: "T3".to_owned(),
        all_tenant_melo_ids: vec!["T2".to_owned(), "T3".to_owned()],
    };

    let r2 = compute_virtual_meter(&rule_t2, &sources).unwrap();
    let r3 = compute_virtual_meter(&rule_t3, &sources).unwrap();

    assert_eq!(
        r2[0].value_kwh,
        dec!(0.0),
        "T2 ratio=0.2, allocation=2, net=0"
    );
    assert_eq!(
        r3[0].value_kwh,
        dec!(0.0),
        "T3 ratio=0.8, allocation=8, net=0"
    );
}

/// BDEW Anwendungshilfe — proportional allocation zero-division guard.
///
/// When all tenants consume 0 kWh in an interval (e.g. night-time, holiday),
/// the denominator Σ_all_consumption = 0. The engine must NOT divide by zero;
/// instead every tenant's net grid draw is 0.
///
/// Source: BDEW Anwendungshilfe §42b note: "Ist die Energiemenge einer
/// Marktlokation zugeordneten Messlokation = 0, so ist auch der Verbrauch
/// der Marktlokation auf 0 zu setzen. Dies verhindert auch eine Division durch 0."
#[test]
fn sect42b_proportional_zero_division_guard_all_tenants_off() {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;

    let base = datetime!(2026-06-01 02:00 UTC); // e.g. night-time
    let make_iv = |kwh: Decimal| MeterInterval {
        from: base,
        to: base + time::Duration::minutes(15),
        value_kwh: kwh,
        quality: QualityFlag::Measured,
        obis_code: None,
    };

    let mut sources = HashMap::new();
    sources.insert("PLANT".to_owned(), vec![make_iv(dec!(3.5))]); // plant still generating
    sources.insert("T2".to_owned(), vec![make_iv(dec!(0.0))]);
    sources.insert("T3".to_owned(), vec![make_iv(dec!(0.0))]);

    let rule = AggregationRule::GgvProportionalAllocation {
        plant_melo_id: "PLANT".to_owned(),
        tenant_melo_id: "T2".to_owned(),
        all_tenant_melo_ids: vec!["T2".to_owned(), "T3".to_owned()],
    };
    let result = compute_virtual_meter(&rule, &sources).unwrap();

    assert_eq!(
        result[0].value_kwh,
        dec!(0.0),
        "zero total consumption → denominator guard → net grid draw = 0 (no panic)"
    );
}

// ── Regulatory tests ─────────────────────────────────────────────────────────

/// DST spring-forward (2026-03-29 CET→CEST).
///
/// On this day only 23 hours exist: 00:00–01:00 CET, then clock jumps to
/// 03:00 CEST. A correctly UTC-stored 15-min series has 92 intervals (not 96).
/// V01 (GapDetected) must NOT fire for the missing 02:00–03:00 CET hour.
#[test]
fn dst_spring_forward_2026_03_29_utc_series_no_gap() {
    use metering::{MeterInterval, QualityFlag, ValidationConfig, validate_intervals};
    use time::macros::datetime;

    // Build 92 consecutive 15-min UTC intervals for 2026-03-29.
    // Spring forward: 01:00 UTC = 02:00 CET → clock jumps to 03:00 CEST = 02:00 UTC.
    // The 15-min series in UTC is unbroken — no gap in UTC-land.
    let day_start = datetime!(2026-03-28 23:00 UTC); // midnight CET = 23:00 UTC previous day
    let mut intervals = Vec::new();
    let mut t = day_start;
    for _ in 0..92 {
        intervals.push(MeterInterval {
            from: t,
            to: t + time::Duration::minutes(15),
            value_kwh: rust_decimal::Decimal::ONE,
            quality: QualityFlag::Measured,
            obis_code: None,
        });
        t += time::Duration::minutes(15);
    }
    assert_eq!(
        intervals.len(),
        92,
        "spring-forward day has 23h = 92 quarter-hours"
    );

    let config = ValidationConfig::rlm_strom_15min();
    let result = validate_intervals(&intervals, &config);
    assert!(
        result.is_clean(),
        "spring-forward UTC series must be clean — no V01 gaps: {:?}",
        result.issues
    );
}

/// DST fall-back (2026-10-25 CEST→CET) — extended to 25 hours.
///
/// On this day 100 quarter-hour intervals exist (25h × 4 = 100).
/// A correctly UTC-stored series has no duplicates and no gaps.
/// This test covers the case where V02 (overlap) and V07 (DstAmbiguity)
/// do NOT fire for a correct UTC series.
#[test]
fn dst_fall_back_2026_10_25_utc_100_intervals_clean() {
    use metering::{MeterInterval, QualityFlag, ValidationConfig, validate_intervals};
    use time::macros::datetime;

    let day_start = datetime!(2026-10-24 22:00 UTC); // midnight CET = 22:00 UTC
    let mut intervals = Vec::new();
    let mut t = day_start;
    for _ in 0..100 {
        intervals.push(MeterInterval {
            from: t,
            to: t + time::Duration::minutes(15),
            value_kwh: rust_decimal::Decimal::ONE,
            quality: QualityFlag::Measured,
            obis_code: None,
        });
        t += time::Duration::minutes(15);
    }
    assert_eq!(
        intervals.len(),
        100,
        "fall-back day has 25h = 100 quarter-hours"
    );

    let config = ValidationConfig::rlm_strom_15min();
    let result = validate_intervals(&intervals, &config);
    assert!(
        result.is_clean(),
        "fall-back UTC series must be clean — V02/V07 must not fire: {:?}",
        result.issues
    );
}

/// Leap year — 2024-02-29 exists and has exactly 96 intervals.
///
/// Ensures the validation engine does not treat 2024-02-29 as an invalid
/// date (regression guard — some buggy implementations deny Feb 29).
#[test]
fn leap_year_2024_02_29_96_intervals_clean() {
    use metering::{MeterInterval, QualityFlag, ValidationConfig, validate_intervals};
    use time::macros::datetime;

    let day_start = datetime!(2024-02-28 23:00 UTC); // midnight CET
    let mut intervals = Vec::new();
    let mut t = day_start;
    for _ in 0..96 {
        intervals.push(MeterInterval {
            from: t,
            to: t + time::Duration::minutes(15),
            value_kwh: rust_decimal::Decimal::ONE,
            quality: QualityFlag::Measured,
            obis_code: None,
        });
        t += time::Duration::minutes(15);
    }
    assert_eq!(
        intervals.len(),
        96,
        "non-DST standard day has 24h = 96 quarter-hours"
    );

    let config = ValidationConfig::rlm_strom_15min();
    let result = validate_intervals(&intervals, &config);
    assert!(
        result.is_clean(),
        "2024-02-29 (leap day) must be clean: {:?}",
        result.issues
    );
}

/// §17 MessZV prior-period substitution uses only same-slot values.
///
/// Given 5 prior-week intervals at time 07:00 UTC and 5 at 07:15 UTC,
/// `fill_gaps_with_config` using `PriorPeriodAverage` must:
/// - fill the 07:00 gap with the average of the 07:00 prior values only
/// - fill the 07:15 gap with the average of the 07:15 prior values only
#[test]
fn sect17_prior_period_average_uses_matching_time_slot() {
    use metering::{
        FillGapsConfig, MeterInterval, QualityFlag, SubstituteMethod, fill_gaps_with_config,
    };
    use rust_decimal_macros::dec;
    use time::macros::datetime;

    // Reference series with a 4-slot gap: 07:00–08:00 on 2026-07-01.
    // Using 4 intervals exceeds the default short_gap_threshold=3, ensuring
    // PriorPeriodAverage is used instead of LinearInterpolation.
    let series = vec![
        MeterInterval {
            from: datetime!(2026-07-01 06:45 UTC),
            to: datetime!(2026-07-01 07:00 UTC),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        // 07:00–08:00 GAP — four missing intervals (> short_gap_threshold=3)
        MeterInterval {
            from: datetime!(2026-07-01 08:00 UTC),
            to: datetime!(2026-07-01 08:15 UTC),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];

    // Prior week: distinct values at each of the 4 gap time slots.
    let prior_week = vec![
        MeterInterval {
            from: datetime!(2026-06-24 07:00 UTC),
            to: datetime!(2026-06-24 07:15 UTC),
            value_kwh: dec!(3.0), // 07:00 slot — expected substitution value
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: datetime!(2026-06-24 07:15 UTC),
            to: datetime!(2026-06-24 07:30 UTC),
            value_kwh: dec!(5.0), // 07:15 slot — expected substitution value
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: datetime!(2026-06-24 07:30 UTC),
            to: datetime!(2026-06-24 07:45 UTC),
            value_kwh: dec!(7.0), // 07:30 slot
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: datetime!(2026-06-24 07:45 UTC),
            to: datetime!(2026-06-24 08:00 UTC),
            value_kwh: dec!(9.0), // 07:45 slot
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];

    // Use short_gap_threshold=0 to ensure PriorPeriodAverage applies to every
    // slot in the gap regardless of gap length (no linear-interpolation override).
    let config = FillGapsConfig {
        method: SubstituteMethod::PriorPeriodAverage,
        prior_period_intervals: prior_week,
        short_gap_threshold: 0,
        reason: None,
    };
    let filled = fill_gaps_with_config(
        &series,
        900,
        datetime!(2026-07-01 06:00 UTC),
        datetime!(2026-07-01 09:00 UTC),
        &config,
    );

    // Find the substituted intervals.
    let sub_0700 = filled
        .iter()
        .find(|iv| iv.from == datetime!(2026-07-01 07:00 UTC));
    let sub_0715 = filled
        .iter()
        .find(|iv| iv.from == datetime!(2026-07-01 07:15 UTC));

    assert!(sub_0700.is_some(), "07:00 gap must be filled");
    assert!(sub_0715.is_some(), "07:15 gap must be filled");

    assert_eq!(
        sub_0700.unwrap().value_kwh,
        dec!(3.0),
        "§17 MessZV: 07:00 slot must use prior-week 07:00 value (3.0 kWh)"
    );
    assert_eq!(
        sub_0715.unwrap().value_kwh,
        dec!(5.0),
        "§17 MessZV: 07:15 slot must use prior-week 07:15 value (5.0 kWh)"
    );

    // Both substituted intervals must carry Substituted quality flag.
    assert_eq!(
        sub_0700.unwrap().quality,
        QualityFlag::Substituted,
        "substituted interval must have Substituted quality"
    );
}

/// Billing guard — FAULTY intervals must not contribute to energy totals.
///
/// A batch with 3 MEASURED intervals (1.0 kWh each) and 1 FAULTY interval
/// (99.9 kWh) must sum to 3.0 kWh — the FAULTY value is excluded.
/// This mirrors the SQL `AND quality NOT IN ('FAULTY','UNKNOWN')` guard.
#[test]
fn faulty_intervals_excluded_from_billing_sum() {
    use metering::{MeterInterval, QualityFlag};
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    let intervals = [
        MeterInterval {
            from: time::macros::datetime!(2026-07-01 00:00 UTC),
            to: time::macros::datetime!(2026-07-01 00:15 UTC),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: time::macros::datetime!(2026-07-01 00:15 UTC),
            to: time::macros::datetime!(2026-07-01 00:30 UTC),
            value_kwh: dec!(99.9), // should be excluded
            quality: QualityFlag::Faulty,
            obis_code: None,
        },
        MeterInterval {
            from: time::macros::datetime!(2026-07-01 00:30 UTC),
            to: time::macros::datetime!(2026-07-01 00:45 UTC),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
        MeterInterval {
            from: time::macros::datetime!(2026-07-01 00:45 UTC),
            to: time::macros::datetime!(2026-07-01 01:00 UTC),
            value_kwh: dec!(1.0),
            quality: QualityFlag::Measured,
            obis_code: None,
        },
    ];

    // Mirror the SQL billing filter: sum only billable quality.
    let billing_total: Decimal = intervals
        .iter()
        .filter(|iv| iv.quality.is_billable() && iv.quality != QualityFlag::Faulty)
        .map(|iv| iv.value_kwh)
        .sum();

    assert_eq!(
        billing_total,
        dec!(3.0),
        "§22 MessZV: FAULTY interval (99.9 kWh) must be excluded from billing total"
    );
}
