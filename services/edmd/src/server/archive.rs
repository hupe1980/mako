//! Iceberg/S3 cold-tier archive: status/OLAP endpoints, REST catalog, DataFusion SQL.

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

    let (stats, batches) = tokio::join!(
        crate::iceberg::worker::archive_stats(&pool),
        crate::iceberg::worker::recent_batches(&pool, 20),
    );

    let stats = stats.unwrap_or(mako_edm::archive::ArchiveStats {
        total_batches: 0,
        committed_batches: 0,
        total_rows_archived: 0,
        total_bytes_written: 0,
        oldest_cutoff: None,
        newest_cutoff: None,
    });
    let batches = batches.unwrap_or_default();

    let enabled = state.olap_engine.is_some();

    Json(serde_json::json!({
        "enabled": enabled,
        "stats": stats,
        "recent_batches": batches,
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

    let Some(engine) = &state.olap_engine else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Iceberg archival is not enabled — set [archive].enabled = true in edmd.toml" })),
        ).into_response();
    };

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

    match engine.mmm_aggregate(&malo_id, from, to).await {
        Ok(Some(result)) => Json(result).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no archived data for this MaLo / period" })),
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
/// Portfolio-level MMM aggregation over the Iceberg cold tier.
/// Returns total kWh per MaLo ordered by consumption descending.
pub(crate) async fn get_archive_portfolio(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<PortfolioParams>,
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

    let Some(engine) = &state.olap_engine else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "archival not enabled" })),
        )
            .into_response();
    };

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

    match engine
        .portfolio_aggregate(from, to, params.limit.min(10_000))
        .await
    {
        Ok(results) => Json(results).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: archive portfolio query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
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

    let Some(engine) = &state.olap_engine else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "archival not enabled" })),
        )
            .into_response();
    };

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

    match engine.time_series(&malo_id, from, to, 50_000).await {
        Ok(rows) if rows.is_empty() => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no archived data for this MaLo / period" })),
        )
            .into_response(),
        Ok(rows) => Json(rows).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: archive timeseries query failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── P2: Iceberg REST Catalog (ICEBERG-89 spec) ────────────────────────────────
//
// Implements the subset of the Apache Iceberg REST Catalog specification
// required for DuckDB ATTACH, Spark, and Snowflake External Table access.
//
// DuckDB: ATTACH 'rest+http://edmd:8380/api/v1/iceberg' AS mako (TYPE ICEBERG);
// Snowflake: CREATE EXTERNAL TABLE ... WITH (ICEBERG_CATALOG_TYPE='rest', ...);
//
// Spec: https://github.com/apache/iceberg/blob/main/open-api/rest-catalog-open-api.yaml

/// `GET /api/v1/iceberg/v1/config`
///
/// Returns the REST catalog configuration.
/// Required first call by all Iceberg REST clients.
pub(crate) async fn iceberg_rest_config(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    // The catalog exposes table locations and schemas for the tenant's archived
    // meter data, so it is gated by the same action as the archive queries it
    // describes rather than left open to any caller that can reach the port.
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    Json(serde_json::json!({
        "defaults": {},
        "overrides": {
            "prefix": format!("/api/v1/iceberg/v1"),
        },
        "_edmd_version": "0.11.0",
        "_edmd_tenant": state.tenant,
    }))
    .into_response()
}

