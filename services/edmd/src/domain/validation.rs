//! Validated ingest batches.
//!
//! `TimeSeriesRepository::store_reads` accepts [`ValidatedReads`] rather than a
//! bare slice, so a new ingest path cannot persist meter data without running
//! the validation first: the only way to obtain the type is to call
//! [`ValidatedReads::validate`], and its field is private to this module.
//!
//! The failure is silent otherwise: an unvalidated path stores rows
//! indistinguishable from validated ones, and `NEGATIVE`, `SPIKE` and
//! `NON_BILLABLE` never fire for that source.
//!
//! ## One register split, every caller
//!
//! [`findings`] is the single validation pass. It splits by `(Sparte, register)`,
//! builds each group into a `metering` series on its observed grid, and returns
//! the findings tagged with the register and the rows they are about. Ingest
//! annotation ([`ValidatedReads::validate`]), the MCP `validate_timeseries`
//! tool and the § 60 Abs. 1 substitute path all go through it, so they cannot
//! disagree about whether an ordinary hourly gas delivery is clean.
//!
//! ## The rule set is `metering`'s
//!
//! The content rules are `metering::vee::validation`'s — `GAP`,
//! `NON_BILLABLE`, `NEGATIVE`, `CAPACITY`, `FUTURE`, `ZERO_RUN`, `STALE`,
//! `SPIKE` and the rest — with the thresholds its preset for the batch's
//! Sparte carries. Before they can run, a register's intervals have to form a
//! series: a slot delivered twice, overlapping spans, a span off the grid or a
//! read with no span at all is a **structural** finding
//! ([`SeriesDefect`](crate::domain::vee::SeriesDefect)) and the content rules
//! are reported as not run for that register. Nothing here enumerates the
//! rules: the findings are stored by their own code.

use std::collections::BTreeMap;

use crate::domain::model::MeterRead;
use crate::domain::vee;
use metering::ObisCode;
use metering::vee::validation::{Rule, Severity};
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
    /// Physical capacity ceiling in kW for **`CAPACITY`**.
    ///
    /// Nameplate capacity or Anschlussleistung. A value whose average power over
    /// its own interval exceeds it is not unusual, it is impossible — which is
    /// why `CAPACITY` is an error and `SPIKE` a warning.
    ///
    /// `None` switches the rule off, and the report says so. edmd holds no
    /// master data of its own, so the ceiling comes from the caller that does —
    /// the head-end, the MSB's push, the bulk import.
    pub max_plant_power_kw: Option<Decimal>,
}

impl<'a> IngestContext<'a> {
    /// A context with no capacity ceiling — `CAPACITY` stays off.
    #[must_use]
    pub fn new(source: &'a str, malo_id: &'a str) -> Self {
        Self {
            source,
            malo_id,
            max_plant_power_kw: None,
        }
    }

    /// Declare the metered plant's physical capacity, enabling `CAPACITY`.
    #[must_use]
    pub fn with_capacity_kw(mut self, kw: Option<Decimal>) -> Self {
        self.max_plant_power_kw = kw.filter(|v| *v > Decimal::ZERO);
        self
    }
}

/// The window a batch actually covers, as `(earliest start, latest end)`.
///
/// Taken as min/max rather than off the first and last row: a batch is not
/// required to arrive sorted, and reading the extent off the ends of an
/// unsorted slice reports a period the delivery does not have. That period is the denominator of the quality score and the
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
    /// Rules that did **not** run on this batch, as their codes.
    ///
    /// Without this a caller cannot tell "the rule ran and found nothing" from
    /// "the rule never ran". `CAPACITY` is the live case: it needs the metered
    /// plant's capacity, which edmd holds no master data for, so it is inert on
    /// every door that is not told one — and a batch would otherwise report a
    /// clean bill of health that no implausible-power check stands behind.
    pub skipped_rules: Vec<String>,
}

impl BatchValidation {
    /// `true` when no rule fired.
    pub fn is_clean(&self) -> bool {
        self.issue_count == 0
    }
}

