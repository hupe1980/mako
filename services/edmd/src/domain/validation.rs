//! Validated ingest batches.
//!
//! `TimeSeriesRepository::store_reads` accepts [`ValidatedReads`] rather than a
//! bare slice, so a new ingest path cannot persist meter data without running
//! V01–V10 first: the only way to obtain the type is to call
//! [`ValidatedReads::validate`], and its field is private to this module.
//!
//! That matters because the failure is silent. An unvalidated path stores rows
//! indistinguishable from validated ones, and V03 (negative energy), V04
//! (impossible spike) and V09 (non-billable quality) simply never fire for that
//! source — while § 147 AO / GoBD requires billed data to have been validated.
//! Making the state unrepresentable is cheaper than remembering to check.

use crate::domain::model::MeterRead;
use time::OffsetDateTime;

/// A batch that has been through the V01–V10 engine and annotated.
///
/// Constructing one runs the validation; there is no other constructor.
pub struct ValidatedReads {
    reads: Vec<MeterRead>,
}

impl ValidatedReads {
    /// Run V01–V10 over `batch`, annotate the offending rows, and return the
    /// batch as persistable together with the summary for the ingest response.
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

/// Outcome of running the V01–V10 engine over a batch, for the ingest response.
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

/// Run V01–V10 over an ingest batch and annotate the rows each issue describes.
///
/// Every ingest family routes through here so a reading lands with the same
/// quality record whichever door it came in by. Issues are attached to the rows
/// they name rather than to the MaLo as a whole, so a downstream § 60 Abs. 2 MsbG
/// substitution decision can see which intervals are actually implicated.
///
/// Validation annotates and never rejects: whether an interval is billable is a
/// separate decision from whether it is stored, and discarding a suspect reading
/// would destroy the evidence the Netzbetreiber needs to resolve it.
fn validate_and_annotate(batch: &mut [MeterRead], source: &str, malo_id: &str) -> BatchValidation {
    if batch.is_empty() {
        return BatchValidation {
            issue_count: 0,
            billing_block_count: 0,
            rules: Vec::new(),
        };
    }

    let to_validate: Vec<metering::MeterInterval> = batch
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            // The read's actual quality flag — hardcoding `Measured` here made
            // V09 (non-billable quality) unfireable on every ingest path: a
            // batch arriving as FAULTY/UNKNOWN validated as if it were clean.
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        })
        .collect();

    let report = metering::validation::validate_intervals(
        &to_validate,
        &metering::validation::ValidationConfig {
            now: Some(OffsetDateTime::now_utc()),
            ..Default::default()
        },
    );

    let summary = BatchValidation {
        issue_count: report.issues.len(),
        billing_block_count: report.billing_block_count(),
        rules: report
            .issues
            .iter()
            .map(|i| i.rule_id.to_string())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
    };

    if report.is_clean() {
        return summary;
    }

    let warnings = serde_json::json!({
        "has_warnings": true,
        "issue_count": report.issues.len(),
        "billing_block_count": report.billing_block_count(),
        "has_errors": report.has_errors(),
        "issues": report.issues.iter().map(|i| serde_json::json!({
            "rule": i.rule_id.to_string(),
            "message": i.message,
            "blocks_billing": i.blocks_billing(),
        })).collect::<Vec<_>>(),
        "source": source,
    });

    tracing::warn!(
        malo_id = %malo_id,
        source = %source,
        issue_count = report.issues.len(),
        billing_block_count = report.billing_block_count(),
        "edmd: ingest validation issues (§ 60 Abs. 2 MsbG)"
    );

    for (idx, read) in batch.iter_mut().enumerate() {
        if !report.issues.iter().any(|i| i.interval_index == Some(idx)) {
            continue;
        }
        // A row may already carry a session-level quality summary from Hampel
        // scoring. The two describe different things, so the rule findings are
        // added alongside it rather than replacing it.
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
