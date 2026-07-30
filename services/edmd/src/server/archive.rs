//! Cold-tier archive surface: status, single-MaLo and portfolio MMM aggregation,
//! raw time-series export, and read-only analytical SQL — all evaluated by
//! meterstore across the hot + cold tiers. External Iceberg clients use
//! meterstore's own catalog facade, not an edmd-hosted REST catalog.

#[allow(unused_imports)]
use super::*;

// ── Archive endpoint handlers ─────────────────────────────────────────────────

/// `GET /api/v1/archive/status`
///
/// Returns archive statistics and the 20 most recent batches.
pub(crate) async fn get_archive_status(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<Arc<PgPool>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-status", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let _ = &pool;
    // Archival is owned by meterstore now (hot Postgres + cold Iceberg). Report
    // the tier it manages rather than the former per-batch export bookkeeping.
    let store = state.repo.store();
    Json(serde_json::json!({
        "enabled": true,
        "backend": "meterstore",
        "tables": {
            "resolved": store.resolved_table(),
            "raw": store.raw_table(),
        },
        "note": "Cold-tier archival is managed by meterstore; per-batch archive \
                 statistics are no longer tracked.",
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub(crate) struct ArchiveOlapParams {
    /// RFC 3339 start (inclusive).
    from: Option<String>,
    /// RFC 3339 end (inclusive).
    to: Option<String>,
}

/// `GET /api/v1/archive/olap/{malo_id}?from=RFC3339&to=RFC3339`
///
/// DataFusion OLAP query over archived `meter_reads` for one MaLo.
///
/// Returns the aggregated MMM result: total kWh, read count, and period bounds.
/// Requires Iceberg/S3 archival to be enabled and configured.
///
/// This is the primary endpoint for MMM aggregation over archived data.
/// For recent data (< 12 months) use `/api/v1/billing-period/{malo_id}`.
pub(crate) async fn get_archive_olap(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ArchiveOlapParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
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

    // MMM aggregation now runs against the version-resolved, tier-split series
    // meterstore hands back — the same numbers a settlement would reconcile.
    match state
        .repo
        .store()
        .series(malo_id.clone())
        .column_eq(
            "tenant",
            datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
        )
        .range(from, to)
        .collect()
        .await
    {
        Ok(Some(series)) => {
            let total: rust_decimal::Decimal = series.intervals.iter().map(|i| i.value_kwh).sum();
            Json(serde_json::json!({
                "malo_id": malo_id,
                "total_kwh": total.to_string(),
                "read_count": series.intervals.len(),
                "from": from.to_string(),
                "to": to.to_string(),
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no data for this MaLo / period" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: archive OLAP query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PortfolioParams {
    from: Option<String>,
    to: Option<String>,
    #[serde(default = "default_portfolio_limit")]
    limit: usize,
}
pub(crate) fn default_portfolio_limit() -> usize {
    100
}

/// `GET /api/v1/archive/portfolio?from=RFC3339&to=RFC3339&limit=N`
///
/// Portfolio-level MMM aggregation across both tiers, evaluated in one
/// version-resolved plan by meterstore. Returns total kWh and read count per
/// MaLo, ordered by consumption descending, capped at `limit`.
pub(crate) async fn get_archive_portfolio(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<PortfolioParams>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use time::format_description::well_known::Rfc3339;

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
    // A hostile `limit` would otherwise reach the SQL text; clamp it to a sane
    // ceiling and render it as a plain integer so it cannot carry an injection.
    let limit = params.limit.clamp(1, 10_000);

    // Portfolio-wide MMM is a cross-MaLo GROUP BY over the version-resolved
    // relation, which meterstore evaluates across both tiers in one plan. `from`
    // is a SQL reserved word, hence quoted; the bounds and the tenant travel as
    // bound parameters so no value reaches the SQL text. The tenant predicate is
    // defence in depth — a deployment writes only its own tenant — but it keeps
    // the aggregate correct even if a store is ever shared across tenants.
    let store = state.repo.store();
    let sql = format!(
        r#"SELECT "malo_id", SUM("value") AS total_kwh, COUNT(*) AS read_count
             FROM "{table}"
            WHERE "from" >= $1 AND "from" < $2 AND "tenant" = $3
            GROUP BY "malo_id"
            ORDER BY total_kwh DESC
            LIMIT {limit}"#,
        table = store.resolved_table(),
    );
    match store
        .query_with_params(
            &sql,
            vec![
                ts_param(from),
                ts_param(to),
                datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
            ],
        )
        .await
    {
        Ok(result) => match result.to_json() {
            Ok(rows) => Json(serde_json::json!({
                "from": from.to_string(),
                "to": to.to_string(),
                "malo_count": rows.len(),
                "spans_tiers": result.spans_tiers(),
                "portfolio": rows,
            }))
            .into_response(),
            Err(e) => {
                tracing::warn!(error = %e, "edmd: portfolio JSON serialisation failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "edmd: portfolio aggregation query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// A UTC instant as the `TimestampMicrosecond` literal the storage schema binds
/// its interval bounds as (`col::FROM` / `col::TO`).
pub(crate) fn ts_param(t: OffsetDateTime) -> datafusion::scalar::ScalarValue {
    datafusion::scalar::ScalarValue::TimestampMicrosecond(
        Some((t.unix_timestamp_nanos() / 1_000) as i64),
        Some("UTC".into()),
    )
}

/// `"true"` / `"false"` for a boolean header value.
fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

/// Serialise meterstore's own `RecordBatch`es to an Arrow IPC stream.
///
/// The schema is passed explicitly so a zero-row result still yields a valid,
/// self-describing stream (schema message + no batches) rather than empty bytes.
/// meterstore's Arrow is the workspace Arrow (one unified 58.x), so its batches
/// write through edmd's `StreamWriter` without a copy.
fn batches_to_arrow_ipc(
    schema: &arrow::datatypes::SchemaRef,
    batches: &[arrow::record_batch::RecordBatch],
) -> anyhow::Result<Vec<u8>> {
    use arrow::ipc::writer::StreamWriter;
    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, schema)?;
    for batch in batches {
        writer.write(batch)?;
    }
    writer.finish()?;
    Ok(buf)
}

/// `GET /api/v1/archive/timeseries/{malo_id}?from=RFC3339&to=RFC3339&limit=N`
///
/// Raw time-series export from the Iceberg cold tier.
/// Returns up to `limit` archived reads in chronological order.
pub(crate) async fn get_archive_timeseries(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ArchiveOlapParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
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

    match state
        .repo
        .store()
        .series(malo_id.clone())
        .column_eq(
            "tenant",
            datafusion::scalar::ScalarValue::Utf8(Some(state.tenant.clone())),
        )
        .range(from, to)
        .collect()
        .await
    {
        Ok(Some(series)) if !series.intervals.is_empty() => {
            let rows: Vec<serde_json::Value> = series
                .intervals
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "dtm_from": i.from.to_string(),
                        "dtm_to": i.to.to_string(),
                        "quantity_kwh": i.value_kwh.to_string(),
                        "quality": i.quality.as_str(),
                        "obis_code": i.obis_code.map(|o| o.to_string()),
                    })
                })
                .collect();
            Json(serde_json::json!({ "malo_id": malo_id, "rows": rows })).into_response()
        }
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no data for this MaLo / period" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: archive timeseries query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Analytical SQL endpoint ─────────────────────────────────────────────────────
//
// Runs read-only SQL across both tiers in meterstore's DataFusion session over
// the version-resolved relation (`store.resolved_table()`). Results come back as
// JSON rows (default) or an Arrow IPC stream (`format: "arrow_ipc"`), and carry
// the tier provenance meterstore computed the answer against.
//
// Scope: an edmd deployment writes exactly one tenant (`cfg.tenant`) to every
// row, so its store holds that tenant alone and the result is tenant-scoped by
// construction. A caller that wants an explicit guard adds `WHERE "tenant" = …`;
// the endpoint does not inject one, because it cannot rewrite arbitrary SQL
// safely, and refuses the raw versioned relation so no query can double-count
// corrected intervals.
//
// The value column is `value` and the interval start is `from` (a reserved word,
// so quote it). Example:
//   POST /api/v1/query/sql
//   {"sql": "SELECT malo_id, SUM(\"value\") AS total_kwh
//            FROM meter_reads
//            WHERE \"from\" >= '2026-01-01' AND \"from\" < '2026-02-01'
//            GROUP BY malo_id ORDER BY total_kwh DESC LIMIT 10"}

#[derive(serde::Deserialize)]
pub(crate) struct SqlQueryRequest {
    sql: String,
    /// Maximum rows to return (default: 10_000).
    #[serde(default = "default_sql_limit")]
    limit: usize,
    /// Output format: "json" (default) or "arrow_ipc".
    #[serde(default)]
    format: SqlOutputFormat,
}

pub(crate) fn default_sql_limit() -> usize {
    10_000
}

#[derive(serde::Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SqlOutputFormat {
    #[default]
    Json,
    ArrowIpc,
}

pub(crate) async fn post_sql_query(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<SqlQueryRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    // Caller-supplied SQL runs against the archive tier, so it is gated by the
    // archive capability, not the generic hot-tier read action.
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Reject obviously dangerous SQL (allow only SELECT/WITH/SHOW).
    let sql_upper = req.sql.trim().to_uppercase();
    if !sql_upper.starts_with("SELECT")
        && !sql_upper.starts_with("WITH")
        && !sql_upper.starts_with("SHOW")
        && !sql_upper.starts_with("DESCRIBE")
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Only SELECT/WITH/SHOW/DESCRIBE queries are allowed"
            })),
        )
            .into_response();
    }

    // Naming the raw, every-version relation would double-count corrected
    // intervals, so a query that mentions it is refused — callers use the
    // resolved `meter_reads` relation.
    let store = state.repo.store();
    if sql_upper.contains(&store.raw_table().to_uppercase()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "query references the raw versioned relation; use the resolved `meter_reads`"
            })),
        )
            .into_response();
    }

    // Run the statement across both tiers in meterstore's own DataFusion session
    // (the resolved relation is registered as `store.resolved_table()`), then take
    // the provenance the result carries. `spans_tiers` / `touched_hot_tier` tell a
    // reporting caller whether the figure is reproducible or read the mutable hot
    // tier — the one fact a bare row set cannot state about itself.
    let limit = req.limit.min(default_sql_limit());
    match store.query(&req.sql).await {
        Ok(result) => {
            let touched_hot = result.touched_hot_tier();
            let spans = result.spans_tiers();
            let total = result.num_rows();

            // Analytical clients ask for the columnar stream directly; the tier
            // provenance rides in headers since the body is now binary.
            if req.format == SqlOutputFormat::ArrowIpc {
                return match batches_to_arrow_ipc(&result.schema(), result.batches()) {
                    Ok(bytes) => (
                        StatusCode::OK,
                        [
                            ("content-type", "application/vnd.apache.arrow.stream"),
                            ("x-meterstore-spans-tiers", bool_str(spans)),
                            ("x-meterstore-touched-hot-tier", bool_str(touched_hot)),
                        ],
                        bytes,
                    )
                        .into_response(),
                    Err(e) => {
                        tracing::warn!(error = %e, "edmd: SQL result Arrow-IPC serialisation failed");
                        StatusCode::INTERNAL_SERVER_ERROR.into_response()
                    }
                };
            }

            match result.to_json() {
                Ok(mut rows) => {
                    let truncated = rows.len() > limit;
                    rows.truncate(limit);
                    Json(serde_json::json!({
                        "row_count": rows.len(),
                        "total_rows": total,
                        "truncated": truncated,
                        "spans_tiers": spans,
                        "touched_hot_tier": touched_hot,
                        "rows": rows,
                    }))
                    .into_response()
                }
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: SQL result JSON serialisation failed");
                    StatusCode::INTERNAL_SERVER_ERROR.into_response()
                }
            }
        }
        Err(e) => (
            // A planning/type error is the caller's SQL, not an edmd fault.
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string(), "sql": req.sql })),
        )
            .into_response(),
    }
}