/// One finding, with the register and the rows it is about.
#[derive(Debug, Clone)]
pub struct RegisterFinding {
    /// The register the finding is about, `None` for unlabelled reads.
    pub obis_code: Option<ObisCode>,
    /// The rows of the caller's batch it covers. Empty for a `GAP`, which is
    /// about slots no row delivered.
    pub rows: Vec<usize>,
    /// The rule's code, or the structural defect's.
    pub code: &'static str,
    /// How serious it is.
    pub severity: Severity,
    /// A sentence for a human.
    pub message: String,
    /// The span it concerns, when it names one.
    pub span: Option<(OffsetDateTime, OffsetDateTime)>,
}

impl RegisterFinding {
    /// `true` when the slots it covers must not be billed as they are.
    #[must_use]
    pub fn blocks_billing(&self) -> bool {
        vee::blocks_billing(self.severity)
    }
}

/// Run the validation over a batch, one series per register.
///
/// The single pass every caller shares. The batch is split by
/// `(Sparte, OBIS register)` and each group validated on its own, because a
/// series is one channel and a MaLo routinely delivers several at once: import
/// beside export on a prosumer MeLo, HT beside NT on a dual-tariff meter.
/// Validated as one flat list, those registers share every timestamp and would
/// read as the same slot delivered twice.
///
/// Each register is validated over **its own span**: a pushed batch declares
/// the window it covers, so its leading and trailing slots are not gaps.
///
/// Findings come back tagged with their register and with the rows of `batch`
/// they cover, so a caller can attach each to the intervals it is actually
/// about.
#[must_use]
pub fn findings(batch: &[MeterRead], max_plant_power_kw: Option<Decimal>) -> Vec<RegisterFinding> {
    findings_with_coverage(batch, max_plant_power_kw).0
}

/// [`findings`], plus the rules that actually ran.
///
/// A clean result means "these rules found nothing", not "nothing is wrong". A
/// rule can be inert because nothing switched it on — `CAPACITY` needs a plant
/// capacity edmd does not hold as master data — or because a register's
/// intervals were not a series it could run on. Either way the caller has to be
/// able to say so.
///
/// The set is the **intersection** across registers, so a rule counts as
/// evaluated only where it ran for every series in the batch.
#[must_use]
pub fn findings_with_coverage(
    batch: &[MeterRead],
    max_plant_power_kw: Option<Decimal>,
) -> (Vec<RegisterFinding>, Vec<Rule>) {
    if batch.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let now = OffsetDateTime::now_utc();

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
    let mut evaluated: Option<Vec<Rule>> = None;
    let mut narrow = |ran: Vec<Rule>| {
        evaluated = Some(match evaluated.take() {
            Some(acc) => acc.into_iter().filter(|r| ran.contains(r)).collect(),
            None => ran,
        });
    };
    for positions in groups.values() {
        let sparte = batch[positions[0]].sparte;
        let obis_code: Option<ObisCode> = batch[positions[0]]
            .obis_code
            .as_deref()
            .and_then(|s| s.parse().ok());
        let structural = |code: &'static str, rows: Vec<usize>, message: String| RegisterFinding {
            obis_code,
            rows,
            code,
            severity: Severity::Blocking,
            message,
            span: None,
        };

        // A read with no span carries no energy over time and cannot join a
        // series; it is a finding of its own.
        let mut intervals = Vec::with_capacity(positions.len());
        let mut rows = Vec::with_capacity(positions.len());
        for &i in positions {
            match batch[i].interval() {
                Ok(iv) => {
                    intervals.push(iv);
                    rows.push(i);
                }
                Err(e) => out.push(structural("EMPTY_SPAN", vec![i], e.to_string())),
            }
        }
        let (Some(from), Some(to)) = (
            intervals.iter().map(metering::MeterInterval::from).min(),
            intervals.iter().map(metering::MeterInterval::to).max(),
        ) else {
            narrow(Vec::new());
            continue;
        };

        let series = match vee::series_of(sparte, intervals) {
            Ok(series) => series,
            Err(defect) => {
                let instants = defect.instants();
                let named = rows
                    .iter()
                    .copied()
                    .filter(|&i| instants.contains(&batch[i].dtm_from))
                    .collect();
                out.push(structural(defect.code(), named, defect.to_string()));
                narrow(Vec::new());
                continue;
            }
        };
        let Some(report) = vee::validate_window(&series, sparte, from, to, now, max_plant_power_kw)
        else {
            out.push(structural(
                "OUT_OF_RANGE",
                rows,
                format!("{from} – {to} lies outside the supported calendar"),
            ));
            narrow(Vec::new());
            continue;
        };
        narrow(report.evaluated());
        for f in report.findings() {
            let covered = if f.rule == Rule::Gap {
                Vec::new()
            } else {
                rows.iter()
                    .copied()
                    .filter(|&i| batch[i].dtm_from < f.to && batch[i].dtm_to > f.from)
                    .collect()
            };
            out.push(RegisterFinding {
                obis_code,
                rows: covered,
                code: f.rule.as_str(),
                severity: f.severity(),
                message: f.message.clone(),
                span: Some((f.from, f.to)),
            });
        }
    }
    (out, evaluated.unwrap_or_default())
}

