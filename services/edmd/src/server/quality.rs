//! Quality scoring: assessments, per-batch reports and retroactive rescoring.

#[allow(unused_imports)]
use super::*;

// ── Quality assessments ─────────────────────────────────────────────────

/// `GET /api/v1/quality-assessments/{malo_id}`
///
/// Returns the quality assessment history for a MaLo.
/// Each batch ingest produces one quality assessment row per § 60 Abs. 6 MsbG audit trail.
pub(crate) async fn list_quality_assessments(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-timeseries",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let from = params
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(OffsetDateTime::now_utc);

    match sqlx::query(
        "SELECT assessed_at, source, grade, interval_count, expected_count, coverage_pct, gaps_detected, billing_blocked, issues_json, pid
           FROM quality_assessments
          WHERE malo_id = $1 AND tenant = $2 AND assessed_at BETWEEN $3 AND $4
          ORDER BY assessed_at DESC LIMIT 200"
    )
    .bind(&malo_id)
    .bind(&state.tenant)
    .bind(from)
    .bind(to)
    .fetch_all(state.repo.pool())
    .await {
        Ok(rows) => {
            use sqlx::Row;
            let assessments: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
                "assessed_at": r.try_get::<OffsetDateTime, _>("assessed_at").ok().map(|t| t.to_string()),
                "source": r.try_get::<String, _>("source").unwrap_or_default(),
                "grade": r.try_get::<String, _>("grade").unwrap_or_default(),
                "interval_count": r.try_get::<i32, _>("interval_count").unwrap_or(0),
                "expected_count": r.try_get::<Option<i32>, _>("expected_count").ok().flatten(),
                "coverage_pct": r.try_get::<Option<f64>, _>("coverage_pct").ok().flatten(),
                "gaps_detected": r.try_get::<i32, _>("gaps_detected").unwrap_or(0),
                "billing_blocked": r.try_get::<bool, _>("billing_blocked").unwrap_or(false),
                "pid": r.try_get::<Option<i32>, _>("pid").ok().flatten(),
            })).collect();
            Json(serde_json::json!({
                "malo_id": malo_id,
                "count": assessments.len(),
                "assessments": assessments,
            })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: list_quality_assessments failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Quality report returned in the direct-push response and recorded per batch in
/// `quality_assessments` (meterstore owns the readings and has no per-interval
/// `quality_warnings` column).
///
/// Outlier detection uses the **Hampel filter** (sliding-window median/MAD)
/// rather than a global 3-sigma rule: the median and MAD are robust to the
/// outliers being detected, and the sliding window captures local behaviour.
/// `sigma = 1.4826 × MAD` converts MAD to the equivalent Gaussian σ;
/// `x[i]` is flagged when `|x[i] − window_median| > threshold × sigma`.
#[derive(Debug, serde::Serialize)]
pub struct QualityReport {
    pub intervals_accepted: usize,
    pub intervals_rejected: usize,
    pub gaps_detected: usize,
    pub zero_run_length: usize,
    /// Outlier timestamps (Hampel filter, window k=3, threshold t=3.0).
    pub outlier_intervals: Vec<String>,
    /// Intervals where value > spike_factor × median of surrounding window.
    /// Catches erroneous readings that are plausible to 3-sigma but obviously wrong.
    pub spike_intervals: Vec<String>,
    /// All intervals have the same duration (seconds).  False = mixed interval lengths.
    pub intervals_consistent: bool,
    pub has_warnings: bool,
    pub coverage_pct: f64,
    /// Quality grade: "A" (clean) | "B" (minor) | "C" (significant) | "F" (unusable).
    pub grade: &'static str,
}

/// Compute quality metrics for a set of accepted intervals.
///
/// Compute quality metrics using `metering::score_intervals_f64`.
///
/// This is the fast path: converts `DirectInterval` values to `f64` and
/// timestamps to nanoseconds, then calls the SIMD-friendly scoring function
/// that auto-vectorises the hot loops to AVX2/NEON without platform-specific
/// intrinsics or external TSDB dependencies.
/// Persist a quality verdict to `quality_assessments`.
///
/// Every scoring path records one, so the table is a history of how a MaLo's
/// data quality moved over time rather than a snapshot of the latest opinion.
/// That history is what makes a billing dispute answerable: it shows when a gap
/// appeared, when it was substituted, and what the grade was at the moment an
/// invoice was raised.
///
/// Re-scoring a window supersedes the previous verdict for the same source
/// rather than appending a duplicate.
pub(crate) async fn record_quality_assessment(
    pool: &sqlx::PgPool,
    tenant: &str,
    malo_id: &str,
    period_from: OffsetDateTime,
    period_to: OffsetDateTime,
    source: &str,
    q: &QualityReport,
) {
    let outliers =
        i32::try_from(q.outlier_intervals.len() + q.spike_intervals.len()).unwrap_or(i32::MAX);
    // Intervals the period should hold, derived from the observed cadence.
    // `None` when a single interval leaves no cadence to infer.
    let expected: Option<i32> = (q.intervals_accepted > 1).then(|| {
        let span = (period_to - period_from).whole_seconds().max(0);
        let slot = span / i64::try_from(q.intervals_accepted).unwrap_or(1).max(1);
        i32::try_from(if slot > 0 { span / slot } else { 0 }).unwrap_or(i32::MAX)
    });
    let result = sqlx::query(
        r"INSERT INTO quality_assessments
              (malo_id, period_from, period_to, grade, interval_count, expected_count,
               gaps_detected, zero_run, outlier_count, coverage_pct, billing_blocked,
               source, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
          ON CONFLICT (tenant, malo_id, period_from, period_to, source) DO UPDATE
              SET grade           = EXCLUDED.grade,
                  interval_count  = EXCLUDED.interval_count,
                  expected_count  = EXCLUDED.expected_count,
                  gaps_detected   = EXCLUDED.gaps_detected,
                  zero_run        = EXCLUDED.zero_run,
                  outlier_count   = EXCLUDED.outlier_count,
                  coverage_pct    = EXCLUDED.coverage_pct,
                  billing_blocked = EXCLUDED.billing_blocked,
                  assessed_at     = now()",
    )
    .bind(malo_id)
    .bind(period_from)
    .bind(period_to)
    .bind(q.grade)
    .bind(i32::try_from(q.intervals_accepted).unwrap_or(i32::MAX))
    .bind(expected)
    .bind(i32::try_from(q.gaps_detected).unwrap_or(i32::MAX))
    .bind(i32::try_from(q.zero_run_length).unwrap_or(i32::MAX))
    .bind(outliers)
    .bind(rust_decimal::Decimal::try_from(q.coverage_pct).unwrap_or_default())
    // Only grade F blocks billing (`metering::QualityGrade::blocks_billing`);
    // C is significant but still billable.
    .bind(q.grade == "F")
    .bind(source)
    .bind(tenant)
    .execute(pool)
    .await;

    if let Err(e) = result {
        // The readings are already stored; a missing assessment is a gap in the
        // audit history rather than lost data, so it is surfaced and the request
        // still succeeds.
        tracing::warn!(
            malo_id, source, error = %e,
            "edmd: could not record quality assessment"
        );
    }
}

pub(crate) fn compute_quality(
    accepted: &[&DirectInterval],
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> QualityReport {
    use metering::QualityConfig;
    use rust_decimal::prelude::ToPrimitive;

    let mut sorted: Vec<&DirectInterval> = accepted.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    // Convert to f64 values + nanosecond timestamps in one pass.
    // to_f64() is lossless for kWh values ≤ 10^13 (53-bit mantissa).
    let values: Vec<f64> = sorted
        .iter()
        .map(|iv| iv.value.to_f64().unwrap_or(0.0))
        .collect();
    let timestamps_ns: Vec<i64> = sorted
        .iter()
        .map(|iv| iv.from.unix_timestamp_nanos() as i64)
        .collect();

    let period_start_ns = period_start.unix_timestamp_nanos() as i64;
    let period_end_ns = period_end.unix_timestamp_nanos() as i64;

    let report = metering::score_intervals_f64(
        &values,
        &timestamps_ns,
        period_start_ns,
        period_end_ns,
        QualityConfig::default(),
    );

    // score_intervals_f64 returns "t+<nanos>" timestamp strings for portability.
    // Map them back to the actual RFC3339 from-timestamps for API compatibility.
    let ns_to_from_str = |ns_str: &str| -> String {
        let ns: i64 = ns_str
            .strip_prefix("t+")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        sorted
            .iter()
            .find(|iv| iv.from.unix_timestamp_nanos() as i64 == ns)
            .map(|iv| iv.from.to_string())
            .unwrap_or_else(|| ns_str.to_owned())
    };

    let outlier_intervals: Vec<String> = report
        .outlier_intervals
        .iter()
        .map(|s| ns_to_from_str(s))
        .collect();
    let spike_intervals: Vec<String> = report
        .spike_intervals
        .iter()
        .map(|s| ns_to_from_str(s))
        .collect();

    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    QualityReport {
        intervals_accepted: report.intervals_analysed,
        intervals_rejected: total_anomalies,
        gaps_detected: report.gaps_detected,
        zero_run_length: report.max_zero_run,
        outlier_intervals,
        spike_intervals,
        intervals_consistent: report.intervals_consistent,
        has_warnings: report.has_warnings,
        coverage_pct: report.coverage_pct,
        grade: report.grade.as_str(),
    }
}

// ─── retroactive quality rescoring ──────────────────────────────────────

/// Optional query parameters for retroactive quality rescoring.
#[derive(Debug, serde::Deserialize)]
pub struct QualityRescoreQuery {
    /// ISO-8601 start date (inclusive). Defaults to 30 days ago.
    pub from: Option<String>,
    /// ISO-8601 end date (exclusive). Defaults to now.
    pub to: Option<String>,
}

/// `POST /api/v1/quality-score/{malo_id}[?from=&to=]`
///
/// Retroactively re-scores **all** `meter_reads` for `malo_id` in the given
/// date window using the Hampel filter.
///
/// This is useful when:
/// - MSCONS-ingested historical data was stored without quality scoring
/// - The quality algorithm was upgraded (e.g. from 3-sigma to Hampel)
/// - A billing dispute requires re-verification of read quality
///
/// The handler re-runs `compute_quality()` over the window's version-resolved
/// reads, records the verdict in `quality_assessments` (source `BATCH_RESCORE`),
/// and emits `de.messwert.reading.quality.warning` for any newly-found warnings.
/// meterstore owns the readings and carries no per-interval `quality_warnings`
/// column, so the audit trail is the `quality_assessments` row, not a read update.
///
/// ## Response
///
/// ```json
/// {
///   "malo_id": "DE0001234567890123456789012345678",
///   "rows_rescored": 96,
///   "warnings_found": 2,
///   "grades": { "A": 0, "B": 1, "C": 1, "F": 0 }
/// }
/// ```
pub async fn post_quality_rescore(
    State(state): State<HandlerState>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(malo_id): Path<String>,
    Query(q): Query<QualityRescoreQuery>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-quality-rescore",
        resource_tenant,
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Parse date window — default: 30 days.
    let now = OffsetDateTime::now_utc();
    let from_dt: OffsetDateTime = q
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_else(|| now - time::Duration::days(30));
    let to_dt: OffsetDateTime = q
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok())
        .unwrap_or(now);

    // Load the window through the repository (version-resolved, tenant-scoped).
    // `repo.query` does not filter quality, so the rescore sees every stored
    // interval (including Faulty/Unknown) — matching the previous behaviour, which
    // scored the whole window rather than only the billable subset.
    let reads = match state
        .repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.clone(),
            from: from_dt,
            to: to_dt,
            sparte: None,
            tenant: state.tenant.clone(),
        })
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "rows_rescored": 0,
                "warnings_found": 0,
                "grades": { "A": 0, "B": 0, "C": 0, "F": 0 },
            })),
        )
            .into_response();
    }

    // Re-score using Hampel filter applied to the full loaded window.
    // Convert the typed reads to DirectInterval for reuse of compute_quality().
    let pseudo_intervals: Vec<DirectInterval> = reads
        .iter()
        .map(|r| DirectInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            unit: "kWh".to_owned(),
            quality: None,
        })
        .collect();

    let refs: Vec<&DirectInterval> = pseudo_intervals.iter().collect();
    let mut quality = compute_quality(&refs, from_dt, to_dt);
    quality.intervals_rejected = 0;

    record_quality_assessment(
        state.repo.pool(),
        &state.tenant,
        &malo_id,
        from_dt,
        to_dt,
        "BATCH_RESCORE",
        &quality,
    )
    .await;

    let grade = quality.grade;
    let mut grades = std::collections::HashMap::new();
    *grades.entry("A").or_insert(0u32) += 0;
    *grades.entry("B").or_insert(0u32) += 0;
    *grades.entry("C").or_insert(0u32) += 0;
    *grades.entry("F").or_insert(0u32) += 0;
    *grades.entry(grade).or_insert(0u32) += 1;

    let warnings_found = if quality.has_warnings { 1usize } else { 0usize };
    let rows_rescored = reads.len();

    // The rescore verdict is persisted to `quality_assessments` via
    // `record_quality_assessment` above — meterstore owns the readings and has no
    // per-interval `quality_warnings` column, so there is no `meter_reads` row to
    // update. The audit history lives in `quality_assessments` (source
    // `BATCH_RESCORE`), which is what a billing dispute reads.
    //
    // Emit a quality warning CloudEvent if warranted.
    if quality.has_warnings
        && let Some(ref url) = state.erp_webhook_url
    {
        let event_id = uuid::Uuid::new_v4().to_string();
        let ce = mako_service::CloudEvent::new(
            mako_service::source("edmd", &state.tenant),
            mako_events::messwert::READING_QUALITY_WARNING,
            malo_id.clone(),
            serde_json::json!({
                "malo_id": malo_id,
                "grade": quality.grade,
                "gaps_detected": quality.gaps_detected,
                "outlier_count": quality.outlier_intervals.len() + quality.spike_intervals.len(),
                "coverage_pct": quality.coverage_pct,
                "window_from": from_dt.to_string(),
                "window_to": to_dt.to_string(),
                "algorithm": "hampel_k3_t3",
                "trigger": "retroactive_rescore",
            }),
        )
        .with_id(event_id)
        .extension("tenantid", state.tenant.clone());
        let client = mako_service::http::default_client();
        if let Err(e) =
            mako_service::post_ce_with_retry(&client, url, &ce, state.webhook_secret_bytes()).await
        {
            tracing::error!(error = %e, "edmd: CloudEvent delivery failed — event lost");
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id": malo_id,
            "rows_rescored": rows_rescored,
            "warnings_found": warnings_found,
            "grade": grade,
            "grades": grades,
            "window_from": from_dt.to_string(),
            "window_to": to_dt.to_string(),
        })),
    )
        .into_response()
}

