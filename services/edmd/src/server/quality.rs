//! Quality scoring: assessments, per-batch reports and retroactive rescoring.

#[allow(
    unused_imports,
    reason = "used only in some feature or target combinations"
)]
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
        return ApiError::forbidden(e.to_string()).into_response();
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
/// The verdict is `metering::vee::validation`'s, run with the preset for the
/// series' Sparte over the window the caller asked about
/// ([`crate::domain::vee::validate_window`]). Outliers are its `SPIKE` rule —
/// a Hampel test over a sliding window, robust to the outliers it is looking
/// for — and its `SEASONAL` rule where one runs.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QualityReport {
    pub intervals_accepted: usize,
    pub intervals_rejected: usize,
    /// Slots of the window no interval delivered.
    pub gaps_detected: usize,
    /// The longest run of zeros the `ZERO_RUN` rule reported, in slots.
    pub zero_run_length: usize,
    /// Instants flagged as statistical outliers (`SPIKE`, `SEASONAL`).
    pub outlier_intervals: Vec<String>,
    /// Instants whose average power exceeds the plant's physical capacity
    /// (`CAPACITY`). Not a statistical judgement — an impossibility.
    pub spike_intervals: Vec<String>,
    /// The register's intervals form one series on one grid. `false` names a
    /// structural defect — a slot delivered twice, a span off the grid — that
    /// kept the content rules from running at all.
    pub intervals_consistent: bool,
    pub has_warnings: bool,
    /// Delivered time inside the window over the window's length, in percent.
    pub coverage_pct: rust_decimal::Decimal,
    /// Quality grade: "A" (clean) | "B" (warnings) | "C" (errors a substitute
    /// repairs) | "F" (blocking).
    pub grade: &'static str,
    /// Observed grid of the series, in seconds, when it is a fixed one.
    pub interval_secs: Option<u32>,
    /// How many slots the requested window holds on that grid — the
    /// denominator `coverage_pct` alone cannot supply. `None` when the series
    /// has no observable grid.
    pub expected_intervals: Option<u32>,
    /// The validation that produced the verdict, e.g. `vee::validation(STROM)`.
    pub algorithm: String,
}

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
    .bind(q.coverage_pct)
    // Grade F is a blocking finding — not repairable by a substitute, so the
    // window as a whole needs clarifying before it is billed. C names errors a
    // substitute repairs slot by slot.
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

/// Score a stored batch and record the verdict, for the ingest doors.
///
/// Every door runs this, and that is the point: the § 147 AO history a billing
/// dispute is answered from is only as complete as its least-covered path, and
/// the MSCONS webhook is *the* door in German MaKo. The V-rules alone are a
/// different signal — they say a rule fired, not how complete or how anomalous
/// the delivery was against its own commodity's thresholds.
///
/// The period is the batch's own extent (`domain::batch_period`), because a
/// pushed batch declares the window it covers. `None` when the batch is empty:
/// there is nothing to grade, and a grade over nothing is not "A".
///
/// Scoring and recording are two calls because the batch has to be **stored
/// first**. A `ValidatedReads` moves into `store_reads`, so the grade has to be
/// computed from the borrow beforehand — but writing the assessment row before
/// the readings land leaves a verdict standing for a delivery that failed and
/// was never stored, in the table the § 147 AO history is read out of.
/// [`BatchQuality::record`] is therefore called after the store succeeds.
pub(crate) struct BatchQuality {
    /// The grade and its findings, for the response and the CloudEvent.
    pub report: QualityReport,
    period_from: OffsetDateTime,
    period_to: OffsetDateTime,
    /// The batch's own [`IngestionSource`], not a label the caller invents
    /// beside it. `quality_assessments.source` is CHECK-constrained, so a door
    /// passing a name outside the vocabulary fails the insert — and the insert
    /// failure is a warning, which means the history goes silently missing for
    /// exactly the door that thought it was recording one.
    ///
    /// [`IngestionSource`]: crate::domain::IngestionSource
    source: &'static str,
}

