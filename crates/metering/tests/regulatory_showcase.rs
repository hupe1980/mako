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