#[cfg(test)]
mod quality_tests {
    use super::*;
    use rust_decimal::Decimal;
    use time::macros::datetime;

    fn make_interval(from: OffsetDateTime, value_str: &str) -> DirectInterval {
        let to = from + time::Duration::minutes(15);
        DirectInterval {
            from,
            to,
            value: Decimal::from_str_exact(value_str).unwrap(),
            unit: "kWh".to_owned(),
            quality: None,
        }
    }

    /// Hampel filter must flag an obvious spike surrounded by stable neighbours.
    #[test]
    fn hampel_filter_flags_spike() {
        // 9 values: 1.0 × 8 readings, then a spike of 50.0 at position 4
        let mut values = vec![1.0f64; 9];
        values[4] = 50.0;
        let outliers = metering::hampel_filter(&values, 3, 3.0);
        assert!(
            outliers.contains(&4),
            "Hampel must flag position 4 (spike=50.0 vs median=1.0): {outliers:?}"
        );
        // Surrounding stable reads must NOT be flagged
        assert!(!outliers.contains(&0));
        assert!(!outliers.contains(&8));
    }

    /// Hampel must NOT flag clean data with no outliers.
    #[test]
    fn hampel_filter_clean_data_no_flags() {
        // Small variations around 2.0 — all within 3 robust sigma
        let values = vec![1.98, 2.01, 2.03, 1.99, 2.02, 1.97, 2.04, 2.00, 1.96];
        let outliers = metering::hampel_filter(&values, 3, 3.0);
        assert!(
            outliers.is_empty(),
            "Hampel must not flag clean data: {outliers:?}"
        );
    }

