//! Validated ingest batches.
//!
//! `TimeSeriesRepository::store_reads` accepts [`ValidatedReads`] rather than a
//! bare slice, so a new ingest path cannot persist meter data without running
//! the V-rules first: the only way to obtain the type is to call
//! [`ValidatedReads::validate`], and its field is private to this module.
//!
//! That matters because the failure is silent. An unvalidated path stores rows
//! indistinguishable from validated ones, and V03 (negative energy), V04
//! (statistical outlier) and V09 (non-billable quality) simply never fire for
//! that source — while § 147 AO / GoBD requires billed data to have been
//! validated. Making the state unrepresentable is cheaper than remembering to
//! check.
//!
//! ## One configuration, one register split, every caller
//!
//! [`findings`] is the single V-rule pass. It splits by `(Sparte, register)`,
//! configures each group from [`config_for`], and returns the issues tagged with
//! the register they are about. Ingest annotation ([`ValidatedReads::validate`]),
//! the MCP `validate_timeseries` tool and the § 60 Abs. 2 substitute path all go
//! through it, because a validator that answers differently depending on which
//! surface asked is worse than one that does not exist: it is consulted exactly
//! when nobody is checking. Each of those three used to carry its own
//! `ValidationConfig::default()` — electricity thresholds, an assumed 900 s
//! grid — and so disagreed with the ingest door about whether a perfectly
//! ordinary hourly gas delivery was clean.
//!
//! ## The rule set is `metering`'s, not a fixed V01–V10
//!
//! `metering` 0.18 runs **V01–V09, V11 and V12**. There is no V10: it was a
//! "register rollover" rule comparing consecutive interval energies, which is
//! meaningless because a [`MeterInterval`](metering::MeterInterval) carries the
//! energy *in* an interval and not a cumulative Zählerstand — for it to fire, one
//! quarter-hour would have had to carry 50 MWh. The number is left unused so a
//! stored `V10` finding cannot be silently reinterpreted. V11 reports an
//! unordered input series and V12 an average power above the plant's physical
//! capacity. Nothing here enumerates the rules: the set is whatever the crate
//! runs, and the findings are stored by their own `rule_id`.

use std::collections::BTreeMap;

use crate::domain::model::MeterRead;
use metering::obis::ObisCode;
use metering::validation::{ValidationConfig, ValidationIssue};
use rust_decimal::Decimal;
use time::OffsetDateTime;

/// What an ingest door knows about the batch beyond the readings themselves.
///
/// Carried as a struct rather than a widening argument list because the pieces
/// arrive from different places — the door names itself, the MaLo comes off the
/// path, and the capacity ceiling comes off the request body — and a positional
/// `(&str, &str, Option<Decimal>)` at five call sites is how two of them end up
/// swapped.
#[derive(Debug, Clone, Copy)]
pub struct IngestContext<'a> {
    /// The ingest door, for the log line and the stored `source` annotation.
    pub source: &'a str,
    /// The measuring point the batch is about.
    pub malo_id: &'a str,
    /// Physical capacity ceiling in kW for **V12** (`ImplausiblePower`).
    ///
    /// Nameplate capacity or Anschlussleistung. A value whose average power over
    /// its own interval exceeds it is not unusual, it is impossible — which is
    /// why V12 is an `Error` and V04 a `Warning`.
    ///
    /// `None` disables the rule, and that was the *only* state edmd could reach:
    /// no ingest door accepted a ceiling and `QualityConfig::for_sparte` sets
    /// none, so V12 was documented, surfaced as `spike_intervals`, and unable to
    /// fire. edmd holds no master data of its own, so the ceiling comes from the
    /// caller that does — the head-end, the MSB's push, the bulk import.
    pub max_plant_power_kw: Option<Decimal>,
}

impl<'a> IngestContext<'a> {
    /// A context with no capacity ceiling — V12 stays off.
    #[must_use]
    pub fn new(source: &'a str, malo_id: &'a str) -> Self {
        Self {
            source,
            malo_id,
            max_plant_power_kw: None,
        }
    }

    /// Declare the metered plant's physical capacity, enabling V12.
    #[must_use]
    pub fn with_capacity_kw(mut self, kw: Option<Decimal>) -> Self {
        self.max_plant_power_kw = kw.filter(|v| *v > Decimal::ZERO);
        self
    }
}

