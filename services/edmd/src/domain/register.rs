//! Which registers may be summed into one energy figure.
//!
//! A Marktlokation does not deliver *a* series. It delivers a **set of OBIS
//! registers** — Bezug beside Einspeisung on a prosumer, HT beside NT on a
//! dual-tariff meter, Blindarbeit beside Wirkarbeit on an industrial connection
//! — and `meterstore` reads span channels: `collect_resolved` returns one series
//! carrying every register the measuring point reported in the window.
//!
//! Summing that series is not an approximation, it is a different number:
//!
//! | Mixed | What the sum becomes |
//! |---|---|
//! | `1-0:1.8.0` + `1-0:1.8.1` + `1-0:1.8.2` | Consumption counted **twice** — the total register already *is* HT + NT |
//! | `1-0:1.8.0` + `1-0:2.8.0` | Grid draw plus feed-in, a quantity with no meaning |
//! | `1-0:1.8.0` + `1-0:3.8.0` | kWh plus **kvarh** |
//! | any + `1-0:1.6.0` | kWh plus a **kW** peak-demand register |
//! | any + `…63` | kWh plus a **fault counter** |
//!
//! So every path that folds readings into an energy figure has to say which
//! registers it means. This module is that decision, made once:
//! [`energy_intervals`] is the canonical projection for anything that sums, and
//! [`register_groups`] is the split for anything that judges a series' shape
//! (cadence, gaps, outliers) and therefore needs one register at a time.
//!
//! # The Messart (value group D) is deliberately *not* filtered
//!
//! Strictly, `D = 8` is a *Zählerstand* — a cumulative meter reading, which is
//! differenced rather than summed — and `D = 29` is the Lastgang. It is tempting
//! to admit only the latter. In the traffic edmd actually receives, `1-0:1.8.0`
//! is the ordinary label for a per-interval energy quantity, so filtering on D
//! would reject the single most common register in the store and silently return
//! zero. D is therefore not a reliable discriminator here, and the axes that
//! genuinely double-count — direction, tariff stage, and the unit — are.
//!
//! The one D-based exclusion that *is* safe is the maximum register (`D = 6`):
//! [`ObisCode::register_unit`] types it `kW`, so it is refused on the unit axis
//! below rather than on the Messart axis.

use std::collections::BTreeMap;

use metering::MeterInterval;
use metering::obis::{ObisCode, RegisterUnit};
use time::OffsetDateTime;

use super::{MeterRead, QualityFlag};

/// The direction of energy flow a figure is about.
///
/// A prosumer measuring point reports both, and no figure is about both at once:
/// § 51 EEG reduces a feed-in payment, a Mehr-/Mindermengensaldo settles a grid
/// draw, and adding them yields neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnergyDirection {
    /// Bezug — energy drawn from the grid (OBIS value group `C = 1`).
    Bezug,
    /// Einspeisung — energy fed into the grid (OBIS value group `C = 2`).
    Einspeisung,
}

impl EnergyDirection {
    /// Whether `code` reports this direction.
    ///
    /// Direction lives in value group **C alone** — `1-0:1.8.0`, `1-0:1.9.0`,
    /// `1-0:1.29.0` and `1-0:1.6.0` are all Bezug — so this asks `metering`,
    /// which encodes the EDI@Energy §2.1 rule.
    ///
    /// **Only electricity encodes a direction there.** For gas, water and heat,
    /// value group C names the Messgröße and nothing else: the gas energy code
    /// `7-1:99.33.17` is neither import nor export, and testing it against either
    /// predicate answers `false` for both. A medium-blind filter therefore
    /// projected every gas, water and heat series onto the empty set, and every
    /// aggregate over one silently became zero. Those media meter a single flow
    /// out of the network, so their registers are Bezug.
    ///
    /// **Einspeisung is never inferred.** It requires an explicit `C = 2`
    /// electricity code. Bezug is what an *unqualified* energy quantity means —
    /// a single-register delivery that never named its register is that
    /// measuring point's consumption — but reading the same silence as feed-in
    /// would put unlabelled consumption into the § 51 EEG reduction, which is a
    /// guess about money. Provenance is recorded, never guessed.
    ///
    /// Public because "which registers is this figure about" is asked outside
    /// the projection too — reporting how much of a direction's series is
    /// billable at all needs the same answer the sum uses, or the two describe
    /// different sets of readings.
    #[must_use]
    pub fn matches(self, code: Option<ObisCode>) -> bool {
        match (self, code) {
            (Self::Bezug, None) => true,
            (Self::Einspeisung, None) => false,
            // A single metered flow out of the network.
            (Self::Bezug, Some(c)) if !c.is_electricity() => true,
            (Self::Einspeisung, Some(c)) if !c.is_electricity() => false,
            (Self::Bezug, Some(c)) => c.is_import(),
            (Self::Einspeisung, Some(c)) => c.is_export(),
        }
    }
}

