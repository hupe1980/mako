//! Virtual meter compute engine.
//!
//! Applies an [`AggregationRule`] to a map of source MaLo time series to produce
//! a derived virtual meter time series.
//!
//! ## Timestamp alignment
//!
//! All source series must be aligned to the same UTC timestamp grid. Only
//! timestamps present in **all** required source series are included in the output.
//! Use [`crate::resample`] first if source series have different resolutions.
//!
//! ## Virtual source column
//!
//! All output intervals carry `obis_code = None` — callers should set the
//! appropriate OBIS code after computing (e.g. `ObisCode::STROM_BEZUG_TOTAL`
//! for a Sum result).
//!
//! ## Legal basis
//!
//! - **§42b EEG 2023 (Solarpaket I)** — GGV community allocation
//! - **§42a EEG** — residual metering for feed-in compensation
//! - **GPKE BK6-22-024** — portfolio aggregation for BKV settlement
//! - **BSI TR-03109** — SMGW sub-metering aggregation for §14a

use std::collections::{HashMap, HashSet};

use rust_decimal::Decimal;
use time::OffsetDateTime;

use crate::aggregation_rule::AggregationRule;
use crate::interval::{MeterInterval, QualityFlag};

// ── VirtualMeterError ─────────────────────────────────────────────────────────

/// Error when computing a virtual meter.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum VirtualMeterError {
    /// A required source MaLo has no entry in the provided data map.
    #[error("missing source MaLo: {0}")]
    MissingSource(String),

    /// GGV tenant fractions are out of range — must be 0 < Σ ≤ 1.
    #[error("GGV tenant fractions sum to {sum} — must be in (0, 1]")]
    InvalidFractions {
        /// Actual sum of the provided fractions.
        sum: Decimal,
    },
}

// ── compute_virtual_meter ─────────────────────────────────────────────────────

/// Apply an [`AggregationRule`] to produce a virtual meter time series.
///
/// `sources` maps MaLo ID → sorted `Vec<MeterInterval>`.
///
/// Only intervals whose timestamp appears in **all** required source series
/// are included in the output (intersection semantics). This is conservative:
/// gaps in any single source propagate to the output rather than silently
/// producing wrong totals.
///
/// # Errors
///
/// - [`VirtualMeterError::MissingSource`] when a required MaLo is absent.
/// - [`VirtualMeterError::InvalidFractions`] for invalid GGV fractions.
pub fn compute_virtual_meter(
    rule: &AggregationRule,
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    match rule {
        AggregationRule::Sum { source_malo_ids } => compute_sum(source_malo_ids, sources),
        AggregationRule::Residual {
            total_malo_id,
            subtract_malo_ids,
        } => compute_residual(total_malo_id, subtract_malo_ids, sources),
        AggregationRule::PvSelfConsumption {
            grid_malo_id,
            generation_malo_id,
        } => compute_net_grid(grid_malo_id, generation_malo_id, sources),
        AggregationRule::GgvAllocation {
            plant_malo_id,
            tenant_fractions,
        } => compute_ggv(plant_malo_id, tenant_fractions, sources),
    }
}

// ── Sum ───────────────────────────────────────────────────────────────────────

fn compute_sum(
    malo_ids: &[String],
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    for id in malo_ids {
        if !sources.contains_key(id.as_str()) {
            return Err(VirtualMeterError::MissingSource(id.clone()));
        }
    }
    let aligned = aligned_timestamps(malo_ids.iter().map(String::as_str), sources);
    let mut result = Vec::with_capacity(aligned.len());
    for ts in aligned {
        let mut sum = Decimal::ZERO;
        let mut quality = QualityFlag::Measured;
        let mut end: Option<OffsetDateTime> = None;
        for id in malo_ids {
            let iv = lookup(sources, id, ts);
            sum += iv.value_kwh;
            quality = worst_quality(quality, iv.quality);
            end = Some(iv.to);
        }
        if let Some(to) = end {
            result.push(MeterInterval {
                from: ts,
                to,
                value_kwh: sum,
                quality,
                obis_code: None,
            });
        }
    }
    Ok(result)
}