/// `GET /api/v1/iceberg/v1/namespaces`
///
/// Lists namespaces. edmd uses one namespace per Sparte (STROM/GAS).
pub(crate) async fn iceberg_list_namespaces(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let tenant = state.tenant.as_str();
    // The catalog exposes table locations and schemas for the tenant's archived
    // meter data, so it is gated by the same action as the archive queries it
    // describes rather than left open to any caller that can reach the port.
    if let Err(e) = enforcer.check(&claims.principal(), "read-archive-olap", tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Each Sparte maps to an Iceberg namespace.
    Json(serde_json::json!({
        "namespaces": [
            ["strom"],
            ["gas"],
        ],
        "_catalog": "edmd",
        "_tenant": state.tenant,
    }))
    .into_response()
}

/// `GET /api/v1/iceberg/v1/namespaces/{namespace}/tables`
///
/// Lists tables in a namespace. edmd exposes `meter_reads` as the primary table.
pub(crate) async fn iceberg_list_tables(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(namespace): Path<String>,
    Extension(pool): Extension<Arc<sqlx::PgPool>>,
) -> impl IntoResponse {
    // The catalog exposes table locations and schemas for the tenant's archived
    // meter data, so it is gated by the same action as the archive queries it
    // describes rather than left open to any caller that can reach the port.
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-archive-olap",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use sqlx::Row as _;
    // Fetch registered catalog entries for this tenant + namespace.
    let rows = sqlx::query(
        r"SELECT table_name FROM iceberg_catalog_entries
          WHERE namespace = $1 AND tenant = $2
          ORDER BY table_name",
    )
    .bind(&namespace)
    .bind(&state.tenant)
    .fetch_all(pool.as_ref())
    .await
    .unwrap_or_default();

    let mut identifiers: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let name: String = r.try_get("table_name").unwrap_or_default();
            serde_json::json!({ "namespace": [namespace], "name": name })
        })
        .collect();

    // Always expose the primary `meter_reads` table.
    if !identifiers
        .iter()
        .any(|i| i.get("name").and_then(|v| v.as_str()) == Some("meter_reads"))
    {
        identifiers.push(serde_json::json!({
            "namespace": [namespace],
            "name": "meter_reads",
        }));
    }

    Json(serde_json::json!({ "identifiers": identifiers })).into_response()
}

/// `GET /api/v1/iceberg/v1/namespaces/{namespace}/tables/{table}`
///
/// Returns the Iceberg table metadata for a named table.
/// This is the primary endpoint DuckDB/Spark use to discover schema and files.
pub(crate) async fn iceberg_load_table(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path((namespace, table)): Path<(String, String)>,
    Extension(pool): Extension<Arc<sqlx::PgPool>>,
) -> impl IntoResponse {
    // The catalog exposes table locations and schemas for the tenant's archived
    // meter data, so it is gated by the same action as the archive queries it
    // describes rather than left open to any caller that can reach the port.
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "read-archive-olap",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    use sqlx::Row as _;

    // Look up the catalog entry from the iceberg_catalog_entries table.
    let entry = sqlx::query(
        r"SELECT location_uri, schema_json, partition_spec, properties, current_snapshot_id
          FROM iceberg_catalog_entries
          WHERE namespace = $1 AND table_name = $2 AND tenant = $3
          LIMIT 1",
    )
    .bind(&namespace)
    .bind(&table)
    .bind(&state.tenant)
    .fetch_optional(pool.as_ref())
    .await;

    match entry {
        Ok(Some(row)) => {
            let location: String = row.try_get("location_uri").unwrap_or_default();
            let schema_json: serde_json::Value = row.try_get("schema_json").unwrap_or_default();
            let snapshot_id: Option<i64> = row.try_get("current_snapshot_id").unwrap_or(None);

            // Build minimal Iceberg REST table response per spec.
            let response = serde_json::json!({
                "metadata-location": format!("{}/metadata/v1.metadata.json", location),
                "metadata": {
                    "format-version": 2,
                    "table-uuid": uuid::Uuid::new_v4().to_string(),
                    "location": location,
                    "last-sequence-number": 1,
                    "last-updated-ms": time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000i128,
                    "last-column-id": 10,
                    "current-schema-id": 0,
                    "schemas": [schema_json],
                    "default-spec-id": 0,
                    "partition-specs": [{"spec-id": 0, "fields": []}],
                    "sort-orders": [{"order-id": 0, "fields": []}],
                    "properties": {"write.format.default": "parquet"},
                    "current-snapshot-id": snapshot_id,
                    "snapshots": [],
                },
                "config": {
                    "s3.region": "eu-central-1",
                }
            });

            (StatusCode::OK, Json(response)).into_response()
        }
        Ok(None) => {
            // Return a synthetic schema for the built-in meter_reads table.
            // This allows DuckDB to query the cold tier even before the catalog
            // entry is explicitly registered.
            if table == "meter_reads" {
                let schema = serde_json::json!({
                    "type": "struct",
                    "schema-id": 0,
                    "fields": [
                        {"id": 1, "name": "malo_id",     "type": "string",  "required": true},
                        {"id": 2, "name": "dtm_from",    "type": "timestamptz", "required": true},
                        {"id": 3, "name": "dtm_to",      "type": "timestamptz", "required": true},
                        {"id": 4, "name": "quantity_kwh","type": "decimal(18,5)", "required": true},
                        {"id": 5, "name": "quality",     "type": "string",  "required": true},
                        {"id": 6, "name": "sparte",      "type": "string",  "required": true},
                        {"id": 7, "name": "obis_code",   "type": "string",  "required": false},
                        {"id": 8, "name": "tenant",      "type": "string",  "required": true},
                        {"id": 9, "name": "sender_mp_id","type": "string",  "required": false},
                        {"id": 10, "name": "allocation_version", "type": "string", "required": false},
                    ]
                });
                let response = serde_json::json!({
                    "metadata-location": "not-yet-archived",
                    "metadata": {
                        "format-version": 2,
                        "table-uuid": uuid::Uuid::new_v4().to_string(),
                        "location": format!("s3://edmd-archive/{}/{}", &state.tenant, namespace),
                        "current-schema-id": 0,
                        "schemas": [schema],
                        "partition-specs": [{"spec-id": 0, "fields": [
                            {"source-id": 1, "field-id": 1000, "name": "malo_id", "transform": "identity"},
                        ]}],
                        "sort-orders": [{"order-id": 0, "fields": []}],
                        "properties": {"write.format.default": "parquet", "write.parquet.compression-codec": "zstd"},
                        "current-snapshot-id": serde_json::Value::Null,
                        "snapshots": [],
                    },
                    "_note": "No archived data yet — run archival worker first or push data via MSCONS ingest"
                });
                (StatusCode::OK, Json(response)).into_response()
            } else {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": {"message": format!("Table {}.{} not found", namespace, table),
                                  "type": "NoSuchTableException",
                                  "code": 404}
                    })),
                )
                    .into_response()
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: iceberg_load_table DB error");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── P2: DataFusion SQL endpoint ────────────────────────────────────────────────
//
// Runs analytical SQL over the Iceberg cold archive using the embedded
// DataFusion engine. Results are returned as JSON arrays.
//
// Example queries that DuckDB users would run (but via DataFusion instead):
//   POST /api/v1/query/sql
//   {"sql": "SELECT malo_id, SUM(quantity_kwh) AS total_kwh
//            FROM edmd.meter_reads
//            WHERE dtm_from >= '2026-01-01' AND dtm_from < '2026-02-01'
//            GROUP BY malo_id ORDER BY total_kwh DESC LIMIT 10"}