/// Whether `code` counts a quantity that may enter a **kWh** sum.
///
/// Three refusals, each for a different reason:
///
/// - the **Fehlerregister** (`E = 63`) counts fault occurrences, not energy;
/// - **reactive** registers (`C = 3…8`) count kvarh, and Blindarbeit is billed
///   separately from Wirkarbeit;
/// - **maximum** registers (`D = 6`) carry a power in kW — `1-0:1.6.0` is the
///   Jahreshöchstleistung priced under § 17 Abs. 2 StromNEV, and adding it to an
///   Arbeitsmenge bills a peak as if it were energy.
///
/// The last two are one test: [`RegisterUnit::is_cumulative`] is false exactly
/// for the power units, and `KiloVarHour` names the reactive energy.
#[must_use]
pub fn is_energy_register(code: ObisCode) -> bool {
    if code.is_fehlerregister() {
        return false;
    }
    match code.register_unit() {
        // kW / kvar are powers; kvarh is reactive energy.
        Some(u) => u.is_cumulative() && u != RegisterUnit::KiloVarHour,
        // Medium 0 (abstract) and 4 (Heizkostenverteiler, dimensionless
        // Verbrauchseinheiten) name no physical energy quantity.
        None => false,
    }
}

/// The canonical spelling a register is keyed under, `""` when unlabelled.
///
/// `1-0:1.8.0` and `1-0:1.8.0*255` are the same register and must land on the
/// same key — in a storage merge key, in a validation group, in a BO4E export
/// object and in an audit row alike, or one register becomes two and every
/// figure folded per register is taken over half the data.
///
/// A code the parser rejects keeps its raw spelling rather than collapsing into
/// the unlabelled bucket: an unparseable register is still a *named* one, and
/// merging it with the reads that named nothing would put it in the total
/// register's bucket (§2.6) — which is the one bucket that changes a sum.
///
/// There were three private copies of this, in the store, the validator and the
/// BO4E export. They agreed, and nothing made them.
#[must_use]
pub fn normalise_obis_code(obis_code: Option<&str>) -> String {
    obis_code.map_or_else(String::new, |s| {
        s.parse::<ObisCode>()
            .map_or_else(|_| s.to_owned(), |c| c.to_string())
    })
}

/// Which bucket a read's register falls in for the tariff-stage rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The total / combined register (`E = 0`), and unlabelled reads.
    Total,
    /// A tariff register — HT (`E = 1`), NT (`E = 2`), up to `E = 62`.
    Tariff,
}

