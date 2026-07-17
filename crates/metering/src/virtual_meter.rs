//! Virtual meter compute engine.
//!
//! Applies an [`AggregationRule`] to a map of source MaLo / MeLo time series
//! to produce a derived virtual meter time series.
//!
//! ## GGV net grid draw (§42b EnWG, Solarpaket I)
//!
//! Both GGV variants compute the tenant's **net grid draw after PV allocation**:
//!
//! - [`AggregationRule::GgvConstantAllocation`]: static fraction from UTILTS CCI+ZG6
//! - [`AggregationRule::GgvProportionalAllocation`]: dynamic ratio from actual consumption
//!
//! The result is the `Malo_i Verbrauch` quantity in the BDEW Anwendungshilfe —
//! the energy each tenant draws from the public grid after their share of the
//! community PV has been credited.
//!
//! ## Timestamp alignment
//!
//! All source series must be aligned to the same UTC timestamp grid. Only
//! timestamps present in **all** required source series are included in the output.
//! Use [`crate::resample()`] first if source series have different resolutions.
//!
//! ## Legal basis
//!
//! - **§42b Abs. 5 EnWG (Solarpaket I)** — GGV community allocation formulas
//! - **BDEW Anwendungshilfe "Berechnungsformeln Solarpaket 1"** v1.0, 25.01.2024
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
        AggregationRule::GgvConstantAllocation {
            plant_melo_id,
            tenant_melo_id,
            fraction,
        } => compute_ggv_constant(plant_melo_id, tenant_melo_id, *fraction, sources),
        AggregationRule::GgvProportionalAllocation {
            plant_melo_id,
            tenant_melo_id,
            all_tenant_melo_ids,
        } => compute_ggv_proportional(plant_melo_id, tenant_melo_id, all_tenant_melo_ids, sources),
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

// ── GGV constant allocation (§42b EnWG Beispiel 1, CCI+ZG6) ──────────────────

/// Constant-fraction GGV allocation per §42b Abs. 5 EnWG.
///
/// Formula (BDEW Anwendungshilfe Beispiel 1):
/// ```text
/// net_grid_draw[t] = max(0, tenant_consumption[t] - fraction × plant_generation[t])
/// ```
///
/// The `max(0, …)` is the `Pos()` operator (UTILTS Z83). It ensures the tenant
/// cannot "receive" more PV energy than they actually consumed in the interval,
/// satisfying §42b Abs. 5 EnWG: "begrenzt auf die durch ihn in diesem
/// Zeitintervall verbrauchte Strommenge."
fn compute_ggv_constant(
    plant_id: &str,
    tenant_id: &str,
    fraction: Decimal,
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    if !sources.contains_key(plant_id) {
        return Err(VirtualMeterError::MissingSource(plant_id.to_owned()));
    }
    if !sources.contains_key(tenant_id) {
        return Err(VirtualMeterError::MissingSource(tenant_id.to_owned()));
    }
    if fraction <= Decimal::ZERO || fraction > Decimal::ONE {
        return Err(VirtualMeterError::InvalidFractions { sum: fraction });
    }

    let aligned = aligned_timestamps([plant_id, tenant_id].iter().copied(), sources);
    let mut result = Vec::with_capacity(aligned.len());
    for ts in aligned {
        let plant_iv = lookup(sources, plant_id, ts);
        let tenant_iv = lookup(sources, tenant_id, ts);

        // allocated = fraction × plant_generation (UTILTS Z82 × ZG6)
        let allocated = fraction * plant_iv.value_kwh;
        // net_grid_draw = Pos(consumption - allocated) = max(0, consumption - allocated)
        let net_grid_draw = (tenant_iv.value_kwh - allocated).max(Decimal::ZERO);

        result.push(MeterInterval {
            from: ts,
            to: tenant_iv.to,
            value_kwh: net_grid_draw,
            quality: worst_quality(plant_iv.quality, tenant_iv.quality),
            obis_code: None,
        });
    }
    Ok(result)
}

// ── GGV proportional allocation (§42b EnWG Beispiel 3) ───────────────────────