impl BatchQuality {
    /// Persist the verdict. Call once the readings are stored.
    pub(crate) async fn record(&self, pool: &sqlx::PgPool, tenant: &str, malo_id: &str) {
        record_quality_assessment(
            pool,
            tenant,
            malo_id,
            self.period_from,
            self.period_to,
            self.source,
            &self.report,
        )
        .await;
    }
}

/// Grade a batch about to be stored, without recording anything yet.
pub(crate) fn score_batch(reads: &[MeterRead]) -> Option<BatchQuality> {
    let (Some(period_from), Some(period_to)) = crate::domain::batch_period(reads) else {
        return None;
    };
    // The commodity is the batch's own — every door builds a batch for one
    // Sparte, and scoring a gas delivery with electricity thresholds is the
    // defect the Sparte presets of `metering::vee::validation::Rules` exist to
    // prevent.
    let sparte = reads.first().map_or(metering::Sparte::Strom, |r| r.sparte);
    // A read with no span is refused by the store and named by the validation
    // pass; it has no time to cover here.
    let samples: Vec<metering::MeterInterval> =
        reads.iter().filter_map(|r| r.interval().ok()).collect();
    Some(BatchQuality {
        report: compute_quality(&samples, sparte, period_from, period_to),
        period_from,
        period_to,
        source: reads
            .first()
            .map_or(IngestionSource::Mscons, |r| r.source)
            .as_str(),
    })
}

/// The compact `hampel` block an ingest response and its CloudEvent both carry.
///
/// One renderer, so the two cannot describe the same batch differently.
pub(crate) fn hampel_summary(q: &QualityReport) -> serde_json::Value {
    serde_json::json!({
        "grade": q.grade,
        "coverage_pct": q.coverage_pct,
        "gaps_detected": q.gaps_detected,
        "outliers": q.outlier_intervals.len() + q.spike_intervals.len(),
        "expected_intervals": q.expected_intervals,
        "interval_secs": q.interval_secs,
        "has_warnings": q.has_warnings,
        "blocks_billing": q.grade == "F",
        "algorithm": q.algorithm,
    })
}