/// The canonical, non-double-counting energy series for one direction.
///
/// The projection every path that sums readings must go through:
///
/// 1. **Non-billable qualities are dropped.** `FAULTY`/`UNKNOWN` must not reach a
///    settled figure (§ 60 Abs. 2 MsbG); a substitute value is what stands in
///    for them.
/// 2. **Registers that are not kWh are dropped** — see [`is_energy_register`].
/// 3. **The other direction is dropped**, per [`EnergyDirection`]. A read whose
///    OBIS code is absent or unparseable carries no direction: it counts as
///    Bezug, because that is what an unqualified energy quantity means, and never
///    as Einspeisung, which has to be claimed explicitly.
/// 4. **The total register wins over the tariff registers it covers, or they
///    are summed.** This is the rule that stops the double count, and it has
///    two halves. Where a total register (`E = 0`) reports, it *is* the answer
///    and the tariff registers are its own decomposition —
///    `1.8.0 = 1.8.1 + 1.8.2` — so a tariff interval overlapping a total one is
///    dropped. Where no total reports, the tariff registers are **summed** per
///    slot, because each covers a disjoint part of the tariff calendar and
///    dropping one loses that part of the consumption outright.
///
/// Step 4's second half is the one that is easy to get backwards. Picking a
/// single winner per slot — the shape a naive dedup takes — silently discards
/// NT consumption for every dual-tariff meter that does not also report a total.
///
/// The preference is **per interval, not per window**. Deciding once for the
/// whole batch — "any total anywhere ⇒ drop every tariff reading" — loses real
/// consumption whenever the two do not span the same time: a meter reconfigured
/// mid-month, a device exchange, a delivery that carries the total for the first
/// week and the HT/NT split for the rest. Those tariff slots overlap no total
/// interval, so nothing is double-counted by keeping them, and everything is
/// lost by dropping them. Overlap rather than an equal start is what the test
/// has to be: an hourly total beside a quarter-hourly HT/NT pair shares no
/// timestamp with it and would otherwise be added to its own decomposition.
#[must_use]
pub fn energy_intervals(reads: &[MeterRead], direction: EnergyDirection) -> Vec<MeterInterval> {
    energy_intervals_from(
        reads.iter().map(|r| MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        }),
        direction,
    )
}

/// [`energy_intervals`] over intervals that are already typed.
///
/// The same projection for callers holding a `metering::MeasurementSeries` —
/// the OLAP path reads one straight out of the store — rather than edmd's own
/// [`MeterRead`]. Both adapters share this body so the two surfaces cannot
/// drift into answering the same question differently.
#[must_use]
pub fn energy_intervals_from(
    intervals: impl IntoIterator<Item = MeterInterval>,
    direction: EnergyDirection,
) -> Vec<MeterInterval> {
    let mut total: BTreeMap<OffsetDateTime, MeterInterval> = BTreeMap::new();
    let mut tariff: BTreeMap<OffsetDateTime, MeterInterval> = BTreeMap::new();

    for iv in intervals.into_iter().filter(|iv| iv.quality.is_billable()) {
        let code = iv.obis_code;
        if !direction.matches(code) {
            continue;
        }
        let stage = match code {
            Some(c) => {
                if !is_energy_register(c) {
                    continue;
                }
                // The tariff stage is an **electricity** reading of value group E.
                // On a gas code like `7-1:99.33.17` the group is part of the
                // Messgröße, so treating `E ≠ 0` as "a tariff register to be
                // summed with its siblings" would add two unrelated gas
                // registers together. Other media get the single canonical
                // bucket instead.
                if c.is_total_register() || !c.is_electricity() {
                    Stage::Total
                } else {
                    Stage::Tariff
                }
            }
            // No register named: an unqualified energy quantity, which is what a
            // total register is. Only reachable for Bezug — see `matches`.
            None => Stage::Total,
        };
        match stage {
            // Two total registers on one slot cannot both be right; the labelled
            // one is the more specific claim, so it wins over an unlabelled read.
            Stage::Total => match total.entry(iv.from) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(iv);
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    if e.get().obis_code.is_none() && code.is_some() {
                        e.insert(iv);
                    }
                }
            },
            // Tariff stages are disjoint slices of one consumption and add up.
            Stage::Tariff => match tariff.entry(iv.from) {
                std::collections::btree_map::Entry::Vacant(e) => {
                    e.insert(iv);
                }
                std::collections::btree_map::Entry::Occupied(mut e) => {
                    let held = e.get_mut();
                    held.value += iv.value;
                    held.to = held.to.max(iv.to);
                    // A sum is only as good as its worst contributor.
                    if iv.quality.severity_rank() > held.quality.severity_rank() {
                        held.quality = iv.quality;
                    }
                    // The sum is no longer any one register.
                    held.obis_code = None;
                }
            },
        }
    }

    // The total register is the measuring point's own statement of the figure the
    // tariff registers decompose, so it wins **where it reports** — and only
    // there. A tariff interval that no total interval overlaps is consumption
    // nothing else accounts for.
    let covered = TotalCoverage::of(&total);
    let mut out: Vec<MeterInterval> = Vec::with_capacity(total.len() + tariff.len());
    out.extend(
        tariff
            .into_values()
            .filter(|iv| !covered.overlaps(iv.from, iv.to)),
    );
    out.extend(total.into_values());
    out.sort_by_key(|iv| iv.from);
    out
}