/// Validate an ingest batch and annotate the rows each finding
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
/// every implicated row would make a downstream § 60 Abs. 1 MsbG substitution
/// decision reread the same month-wide list on each of 2 976 intervals and learn
/// nothing about the one in front of it. The batch-level
/// counts stay, because "how bad is this delivery" is a real question too; they
/// are just not the same question.
fn validate_and_annotate(batch: &mut [MeterRead], ctx: IngestContext<'_>) -> BatchValidation {
    let (found, evaluated) = findings_with_coverage(batch, ctx.max_plant_power_kw);

    let issue_count = found.len();
    let billing_block_count = found.iter().filter(|f| f.blocks_billing()).count();
    let skipped_rules: Vec<String> = Rule::ALL
        .into_iter()
        .filter(|r| !evaluated.contains(r))
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
            .map(|f| f.code.to_owned())
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
        "edmd: ingest validation issues (§ 60 Abs. 1 MsbG)"
    );

    let mut per_row: BTreeMap<usize, Vec<serde_json::Value>> = BTreeMap::new();
    for f in &found {
        for &idx in &f.rows {
            per_row.entry(idx).or_default().push(serde_json::json!({
                "rule": f.code,
                "severity": f.severity.as_str(),
                "message": f.message,
                "obis_code": f.obis_code.map(|c| c.to_string()),
                "blocks_billing": f.blocks_billing(),
            }));
        }
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
        // A row may already carry a session-level quality summary from batch
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
    /// Validated as one flat list they share every timestamp and would read as
    /// one slot delivered twice — a structural defect that stops every content
    /// rule. Two registers are two series.
    #[test]
    fn two_registers_at_the_same_slot_are_not_a_duplicate() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let batch = vec![
            read(slot, 15, "5.0", Sparte::Strom, Some("1-0:1.8.0")),
            read(slot, 15, "3.0", Sparte::Strom, Some("1-0:2.8.0")),
        ];
        let rules = rules_of(batch);
        assert!(
            !rules.contains(&"DUPLICATE".to_owned()),
            "import and export are two series, not one series delivered twice: {rules:?}"
        );
    }

    /// The same register twice at one slot is a duplicate delivery.
    #[test]
    fn one_register_delivered_twice_for_a_slot_is_a_duplicate() {
        let slot = datetime!(2026-07-01 10:00 UTC);
        let batch = vec![
            read(slot, 15, "5.0", Sparte::Strom, Some("1-0:1.8.0")),
            read(slot, 15, "6.0", Sparte::Strom, Some("1-0:1.8.0*255")),
        ];
        let (validated, summary) = ValidatedReads::validate(batch, ctx());
        assert!(
            summary.rules.contains(&"DUPLICATE".to_owned()),
            "`*255` is the same register, so this is a genuine duplicate delivery"
        );
        assert!(summary.billing_block_count > 0);
        assert!(
            validated
                .as_slice()
                .iter()
                .all(|r| r.quality_warnings.is_some()),
            "the defect names both rows"
        );
    }

    /// An hourly gas series is not a broken quarter-hour one.
    ///
    /// The grid is observed, not assumed: an hourly gas delivery is a series on
    /// an hourly grid with no gaps.
    #[test]
    fn an_hourly_gas_series_is_validated_on_its_own_grid() {
        // 05:00 UTC is 07:00 Berlin, inside the Gastag.
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
        assert!(rules.is_empty(), "a clean hourly gas day: {rules:?}");
    }

    /// A vacant flat's water meter reads zero for weeks; that is not a fault.
    ///
    /// The electricity preset reports a zero run of an hour, which would flag
    /// every quiet water and heat series as a stuck meter.
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
            !rules.contains(&"ZERO_RUN".to_owned()),
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
        assert!(rules_of(strom).contains(&"ZERO_RUN".to_owned()));
    }

    /// A genuine gap is reported, blocks billing, and drops no row.
    #[test]
    fn a_gap_is_detected_and_blocks_billing() {
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
        assert!(summary.rules.contains(&"GAP".to_owned()));
        assert!(summary.billing_block_count > 0, "a gap blocks billing");
        assert_eq!(validated.len(), 16, "validation annotates, never drops");
    }

    /// `CAPACITY` fires only when the caller states the plant's capacity — and
    /// it must actually fire when they do.
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
            !silent.contains(&"CAPACITY".to_owned()),
            "no ceiling was declared, so there is nothing to be implausible against"
        );

        let with_ceiling =
            ValidatedReads::validate(batch, ctx().with_capacity_kw(Some(Decimal::from(30))))
                .1
                .rules;
        assert!(
            with_ceiling.contains(&"CAPACITY".to_owned()),
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
        // A second register with a negative value, so the batch carries findings
        // about two different series.
        batch.push(read(base, 15, "-1.0", Sparte::Strom, Some("1-0:2.8.0")));

        let (validated, summary) = ValidatedReads::validate(batch, ctx());
        assert!(summary.rules.contains(&"NEGATIVE".to_owned()));

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

    /// Delivery order is not a finding: a reversed complete series validates
    /// exactly as the sorted one does.
    #[test]
    fn an_unordered_batch_validates_like_the_sorted_one() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let mut batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    &format!("2.{i}"),
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        batch.reverse();
        let rules = rules_of(batch);
        assert!(
            rules.is_empty(),
            "a reversed complete series is clean: {rules:?}"
        );
    }

    /// A read with no span is refused as a finding of its own, and the rest of
    /// the register is still validated.
    #[test]
    fn a_read_without_a_span_is_a_structural_finding() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let mut batch: Vec<MeterRead> = (0..4)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    &format!("2.{i}"),
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();
        batch.push(read(base, 0, "2.0", Sparte::Strom, Some("1-0:1.8.0")));
        let rules = rules_of(batch);
        assert_eq!(rules, vec!["EMPTY_SPAN".to_owned()]);
    }

    /// A clean batch says which rules stood behind that verdict.
    ///
    /// `CAPACITY` needs the metered plant's capacity, and edmd holds none as
    /// master data — so on every door that is not told one, the rule is inert.
    /// A report that could not distinguish "no implausible power" from "nobody
    /// looked" would let an error-severity rule sit switched off unannounced.
    #[test]
    fn a_clean_batch_names_the_rules_that_did_not_run() {
        let base = datetime!(2026-07-01 00:00 UTC);
        let batch: Vec<MeterRead> = (0..8)
            .map(|i| {
                read(
                    base + time::Duration::minutes(15 * i),
                    15,
                    &format!("2.{i}"),
                    Sparte::Strom,
                    Some("1-0:1.8.0"),
                )
            })
            .collect();

        // No ceiling supplied — CAPACITY cannot fire.
        let (found, evaluated) = findings_with_coverage(&batch, None);
        assert!(found.is_empty(), "this batch is clean: {found:?}");
        assert!(
            !evaluated.contains(&Rule::Capacity),
            "without a capacity ceiling CAPACITY must be reported as skipped"
        );

        // Supplied — CAPACITY runs, and the same clean batch now says so.
        let (_, evaluated) = findings_with_coverage(&batch, Some(Decimal::from(30)));
        assert!(
            evaluated.contains(&Rule::Capacity),
            "a capacity ceiling must bring CAPACITY into the evaluated set"
        );
    }
}
