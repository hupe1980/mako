//! Quality scoring: assessments, per-batch reports and retroactive rescoring.

#[allow(unused_imports)]
use super::*;

// ── Quality assessments ─────────────────────────────────────────────────

/// `GET /api/v1/quality-assessments/{malo_id}`
///
/// Returns the quality assessment history for a MaLo.
/// Each batch ingest produces one quality assessment row per § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) audit trail.
pub(crate) async fn list_quality_assessments(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
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
    let (from, to) = match read_window(params.from.as_deref(), params.to.as_deref()) {
        Ok(w) => w,
        Err(refusal) => return refusal.into_response(),
    };
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

/// Quality report returned in the ingest response and recorded per batch in
/// `quality_assessments` (meterstore owns the readings and has no per-interval
/// `quality_warnings` column).
///
/// Outlier detection uses the **Hampel filter** (sliding-window median/MAD)
/// rather than a global 3-sigma rule: the median and MAD are robust to the
/// outliers being detected, and the sliding window captures local behaviour.
/// `sigma = 1.4826 × MAD` converts MAD to the equivalent Gaussian σ;
/// `x[i]` is flagged when `|x[i] − window_median| > threshold × sigma`.
///
/// The window and threshold are `metering`\'s, per commodity
/// ([`metering::QualityConfig::for_sparte`]), and are reported in
/// [`Self::algorithm`] rather than named in prose — a hardcoded label drifts
/// from the numbers actually used. The response used to advertise
/// `hampel_k3_t3` while the crate ran a 12-interval half-window at 6 robust
/// sigma.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QualityReport {
    pub intervals_accepted: usize,
    pub intervals_rejected: usize,
    pub gaps_detected: usize,
    pub zero_run_length: usize,
    /// Instants flagged as statistical outliers (V04).
    pub outlier_intervals: Vec<String>,
    /// Instants whose average power exceeds the plant\'s physical capacity
    /// (V12). Not a statistical judgement — an impossibility.
    pub spike_intervals: Vec<String>,
    /// All intervals have the same duration (seconds).  False = mixed interval lengths.
    pub intervals_consistent: bool,
    pub has_warnings: bool,
    pub coverage_pct: f64,
    /// Quality grade: "A" (clean) | "B" (minor) | "C" (significant) | "F" (unusable).
    pub grade: &'static str,
    /// Observed cadence of the series, in seconds, when it has one.
    pub interval_secs: Option<u32>,
    /// How many intervals the requested period should hold at that cadence —
    /// the denominator `coverage_pct` alone cannot supply. `None` when the
    /// series is too short to have an observable cadence.
    pub expected_intervals: Option<u32>,
    /// The scorer and its actual parameters, e.g. `hampel(window=12,sigma=6)`.
    pub algorithm: String,
}

/// Compute quality metrics for a set of accepted intervals over a window.
///
/// `metering::score_intervals` over typed `MeterInterval`s, with the window
/// passed via `over_period` so `coverage_pct` measures coverage of what the
/// caller asked about rather than of whatever data happened to arrive.
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
    // Intervals the period should hold at the series' observed cadence — the
    // denominator that turns `interval_count` into "how much is missing".
    let expected: Option<i32> = q
        .expected_intervals
        .map(|n| i32::try_from(n).unwrap_or(i32::MAX));
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

