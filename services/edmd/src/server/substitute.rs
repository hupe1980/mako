//! § 60 Abs. 2 MsbG auto-substitute (Ersatzwertbildung).

#[allow(unused_imports)]
use super::*;

// ── § 60 Abs. 2 MsbG Auto-Substitute (post_substitute_values) ───────────────────────

/// Request body for `POST /api/v1/meter-reads/{malo_id}/substitute`.
#[derive(Debug, serde::Deserialize)]
pub struct SubstituteRequest {
    /// Gap start (UTC, RFC3339).
    pub gap_from: String,
    /// Gap end (UTC, RFC3339).
    pub gap_to: String,
    /// Interval length in seconds (default: 900).
    pub interval_secs: Option<u32>,
    /// Substitution method: `LinearInterpolation`, `PriorPeriodAverage`,
    /// `ZeroFill`, or `LastValueCarryForward`. Default: `PriorPeriodAverage`.
    pub method: Option<String>,
    /// How far back to read reference data, in days (default: 7).
    ///
    /// **`PriorPeriodAverage` ignores anything past 7.** The Vergleichstag rule
    /// averages the matching slot over `metering::substitute::REFERENCE_PERIOD_DAYS`
    /// (= 7) preceding each gap, so a larger value adds no reference days to
    /// that method. It is not rejected, because it still does something for the
    /// others: it widens the window this handler reads, which is where the
    /// bracketing values for `LinearInterpolation` and `LastValueCarryForward`
    /// are found.
    pub prior_days: Option<u32>,
    /// Operator ID for audit trail.
    pub operator_id: Option<String>,
    /// `STROM` (default) · `GAS` · `WAERME` · `WASSER`. Determines the `unit`
    /// the substitute is stored in — a substituted water gap is m³, not kWh.
    pub sparte: Option<String>,
    /// Why a substitute is required (§ 60 Abs. 6 MsbG audit trail).
    ///
    /// One of the `substitute_value_log.reason` values. Defaults to
    /// `NoMeasurementAvailable`.
    pub reason: Option<String>,
    /// OBIS register the gap belongs to.
    ///
    /// Part of the primary key, so omitting it files the substitute under the
    /// empty-string register rather than against the reading it stands in for —
    /// leaving both rows in the table and double-counting the interval in every
    /// aggregate that sums without an OBIS filter.
    pub obis_code: Option<String>,
}

/// Map a [`metering::SubstituteMethod`] onto the `substitute_value_log.method`
/// vocabulary.
///
/// One-to-one since `metering` 0.17. It was not: the crate used to expose a
/// `ForecastMethod` describing *how* a value was derived — prior-period,
/// weighted rolling average, profile-based, annual projection — while this
/// column records the § 60 Abs. 2 MsbG substitution *categories*, and three of
/// those methods had no category to map onto. They were folded into
/// `LinearInterpolation` as the closest admissible description, which recorded
/// something that had not happened.
///
/// `SubstituteMethod` is now exactly the § 60 Abs. 2 vocabulary, so the audit
/// trail records the method that actually ran.
pub(crate) fn substitute_method_to_db(method: metering::SubstituteMethod) -> &'static str {
    use metering::SubstituteMethod as M;
    match method {
        M::PriorPeriodAverage => "PriorPeriodAverage",
        M::LastValueCarryForward => "LastValueCarryForward",
        M::ZeroFill => "ZeroFill",
        M::LinearInterpolation => "LinearInterpolation",
    }
}

/// `POST /api/v1/meter-reads/{malo_id}/substitute`
///
/// Generate and store § 60 Abs. 2 MsbG substitute values for a gap interval.
///
/// This endpoint:
/// 1. Validates the requested gap window.
/// 2. Fetches prior-period reference data from `meter_reads`.
/// 3. Calls `metering::prior_period_substitutes()` to generate values.
/// 4. Stores the generated intervals as `AUTO_SUBSTITUTE` source.
/// 5. Records each substitution in `substitute_value_log` for § 60 Abs. 6 MsbG audit.
/// 6. Returns the generated intervals with their methods and confidence notes.
///
/// **Cedar action**: `write-meter-reads`
pub async fn post_substitute_values(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<SubstituteRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    run_substitute_values(&state.repo, &state.tenant, &malo_id, &req).await
}

