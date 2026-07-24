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
    /// Number of prior-period days to use for `PriorPeriodAverage` (default: 7).
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

/// Map a [`metering::ForecastMethod`] onto the `substitute_value_log.method`
/// vocabulary.
///
/// The two vocabularies were never reconciled: `ForecastMethod` describes *how*
/// a value was derived, the CHECK list describes § 60 Abs. 2 MsbG substitution
/// categories. Methods with no §17 category map to `LinearInterpolation`, the
/// closest admissible description, rather than failing the write.
pub(crate) fn forecast_method_to_db(method: metering::ForecastMethod) -> &'static str {
    use metering::ForecastMethod as F;
    match method {
        F::PriorPeriodSameSlot | F::WeightedRollingAverage => "PriorPeriodAverage",
        F::LastValueCarryForward => "LastValueCarryForward",
        F::ZeroFill => "ZeroFill",
        F::LinearInterpolation | F::ProfileBased | F::AnnualProjection => "LinearInterpolation",
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

    run_substitute_values(state.repo.pool(), &state.tenant, &malo_id, &req).await
}

/// Core of the § 60 Abs. 2 MsbG substitute flow.
///
/// Shared by the HTTP handler (above) and the `trigger_substitution` MCP tool,
/// so both write substitutes under identical guards: never over a billable
/// reading, always with a `substitute_value_log` audit row, atomically.
pub(crate) async fn run_substitute_values(
    pool: &sqlx::PgPool,
    tenant: &str,
    malo_id: &str,
    req: &SubstituteRequest,
) -> axum::response::Response {
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

    // Fetch prior-period reference data
    let prior_from = gap_from - time::Duration::days(prior_days);
    let prior_reads = sqlx::query(
        r"SELECT dtm_from, dtm_to, quantity_kwh, quality
          FROM meter_reads
          WHERE malo_id = $1 AND dtm_from >= $2 AND dtm_to <= $3
            AND quality IN ('MEASURED','ESTIMATED','CALCULATED')
            AND tenant = $4
          ORDER BY dtm_from ASC LIMIT 10000",
    )
    .bind(malo_id)
    .bind(prior_from)
    .bind(gap_from)
    .bind(tenant)
    .fetch_all(pool)
    .await;

    let prior_intervals: Vec<MeterInterval> = match prior_reads {
        Ok(rows) => {
            use sqlx::Row;
            rows.iter()
                .filter_map(|r| {
                    // 0010: quantity_kwh is NUMERIC(18,5) — read as Decimal directly.
                    let qty: rust_decimal::Decimal = r.try_get("quantity_kwh").ok()?;
                    let quality_str: &str = r.try_get("quality").ok()?;
                    let quality = match quality_str {
                        "MEASURED" => QualityFlag::Measured,
                        "ESTIMATED" => QualityFlag::Estimated,
                        _ => QualityFlag::Calculated,
                    };
                    Some(MeterInterval {
                        from: r.try_get("dtm_from").ok()?,
                        to: r.try_get("dtm_to").ok()?,
                        value_kwh: qty,
                        quality,
                        obis_code: None,
                    })
                })
                .collect()
        }
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: substitute prior-period fetch failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Values bracketing the gap. Linear interpolation needs both ends to have a
    // slope to follow; the other strategies use only the leading value.
    let last_known = prior_intervals.last().map(|iv| iv.value_kwh);
    let next_known: Option<rust_decimal::Decimal> = sqlx::query_scalar(
        r"SELECT quantity_kwh FROM meter_reads
          WHERE malo_id = $1 AND tenant = $2 AND dtm_from >= $3
            AND quality IN ('MEASURED','ESTIMATED','CALCULATED')
          ORDER BY dtm_from ASC LIMIT 1",
    )
    .bind(malo_id)
    .bind(tenant)
    .bind(gap_to)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    // Generate substitute values
    let substitute_entries = metering::substitute_values(
        gap_from,
        gap_to,
        interval_secs,
        method,
        &prior_intervals,
        last_known,
        next_known,
    );

    if substitute_entries.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "No substitute values could be generated for this gap window"
            })),
        )
            .into_response();
    }

    // Store generated intervals and log them
    let sparte = match req
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
    };

    let mut stored = 0usize;
    let mut log_entries: Vec<serde_json::Value> = Vec::new();
    // Intervals left alone because they already carry a billable reading.
    let mut skipped: Vec<String> = Vec::new();

    // Every interval's reading and its § 60 Abs. 6 MsbG audit row commit together. As
    // two independent statements, a failure part-way left billable SUBSTITUTED
    // values in `meter_reads` with no record of who substituted them or why.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute transaction failed to begin");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // Normalised once, the same way `store_reads` does it, so the substitute
    // lands on the register it stands in for rather than on the empty key.
    let obis_norm: String = req.obis_code.as_deref().map_or_else(String::new, |c| {
        c.parse::<metering::obis::ObisCode>()
            .map_or_else(|_| c.to_owned(), |p| p.to_string())
    });

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
        // `entry.method` is a `ForecastMethod`, whose Debug names (e.g.
        // `PriorPeriodSameSlot`) are not the vocabulary the
        // `substitute_value_log.method` CHECK accepts. Writing the Debug form
        // failed the CHECK on the default code path, so the audit INSERT errored
        // *after* the billable substitute had already been committed to
        // `meter_reads` — a §17 Ersatzwert with no §22 audit record.
        let method_str = forecast_method_to_db(entry.method);
        let reason_str = req.reason.as_deref().unwrap_or("NoMeasurementAvailable");

        // Upsert into meter_reads.
        //
        // The `WHERE` on the conflict action is what keeps a substitution from
        // destroying a real reading: § 60 Abs. 2 MsbG authorises an Ersatzwert where
        // no usable measurement exists, not in place of one. A window that
        // overlaps billable data leaves that data untouched and reports the
        // interval as skipped.
        // The CTE snapshots the value being replaced before the upsert runs, so
        // `substitute_value_log.original_kwh` records what was actually there.
        // Without it the § 60 Abs. 6 MsbG trail says a substitute was written but not
        // what it displaced.
        let upserted = sqlx::query(
            r"WITH prior AS (
                  SELECT quantity_kwh
                  FROM meter_reads
                  WHERE tenant = $7 AND malo_id = $1
                    AND dtm_from = $2 AND obis_code_norm = $9
              )
              INSERT INTO meter_reads
                (malo_id, dtm_from, dtm_to, quantity_kwh, quality, pid, sparte, unit,
                 obis_code, obis_code_norm, source, tenant, quality_warnings)
              VALUES ($1, $2, $3, $4, 'SUBSTITUTED', 0, $5, $6, $8, $9, 'AUTO_SUBSTITUTE', $7, $10)
              ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO UPDATE
                SET quantity_kwh = EXCLUDED.quantity_kwh,
                    quality = EXCLUDED.quality,
                    source = EXCLUDED.source,
                    quality_warnings = EXCLUDED.quality_warnings,
                    archived = false
                WHERE meter_reads.quality IN ('FAULTY', 'UNKNOWN')
              RETURNING (SELECT quantity_kwh FROM prior) AS original_kwh",
        )
        .bind(malo_id)
        .bind(iv.from)
        .bind(iv.to)
        .bind(iv.value_kwh)
        .bind(sparte.as_str())
        .bind(sparte.billing_unit().as_str())
        .bind(tenant)
        .bind(req.obis_code.as_deref())
        .bind(&obis_norm)
        .bind(substitute_warnings.get(&entry_idx))
        .fetch_optional(&mut *tx)
        .await;

        let original_kwh: Option<rust_decimal::Decimal> = match upserted {
            Err(e) => {
                tracing::error!(malo_id = %malo_id, error = %e, "edmd: substitute upsert failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
            // No row returned: the conflict action declined because a billable
            // reading already covers this interval.
            Ok(None) => {
                skipped.push(iv.from.format(&Rfc3339).unwrap_or_default());
                continue;
            }
            Ok(Some(row)) => {
                use sqlx::Row as _;
                row.try_get("original_kwh").ok().flatten()
            }
        };

        // § 60 Abs. 6 MsbG audit trail: which value was replaced, by what method, on
        // whose authority.
        if let Err(e) = sqlx::query(
            r"INSERT INTO substitute_value_log
                (malo_id, dtm_from, dtm_to, method, reason, substitute_kwh,
                 original_kwh, created_by, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(malo_id)
        .bind(iv.from)
        .bind(iv.to)
        .bind(method_str)
        .bind(reason_str)
        .bind(iv.value_kwh)
        .bind(original_kwh)
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

        stored += 1;
        log_entries.push(serde_json::json!({
            "from": iv.from,
            "to": iv.to,
            "value_kwh": iv.value_kwh.to_string(),
            "method": method_str,
            "reference_count": entry.reference_count,
            "confidence_note": entry.confidence_note,
        }));
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
            "legal_basis": "§ 60 Abs. 2 MsbG Abs. 2 Ersatzwertbildung",
            "intervals": log_entries,
        })),
    )
        .into_response()
}