/// The time the total registers actually report, as an interval-overlap oracle.
///
/// `[from, to)` overlaps some total interval exactly when one of them starts
/// before `to` *and* ends after `from`. The starts are sorted, so "starts before
/// `to`" is a prefix — and whether any interval in that prefix ends after `from`
/// is answered by a running maximum of the ends. One binary search per query.
///
/// A backwards scan for "the last total starting at or before `from`" is the
/// obvious shape and is wrong: the total registers need not be disjoint among
/// themselves — a point may report both an hourly and a daily total — and a long
/// interval starting well before `from` is invisible to a query that only looks
/// at the nearest start. The running maximum is what makes an enclosing interval
/// visible.
struct TotalCoverage {
    /// Sorted by start; `.1` is the largest end at or before that position.
    starts: Vec<(OffsetDateTime, OffsetDateTime)>,
}

impl TotalCoverage {
    fn of(total: &BTreeMap<OffsetDateTime, MeterInterval>) -> Self {
        // `BTreeMap` iterates in key order, so the starts are already sorted.
        let mut max_end = OffsetDateTime::UNIX_EPOCH;
        let starts = total
            .values()
            .map(|iv| {
                max_end = max_end.max(iv.to);
                (iv.from, max_end)
            })
            .collect();
        Self { starts }
    }

    fn overlaps(&self, from: OffsetDateTime, to: OffsetDateTime) -> bool {
        // The prefix of totals starting strictly before `to`.
        let prefix = self.starts.partition_point(|(start, _)| *start < to);
        prefix > 0 && self.starts[prefix - 1].1 > from
    }
}

/// One register's readings, kept apart from every other register's.
#[derive(Debug, Clone)]
pub struct RegisterGroup {
    /// The register these intervals were reported on, `None` for unlabelled reads.
    pub obis_code: Option<ObisCode>,
    /// The intervals, ascending by start.
    pub intervals: Vec<MeterInterval>,
}

/// Split a MaLo's readings into one series per register.
///
/// The split every path that judges a series' *shape* needs. Cadence detection,
/// gap counting, overlap detection and the Hampel outlier filter are all
/// statements about a single series, and a MaLo routinely delivers several at
/// once. Flattened together the registers share every timestamp, so:
///
/// - the observed cadence becomes the median duration *across* registers rather
///   than the series' own — an hourly secondary register beside a quarter-hourly
///   Lastgang can decide the grid every gap is then divided by,
/// - every same-slot pair reads as an overlap,
/// - and coverage is inflated by the number of registers.
///
/// Unlike [`energy_intervals`] this keeps **every** quality — a scorer that only
/// saw billable readings could never report the `FAULTY` run it exists to find —
/// and every register, including reactive ones, because they are graded too.
#[must_use]
pub fn register_groups(reads: &[MeterRead]) -> Vec<RegisterGroup> {
    let mut groups: BTreeMap<Option<String>, RegisterGroup> = BTreeMap::new();
    for r in reads {
        let code: Option<ObisCode> = r.obis_code.as_deref().and_then(|s| s.parse().ok());
        // Key on the canonical rendering, because `1-0:1.8.0` and `1-0:1.8.0*255`
        // are the same register and must not become two series.
        let key = code.map(|c| c.to_string());
        groups
            .entry(key)
            .or_insert_with(|| RegisterGroup {
                obis_code: code,
                intervals: Vec::new(),
            })
            .intervals
            .push(MeterInterval {
                from: r.dtm_from,
                to: r.dtm_to,
                value: r.quantity_kwh,
                quality: r.quality,
                obis_code: code,
            });
    }
    let mut out: Vec<RegisterGroup> = groups.into_values().collect();
    for g in &mut out {
        g.intervals.sort_by_key(|iv| iv.from);
    }
    out
}