#[derive(serde::Deserialize)]
pub(crate) struct SqlQueryRequest {
    sql: String,
    /// Maximum rows to return (default: 10_000).
    #[serde(default = "default_sql_limit")]
    limit: usize,
    /// Output format: "json" (default) or "arrow_ipc".
    #[serde(default)]
    #[allow(dead_code)]
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

    // Tenant scoping is carried by the `meter_reads_archive` view. Naming the
    // physical table behind it would read every tenant's rows, so a query that
    // mentions it at all is refused.
    if sql_upper.contains(&crate::iceberg::query::ARCHIVE_PHYSICAL_TABLE.to_uppercase()) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "query references an internal table; use `meter_reads_archive`"
            })),
        )
            .into_response();
    }

    // Execute via DataFusion on the Iceberg cold archive.
    if let Some(ref olap) = state.olap_engine {
        match olap.query_to_json(&req.sql, req.limit).await {
            Ok(rows) => {
                return Json(serde_json::json!({
                    "rows": rows,
                    "row_count": rows.len(),
                    "sql": req.sql,
                    "source": "iceberg_cold_archive",
                }))
                .into_response();
            }
            Err(e) => {
                tracing::warn!(error = %e, sql = %req.sql, "edmd: DataFusion SQL query failed");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": e.to_string(),
                        "sql": req.sql,
                        "hint": "Cold archive may be empty — ensure archival worker has run"
                    })),
                )
                    .into_response();
            }
        }
    }

    // No OLAP engine configured — return helpful error.
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({
            "error": "OLAP engine not configured",
            "hint": "Set [archive] enabled=true and storage_uri in edmd.toml to enable DataFusion SQL"
        })),
    )
        .into_response()
}