// ── Residual ──────────────────────────────────────────────────────────────────

fn compute_residual(
    total_id: &str,
    subtract_ids: &[String],
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    if !sources.contains_key(total_id) {
        return Err(VirtualMeterError::MissingSource(total_id.to_owned()));
    }
    for id in subtract_ids {
        if !sources.contains_key(id.as_str()) {
            return Err(VirtualMeterError::MissingSource(id.clone()));
        }
    }
    let all_ids = std::iter::once(total_id).chain(subtract_ids.iter().map(String::as_str));
    let aligned = aligned_timestamps(all_ids, sources);
    let mut result = Vec::with_capacity(aligned.len());
    for ts in aligned {
        let total_iv = lookup(sources, total_id, ts);
        let mut subtract_sum = Decimal::ZERO;
        let mut quality = total_iv.quality;
        for id in subtract_ids {
            let iv = lookup(sources, id, ts);
            subtract_sum += iv.value_kwh;
            quality = worst_quality(quality, iv.quality);
        }
        result.push(MeterInterval {
            from: ts,
            to: total_iv.to,
            value_kwh: total_iv.value_kwh - subtract_sum,
            quality,
            obis_code: None,
        });
    }
    Ok(result)
}

// ── PV net grid ───────────────────────────────────────────────────────────────

fn compute_net_grid(
    grid_id: &str,
    gen_id: &str,
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    if !sources.contains_key(grid_id) {
        return Err(VirtualMeterError::MissingSource(grid_id.to_owned()));
    }
    if !sources.contains_key(gen_id) {
        return Err(VirtualMeterError::MissingSource(gen_id.to_owned()));
    }
    let aligned = aligned_timestamps([grid_id, gen_id].iter().copied(), sources);
    let mut result = Vec::with_capacity(aligned.len());
    for ts in aligned {
        let grid_iv = lookup(sources, grid_id, ts);
        let gen_iv = lookup(sources, gen_id, ts);
        // Net grid draw: positive = consuming from grid, negative = exporting
        result.push(MeterInterval {
            from: ts,
            to: grid_iv.to,
            value_kwh: grid_iv.value_kwh - gen_iv.value_kwh,
            quality: worst_quality(grid_iv.quality, gen_iv.quality),
            obis_code: None,
        });
    }
    Ok(result)
}

// ── GGV allocation ────────────────────────────────────────────────────────────

fn compute_ggv(
    plant_id: &str,
    tenant_fractions: &[(String, Decimal)],
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    if !sources.contains_key(plant_id) {
        return Err(VirtualMeterError::MissingSource(plant_id.to_owned()));
    }
    let total_fraction: Decimal = tenant_fractions.iter().map(|(_, f)| *f).sum();
    if total_fraction <= Decimal::ZERO || total_fraction > Decimal::ONE {
        return Err(VirtualMeterError::InvalidFractions {
            sum: total_fraction,
        });
    }
    // The virtual meter = total generation allocated to all tenants combined
    let plant_ivs = &sources[plant_id];
    let result = plant_ivs
        .iter()
        .map(|iv| MeterInterval {
            from: iv.from,
            to: iv.to,
            value_kwh: iv.value_kwh * total_fraction,
            quality: iv.quality,
            obis_code: None,
        })
        .collect();
    Ok(result)
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Compute the intersection of timestamps across all named source series.
fn aligned_timestamps<'a>(
    malo_ids: impl Iterator<Item = &'a str>,
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Vec<OffsetDateTime> {
    let ids: Vec<&str> = malo_ids.collect();
    if ids.is_empty() {
        return Vec::new();
    }
    let first_set: HashSet<i64> = sources
        .get(ids[0])
        .map(|ivs| ivs.iter().map(|iv| iv.from.unix_timestamp()).collect())
        .unwrap_or_default();

    let intersection = ids[1..].iter().fold(first_set, |acc, id| {
        let other: HashSet<i64> = sources
            .get(*id)
            .map(|ivs| ivs.iter().map(|iv| iv.from.unix_timestamp()).collect())
            .unwrap_or_default();
        acc.intersection(&other).copied().collect()
    });

    let mut sorted: Vec<i64> = intersection.into_iter().collect();
    sorted.sort_unstable();
    sorted
        .into_iter()
        .filter_map(|t| OffsetDateTime::from_unix_timestamp(t).ok())
        .collect()
}

fn lookup<'a>(
    sources: &'a HashMap<String, Vec<MeterInterval>>,
    malo_id: &str,
    ts: OffsetDateTime,
) -> &'a MeterInterval {
    let unix = ts.unix_timestamp();
    sources
        .get(malo_id)
        .and_then(|ivs| ivs.iter().find(|iv| iv.from.unix_timestamp() == unix))
        .unwrap_or_else(|| {
            panic!("timestamp {ts} was in aligned set but missing from source {malo_id}")
        })
}