/// The window a batch actually covers, as `(earliest start, latest end)`.
///
/// Taken as min/max rather than off the first and last row: a batch is not
/// required to arrive sorted — V11 exists to say when it did not — and reading
/// the extent off the ends of an unsorted slice reports a period the delivery
/// does not have. That period is the denominator of the quality score and the
/// window named in the `de.messwert.reading.quality.warning` event.
#[must_use]
pub fn batch_period(reads: &[MeterRead]) -> (Option<OffsetDateTime>, Option<OffsetDateTime>) {
    (
        reads.iter().map(|r| r.dtm_from).min(),
        reads.iter().map(|r| r.dtm_to).max(),
    )
}

/// A batch that has been through the V-rule engine and annotated.
///
/// Constructing one runs the validation; there is no other constructor.
pub struct ValidatedReads {
    reads: Vec<MeterRead>,
}

impl ValidatedReads {
    /// Run the V-rules over `batch`, annotate the offending rows, and return
    /// the batch as persistable together with the summary for the ingest
    /// response.
    ///
    /// Takes ownership: a caller that still held the raw batch could persist it
    /// by another route, which is the hole this type exists to close.
    #[must_use]
    pub fn validate(mut batch: Vec<MeterRead>, ctx: IngestContext<'_>) -> (Self, BatchValidation) {
        let summary = validate_and_annotate(&mut batch, ctx);
        (Self { reads: batch }, summary)
    }

    /// The validated rows, for the repository to persist.
    #[must_use]
    pub fn as_slice(&self) -> &[MeterRead] {
        &self.reads
    }

    /// Number of rows in the batch.
    #[must_use]
    pub fn len(&self) -> usize {
        self.reads.len()
    }

    /// `true` when the batch carries no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.reads.is_empty()
    }
}

/// Outcome of running the V-rule engine over a batch, for the ingest response.
pub struct BatchValidation {
    pub issue_count: usize,
    pub billing_block_count: usize,
    pub rules: Vec<String>,
    /// Rules that did **not** run on this batch, as their `Vnn` codes.
    ///
    /// Without this a caller cannot tell "the rule ran and found nothing" from
    /// "the rule never ran". V12 is the live case: it needs the metered plant's
    /// capacity, which edmd holds no master data for, so it is inert on every
    /// door that is not told one — and a batch would otherwise report a clean
    /// bill of health that no implausible-power check stands behind.
    pub skipped_rules: Vec<String>,
}

impl BatchValidation {
    /// `true` when no rule fired.
    pub fn is_clean(&self) -> bool {
        self.issue_count == 0
    }
}

/// One V-rule finding, with the series it is about.
///
/// `interval_index` inside a `ValidationIssue` points into *one register's*
/// slice, so it means nothing on its own — the register has to travel with it.
#[derive(Debug, Clone)]
pub struct RegisterFinding {
    /// The register the finding is about, `None` for unlabelled reads.
    pub obis_code: Option<ObisCode>,
    /// Which row of the caller's batch it names, when it names one.
    pub batch_index: Option<usize>,
    /// The finding itself.
    pub issue: ValidationIssue,
}

/// The rule configuration a series of this commodity and cadence should be
/// judged against.
///
/// Two things were wrong with running `ValidationConfig::default()` over every
/// batch, as this used to:
///
/// - **The default is electricity.** It allows four consecutive zeros before V05
///   calls the meter stuck. A gas heating profile is near zero for a summer
///   week and a vacant flat's water meter reads exactly zero for months, so
///   every heat, water and gas delivery arrived pre-flagged.
///   [`metering::QualityConfig::for_sparte`] exists for precisely this and
///   carries the media-specific zero-run tolerance and sigma floor.
/// - **The default cadence is 900 s.** V06 fires on every interval of an hourly
///   gas series, and V01 divides a real gap by the wrong grid — a one-hour hole
///   in an hourly series is reported as "4 missing intervals". The cadence is
///   observable, so it is measured (`detect_interval_length`) rather than
///   assumed, and only falls back to the Sparte default when the series is too
///   short to have one.
///
/// `period` stays `None`: a pushed batch declares its own span, so leading and
/// trailing gaps are not findings here. Coverage against a *requested* window is
/// the quality scorer's job (`QualityConfig::over_period`), not the ingest
/// validator's.
#[must_use]
pub fn config_for(
    sparte: metering::Sparte,
    series: &[metering::MeterInterval],
    max_plant_power_kw: Option<Decimal>,
) -> ValidationConfig {
    let mut config = metering::QualityConfig::for_sparte(sparte).validation;
    if let Some(resolution) = metering::classification::detect_interval_length(series) {
        config.expected_interval_secs = Some(resolution.nominal_seconds());
    }
    config.now = Some(OffsetDateTime::now_utc());
    config.max_plant_power_kw = max_plant_power_kw;
    config
}