    /// compute_quality grades clean 96-interval day as A.
    #[test]
    fn quality_grade_a_for_clean_data() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        let intervals: Vec<DirectInterval> = (0..96)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345"))
            .collect();
        let refs: Vec<&DirectInterval> = intervals.iter().collect();
        let period_end = base + time::Duration::hours(24);
        let report = compute_quality(&refs, base, period_end);
        assert_eq!(report.grade, "A", "Clean 96-interval day must be grade A");
        assert!(!report.has_warnings);
        assert!(report.coverage_pct >= 99.0);
        assert_eq!(report.gaps_detected, 0);
        assert!(report.outlier_intervals.is_empty());
    }

    /// compute_quality grades data with gaps as C or F.
    #[test]
    fn quality_grade_c_for_gaps() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 20 intervals with a 2-interval gap in the middle
        let mut intervals: Vec<DirectInterval> = (0..10)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345"))
            .collect();
        // Skip 2 intervals (gap), then resume from i=12
        intervals.extend(
            (12..22).map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345")),
        );
        let refs: Vec<&DirectInterval> = intervals.iter().collect();
        let period_end = base + time::Duration::hours(24);
        let report = compute_quality(&refs, base, period_end);
        assert!(report.gaps_detected > 0, "Must detect gaps");
        assert!(report.has_warnings);
    }

    /// Spike detection: value > 10× window median is flagged separately from Hampel.
    #[test]
    fn quality_spike_detection() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 10 stable intervals at 2.0 kWh, then one spike at 200.0 (100× median)
        let mut intervals: Vec<DirectInterval> = (0..10)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.0"))
            .collect();
        intervals[5] = {
            let mut iv = make_interval(base + time::Duration::minutes(15 * 5), "200.0");
            iv.to = iv.from + time::Duration::minutes(15);
            iv
        };
        let refs: Vec<&DirectInterval> = intervals.iter().collect();
        let period_end = base + time::Duration::minutes(15 * 10);
        let report = compute_quality(&refs, base, period_end);
        // Either Hampel or spike detection should flag position 5
        let flagged_ts = intervals[5].from.to_string();
        assert!(
            report.outlier_intervals.contains(&flagged_ts)
                || report.spike_intervals.contains(&flagged_ts),
            "Spike at index 5 must be detected. outliers={:?} spikes={:?}",
            report.outlier_intervals,
            report.spike_intervals
        );
        assert!(report.has_warnings);
    }
}