/// Core of the § 60 Abs. 2 MsbG substitute flow.
///
/// Shared by the HTTP handler (above) and the `trigger_substitution` MCP tool,
/// so both write substitutes under identical guards: never over a billable
/// reading, always with a `substitute_value_log` audit row, atomically.
///
/// Public so the § 60 Abs. 2 MsbG obligation can be pinned against a real
/// database rather than only through an authenticated HTTP round trip — the
/// numbers it produces are the regulated artefact, and they are what the
/// integration suite asserts.
pub async fn run_substitute_values(
    repo: &crate::store::MeterStoreTimeSeriesRepository,
    tenant: &str,
    malo_id: &str,
    req: &SubstituteRequest,
) -> axum::response::Response {
    use crate::domain::repository::TimeSeriesRepository as _;
    use crate::domain::{IngestionSource, MeterRead, TimeSeriesQuery};
    use metering::{MeterInterval, QualityFlag, SubstituteMethod};
    use time::format_description::well_known::Rfc3339;

    let gap_from = match OffsetDateTime::parse(&req.gap_from, &Rfc3339) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid gap_from: {e}") })),
            )
                .into_response();
        }
    };
    let gap_to = match OffsetDateTime::parse(&req.gap_to, &Rfc3339) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid gap_to: {e}") })),
            )
                .into_response();
        }
    };

    if gap_from >= gap_to {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "gap_from must be before gap_to" })),
        )
            .into_response();
    }

    let interval_secs = req.interval_secs.unwrap_or(900);
    let prior_days = req.prior_days.unwrap_or(7) as i64;
    let operator_id = req.operator_id.as_deref().unwrap_or("AUTO");

    let method = match req.method.as_deref().unwrap_or("PriorPeriodAverage") {
        "PriorPeriodAverage" => SubstituteMethod::PriorPeriodAverage,
        "LinearInterpolation" => SubstituteMethod::LinearInterpolation,
        "ZeroFill" => SubstituteMethod::ZeroFill,
        "LastValueCarryForward" => SubstituteMethod::LastValueCarryForward,
        other => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!("unknown substitute method `{other}`"),
                    "supported": [
                        "PriorPeriodAverage",
                        "LinearInterpolation",
                        "ZeroFill",
                        "LastValueCarryForward",
                    ],
                })),
            )
                .into_response();
        }
    };

    // Read the MaLo's existing intervals across the reference + gap + trailing
    // window through the repository. `query` is version-resolved and
    // tenant-scoped but does NOT filter quality, so the billable filter is
    // applied here: a Faulty/Unknown slot must read as a gap — exactly what a
    // § 60 Abs. 2 Ersatzwert may fill — and must never serve as reference data
    // or bracket an interpolation.
    let prior_from = gap_from - time::Duration::days(prior_days);
    let resolved = match repo
        .query(&TimeSeriesQuery {
            malo_id: malo_id.to_string(),
            from: prior_from,
            to: gap_to + time::Duration::days(prior_days),
            sparte: None,
            tenant: tenant.to_string(),
        })
        .await
    {
        Ok(reads) => reads,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: substitute reference read failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    let existing: Vec<MeterRead> = resolved
        .iter()
        .filter(|r| r.quality.is_billable())
        .cloned()
        .collect();

    // The reporting operator keys the meterstore version scope, and versions
    // only supersede each other *within* a scope. A substitute filed under the
    // tenant would therefore never displace the faulty row it replaces: both
    // would stand and the slot would be counted twice. So it inherits the
    // operator of the reading it stands in for — preferring one from inside the
    // window, since that is the row being displaced.
    let sender_mp_id = resolved
        .iter()
        .filter(|r| r.dtm_from >= gap_from && r.dtm_from < gap_to)
        .find_map(|r| r.sender_mp_id.clone())
        .or_else(|| resolved.iter().find_map(|r| r.sender_mp_id.clone()));

    // Prior-period reference: billable reads strictly before the gap.
    let prior_intervals: Vec<MeterInterval> = existing
        .iter()
        .filter(|r| r.dtm_from >= prior_from && r.dtm_to <= gap_from)
        .map(|r| MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        })
        .collect();

    // Slots inside the gap window that already carry a billable reading: a
    // substitute must never overwrite a real measurement (§ 60 Abs. 2 MsbG).
    let billable_slots: std::collections::HashSet<OffsetDateTime> = existing
        .iter()
        .filter(|r| r.dtm_from >= gap_from && r.dtm_from < gap_to)
        .map(|r| r.dtm_from)
        .collect();

    // Generate substitute values.
    //
    // `fill_gaps` fills the whole configured grid, so the values bracketing the
    // gap no longer have to be found and passed in by hand: it is given the
    // measured series and works out for itself what each gap is bounded by.
    // The whole queried series goes in, not just the slice inside the gap
    // window: the engine finds a gap's *closing* bracket by lookahead over
    // whatever series it was handed, so clipping at `gap_to` would hide a
    // measurement one slot past the window and degrade a genuine
    // interpolation to a one-sided copy.
    let measured: Vec<MeterInterval> = existing
        .iter()
        .map(|r| MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.as_deref().and_then(|s| s.parse().ok()),
        })
        .collect();

    // The *opening* bracket is different: the engine derives it only from
    // slots its walk has visited, so the fill period must start at the last
    // billable reading before the gap — starting at the gap itself leaves a
    // leading short gap bracketless, and "interpolation" silently becomes a
    // flat copy of the closing value. Substitutes generated for the run-up
    // are dropped again below; only the requested window is ever stored.
    let fill_from = existing
        .iter()
        .filter(|r| r.dtm_from < gap_from)
        .map(|r| r.dtm_from)
        .max()
        .unwrap_or(gap_from);

    let filled = metering::substitute::fill_gaps(
        &measured,
        &metering::substitute::FillGapsConfig {
            // `from_seconds` is fallible now: a resolution the crate does not model
            // (a 7-minute grid) has no Berlin-calendar meaning, and defaulting it
            // would silently fill against the wrong grid.
            resolution: match metering::IntervalResolution::from_seconds(interval_secs) {
                Some(r) => r,
                None => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!(
                                "unsupported interval length {interval_secs}s — \
                                 use 900 (15 min), 1800, 3600 or a calendar resolution"
                            )
                        })),
                    )
                        .into_response();
                }
            },
            period: (fill_from, gap_to),
            method,
            prior_period_intervals: prior_intervals.clone(),
            short_gap_threshold: 3,
            // § 60 Abs. 2 MsbG: this endpoint fills a gap, which is what
            // `NoMeasurementAvailable` records. A caller that knows better —
            // a meter fault, a gateway outage — is a richer request than this
            // endpoint currently accepts, and inventing a reason it was not
            // told would put a false one in an audit trail.
            reason: metering::substitute::SubstitutionReason::NoMeasurementAvailable,
        },
    );
    // Only the requested window is stored. The run-up from `fill_from` exists
    // to give the walk its opening bracket; anything synthesised there was
    // never asked for.
    let substitute_entries: Vec<_> = filled
        .substitutions
        .into_iter()
        .filter(|e| e.interval.from >= gap_from && e.interval.from < gap_to)
        .collect();

    if substitute_entries.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "No substitute values could be generated for this gap window"
            })),
        )
            .into_response();
    }

    // Store generated intervals and log them. The Sparte is authoritative on
    // the stored series the gap belongs to; the request value is only a
    // fallback for a MaLo with no resolved reads in the window.
    let sparte = resolved.first().map(|r| r.sparte).unwrap_or_else(|| {
        match req
            .sparte
            .as_deref()
            .unwrap_or("STROM")
            .to_uppercase()
            .as_str()
        {
            "GAS" => metering::interval::Sparte::Gas,
            "WAERME" | "WÄRME" => metering::interval::Sparte::Waerme,
            "WASSER" => metering::interval::Sparte::Wasser,
            _ => metering::interval::Sparte::Strom,
        }
    });

    let reason_str = req
        .reason
        .as_deref()
        .unwrap_or("NoMeasurementAvailable")
        .to_owned();
    let mut stored = 0usize;
    let mut log_entries: Vec<serde_json::Value> = Vec::new();
    // Intervals left alone because they already carry a billable reading.
    let mut skipped: Vec<String> = Vec::new();
    // The substitute readings to append, and their per-interval audit tuples
    // (from, to, method, substitute_kwh).
    let mut substitute_reads: Vec<MeterRead> = Vec::new();
    let mut audit: Vec<(
        OffsetDateTime,
        OffsetDateTime,
        &'static str,
        rust_decimal::Decimal,
    )> = Vec::new();

    // The same V01–V10 pass every ingest path runs: engine-generated values
    // are still stored values, and an Ersatzwert derived from anomalous prior
    // data (negative carry-forward, implausible spike) must carry its warning
    // annotation into `quality_warnings` like any other reading.
    let substitute_warnings: std::collections::HashMap<usize, serde_json::Value> = {
        let series: Vec<MeterInterval> = substitute_entries
            .iter()
            .map(|e| e.interval.clone())
            .collect();
        let report = metering::validation::validate_intervals(
            &series,
            &metering::validation::ValidationConfig {
                expected_interval_secs: Some(interval_secs),
                now: Some(OffsetDateTime::now_utc()),
                ..Default::default()
            },
        );
        let mut by_index: std::collections::HashMap<usize, Vec<serde_json::Value>> =
            std::collections::HashMap::new();
        for issue in &report.issues {
            if let Some(idx) = issue.interval_index {
                by_index.entry(idx).or_default().push(serde_json::json!({
                    "rule": issue.rule_id.to_string(),
                    "message": issue.message,
                    "blocks_billing": issue.blocks_billing(),
                }));
            }
        }
        by_index
            .into_iter()
            .map(|(idx, issues)| {
                (
                    idx,
                    serde_json::json!({
                        "has_warnings": true,
                        "issue_count": issues.len(),
                        "issues": issues,
                        "source": "SUBSTITUTE_VALIDATION",
                    }),
                )
            })
            .collect()
    };

    for (entry_idx, entry) in substitute_entries.iter().enumerate() {
        let iv = &entry.interval;
        // A § 60 Abs. 2 MsbG Ersatzwert fills a gap; it never overwrites a real
        // measurement. A slot already carrying a billable reading is left untouched
        // and reported as skipped.
        if billable_slots.contains(&iv.from) {
            skipped.push(iv.from.format(&Rfc3339).unwrap_or_default());
            continue;
        }
        // `entry.method` is the method that *actually ran* — `fill_gaps` may
        // interpolate a short gap whatever the request asked for, and the audit
        // trail must record what happened rather than what was requested.
        let method_str = substitute_method_to_db(entry.method);
        substitute_reads.push(MeterRead {
            malo_id: malo_id.to_string(),
            melo_id: None,
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: iv.value,
            quality: QualityFlag::Substituted,
            pid: 0,
            sparte,
            obis_code: req.obis_code.clone(),
            tenant: tenant.to_string(),
            source: IngestionSource::AutoSubstitute,
            push_session: None,
            quality_warnings: substitute_warnings.get(&entry_idx).cloned(),
            sender_mp_id: sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: None,
            // edmd's own output, not a delivery — arrival order is the order.
            mscons_version: None,
        });
        audit.push((iv.from, iv.to, method_str, iv.value));
        stored += 1;
        log_entries.push(serde_json::json!({
            "from": iv.from,
            "to": iv.to,
            "value": iv.value.to_string(),
            "method": method_str,
            "reference_count": entry.reference_count,
            "reason": format!("{:?}", entry.reason),
            "reference_count": entry.reference_count,
        }));
    }

    if substitute_reads.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "generated_count": 0,
                "skipped_measured": skipped,
                "message": "every interval in the window already carries a billable reading",
            })),
        )
            .into_response();
    }

    // Persist through the repository. meterstore's `append` routes each interval to
    // the tier that owns it, opens the § 60 Abs. 2 MsbG confirmation obligation
    // (so an operator is later nudged to replace the estimate with a real value),
    // and writes the § 60 Abs. 6 MsbG displacement audit — none of which the old
    // raw upsert did.
    // Ersatzwerte are edmd's own output, but they are billed like any other
    // reading, so they run the same V-rules — a generator that emits a wrong
    // interval length or a duplicate slot must fail here, not at settlement.
    let (validated, _) = crate::domain::validation::ValidatedReads::validate(
        substitute_reads,
        "SUBSTITUTE_VALIDATION",
        malo_id,
    );

    if let Err(e) = repo.store_reads(validated).await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute store failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // The per-interval method/reason audit row. `original_kwh` is NULL: a
    // substitute is written only where no billable reading existed, so there was no
    // usable prior value to record.
    let mut tx = match repo.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute audit tx failed to begin");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };
    for (from, to, method_str, kwh) in &audit {
        if let Err(e) = sqlx::query(
            r"INSERT INTO substitute_value_log
                (malo_id, dtm_from, dtm_to, method, reason, substitute_kwh,
                 original_kwh, created_by, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, NULL, $7, $8)",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(method_str)
        .bind(&reason_str)
        .bind(kwh)
        .bind(operator_id)
        .bind(tenant)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute audit log failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    if let Err(e) = tx.commit().await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute commit failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string(), "generated_count": 0 })),
        )
            .into_response();
    }

    tracing::info!(
        malo_id, stored, operator_id,
        gap_from = %gap_from, gap_to = %gap_to,
        "edmd: § 60 Abs. 2 MsbG substitute values generated"
    );

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "malo_id": malo_id,
            "gap_from": gap_from,
            "gap_to": gap_to,
            "generated_count": stored,
            "method_requested": format!("{method:?}"),
            // What each interval was actually produced by. A requested strategy
            // with no data to work from degrades — prior-period to carry-forward
            // to zero — and the § 60 Abs. 6 MsbG record must name what ran, not what
            // was asked for.
            "methods_applied": log_entries
                .iter()
                .filter_map(|e| e["method"].as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            // Intervals already covered by a billable reading; § 60 Abs. 2 MsbG
            // authorises a substitute only where no measurement exists.
            "skipped_measured": skipped,
            "legal_basis": "§ 60 Abs. 2 MsbG Ersatzwertbildung",
            "intervals": log_entries,
        })),
    )
        .into_response()
}