/// How much of one direction's series is billable, by **duration**.
///
/// The companion figure to [`energy_intervals`], which drops non-billable
/// readings: a caller that only sees the projection cannot tell a complete month
/// from one where a third of the intervals arrived `FAULTY` and were filtered
/// out. § 60 Abs. 2 MsbG turns on exactly that difference, so a settlement path
/// gated on data quality — § 51 EEG's negative-price reduction is the live one —
/// needs both numbers.
///
/// Measured over the same register set the sum is about, so the two agree:
/// energy registers of the requested direction, and no others. `None` when that
/// set is empty, because 0 % and "nothing to say" are different answers.
#[must_use]
pub fn billable_share_pct(reads: &[MeterRead], direction: EnergyDirection) -> Option<f64> {
    let mut total = 0i64;
    let mut billable = 0i64;
    for r in reads {
        let code: Option<ObisCode> = r.obis_code.as_deref().and_then(|s| s.parse().ok());
        if !direction.matches(code) || code.is_some_and(|c| !is_energy_register(c)) {
            continue;
        }
        let secs = (r.dtm_to - r.dtm_from).whole_seconds().max(0);
        total += secs;
        if r.quality.is_billable() {
            billable += secs;
        }
    }
    #[allow(clippy::cast_precision_loss)]
    (total > 0).then(|| billable as f64 / total as f64 * 100.0)
}