fn quality_rank(q: QualityFlag) -> u8 {
    match q {
        QualityFlag::Faulty | QualityFlag::Unknown => 5,
        QualityFlag::Preliminary => 4,
        QualityFlag::Estimated => 3,
        QualityFlag::Corrected | QualityFlag::Substituted => 2,
        QualityFlag::Calculated => 1,
        QualityFlag::Measured => 0,
    }
}

fn worst_quality(a: QualityFlag, b: QualityFlag) -> QualityFlag {
    if quality_rank(a) >= quality_rank(b) {
        a
    } else {
        b
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::{Duration, macros::datetime};

    fn make_iv(from: OffsetDateTime, kwh: Decimal, quality: QualityFlag) -> MeterInterval {
        MeterInterval {
            from,
            to: from + Duration::minutes(15),
            value_kwh: kwh,
            quality,
            obis_code: None,
        }
    }

    fn ts(offset_min: i64) -> OffsetDateTime {
        datetime!(2026-01-01 00:00 UTC) + Duration::minutes(offset_min)
    }

    fn source(id: &str, values: Vec<(i64, Decimal)>) -> (String, Vec<MeterInterval>) {
        let ivs = values
            .into_iter()
            .map(|(min, kwh)| make_iv(ts(min), kwh, QualityFlag::Measured))
            .collect();
        (id.to_owned(), ivs)
    }

    // ── Sum ───────────────────────────────────────────────────────────────────

    #[test]
    fn sum_rule_adds_two_series() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (ka, va) = source("A", vec![(0, dec!(3.0)), (15, dec!(3.0))]);
        let (kb, vb) = source("B", vec![(0, dec!(2.0)), (15, dec!(2.0))]);
        map.insert(ka, va);
        map.insert(kb, vb);

        let rule = AggregationRule::Sum {
            source_malo_ids: vec!["A".to_owned(), "B".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].value_kwh, dec!(5.0));
    }

    #[test]
    fn sum_missing_source_returns_error() {
        let map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let rule = AggregationRule::Sum {
            source_malo_ids: vec!["MISSING".to_owned()],
        };
        assert!(matches!(
            compute_virtual_meter(&rule, &map),
            Err(VirtualMeterError::MissingSource(_))
        ));
    }

    // ── Residual ──────────────────────────────────────────────────────────────

    #[test]
    fn residual_rule_subtracts_generation() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (kt, vt) = source("TOTAL", vec![(0, dec!(10.0)), (15, dec!(8.0))]);
        let (kp, vp) = source("PV", vec![(0, dec!(3.0)), (15, dec!(2.0))]);
        map.insert(kt, vt);
        map.insert(kp, vp);

        let rule = AggregationRule::Residual {
            total_malo_id: "TOTAL".to_owned(),
            subtract_malo_ids: vec!["PV".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].value_kwh, dec!(7.0));
        assert_eq!(result[1].value_kwh, dec!(6.0));
    }

    #[test]
    fn residual_can_produce_negative_for_net_exporter() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (kt, vt) = source("GRID", vec![(0, dec!(1.0))]);
        let (kp, vp) = source("PV", vec![(0, dec!(5.0))]);
        map.insert(kt, vt);
        map.insert(kp, vp);

        let rule = AggregationRule::Residual {
            total_malo_id: "GRID".to_owned(),
            subtract_malo_ids: vec!["PV".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result[0].value_kwh, dec!(-4.0));
    }

    // ── PV net grid ───────────────────────────────────────────────────────────

    #[test]
    fn pv_self_consumption_net_grid() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (kg, vg) = source("GRID", vec![(0, dec!(4.0))]);
        let (kp, vp) = source("GEN", vec![(0, dec!(2.0))]);
        map.insert(kg, vg);
        map.insert(kp, vp);

        let rule = AggregationRule::PvSelfConsumption {
            grid_malo_id: "GRID".to_owned(),
            generation_malo_id: "GEN".to_owned(),
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result[0].value_kwh, dec!(2.0));
    }

    // ── GGV ───────────────────────────────────────────────────────────────────

    #[test]
    fn ggv_allocation_scales_by_total_fraction() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (kp, vp) = source("PLANT", vec![(0, dec!(10.0)), (15, dec!(8.0))]);
        map.insert(kp, vp);

        let rule = AggregationRule::GgvAllocation {
            plant_malo_id: "PLANT".to_owned(),
            tenant_fractions: vec![("T1".to_owned(), dec!(0.4)), ("T2".to_owned(), dec!(0.35))],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        // Total fraction = 0.75
        assert_eq!(result[0].value_kwh, dec!(7.5));
        assert_eq!(result[1].value_kwh, dec!(6.0));
    }

    #[test]
    fn ggv_invalid_fractions_rejected() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let (kp, vp) = source("PLANT", vec![(0, dec!(10.0))]);
        map.insert(kp, vp);

        let rule = AggregationRule::GgvAllocation {
            plant_malo_id: "PLANT".to_owned(),
            tenant_fractions: vec![
                ("T1".to_owned(), dec!(0.7)),
                ("T2".to_owned(), dec!(0.5)), // sum = 1.2 — invalid
            ],
        };
        assert!(matches!(
            compute_virtual_meter(&rule, &map),
            Err(VirtualMeterError::InvalidFractions { .. })
        ));
    }

    // ── Alignment ─────────────────────────────────────────────────────────────

    #[test]
    fn misaligned_timestamps_produce_intersection() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        // A has ts 0 and 15; B has only ts 15
        let (ka, va) = source("A", vec![(0, dec!(1.0)), (15, dec!(1.0))]);
        let (kb, vb) = source("B", vec![(15, dec!(2.0))]);
        map.insert(ka, va);
        map.insert(kb, vb);

        let rule = AggregationRule::Sum {
            source_malo_ids: vec!["A".to_owned(), "B".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result.len(), 1, "only ts=15 is in both series");
        assert_eq!(result[0].value_kwh, dec!(3.0));
    }

    // ── Quality propagation ────────────────────────────────────────────────────

    #[test]
    fn worst_quality_propagates_in_sum() {
        let base = ts(0);
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "A".to_owned(),
            vec![make_iv(base, dec!(1.0), QualityFlag::Measured)],
        );
        map.insert(
            "B".to_owned(),
            vec![make_iv(base, dec!(1.0), QualityFlag::Estimated)],
        );

        let rule = AggregationRule::Sum {
            source_malo_ids: vec!["A".to_owned(), "B".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result[0].quality, QualityFlag::Estimated);
    }
}