/// Score a series over the window the caller asked about.
///
/// Takes typed intervals so the values keep their `Decimal` precision and the
/// quality flags survive: a caller that hands over `FAULTY` readings must not
/// have them graded as if they were measured.
///
/// Three things are commodity- and grid-dependent, and all three come from
/// the series rather than from a constant:
///
/// - **Thresholds** are `metering`'s preset for the Sparte. The electricity
///   preset reports a zero run of an hour, which flags every quiet water and
///   heat profile as a stuck meter.
/// - **The grid** is observed ([`crate::domain::vee::observed_resolution`]).
///   Assuming 900 s divides the gaps of an hourly gas series by the wrong step.
/// - **Coverage** is measured against `[period_start, period_end)` — the
///   window the caller asked about. Measured against the extent of the data
///   itself, a truncated delivery reads as 100 %.
pub(crate) fn compute_quality(
    samples: &[metering::MeterInterval],
    sparte: metering::Sparte,
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> QualityReport {
    // Gaps, duplicates and the Hampel test are all statements about a
    // **single** series, and a MaLo delivers several registers at once. Scored
    // flat, every same-slot pair reads as a duplicate and coverage is
    // multiplied by the number of registers. So each register is scored on its
    // own and the verdicts folded conservatively.
    //
    // Only the **energy** registers are scored. A Fehlerregister or a reactive
    // channel legitimately reports far less often than the Lastgang, and letting
    // it set the floor would grade every industrial connection F for a channel
    // nobody bills.
    let mut by_register: BTreeMap<Option<String>, Vec<metering::MeterInterval>> = BTreeMap::new();
    for iv in samples {
        let energy = iv
            .obis()
            .is_none_or(crate::domain::register::is_energy_register);
        if !energy {
            continue;
        }
        by_register
            .entry(iv.obis().map(|c| c.to_string()))
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
    // grid and the algorithm label; the rest is folded worst-first.
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
    use metering::vee::validation::Rule;

    let algorithm = format!("vee::validation({})", sparte.as_str());
    let intervals_accepted = samples.len();
    // A register whose intervals are not one series, or a window outside the
    // calendar, cannot be graded by the content rules at all. That is a
    // blocking verdict, not a clean one.
    let unusable = |algorithm: String| QualityReport {
        intervals_accepted,
        intervals_rejected: 0,
        gaps_detected: 0,
        zero_run_length: 0,
        outlier_intervals: Vec::new(),
        spike_intervals: Vec::new(),
        intervals_consistent: false,
        has_warnings: true,
        coverage_pct: rust_decimal::Decimal::ZERO,
        grade: "F",
        interval_secs: None,
        expected_intervals: None,
        algorithm,
    };
    let series = match crate::domain::vee::series_of(sparte, samples.to_vec()) {
        Ok(series) => series,
        Err(defect) => return unusable(format!("{algorithm}; {}", defect.code())),
    };
    let Some(report) = crate::domain::vee::validate_window(
        &series,
        sparte,
        period_start,
        period_end,
        OffsetDateTime::now_utc(),
        None,
    ) else {
        return unusable(format!("{algorithm}; OUT_OF_RANGE"));
    };

    // The report counts findings; the API also names when they happened.
    let at = |rules: &[Rule]| -> Vec<String> {
        report
            .findings()
            .filter(|f| rules.contains(&f.rule))
            .map(|f| f.from.to_string())
            .collect()
    };
    let outlier_intervals = at(&[Rule::Spike, Rule::Seasonal]);
    let spike_intervals = at(&[Rule::Capacity]);
    let slot_secs = series.resolution().fixed_seconds();
    let slots_of = |from: OffsetDateTime, to: OffsetDateTime| -> usize {
        slot_secs.map_or(1, |secs| {
            usize::try_from((to - from).whole_seconds() / i64::from(secs.max(1))).unwrap_or(0)
        })
    };
    let gaps_detected = report
        .findings()
        .filter(|f| f.rule == Rule::Gap)
        .map(|f| {
            f.reference
                .and_then(|n| rust_decimal::prelude::ToPrimitive::to_usize(&n))
                .unwrap_or(0)
        })
        .sum();
    let zero_run_length = report
        .findings()
        .filter(|f| f.rule == Rule::ZeroRun)
        .map(|f| slots_of(f.from, f.to))
        .max()
        .unwrap_or(0);

    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    let grade = report.grade().as_str();
    QualityReport {
        intervals_accepted,
        intervals_rejected: total_anomalies,
        gaps_detected,
        zero_run_length,
        outlier_intervals,
        spike_intervals,
        intervals_consistent: true,
        has_warnings: grade != "A",
        coverage_pct: report.coverage_pct(),
        grade,
        interval_secs: slot_secs,
        expected_intervals: Some(report.expected_slots()),
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
/// date window with the same validation every ingest door runs.
///
/// This is useful when:
/// - MSCONS-ingested historical data was stored without quality scoring
/// - The validation rules changed
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
        return ApiError::forbidden(e.to_string()).into_response();
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
            return ApiError::internal(e).into_response();
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
    // flags: presented as `MEASURED`, `NON_BILLABLE` could never fire and a
    // window full of FAULTY readings would re-grade clean — the exact question
    // a rescore is usually asked to settle.
    let samples: Vec<metering::MeterInterval> =
        reads.iter().filter_map(|r| r.interval().ok()).collect();
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
    if quality.has_warnings {
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
        state.emit(&ce).await;
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
        metering::MeterInterval::quarter_hour(
            from,
            Decimal::from_str_exact(value_str).unwrap(),
            metering::QualityFlag::Measured,
        )
        .unwrap()
    }

    /// A ramp, so no run of equal values reads as a frozen register.
    fn ramp(i: i64) -> String {
        format!("2.{:03}", i)
    }

    fn score(
        intervals: &[metering::MeterInterval],
        from: OffsetDateTime,
        to: OffsetDateTime,
    ) -> QualityReport {
        compute_quality(intervals, metering::Sparte::Strom, from, to)
    }

    /// Coverage is measured against the *requested* window, not the data's own
    /// span: measured against the data, a truncated delivery reads as 100 %.
    #[test]
    fn coverage_is_against_the_requested_window() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 24 quarter-hours delivered of a 48-quarter-hour window.
        let intervals: Vec<metering::MeterInterval> = (0..24)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i)))
            .collect();
        let report = score(&intervals, base, base + time::Duration::hours(12));
        assert_eq!(report.coverage_pct, Decimal::from(50));
        assert_eq!(report.gaps_detected, 24);
        assert_eq!(report.expected_intervals, Some(48));
        assert_ne!(
            report.grade, "A",
            "a half-delivered window is not clean data"
        );
    }

    /// A clean day is graded A.
    #[test]
    fn quality_grade_a_for_clean_data() {
        let base = datetime!(2026-06-30 22:00:00 UTC);
        let intervals: Vec<metering::MeterInterval> = (0..96)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i)))
            .collect();
        let period_end = base + time::Duration::hours(24);
        let report = score(&intervals, base, period_end);
        assert_eq!(
            report.grade, "A",
            "Clean 96-interval day must be grade A: {report:?}"
        );
        assert!(!report.has_warnings);
        assert_eq!(report.coverage_pct, Decimal::from(100));
        assert_eq!(report.gaps_detected, 0);
        assert!(report.outlier_intervals.is_empty());
    }

    /// Missing slots are counted and graded as errors a substitute repairs.
    #[test]
    fn quality_grade_c_for_gaps() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        // 20 intervals with a 2-interval gap in the middle
        let mut intervals: Vec<metering::MeterInterval> = (0..10)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i)))
            .collect();
        intervals.extend(
            (12..22).map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i))),
        );
        let period_end = base + time::Duration::minutes(15 * 22);
        let report = score(&intervals, base, period_end);
        assert_eq!(report.gaps_detected, 2, "Must count the missing slots");
        assert_eq!(report.grade, "C");
        assert!(report.has_warnings);
    }

    /// A spike in a series long enough to assess is flagged.
    ///
    /// The Hampel test judges each value against twelve neighbours either side,
    /// so the series is long enough to give it a window.
    #[test]
    fn quality_spike_detection() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        let mut intervals: Vec<metering::MeterInterval> = (0..30)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i)))
            .collect();
        intervals[15] = make_interval(base + time::Duration::minutes(15 * 15), "200.0");
        let period_end = base + time::Duration::minutes(15 * 30);
        let report = score(&intervals, base, period_end);
        let flagged_ts = intervals[15].from().to_string();
        assert!(
            report.outlier_intervals.contains(&flagged_ts),
            "Spike at index 15 must be detected. outliers={:?}",
            report.outlier_intervals,
        );
        assert!(report.has_warnings);
    }

    /// One slot delivered twice is not a series; the register is graded F
    /// rather than scored as if it were clean.
    #[test]
    fn a_duplicate_slot_grades_the_register_unusable() {
        let base = datetime!(2026-07-01 00:00:00 UTC);
        let mut intervals: Vec<metering::MeterInterval> = (0..4)
            .map(|i| make_interval(base + time::Duration::minutes(15 * i), &ramp(i)))
            .collect();
        intervals.push(make_interval(base, "9.0"));
        let report = score(&intervals, base, base + time::Duration::hours(1));
        assert_eq!(report.grade, "F");
        assert!(!report.intervals_consistent);
    }
}