/// The worst quality among `intervals`, or `Unknown` when there are none.
///
/// A figure folded from several registers is only as trustworthy as its worst
/// contributor: a saldo built partly from Ersatzwerte is a different fact from
/// one built entirely from measurements, and the settlement side must see it.
/// An empty set has no measurement to speak for it, so the neutral answer is
/// "not known" rather than "measured".
///
/// The ranking is [`QualityFlag::worst_of`]'s, not a second one here: `metering`
/// publishes it precisely so every aggregation inside and outside the crate
/// reaches the same verdict.
#[must_use]
pub fn worst_quality(intervals: &[MeterInterval]) -> QualityFlag {
    QualityFlag::worst_of(intervals.iter().map(|iv| iv.quality))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use time::Duration;

    fn read(obis: Option<&str>, slot: i64, kwh: i64, quality: QualityFlag) -> MeterRead {
        let from = OffsetDateTime::UNIX_EPOCH + Duration::minutes(15 * slot);
        MeterRead {
            malo_id: "51238696012".into(),
            melo_id: None,
            dtm_from: from,
            dtm_to: from + Duration::minutes(15),
            quantity_kwh: Decimal::from(kwh),
            quality,
            pid: 13002,
            sparte: crate::domain::Sparte::Strom,
            obis_code: obis.map(str::to_owned),
            tenant: "t".into(),
            source: crate::domain::IngestionSource::Mscons,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".into(),
            valid_from_tx: None,
            mscons_version: None,
        }
    }

    fn measured(obis: Option<&str>, slot: i64, kwh: i64) -> MeterRead {
        read(obis, slot, kwh, QualityFlag::Measured)
    }

    fn sum(intervals: &[MeterInterval]) -> Decimal {
        intervals.iter().map(|iv| iv.value).sum()
    }

    #[test]
    fn the_total_register_is_not_added_to_its_own_tariff_split() {
        // 1.8.0 = 1.8.1 + 1.8.2. Summing all three bills the consumption twice.
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            measured(Some("1-0:1.8.1"), 0, 6),
            measured(Some("1-0:1.8.2"), 0, 4),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(sum(&picked), Decimal::from(10));
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn tariff_registers_are_summed_when_no_total_is_reported() {
        // The half that a "pick one winner per slot" dedup gets wrong: without a
        // total register, dropping NT loses that consumption outright.
        let reads = vec![
            measured(Some("1-0:1.8.1"), 0, 6),
            measured(Some("1-0:1.8.2"), 0, 4),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(sum(&picked), Decimal::from(10));
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn feed_in_is_never_added_to_grid_draw() {
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            measured(Some("1-0:2.8.0"), 0, 7),
        ];
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Bezug)),
            Decimal::from(10)
        );
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Einspeisung)),
            Decimal::from(7)
        );
    }

    #[test]
    fn reactive_energy_and_the_fault_counter_stay_out_of_a_kwh_sum() {
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            // Blindarbeit — kvarh, billed separately.
            measured(Some("1-0:3.8.0"), 0, 99),
            // Fehlerregister — a count of faults.
            measured(Some("1-0:1.8.63"), 0, 99),
            // Jahreshöchstleistung — a kW power, not an Arbeitsmenge.
            measured(Some("1-0:1.6.0"), 0, 99),
        ];
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Bezug)),
            Decimal::from(10)
        );
    }

    #[test]
    fn non_billable_readings_never_reach_a_settled_figure() {
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            read(Some("1-0:1.8.0"), 1, 99, QualityFlag::Faulty),
        ];
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Bezug)),
            Decimal::from(10)
        );
    }

    #[test]
    fn an_unlabelled_read_is_treated_as_the_total_register() {
        // The common single-register delivery: no PIA segment, one series.
        let reads = vec![measured(None, 0, 10), measured(None, 1, 5)];
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Bezug)),
            Decimal::from(15)
        );
    }

    #[test]
    fn a_summed_tariff_slot_carries_its_worst_contributor() {
        let reads = vec![
            measured(Some("1-0:1.8.1"), 0, 6),
            read(Some("1-0:1.8.2"), 0, 4, QualityFlag::Substituted),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(picked.len(), 1);
        assert_eq!(picked[0].value, Decimal::from(10));
        assert_eq!(picked[0].quality, QualityFlag::Substituted);
    }

    #[test]
    fn an_unlabelled_read_is_never_read_as_feed_in() {
        // Bezug is what an unqualified energy quantity means; feed-in is a claim
        // that has to be made explicitly, because § 51 EEG pays on it.
        let reads = vec![measured(None, 0, 10)];
        assert!(
            energy_intervals(&reads, EnergyDirection::Einspeisung).is_empty(),
            "an unlabelled reading must not be counted as Einspeisung"
        );
        assert_eq!(
            sum(&energy_intervals(&reads, EnergyDirection::Bezug)),
            Decimal::from(10)
        );
    }

    #[test]
    fn a_gas_meter_has_no_feed_in_direction() {
        let mut r = measured(Some("7-1:99.33.17"), 0, 12);
        r.sparte = crate::domain::Sparte::Gas;
        assert!(
            energy_intervals(std::slice::from_ref(&r), EnergyDirection::Einspeisung).is_empty(),
            "group C is a Messgröße on a gas code, not a direction"
        );
    }

    #[test]
    fn a_gas_series_is_not_projected_onto_the_empty_set() {
        // `7-1:99.33.17` is how edmd labels gas energy. Value group C is the
        // Messgröße there, not a direction, so an import/export test answers
        // `false` both ways and a medium-blind filter drops the whole series.
        let mut r = measured(Some("7-1:99.33.17"), 0, 12);
        r.sparte = crate::domain::Sparte::Gas;
        let picked = energy_intervals(std::slice::from_ref(&r), EnergyDirection::Bezug);
        assert_eq!(sum(&picked), Decimal::from(12));
    }

    #[test]
    fn two_gas_registers_on_one_slot_are_not_added_together() {
        // Value group E is a tariff stage only for electricity; on a gas code it
        // is part of the Messgröße, so these are not HT and NT of one quantity.
        let a = measured(Some("7-1:99.33.17"), 0, 12);
        let b = measured(Some("7-1:99.33.18"), 0, 90);
        let picked = energy_intervals(&[a, b], EnergyDirection::Bezug);
        assert_eq!(picked.len(), 1);
        assert_ne!(sum(&picked), Decimal::from(102));
    }

    #[test]
    fn registers_are_grouped_apart_for_shape_analysis() {
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            measured(Some("1-0:2.8.0"), 0, 7),
            // The same register in its long spelling must not become a second series.
            measured(Some("1-0:1.8.0*255"), 1, 11),
        ];
        let groups = register_groups(&reads);
        assert_eq!(groups.len(), 2);
        let bezug = groups
            .iter()
            .find(|g| g.obis_code.is_some_and(|c| c.is_import()))
            .expect("bezug group");
        assert_eq!(bezug.intervals.len(), 2);
    }

    #[test]
    fn tariff_slots_the_total_register_does_not_cover_are_kept() {
        // A meter reconfigured mid-window: the total register reports for the
        // first two slots, the HT/NT pair for the next two. Deciding once for
        // the whole batch — "a total exists, drop every tariff reading" — threw
        // the second half of the consumption away.
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            measured(Some("1-0:1.8.0"), 1, 10),
            measured(Some("1-0:1.8.1"), 2, 6),
            measured(Some("1-0:1.8.2"), 2, 4),
            measured(Some("1-0:1.8.1"), 3, 7),
            measured(Some("1-0:1.8.2"), 3, 3),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(sum(&picked), Decimal::from(40));
        assert_eq!(picked.len(), 4);
        assert!(
            picked.windows(2).all(|w| w[0].from <= w[1].from),
            "the projection is ascending by interval start"
        );
    }

    #[test]
    fn an_hourly_total_still_beats_a_quarter_hourly_tariff_split() {
        // Overlap, not an equal start, is the test: the hourly total shares no
        // timestamp with the quarter-hours it decomposes, so a same-slot rule
        // would add the register to its own decomposition.
        let hour = OffsetDateTime::UNIX_EPOCH;
        let total = MeterRead {
            dtm_to: hour + Duration::hours(1),
            ..measured(Some("1-0:1.8.0"), 0, 20)
        };
        let reads = vec![
            total,
            measured(Some("1-0:1.8.1"), 0, 6),
            measured(Some("1-0:1.8.2"), 1, 4),
            measured(Some("1-0:1.8.1"), 2, 6),
            measured(Some("1-0:1.8.2"), 3, 4),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(sum(&picked), Decimal::from(20));
        assert_eq!(picked.len(), 1);
    }

    #[test]
    fn a_long_total_hides_the_tariff_slots_inside_it() {
        // Two total registers that are not disjoint among themselves: a daily
        // figure spanning the window and an hourly one inside it. A backwards
        // scan for "the nearest total starting at or before this slot" finds the
        // hourly one, sees it ended, and lets the tariff readings through — so
        // the day's consumption is counted twice.
        let base = OffsetDateTime::UNIX_EPOCH;
        let day = MeterRead {
            dtm_to: base + Duration::days(1),
            ..measured(Some("1-0:1.8.0"), 0, 240)
        };
        let hour = MeterRead {
            dtm_from: base + Duration::hours(1),
            dtm_to: base + Duration::hours(2),
            ..measured(Some("1-0:1.8.0"), 0, 10)
        };
        let reads = vec![
            day,
            hour,
            // Quarter-hours well after the hourly total but inside the daily one.
            measured(Some("1-0:1.8.1"), 40, 6),
            measured(Some("1-0:1.8.2"), 40, 4),
        ];
        let picked = energy_intervals(&reads, EnergyDirection::Bezug);
        assert_eq!(
            sum(&picked),
            Decimal::from(250),
            "the tariff slots lie inside the daily total and must not be added              to it: {picked:#?}"
        );
    }

    #[test]
    fn an_empty_projection_has_no_measurement_to_speak_for_it() {
        assert_eq!(worst_quality(&[]), QualityFlag::Unknown);
    }

    #[test]
    fn grouping_keeps_non_billable_readings_for_the_scorer() {
        let reads = vec![
            measured(Some("1-0:1.8.0"), 0, 10),
            read(Some("1-0:1.8.0"), 1, 0, QualityFlag::Faulty),
        ];
        let groups = register_groups(&reads);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].intervals.len(), 2);
    }
}
