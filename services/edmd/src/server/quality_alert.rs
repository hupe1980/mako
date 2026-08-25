//! One place that raises `de.messwert.reading.quality.warning`.
//!
//! # Why this is shared rather than per-handler
//!
//! A batch can fail two independent ways: the **Hampel scorer** flags a session
//! as grade C/F, and the **V-rule engine** annotates individual intervals
//! (V03 negative energy, V04 impossible spike, V09 non-billable quality). Only
//! the first used to raise the event, and only on the RLM direct-push door — so
//! a `FAULTY` reading with an unremarkable statistical profile, or any reading
//! arriving over IoT push, bulk import or Kafka, was annotated in
//! `quality_warnings` and then went quiet.
//!
//! Quiet is the problem. The event is what starts `agentd`'s `meter-data-agent`
//! (grade-F investigation) and `replacement-value-agent` (§ 60 Abs. 2 MsbG
//! Ersatzwertbildung). A billing-blocking interval nobody is told about sits in
//! the store until a settlement run trips over it — by which point the
//! measurement window that could have been re-read has closed.
//!
//! So the trigger lives here, is the union of both signals, and every ingest
//! door calls it with the same [`QualityAlert`].

use crate::domain::validation::BatchValidation;
use time::OffsetDateTime;

/// What an ingest door reports about one stored batch.
pub(crate) struct QualityAlert<'a> {
    pub malo_id: &'a str,
    /// Which ingest door this batch came through — `rlm-direct-push`,
    /// `iot-push`, `bulk-import`, `kafka-ingest`, `mscons`. Carried on the event
    /// so a recipient can tell a head-end system's feed from an operator upload
    /// without correlating back to edmd.
    pub door: &'a str,
    pub correlation_id: &'a str,
    pub causation_id: &'a str,
    pub sparte: Option<&'a str>,
    pub period_from: Option<OffsetDateTime>,
    pub period_to: Option<OffsetDateTime>,
    /// V-rule outcome for the batch.
    pub validation: &'a BatchValidation,
    /// The Hampel session summary, where the door computes one.
    pub hampel: Option<serde_json::Value>,
}

impl QualityAlert<'_> {
    /// Whether either signal fired. Doors use this for their own `202` vs `201`
    /// decision, so the status code and the event can never disagree.
    pub(crate) fn is_warning(&self) -> bool {
        !self.validation.is_clean()
            || self
                .hampel
                .as_ref()
                .and_then(|h| h["has_warnings"].as_bool())
                .unwrap_or(false)
    }
}

/// Raise the warning if either signal fired; do nothing otherwise.
///
/// Delivery failure is logged, not propagated: the readings are already stored
/// and a lost notification must not turn a successful ingest into an error the
/// sender will retry, which would duplicate nothing but would hide the store.
pub(crate) async fn raise_quality_warning(
    webhook_url: Option<&str>,
    secret: Option<&[u8]>,
    tenant: &str,
    alert: &QualityAlert<'_>,
) {
    if !alert.is_warning() {
        return;
    }
    let Some(url) = webhook_url else {
        // No ERP webhook configured. Still record it — an operator reading logs
        // after a settlement surprise needs to see the finding was made.
        tracing::warn!(
            malo_id = %alert.malo_id,
            door = %alert.door,
            billing_blocks = alert.validation.billing_block_count,
            rules = ?alert.validation.rules,
            "edmd: quality warning raised but no ERP webhook is configured"
        );
        return;
    };

    let ce = mako_service::CloudEvent::new(
        mako_service::source("edmd", tenant),
        mako_events::messwert::READING_QUALITY_WARNING,
        alert.malo_id,
        serde_json::json!({
            "malo_id": alert.malo_id,
            "ingest_door": alert.door,
            "sparte": alert.sparte,
            "period_from": alert.period_from.map(|t| t.date().to_string()),
            "period_to": alert.period_to.map(|t| t.date().to_string()),
            "quality": alert.hampel,
            "validation": {
                "issue_count": alert.validation.issue_count,
                "billing_block_count": alert.validation.billing_block_count,
                "rules": alert.validation.rules,
            },
            "legal_basis": "§ 60 Abs. 2 MsbG Plausibilisierung",
            "recommended_action":
                "Investigate with agentd meter-data-agent or edmd MCP get_timeseries \
                 before the next billing run; § 60 Abs. 2 Ersatzwertbildung via \
                 trigger_substitution where no measurement can be recovered",
        }),
    )
    .extension("tenantid", tenant.to_owned())
    .extension("correlationid", alert.correlation_id.to_owned())
    .extension("causationid", alert.causation_id.to_owned());

    let client = mako_service::http::default_client();
    if let Err(e) = mako_service::post_ce_with_retry(&client, url, &ce, secret).await {
        tracing::error!(
            error = %e,
            malo_id = %alert.malo_id,
            door = %alert.door,
            "edmd: quality-warning CloudEvent delivery failed — event lost"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validation(issues: usize, blocks: usize) -> BatchValidation {
        BatchValidation {
            issue_count: issues,
            billing_block_count: blocks,
            rules: if issues == 0 {
                Vec::new()
            } else {
                vec!["V09".to_owned()]
            },
            skipped_rules: Vec::new(),
        }
    }

    fn alert<'a>(v: &'a BatchValidation, hampel: Option<serde_json::Value>) -> QualityAlert<'a> {
        QualityAlert {
            malo_id: "51238696012",
            door: "test",
            correlation_id: "c",
            causation_id: "s",
            sparte: Some("STROM"),
            period_from: None,
            period_to: None,
            validation: v,
            hampel,
        }
    }

    #[test]
    fn a_clean_batch_with_a_clean_score_raises_nothing() {
        let v = validation(0, 0);
        assert!(!alert(&v, Some(serde_json::json!({ "has_warnings": false }))).is_warning());
        assert!(!alert(&v, None).is_warning());
    }

    /// The defect this module exists to close: V09 fires on a batch whose
    /// statistical profile is unremarkable, so the Hampel summary says nothing.
    #[test]
    fn a_validation_finding_alone_raises_the_warning() {
        let v = validation(1, 1);
        assert!(
            alert(&v, Some(serde_json::json!({ "has_warnings": false }))).is_warning(),
            "a non-billable interval must raise the warning even when the Hampel \
             scorer sees nothing unusual"
        );
    }

    #[test]
    fn a_hampel_finding_alone_still_raises_the_warning() {
        let v = validation(0, 0);
        assert!(alert(&v, Some(serde_json::json!({ "has_warnings": true }))).is_warning());
    }
}