/// Variable consumption-proportional GGV allocation per §42b Abs. 5 EnWG.
///
/// Formula (BDEW Anwendungshilfe Beispiel 3):
/// ```text
/// total[t]        = Σ all_tenant_consumption_j[t]
/// ratio[t]        = tenant_consumption[t] / total[t]  (0 when total = 0)
/// net_grid_draw[t] = max(0, tenant_consumption[t] - ratio[t] × plant_generation[t])
/// ```
///
/// Division-by-zero protection: when `total[t] = 0`, `ratio[t] = 0` and
/// `net_grid_draw[t] = 0`. This matches the BDEW Anwendungshilfe note:
/// "Ist die Energiemenge einer Marktlokation zugeordneten Messlokation = 0,
/// so ist auch der Verbrauch der Marktlokation auf 0 zu setzen."
fn compute_ggv_proportional(
    plant_id: &str,
    tenant_id: &str,
    all_tenant_ids: &[String],
    sources: &HashMap<String, Vec<MeterInterval>>,
) -> Result<Vec<MeterInterval>, VirtualMeterError> {
    if !sources.contains_key(plant_id) {
        return Err(VirtualMeterError::MissingSource(plant_id.to_owned()));
    }
    for id in all_tenant_ids {
        if !sources.contains_key(id.as_str()) {
            return Err(VirtualMeterError::MissingSource(id.clone()));
        }
    }
    // tenant_id must be present in all_tenant_ids (sanity — not hard-errored here,
    // a missing tenant_id is caught by the loop above if it's correctly listed)
    if !sources.contains_key(tenant_id) {
        return Err(VirtualMeterError::MissingSource(tenant_id.to_owned()));
    }

    // Align all IDs: plant + every tenant
    let all_ids: Vec<&str> = std::iter::once(plant_id)
        .chain(all_tenant_ids.iter().map(String::as_str))
        .collect();
    let aligned = aligned_timestamps(all_ids.iter().copied(), sources);
    let mut result = Vec::with_capacity(aligned.len());

    for ts in aligned {
        let plant_iv = lookup(sources, plant_id, ts);
        let tenant_iv = lookup(sources, tenant_id, ts);

        // Denominator: Σ all tenant consumptions
        let total_consumption: Decimal = all_tenant_ids
            .iter()
            .map(|id| lookup(sources, id, ts).value_kwh)
            .sum();

        // Dynamic ratio: protect against zero-division
        let net_grid_draw = if total_consumption > Decimal::ZERO {
            let ratio = tenant_iv.value_kwh / total_consumption;
            let allocated = ratio * plant_iv.value_kwh;
            (tenant_iv.value_kwh - allocated).max(Decimal::ZERO)
        } else {
            // All tenants consume 0 → grid draw is 0
            Decimal::ZERO
        };

        // Worst quality across plant + all tenants (any estimated interval affects output)
        let quality = all_tenant_ids.iter().fold(plant_iv.quality, |q, id| {
            worst_quality(q, lookup(sources, id, ts).quality)
        });

        result.push(MeterInterval {
            from: ts,
            to: tenant_iv.to,
            value_kwh: net_grid_draw,
            quality,
            obis_code: None,
        });
    }
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

    // ── GGV constant allocation (§42b Beispiel 1) ────────────────────────────

    #[test]
    fn ggv_constant_tenant_draws_residual_after_pv() {
        // Beispiel 1: Melo2=10%, Melo3=90%
        // Interval: plant generates 10 kWh, tenant consumes 5 kWh
        // allocated = 10% × 10 = 1 kWh → net_grid_draw = max(0, 5 - 1) = 4 kWh
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(10.0), QualityFlag::Measured)],
        );
        map.insert(
            "T2".to_owned(),
            vec![make_iv(ts(0), dec!(5.0), QualityFlag::Measured)],
        );

        let rule = AggregationRule::GgvConstantAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T2".to_owned(),
            fraction: dec!(0.10),
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].value_kwh, dec!(4.0), "net grid draw = 5 - 1 = 4");
    }

    #[test]
    fn ggv_constant_allocation_capped_by_tenant_consumption() {
        // §42b Abs. 5: allocated PV ≤ tenant consumption
        // plant = 10 kWh, fraction = 90%, allocation attempt = 9 kWh
        // but tenant only consumes 2 kWh → net_grid_draw = max(0, 2 - 9) = 0
        // (tenant gets 2 kWh of PV, excess 7 kWh feeds back to grid)
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(10.0), QualityFlag::Measured)],
        );
        map.insert(
            "T3".to_owned(),
            vec![make_iv(ts(0), dec!(2.0), QualityFlag::Measured)],
        );

        let rule = AggregationRule::GgvConstantAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T3".to_owned(),
            fraction: dec!(0.90),
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(
            result[0].value_kwh,
            dec!(0.0),
            "pos(2 - 9) = 0 — cap enforced"
        );
    }

    #[test]
    fn ggv_constant_feedin_balance_check() {
        // BDEW Beispiel 1: plant=10 kWh, T2=10%, T3=90%
        // T2 consumes 5, T3 consumes 20
        // T2 net = max(0, 5 - 1) = 4  → PV to T2 = 1
        // T3 net = max(0, 20 - 9) = 11 → PV to T3 = 9
        // Total PV delivered = 1 + 9 = 10 = full plant generation (no grid feed-in)
        let plant_gen = dec!(10.0);
        let t2_consumption = dec!(5.0);
        let t3_consumption = dec!(20.0);

        let t2_net = (t2_consumption - dec!(0.10) * plant_gen).max(Decimal::ZERO);
        let t3_net = (t3_consumption - dec!(0.90) * plant_gen).max(Decimal::ZERO);

        let pv_to_t2 = t2_consumption - t2_net;
        let pv_to_t3 = t3_consumption - t3_net;
        let grid_feedin = plant_gen - pv_to_t2 - pv_to_t3;

        assert_eq!(t2_net, dec!(4.0));
        assert_eq!(t3_net, dec!(11.0));
        assert_eq!(pv_to_t2 + pv_to_t3, dec!(10.0));
        assert_eq!(grid_feedin, dec!(0.0), "all PV consumed locally");
    }

    #[test]
    fn ggv_constant_multiple_intervals() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![
                make_iv(ts(0), dec!(10.0), QualityFlag::Measured),
                make_iv(ts(15), dec!(0.0), QualityFlag::Measured),
            ],
        );
        map.insert(
            "T".to_owned(),
            vec![
                make_iv(ts(0), dec!(3.0), QualityFlag::Measured),
                make_iv(ts(15), dec!(3.0), QualityFlag::Measured),
            ],
        );

        let rule = AggregationRule::GgvConstantAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T".to_owned(),
            fraction: dec!(0.5),
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result[0].value_kwh, dec!(0.0), "3 - 5 < 0 → max(0)");
        assert_eq!(
            result[1].value_kwh,
            dec!(3.0),
            "no PV → full load from grid"
        );
    }

    #[test]
    fn ggv_constant_invalid_fraction_rejected() {
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(1.0), QualityFlag::Measured)],
        );
        map.insert(
            "T".to_owned(),
            vec![make_iv(ts(0), dec!(1.0), QualityFlag::Measured)],
        );

        let rule = AggregationRule::GgvConstantAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T".to_owned(),
            fraction: dec!(1.5), // > 1 — invalid
        };
        assert!(matches!(
            compute_virtual_meter(&rule, &map),
            Err(VirtualMeterError::InvalidFractions { .. })
        ));
    }

    // ── GGV proportional allocation (§42b Beispiel 3) ────────────────────────

    #[test]
    fn ggv_proportional_tenant_gets_consumption_weighted_pv() {
        // Beispiel 3: plant=10 kWh, T2 consumes 2, T3 consumes 8
        // T2 ratio = 2/10 = 0.2 → allocation = 0.2 × 10 = 2 → net = max(0, 2-2) = 0
        // T3 ratio = 8/10 = 0.8 → allocation = 0.8 × 10 = 8 → net = max(0, 8-8) = 0
        // both tenants fully covered by PV
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(10.0), QualityFlag::Measured)],
        );
        map.insert(
            "T2".to_owned(),
            vec![make_iv(ts(0), dec!(2.0), QualityFlag::Measured)],
        );
        map.insert(
            "T3".to_owned(),
            vec![make_iv(ts(0), dec!(8.0), QualityFlag::Measured)],
        );

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

        let r2 = compute_virtual_meter(&rule_t2, &map).unwrap();
        let r3 = compute_virtual_meter(&rule_t3, &map).unwrap();
        assert_eq!(r2[0].value_kwh, dec!(0.0), "T2 fully covered by PV");
        assert_eq!(r3[0].value_kwh, dec!(0.0), "T3 fully covered by PV");
    }

    #[test]
    fn ggv_proportional_partial_coverage() {
        // plant=6 kWh, T2 consumes 2, T3 consumes 8 → total=10
        // T2 ratio = 0.2 → allocated = 0.2 × 6 = 1.2 → net = 2 - 1.2 = 0.8
        // T3 ratio = 0.8 → allocated = 0.8 × 6 = 4.8 → net = 8 - 4.8 = 3.2
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(6.0), QualityFlag::Measured)],
        );
        map.insert(
            "T2".to_owned(),
            vec![make_iv(ts(0), dec!(2.0), QualityFlag::Measured)],
        );
        map.insert(
            "T3".to_owned(),
            vec![make_iv(ts(0), dec!(8.0), QualityFlag::Measured)],
        );

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

        let r2 = compute_virtual_meter(&rule_t2, &map).unwrap();
        let r3 = compute_virtual_meter(&rule_t3, &map).unwrap();
        assert_eq!(r2[0].value_kwh, dec!(0.8));
        assert_eq!(r3[0].value_kwh, dec!(3.2));
        // total PV delivered = (2-0.8) + (8-3.2) = 1.2 + 4.8 = 6 = plant generation
        assert_eq!(
            (dec!(2.0) - r2[0].value_kwh) + (dec!(8.0) - r3[0].value_kwh),
            dec!(6.0)
        );
    }

    #[test]
    fn ggv_proportional_zero_division_guard() {
        // All tenants consume 0 → denominator = 0 → no PV allocated, no grid draw
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(5.0), QualityFlag::Measured)],
        );
        map.insert(
            "T2".to_owned(),
            vec![make_iv(ts(0), dec!(0.0), QualityFlag::Measured)],
        );
        map.insert(
            "T3".to_owned(),
            vec![make_iv(ts(0), dec!(0.0), QualityFlag::Measured)],
        );

        let rule = AggregationRule::GgvProportionalAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T2".to_owned(),
            all_tenant_melo_ids: vec!["T2".to_owned(), "T3".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(
            result[0].value_kwh,
            dec!(0.0),
            "zero total → zero draw (no division by zero)"
        );
    }

    #[test]
    fn ggv_proportional_cap_when_allocation_exceeds_consumption() {
        // plant=100 kWh, T2 consumes 1, T3 consumes 1 → total=2
        // T2 ratio = 0.5, allocated = 50 kWh but T2 only consumed 1 → net = max(0, 1-50) = 0
        let mut map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        map.insert(
            "PLANT".to_owned(),
            vec![make_iv(ts(0), dec!(100.0), QualityFlag::Measured)],
        );
        map.insert(
            "T2".to_owned(),
            vec![make_iv(ts(0), dec!(1.0), QualityFlag::Measured)],
        );
        map.insert(
            "T3".to_owned(),
            vec![make_iv(ts(0), dec!(1.0), QualityFlag::Measured)],
        );

        let rule = AggregationRule::GgvProportionalAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T2".to_owned(),
            all_tenant_melo_ids: vec!["T2".to_owned(), "T3".to_owned()],
        };
        let result = compute_virtual_meter(&rule, &map).unwrap();
        assert_eq!(result[0].value_kwh, dec!(0.0), "§42b cap: no negative draw");
    }

    #[test]
    fn ggv_proportional_missing_source_returns_error() {
        let map: HashMap<String, Vec<MeterInterval>> = HashMap::new();
        let rule = AggregationRule::GgvProportionalAllocation {
            plant_melo_id: "PLANT".to_owned(),
            tenant_melo_id: "T".to_owned(),
            all_tenant_melo_ids: vec!["T".to_owned()],
        };
        assert!(matches!(
            compute_virtual_meter(&rule, &map),
            Err(VirtualMeterError::MissingSource(_))
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