/// Score a series over the window the caller asked about.
///
/// Takes typed intervals so the values keep their `Decimal` precision and the
/// quality flags survive: a caller that hands over `FAULTY` readings must not
/// have them graded as if they were measured.
///
/// Three things are commodity- and cadence-dependent, and all three come from
/// the series rather than from a constant:
///
/// - **Thresholds** are [`metering::QualityConfig::for_sparte`]. The electricity
///   defaults tolerate four consecutive zeros, which flags every quiet water and
///   heat profile as a stuck meter.
/// - **Cadence** is observed (`detect_interval_length`). Assuming 900 s makes
///   every interval of an hourly gas series a V06 finding and divides real gaps
///   by the wrong grid.
/// - **Coverage** is measured against `over_period(period_start, period_end)` —
///   the window the caller asked about. Without it the crate measures against
///   the extent of the data itself, and its own docs say what follows: "a
///   truncated delivery reads as 100 %".
pub(crate) fn compute_quality(
    samples: &[metering::MeterInterval],
    sparte: metering::Sparte,
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> QualityReport {
    // Cadence, gaps, overlaps and the Hampel filter are all statements about a
    // **single** series, and a MaLo delivers several registers at once. Scored
    // flat they share every timestamp, so consecutive starts are equal, the
    // observed cadence collapses towards zero, every same-slot pair reads as an
    // overlap, and coverage is multiplied by the number of registers. So each
    // register is scored on its own and the verdicts folded conservatively.
    //
    // Only the **energy** registers are scored. A Fehlerregister or a reactive
    // channel legitimately reports far less often than the Lastgang, and letting
    // it set the floor would grade every industrial connection F for a channel
    // nobody bills.
    let mut by_register: BTreeMap<Option<String>, Vec<metering::MeterInterval>> = BTreeMap::new();
    for iv in samples {
        let energy = iv
            .obis_code
            .is_none_or(crate::domain::register::is_energy_register);
        if !energy {
            continue;
        }
        by_register
            .entry(iv.obis_code.map(|c| c.to_string()))
            .or_default()
            .push(iv.clone());
    }
    // Nothing recognisable as energy: score what there is rather than answer
    // about an empty series.
    if by_register.is_empty() {
        return score_one_register(samples, sparte, period_start, period_end);
    }
    let mut reports: Vec<(usize, QualityReport)> = by_register
        .into_values()
        .map(|ivs| {
            (
                ivs.len(),
                score_one_register(&ivs, sparte, period_start, period_end),
            )
        })
        .collect();
    // The dominant register — the one carrying the most intervals — supplies the
    // cadence and the algorithm label; the rest is folded worst-first.
    reports.sort_by_key(|(n, _)| std::cmp::Reverse(*n));
    let mut folded = reports
        .first()
        .map(|(_, r)| r.clone())
        .unwrap_or_else(|| score_one_register(&[], sparte, period_start, period_end));
    for (_, r) in reports.iter().skip(1) {
        folded.intervals_accepted += r.intervals_accepted;
        folded.intervals_rejected += r.intervals_rejected;
        folded.gaps_detected += r.gaps_detected;
        folded.zero_run_length = folded.zero_run_length.max(r.zero_run_length);
        folded.outlier_intervals.extend(r.outlier_intervals.clone());
        folded.spike_intervals.extend(r.spike_intervals.clone());
        folded.intervals_consistent &= r.intervals_consistent;
        folded.has_warnings |= r.has_warnings;
        // A measuring point is no better covered than its worst energy register,
        // and no better graded than its worst.
        folded.coverage_pct = folded.coverage_pct.min(r.coverage_pct);
        if grade_rank(r.grade) > grade_rank(folded.grade) {
            folded.grade = r.grade;
        }
        folded.expected_intervals = match (folded.expected_intervals, r.expected_intervals) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (a, b) => a.or(b),
        };
    }
    folded
}

/// Severity order of a quality grade, so the worst of several can be taken.
fn grade_rank(grade: &str) -> u8 {
    match grade {
        "A" => 0,
        "B" => 1,
        "C" => 2,
        _ => 3,
    }
}

