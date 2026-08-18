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
//! ## The rule set is `metering`\'s, not a fixed V01–V10
//!
//! `metering` 0.18 runs **V01–V09, V11 and V12**. There is no V10: it was a
//! "register rollover" rule comparing consecutive interval energies, which is
//! meaningless because a [`MeterInterval`](metering::MeterInterval) carries the
//! energy *in* an interval and not a cumulative Zählerstand — for it to fire, one
//! quarter-hour would have had to carry 50 MWh. The number is left unused so a
//! stored `V10` finding cannot be silently reinterpreted. V11 reports an
//! unordered input series and V12 an average power above the plant\'s physical
//! capacity. Nothing here enumerates the rules: the set is whatever the crate
//! runs, and the findings are stored by their own `rule_id`.

use std::collections::BTreeMap;

use crate::domain::model::MeterRead;
use metering::validation::{ValidationConfig, ValidationIssue};
use time::OffsetDateTime;

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
    pub fn validate(
        mut batch: Vec<MeterRead>,
        source: &str,
        malo_id: &str,
    ) -> (Self, BatchValidation) {
        let summary = validate_and_annotate(&mut batch, source, malo_id);
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
}

impl BatchValidation {
    /// `true` when no rule fired.
    pub fn is_clean(&self) -> bool {
        self.issue_count == 0
    }
}

/// The register a reading belongs to, canonicalised.
///
/// `1-0:1.8.0` and `1-0:1.8.0*255` are the same register, so they must land in
/// the same validation group; anything the OBIS parser rejects keeps its raw
/// spelling rather than collapsing into the no-register bucket.
fn register_key(obis_code: Option<&str>) -> String {
    obis_code.map_or_else(String::new, |s| {
        s.parse::<metering::obis::ObisCode>()
            .map_or_else(|_| s.to_owned(), |c| c.to_string())
    })
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
///   [`QualityConfig::for_sparte`] exists for precisely this and carries the
///   media-specific zero-run tolerance and sigma floor.
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
fn config_for(sparte: metering::Sparte, series: &[metering::MeterInterval]) -> ValidationConfig {
    let mut config = metering::QualityConfig::for_sparte(sparte).validation;
    if let Some(resolution) = metering::classification::detect_interval_length(series) {
        config.expected_interval_secs = Some(resolution.nominal_seconds());
    }
    config.now = Some(OffsetDateTime::now_utc());
    config
}

/// Run the V-rule engine over an ingest batch and annotate the rows each issue
/// describes.
///
/// Every ingest family routes through here so a reading lands with the same
/// quality record whichever door it came in by. Issues are attached to the rows
/// they name rather than to the MaLo as a whole, so a downstream § 60 Abs. 2 MsbG
/// substitution decision can see which intervals are actually implicated.
///
/// Validation annotates and never rejects: whether an interval is billable is a
/// separate decision from whether it is stored, and discarding a suspect reading
/// would destroy the evidence the Netzbetreiber needs to resolve it.
///
/// ## One series per register
///
/// The batch is split by `(Sparte, OBIS register)` and each group validated on
/// its own. The adjacency rules — V01 gap, V02 overlap — are statements about a
/// *single* series, and a MaLo routinely delivers several at once: import beside
/// export on a prosumer MeLo, HT beside NT on a dual-tariff meter. Validated as
/// one flat list, those registers share every timestamp, so V02 reported each
/// same-slot pair as an overlapping interval — severity `Error`, which blocks
/// billing. A bidirectional delivery could not be ingested cleanly at all.
fn validate_and_annotate(batch: &mut [MeterRead], source: &str, malo_id: &str) -> BatchValidation {
    if batch.is_empty() {
        return BatchValidation {
            issue_count: 0,
            billing_block_count: 0,
            rules: Vec::new(),
        };
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
                register_key(read.obis_code.as_deref()),
            ))
            .or_default()
            .push(idx);
    }

    let mut issues: Vec<(usize, ValidationIssue)> = Vec::new();
    let mut unanchored: Vec<ValidationIssue> = Vec::new();

    for positions in groups.values() {
        let sparte = batch[positions[0]].sparte;
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

        let report =
            metering::validation::validate_intervals(&series, &config_for(sparte, &series));
        for issue in report.issues {
            // `interval_index` points into this group's slice; map it back onto
            // the caller's batch so the annotation lands on the right row.
            match issue.interval_index.and_then(|i| positions.get(i).copied()) {
                Some(batch_idx) => issues.push((batch_idx, issue)),
                None => unanchored.push(issue),
            }
        }
    }

    let all = || issues.iter().map(|(_, i)| i).chain(unanchored.iter());
    let issue_count = all().count();
    let billing_block_count = all().filter(|i| i.blocks_billing()).count();
    let summary = BatchValidation {
        issue_count,
        billing_block_count,
        rules: all()
            .map(|i| i.rule_id.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };

    if issue_count == 0 {
        return summary;
    }

    let render = |i: &ValidationIssue| {
        serde_json::json!({
            "rule": i.rule_id.to_string(),
            "message": i.message,
            "blocks_billing": i.blocks_billing(),
        })
    };
    let batch_wide: Vec<serde_json::Value> = all().map(render).collect();
    let warnings = serde_json::json!({
        "has_warnings": true,
        "issue_count": issue_count,
        "billing_block_count": billing_block_count,
        "has_errors": billing_block_count > 0,
        "issues": batch_wide,
        "source": source,
    });

    tracing::warn!(
        malo_id = %malo_id,
        source = %source,
        issue_count,
        billing_block_count,
        "edmd: ingest validation issues (§ 60 Abs. 2 MsbG)"
    );

    let annotated: std::collections::BTreeSet<usize> = issues.iter().map(|(i, _)| *i).collect();
    for idx in annotated {
        // A row may already carry a session-level quality summary from Hampel
        // scoring. The two describe different things, so the rule findings are
        // added alongside it rather than replacing it.
        let read = &mut batch[idx];
        read.quality_warnings = Some(match read.quality_warnings.take() {
            Some(serde_json::Value::Object(mut existing)) => {
                existing.insert("validation".to_owned(), warnings.clone());
                existing.insert("has_warnings".to_owned(), serde_json::Value::Bool(true));
                serde_json::Value::Object(existing)
            }
            _ => warnings.clone(),
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

    fn rules_of(batch: Vec<MeterRead>) -> Vec<String> {
        crate::domain::validation::ValidatedReads::validate(batch, "TEST", "51238696012")
            .1
            .rules
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
        let (validated, summary) =
            crate::domain::validation::ValidatedReads::validate(batch, "TEST", "51238696012");
        assert!(summary.rules.contains(&"V01".to_owned()));
        assert!(summary.billing_block_count > 0, "a gap blocks billing");
        assert_eq!(validated.len(), 16, "validation annotates, never drops");
    }
}