/// Run the V-rule engine over a batch, one series per register.
///
/// The single pass every caller shares. The batch is split by
/// `(Sparte, OBIS register)` and each group validated on its own, because the
/// adjacency rules — V01 gap, V02 overlap — are statements about a *single*
/// series and a MaLo routinely delivers several at once: import beside export on
/// a prosumer MeLo, HT beside NT on a dual-tariff meter. Validated as one flat
/// list, those registers share every timestamp, so V02 reported each same-slot
/// pair as an overlapping interval — severity `Error`, which blocks billing. A
/// bidirectional delivery could not be ingested cleanly at all.
///
/// Findings come back tagged with their register and with the index of the row
/// in `batch` they name, so a caller can attach each to the interval it is
/// actually about.
#[must_use]
pub fn findings(batch: &[MeterRead], max_plant_power_kw: Option<Decimal>) -> Vec<RegisterFinding> {
    findings_with_coverage(batch, max_plant_power_kw).0
}

/// [`findings`], plus the rules that actually ran.
///
/// A clean result means "these rules found nothing", not "nothing is wrong". A
/// rule can be inert because the config gave it nothing to work with — V12
/// needs a plant capacity edmd does not hold as master data — or because the
/// data was too thin for it. Either way the caller has to be able to say so:
/// reporting a spotless batch while V12 never ran is the shape of defect this
/// exists to prevent.
///
/// The set is the **intersection** across registers, so a rule counts as
/// evaluated only where it ran for every series in the batch.
#[must_use]
pub fn findings_with_coverage(
    batch: &[MeterRead],
    max_plant_power_kw: Option<Decimal>,
) -> (Vec<RegisterFinding>, metering::validation::RuleSet) {
    if batch.is_empty() {
        return (Vec::new(), metering::validation::RuleSet::EMPTY);
    }

    // Group the batch's positions by the series each belongs to, preserving the
    // caller's indices so a finding can be attached to the row it is about.
    // Keyed on the Sparte's stable wire label rather than the enum: `Sparte` is
    // not `Ord`, and the label is what a grouping key needs anyway.
    let mut groups: BTreeMap<(&'static str, String), Vec<usize>> = BTreeMap::new();
    for (idx, read) in batch.iter().enumerate() {
        groups
            .entry((
                read.sparte.as_str(),
                crate::domain::normalise_obis_code(read.obis_code.as_deref()),
            ))
            .or_default()
            .push(idx);
    }

    let mut out: Vec<RegisterFinding> = Vec::new();
    let mut evaluated: Option<metering::validation::RuleSet> = None;
    for positions in groups.values() {
        let sparte = batch[positions[0]].sparte;
        // Handed in the caller's order, deliberately. `validate_intervals`
        // evaluates the adjacency rules in timestamp order internally while
        // still reporting the caller's indices, and V11 (`UnorderedSeries`) is
        // its statement that the input arrived shuffled — usually a broken merge
        // upstream. Sorting here would make every batch look ordered and delete
        // that signal for nothing: the findings are already order-correct.
        let series: Vec<metering::MeterInterval> = positions
            .iter()
            .map(|&i| {
                let r = &batch[i];
                metering::MeterInterval {
                    from: r.dtm_from,
                    to: r.dtm_to,
                    value: r.quantity_kwh,
                    // The read's actual quality flag — hardcoding `Measured`
                    // here made V09 (non-billable quality) unfireable on every
                    // ingest path: a batch arriving as FAULTY/UNKNOWN validated
                    // as if it were clean.
                    quality: r.quality,
                    obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
                }
            })
            .collect();
        let obis_code = series.first().and_then(|iv| iv.obis_code);

        let report = metering::validation::validate_intervals(
            &series,
            &config_for(sparte, &series, max_plant_power_kw),
        );
        evaluated = Some(match evaluated {
            Some(acc) => acc.intersection(report.evaluated),
            None => report.evaluated,
        });
        for issue in report.issues {
            // `interval_index` points into this group's slice; map it back onto
            // the caller's batch so the annotation lands on the right row.
            let batch_index = issue.interval_index.and_then(|i| positions.get(i).copied());
            out.push(RegisterFinding {
                obis_code,
                batch_index,
                issue,
            });
        }
    }
    (
        out,
        evaluated.unwrap_or(metering::validation::RuleSet::EMPTY),
    )
}

/// Run the V-rule engine over an ingest batch and annotate the rows each issue
/// describes.
///
/// Every ingest family routes through here so a reading lands with the same
/// quality record whichever door it came in by.
///
/// Validation annotates and never rejects: whether an interval is billable is a
/// separate decision from whether it is stored, and discarding a suspect reading
/// would destroy the evidence the Netzbetreiber needs to resolve it.
///
/// ## An annotation names the interval, not the batch
///
/// A row carries **its own** findings. Copying the whole batch's issue list onto
/// every implicated row — as this used to — makes a downstream § 60 Abs. 2 MsbG
/// substitution decision reread the same month-wide list on each of 2 976
/// intervals and learn nothing about the one in front of it. The batch-level
/// counts stay, because "how bad is this delivery" is a real question too; they
/// are just not the same question.
fn validate_and_annotate(batch: &mut [MeterRead], ctx: IngestContext<'_>) -> BatchValidation {
    let (found, evaluated) = findings_with_coverage(batch, ctx.max_plant_power_kw);

    let issue_count = found.len();
    let billing_block_count = found.iter().filter(|f| f.issue.blocks_billing()).count();
    let skipped_rules: Vec<String> = evaluated
        .complement()
        .iter()
        .map(|r| r.as_str().to_owned())
        .collect();
    if !skipped_rules.is_empty() {
        tracing::debug!(
            malo_id = %ctx.malo_id,
            source = %ctx.source,
            skipped = %skipped_rules.join(","),
            "edmd: validation rules that did not run on this batch"
        );
    }
    let summary = BatchValidation {
        issue_count,
        billing_block_count,
        rules: found
            .iter()
            .map(|f| f.issue.rule_id.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        skipped_rules,
    };

    if issue_count == 0 {
        return summary;
    }

    tracing::warn!(
        malo_id = %ctx.malo_id,
        source = %ctx.source,
        issue_count,
        billing_block_count,
        "edmd: ingest validation issues (§ 60 Abs. 2 MsbG)"
    );

    let mut per_row: BTreeMap<usize, Vec<serde_json::Value>> = BTreeMap::new();
    for f in &found {
        let Some(idx) = f.batch_index else { continue };
        per_row.entry(idx).or_default().push(serde_json::json!({
            "rule": f.issue.rule_id.to_string(),
            "message": f.issue.message,
            "obis_code": f.obis_code.map(|c| c.to_string()),
            "blocks_billing": f.issue.blocks_billing(),
        }));
    }

    for (idx, issues) in per_row {
        let warnings = serde_json::json!({
            "has_warnings": true,
            "issue_count": issues.len(),
            "billing_block_count": issues
                .iter()
                .filter(|i| i["blocks_billing"] == serde_json::Value::Bool(true))
                .count(),
            "has_errors": issues
                .iter()
                .any(|i| i["blocks_billing"] == serde_json::Value::Bool(true)),
            "issues": issues,
            "source": ctx.source,
            // What the delivery as a whole looked like, so a row still answers
            // "was this batch clean" without carrying every other row's findings.
            "batch": {
                "issue_count": issue_count,
                "billing_block_count": billing_block_count,
            },
        });
        // A row may already carry a session-level quality summary from Hampel
        // scoring. The two describe different things, so the rule findings are
        // added alongside it rather than replacing it.
        let read = &mut batch[idx];
        read.quality_warnings = Some(match read.quality_warnings.take() {
            Some(serde_json::Value::Object(mut existing)) => {
                existing.insert("validation".to_owned(), warnings);
                existing.insert("has_warnings".to_owned(), serde_json::Value::Bool(true));
                serde_json::Value::Object(existing)
            }
            _ => warnings,
        });
    }

    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::model::{IngestionSource, MeterRead, QualityFlag, Sparte};
    use rust_decimal::Decimal;
    use time::macros::datetime;

    fn read(
        from: OffsetDateTime,
        minutes: i64,
        value: &str,
        sparte: Sparte,
        obis: Option<&str>,
    ) -> MeterRead {
        MeterRead {
            malo_id: "51238696012".to_owned(),
            melo_id: None,
            dtm_from: from,
            dtm_to: from + time::Duration::minutes(minutes),
            quantity_kwh: Decimal::from_str_exact(value).expect("decimal"),
            quality: QualityFlag::Measured,
            pid: 0,
            sparte,
            obis_code: obis.map(str::to_owned),
            tenant: "9900357000004".to_owned(),
            source: IngestionSource::Mscons,
            push_session: None,
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            mscons_version: None,
        }
    }

    fn ctx<'a>() -> IngestContext<'a> {
        IngestContext::new("TEST", "51238696012")
    }

    fn rules_of(batch: Vec<MeterRead>) -> Vec<String> {
        ValidatedReads::validate(batch, ctx()).1.rules
    }

    /// A prosumer MeLo delivers import and export for the same quarter-hour.
    ///
    /// Validated as one flat list they share every timestamp, so V02 called each
    /// pair an overlap — an `Error`, which blocks billing. Every bidirectional
    /// delivery was unbillable on arrival.
    #[test]
    fn two_registers_at_the_same_slot_are_not_an_overlap() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let batch = vec![
            read(slot, 15, "5.0", Sparte::Strom, Some("1-0:1.8.0")),
            read(slot, 15, "3.0", Sparte::Strom, Some("1-0:2.8.0")),
        ];
        assert!(
            !rules_of(batch).contains(&"V02".to_owned()),
            "import and export are two series, not one series overlapping itself"
        );
    }

    /// The same register twice at one slot still is an overlap.
    #[test]
    fn one_register_delivered_twice_for_a_slot_is_still_an_overlap() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let batch = vec![
            read(slot, 15, "5.0", Sparte::Strom, Some("1-0:1.8.0")),
            read(slot, 15, "6.0", Sparte::Strom, Some("1-0:1.8.0*255")),
        ];
        assert!(
            rules_of(batch).contains(&"V02".to_owned()),
            "`*255` is the same register, so this is a genuine duplicate delivery"
        );
    }

    /// An hourly gas series is not a broken quarter-hour one.
    ///
    /// With the hardcoded 900 s grid every gas interval tripped V06, and any
    /// real gap was divided by the wrong step.
    #[test]
    fn an_hourly_gas_series_does_not_trip_the_interval_length_rule() {
        let base = datetime!(2026-07-01 04:00 UTC);
        let batch: Vec<MeterRead> = (0..24)
            .map(|i| {
                read(
                    base + time::Duration::hours(i),
                    60,
                    "12.5",
                    Sparte::Gas,
                    Some("7-1:99.33.17"),
                )
            })
            .collect();
        let rules = rules_of(batch);
        assert!(
            !rules.contains(&"V06".to_owned()),
            "the cadence is observed, not assumed: {rules:?}"
        );
        assert!(!rules.contains(&"V01".to_owned()), "no gaps: {rules:?}");
    }

    /// A vacant flat's water meter reads zero for weeks; that is not a fault.
    ///
    /// The electricity default tolerates four consecutive zeros, so every quiet
    /// water and heat series arrived pre-flagged as a stuck meter.
    #[test]
    fn a_long_zero_run_on_a_water_meter_is_not_a_stuck_meter() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let batch: Vec<MeterRead> = (0..48)
            .map(|i| {
                read(
                    base + time::Duration::hours(i),
                    60,
                    "0",
                    Sparte::Wasser,
                    Some("8-0:1.0.0"),
                )
            })
            .collect();
        let rules = rules_of(batch);
        assert!(
            !rules.contains(&"V05".to_owned()),
            "water has no standby floor — a zero run is ordinary: {rules:?}"
        );

        // The same run on electricity *is* a finding: a household never draws
        // exactly nothing for twelve hours.
        let strom: Vec<MeterRead> = (0..48)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    "0",
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        assert!(rules_of(strom).contains(&"V05".to_owned()));
    }

    /// A genuine gap is still reported, and the annotation lands on a real row.
    #[test]
    fn a_gap_is_still_detected_and_annotated() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let mut batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    "2.0",
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        batch.extend((12..20).map(|i| {
            read(
                base + time::Duration::minutes(15 * i),
                15,
                "2.0",
                Sparte::Strom,
                Some("1-0:1.8.0"),
            )
        }));
        let (validated, summary) = ValidatedReads::validate(batch, ctx());
        assert!(summary.rules.contains(&"V01".to_owned()));
        assert!(summary.billing_block_count > 0, "a gap blocks billing");
        assert_eq!(validated.len(), 16, "validation annotates, never drops");
    }

    /// V12 fires only when the caller states the plant's capacity — and it must
    /// actually fire when they do.
    #[test]
    fn implausible_power_needs_a_declared_capacity() {
        let base = datetime!(2026-07-01 00:00 UTC);
        // 100 kWh in a quarter-hour is 400 kW average.
        let batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    if i == 4 { "100.0" } else { "2.0" },
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();

        let silent = ValidatedReads::validate(batch.clone(), ctx()).1.rules;
        assert!(
            !silent.contains(&"V12".to_owned()),
            "no ceiling was declared, so there is nothing to be implausible against"
        );

        let with_ceiling =
            ValidatedReads::validate(batch, ctx().with_capacity_kw(Some(Decimal::from(30))))
                .1
                .rules;
        assert!(
            with_ceiling.contains(&"V12".to_owned()),
            "400 kW average through a 30 kW connection is impossible: {with_ceiling:?}"
        );
    }

    /// A row carries its own findings, not the whole batch's.
    #[test]
    fn an_annotation_names_the_interval_it_is_about() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let mut batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    "2.0",
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        // A second register with a gap of its own, so the batch carries findings
        // about two different series.
        batch.push(read(base, 15, "-1.0", Sparte::Strom, Some("1-0:2.8.0")));

        let (validated, summary) = ValidatedReads::validate(batch, ctx());
        assert!(summary.rules.contains(&"V03".to_owned()));

        let annotated = validated
            .as_slice()
            .iter()
            .find(|r| r.obis_code.as_deref() == Some("1-0:2.8.0"))
            .expect("the negative reading is annotated");
        let issues = annotated.quality_warnings.as_ref().expect("warnings")["issues"]
            .as_array()
            .expect("issue array")
            .clone();
        assert!(
            issues
                .iter()
                .all(|i| i["obis_code"] == serde_json::json!("1-0:2.8.0")),
            "a row's findings are its own register's: {issues:?}"
        );
    }

    /// Findings survive an unsorted input, and V11 says the input was unsorted.
    #[test]
    fn an_unordered_batch_is_reported_and_still_validated_in_order() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let mut batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    "2.0",
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        batch.reverse();
        let rules = rules_of(batch);
        assert!(rules.contains(&"V11".to_owned()), "{rules:?}");
        assert!(
            !rules.contains(&"V01".to_owned()),
            "a reversed complete series has no gaps: {rules:?}"
        );
    }

    /// A clean batch says which rules stood behind that verdict.
    ///
    /// V12 needs the metered plant's capacity, and edmd holds none as master
    /// data — so on every door that is not told one, the rule is inert. A report
    /// that could not distinguish "no implausible power" from "nobody looked"
    /// let an Error-severity rule sit switched off while the docs, the API and
    /// the operator guide all described it as active.
    #[test]
    fn a_clean_batch_names_the_rules_that_did_not_run() {
        use metering::validation::ValidationRuleId;

        let base = datetime!(2026 - 07 - 01 00:00 UTC);
        let batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    "2.0",
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();

        // No ceiling supplied — V12 cannot fire.
        let (found, evaluated) = findings_with_coverage(&batch, None);
        assert!(found.is_empty(), "this batch is clean: {found:?}");
        assert!(
            !evaluated.contains(ValidationRuleId::ImplausiblePower),
            "without a capacity ceiling V12 must be reported as skipped"
        );

        // Supplied — V12 runs, and the same clean batch now says so.
        let (_, evaluated) = findings_with_coverage(&batch, Some(Decimal::from(30)));
        assert!(
            evaluated.contains(ValidationRuleId::ImplausiblePower),
            "a capacity ceiling must bring V12 into the evaluated set"
        );
    }
}