/// [`compute_quality`] for one register's series — the actual scoring.
fn score_one_register(
    samples: &[metering::MeterInterval],
    sparte: metering::Sparte,
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> QualityReport {
    use metering::validation::ValidationRuleId;

    let mut sorted: Vec<metering::MeterInterval> = samples.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    let resolution = metering::classification::detect_interval_length(&sorted);
    let interval_secs = resolution.map(|r| r.nominal_seconds());

    let mut config = metering::QualityConfig::for_sparte(sparte);
    if let Some(secs) = interval_secs {
        config.validation.expected_interval_secs = Some(secs);
    }
    let config = config.over_period(period_start, period_end);
    let algorithm = format!(
        "hampel(window={},sigma={},min_sigma={})",
        config.validation.outlier_window,
        config.validation.outlier_sigma.unwrap_or(0.0),
        config.validation.outlier_min_sigma,
    );

    let report = metering::score_intervals(&sorted, &config);

    // The report counts anomalies; the API also names when they happened. Both
    // come from `issues`, filtered by the rule that raised them.
    let at = |rule: ValidationRuleId| -> Vec<String> {
        report
            .issues
            .iter()
            .filter(|i| i.rule_id == rule)
            .filter_map(|i| i.affected_from)
            .map(|t| t.to_string())
            .collect()
    };
    let outlier_intervals = at(ValidationRuleId::StatisticalOutlier);
    let spike_intervals = at(ValidationRuleId::ImplausiblePower);

    // The honest denominator: how many intervals the requested window holds at
    // the observed cadence. It used to be derived as `span / (span / count)`,
    // which is `count` again — so `expected_count` always equalled
    // `interval_count` and could never show that anything was missing.
    let expected_intervals = interval_secs.filter(|s| *s > 0).map(|secs| {
        let span = (period_end - period_start).whole_seconds().max(0);
        u32::try_from(span / i64::from(secs)).unwrap_or(u32::MAX)
    });

    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    QualityReport {
        intervals_accepted: report.intervals_analysed,
        intervals_rejected: total_anomalies,
        gaps_detected: report.gaps_detected,
        zero_run_length: report.max_zero_run,
        outlier_intervals,
        spike_intervals,
        intervals_consistent: report.intervals_consistent,
        has_warnings: report.has_warnings(),
        coverage_pct: report.coverage_pct,
        grade: report.grade.as_str(),
        interval_secs,
        expected_intervals,
        algorithm,
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
///   "malo_id": "DE0001234567890723456789012345678",
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

    // Re-score the window as stored. The reads go in with their own quality
    // flags: routing them through a `DirectInterval` with `quality: None`
    // presented every one of them to the scorer as `MEASURED`, so V09 could
    // never fire and a window full of FAULTY readings re-graded clean — the
    // exact question a rescore is usually asked to settle.
    let samples: Vec<metering::MeterInterval> = reads
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        })
        .collect();
    let sparte = reads.first().map_or(metering::Sparte::Strom, |r| r.sparte);

    let mut quality = compute_quality(&samples, sparte, from_dt, to_dt);
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
    // One window, one verdict — the map reports which of the four it is.
    let grades: std::collections::BTreeMap<&str, u32> = ["A", "B", "C", "F"]
        .into_iter()
        .map(|g| (g, u32::from(g == grade)))
        .collect();

    let warnings_found = usize::from(quality.has_warnings);
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
                "algorithm": quality.algorithm,
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

    fn make_interval(from: OffsetDateTime, value_str: &str) -> metering::MeterInterval {
        metering::MeterInterval {
            from,
            to: from + time::Duration::minutes(15),
            value: Decimal::from_str_exact(value_str).unwrap(),
            quality: metering::QualityFlag::Measured,
            obis_code: None,
        }
    }

    fn score(
        intervals: &[metering::MeterInterval],
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> QualityReport {
        compute_quality(intervals, metering::Sparte::Strom, from, to)
    }

    /// Coverage is measured against the *requested* window, not the data's own
    /// span.
    ///
    /// Pins the regression the metering-0.17 migration introduced: the period
    /// bounds were dropped from the scoring call, and the crate then measures
    /// coverage against the extent of the data itself — its docs say it
    /// plainly, "a truncated delivery reads as 100 %". Half a window of data
    /// graded as fully covered, A-grade, billable.
    #[test]
    fn coverage_is_against_the_requested_window() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 24 quarter-hours delivered of a 48-quarter-hour window.
        let intervals: Vec<metering::MeterInterval> = (0..24)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.0"))
            .collect();
        let report = score(&intervals, base, base + time::Duration::hours(12));
        assert!(
            report.coverage_pct < 60.0,
            "half-empty window must not read as covered: {}",
            report.coverage_pct,
        );
        assert_ne!(
            report.grade, "A",
            "a half-delivered window is not clean data"
        );
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
        let intervals: Vec<metering::MeterInterval> = (0..96)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345"))
            .collect();
        let period_end = base + time::Duration::hours(24);
        let report = score(&intervals, base, period_end);
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
        let mut intervals: Vec<metering::MeterInterval> = (0..10)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345"))
            .collect();
        // Skip 2 intervals (gap), then resume from i=12
        intervals.extend(
            (12..22).map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.345")),
        );
        let period_end = base + time::Duration::hours(24);
        let report = score(&intervals, base, period_end);
        assert!(report.gaps_detected > 0, "Must detect gaps");
        assert!(report.has_warnings);
    }

    /// A spike in a series long enough to assess is flagged.
    ///
    /// **Long enough is a real constraint.** The Hampel filter runs a window of
    /// `outlier_window` (12) either side of each point, and `metering` 0.17
    /// refuses to run it at all on a series of `2 × window` or fewer — with
    /// twelve neighbours to draw on and ten points to draw from, every point is
    /// its own median and nothing can deviate from it. This test used ten
    /// intervals and passed under 0.16, which scored short series anyway; the
    /// upgrade turned it red, and the crate is right. Detection on a ten-point
    /// series was a claim the statistics could not support.
    ///
    /// So the series is 30 intervals — a length a quarter-hour profile reaches
    /// in under eight hours, and below which edmd now reports no outliers.
    #[test]
    fn quality_spike_detection() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 30 stable intervals at 2.0 kWh, one spike at 200.0 (100× the median).
        let mut intervals: Vec<metering::MeterInterval> = (0..30)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), "2.0"))
            .collect();
        intervals[5] = {
            let mut iv = make_interval(base + time::Duration::minutes(15 * 5), "200.0");
            iv.to = iv.from + time::Duration::minutes(15);
            iv
        };
        let period_end = base + time::Duration::minutes(15 * 30);
        let report = score(&intervals, base, period_end);
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
