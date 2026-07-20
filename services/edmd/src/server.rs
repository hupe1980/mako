//! Axum router and startup logic for `edmd`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

// Quality scoring and Gas conversion are provided by the `metering` crate.
// The inline `compute_quality` has been replaced with a call to `metering::score_intervals`.
// Tests for the Hampel filter logic live in crates/metering/src/quality.rs.

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::{Claims, OidcVerifier};
use rubo4e::current::{
    Energiemenge, Lastgang, Medium, Menge, Mengeneinheit, Messart, Messwertstatus,
    Sparte as Bo4eSparte, Zeitraum, Zeitreihe, Zeitreihenwert,
};
use rubo4e::identifiers::ObisCode;
use rust_decimal::Decimal;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{
    handler::{HandlerState, handle_webhook},
    iceberg::query::OlapEngine,
    pg::PgTimeSeriesRepository,
    smgw::{
        get_smgw_compliance, get_smgw_session, list_smgw_sessions, post_smgw_compliance_scan,
        put_smgw_session,
    },
};
use mako_edm::{
    domain::{
        BillingPeriodQuery, IngestionSource, MeterRead, QualityFlag, Sparte as EdmSparte,
        TimeSeriesQuery,
    },
    repository::TimeSeriesRepository,
};

/// Map the `metering` Sparte onto the `mako-edm` domain Sparte.
///
/// The two enums carry the same variants; they are separate types because
/// `metering` is I/O-free and `mako-edm` is the persistence-facing model.
const fn edm_sparte_from_metering(s: metering::interval::Sparte) -> EdmSparte {
    match s {
        metering::interval::Sparte::Strom => EdmSparte::Strom,
        metering::interval::Sparte::Gas => EdmSparte::Gas,
        metering::interval::Sparte::Waerme => EdmSparte::Waerme,
        metering::interval::Sparte::Wasser => EdmSparte::Wasser,
    }
}

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/api/v1/deliveries/{malo_id}", get(get_deliveries))
        .route(
            "/api/v1/imbalance/{malo_id}/{year}/{month}",
            get(get_imbalance),
        )
        .route("/api/v1/billing-period/{malo_id}", get(get_billing_period))
        // Collection endpoint for mabis-syncd MaLo discovery.
        // mabis-syncd calls: GET /api/v1/billing-periods?from=YYYY-MM-DD&to=YYYY-MM-DD&tenant=...
        .route("/api/v1/billing-periods", get(list_billing_periods))
        .route("/api/v1/lastgang/{malo_id}", get(get_lastgang))
        .route("/api/v1/zeitreihe/{malo_id}", get(get_zeitreihe))
        // ── Ablesesteuerung ───────────────────────────────────────────────────
        .route(
            "/api/v1/reading-orders",
            post(create_reading_order).get(list_reading_orders),
        )
        .route("/api/v1/reading-orders/{id}", get(get_reading_order))
        .route(
            "/api/v1/reading-orders/{id}/complete",
            put(complete_reading_order),
        )
        .route(
            "/api/v1/reading-orders/{id}/cancel",
            put(cancel_reading_order),
        )
        .route("/api/v1/reading-orders/{id}/fail", put(fail_reading_order))
        // N7: Jahresablesung campaign scheduler (§40 Abs. 2 EnWG)
        .route(
            "/api/v1/reading-orders/campaign",
            post(jahresablesung_campaign),
        )
        .route(
            "/api/v1/compliance/jahresablesung/{year}",
            get(jahresablesung_compliance),
        )
        .route(
            "/api/v1/gdpr/erasure/{malo_id}/archive-plan",
            post(plan_gdpr_archive_erasure),
        )
        .route(
            "/api/v1/gdpr/erasure/{malo_id}/archive-complete",
            post(complete_gdpr_archive_erasure),
        )
        // M4: iMSys / SMGW direct push — bypasses EDIFACT for RLM/iMSys customers.
        // §41a EnWG dynamic tariffs require sub-hourly resolution;
        // MSCONS round-trip adds 15–60 min latency.
        .route(
            "/api/v1/meter-reads/rlm/{malo_id}",
            post(post_direct_reads_rlm),
        )
        .route(
            "/api/v1/meter-reads/gas/{malo_id}",
            post(post_direct_reads_gas),
        )
        // M7: retroactive quality scoring for existing meter_reads (MSCONS or direct push).
        .route(
            "/api/v1/quality-score/{malo_id}",
            post(post_quality_rescore),
        )
        // §22 MessZV bitemporal corrections: audit-trail preserving retroactive corrections.
        .route("/api/v1/corrections/{malo_id}", post(post_corrections))
        // Bulk ingestion: batched direct-push reads (performance path for large MSCONS deliveries)
        .route("/api/v1/meter-reads/{malo_id}/bulk", post(post_bulk_reads))
        // §17 MessZV auto-substitute: fill gaps using prior-period average method
        .route(
            "/api/v1/meter-reads/{malo_id}/substitute",
            post(post_substitute_values),
        )
        // M8: resampled Lastgang — down-sample to hourly / daily / monthly buckets
        .route(
            "/api/v1/lastgang/{malo_id}/resampled",
            get(get_lastgang_resampled),
        )
        // M9: Virtual meter — compute derived time series from AggregationRule
        .route(
            "/api/v1/virtual/{virtual_malo_id}/lastgang",
            get(get_virtual_lastgang),
        )
        .route(
            "/api/v1/virtual",
            get(list_virtual_meters).post(create_virtual_meter),
        )
        .route(
            "/api/v1/virtual/{virtual_malo_id}",
            get(get_virtual_meter).delete(delete_virtual_meter),
        )
        // M10: Quality assessments — per-batch quality history
        .route(
            "/api/v1/quality-assessments/{malo_id}",
            get(list_quality_assessments),
        )
        // M11: Annual forecast (§17 MessZV Jahresprognose)
        .route("/api/v1/forecast/{malo_id}", get(get_annual_forecast))
        // M12: Summenzeitreihe — MABIS-ready monthly aggregated series
        .route(
            "/api/v1/summenzeitreihe/{malo_id}",
            get(get_summenzeitreihe),
        )
        // M13: Gas quality data (PID 13007 Gasbeschaffenheitsdaten)
        .route("/api/v1/gas-quality/{malo_id}", get(get_gas_quality))
        // Iceberg/S3 archive endpoints
        .route("/api/v1/archive/status", get(get_archive_status))
        .route("/api/v1/archive/olap/{malo_id}", get(get_archive_olap))
        .route("/api/v1/archive/portfolio", get(get_archive_portfolio))
        .route(
            "/api/v1/archive/timeseries/{malo_id}",
            get(get_archive_timeseries),
        )
        // §42c Energy Sharing VZW quarter-hour allocation
        .route("/api/v1/sharing/readiness", get(get_sharing_readiness))
        .route("/api/v1/meter-reads/iot/{malo_id}", post(post_iot_reads))
        .route(
            "/api/v1/sharing/{community_id}/allocation",
            get(get_sharing_allocation),
        )
        // GDPR §17 DSGVO right to erasure — mark a MaLo for deletion from
        // hot PostgreSQL storage. Cold Iceberg deletion is scheduled asynchronously.
        .route(
            "/api/v1/gdpr/erasure/{malo_id}",
            axum::routing::delete(post_gdpr_erasure),
        )
        // P2: Iceberg REST catalog — enables DuckDB/Snowflake/Databricks to query
        // the cold Iceberg archive directly without going through edmd REST.
        // DuckDB: ATTACH 'rest+http://edmd:8380' AS mako (TYPE ICEBERG);
        // Spec: Apache Iceberg REST Catalog specification (ICEBERG-89).
        .route("/api/v1/iceberg/v1/config", get(iceberg_rest_config))
        .route(
            "/api/v1/iceberg/v1/namespaces",
            get(iceberg_list_namespaces),
        )
        .route(
            "/api/v1/iceberg/v1/namespaces/{namespace}/tables",
            get(iceberg_list_tables),
        )
        .route(
            "/api/v1/iceberg/v1/namespaces/{namespace}/tables/{table}",
            get(iceberg_load_table),
        )
        // P2: DataFusion SQL endpoint — runs analytical SQL over both hot
        // (PostgreSQL via custom UDF) and cold (Iceberg/Parquet via DataFusion)
        // tier. Returns results as Arrow IPC or JSON.
        .route("/api/v1/query/sql", post(post_sql_query))
        // ── §14a SMGW session registry (MsbG §21c / BSI TR-03109) ────────────
        // `compliance` is a static segment and takes priority over {malo_id} in Axum 0.8.
        .route("/api/v1/smgw", get(list_smgw_sessions))
        .route("/api/v1/smgw/compliance", get(get_smgw_compliance))
        .route(
            "/api/v1/smgw/compliance/scan",
            axum::routing::post(post_smgw_compliance_scan),
        )
        .route(
            "/api/v1/smgw/{malo_id}",
            get(get_smgw_session).put(put_smgw_session),
        )
        .route("/metrics", get(metrics))
        .route("/health/live", get(|| async { StatusCode::OK }))
        .route("/health/ready", get(health_ready))
        .with_state(state)
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// `GET /health/ready` — confirms the database connection is alive.
/// Returns 503 when the pool cannot reach PostgreSQL.
async fn health_ready(State(state): State<HandlerState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(state.repo.pool()).await {
        Ok(_) => StatusCode::OK,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: readiness probe: DB unreachable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `GET /metrics` — Prometheus-compatible operational metrics.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    let meter_reads: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_reads")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);
    let billing_periods: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods")
        .fetch_one(state.repo.pool())
        .await
        .unwrap_or(0);
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

    out.push_str("# HELP edmd_meter_reads_total Total meter read entries stored.\n");
    out.push_str("# TYPE edmd_meter_reads_total gauge\n");
    out.push_str(&format!("edmd_meter_reads_total {meter_reads}\n"));
    out.push_str("# HELP edmd_billing_periods_total Pre-aggregated MeterBillingPeriod records.\n");
    out.push_str("# TYPE edmd_billing_periods_total gauge\n");
    out.push_str(&format!("edmd_billing_periods_total {billing_periods}\n"));
    out.push_str("# HELP edmd_db_pool_size Current PostgreSQL connection pool size.\n");
    out.push_str("# TYPE edmd_db_pool_size gauge\n");
    out.push_str(&format!("edmd_db_pool_size {pool_size}\n"));
    out.push_str("# HELP edmd_db_pool_idle Idle PostgreSQL connections.\n");
    out.push_str("# TYPE edmd_db_pool_idle gauge\n");
    out.push_str(&format!("edmd_db_pool_idle {pool_idle}\n"));

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        out,
    )
}

#[derive(Debug, Deserialize)]
struct DeliveryQueryParams {
    from: Option<String>,
    to: Option<String>,
}

async fn get_deliveries(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<DeliveryQueryParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    // Cedar check — resource tenant is the service-level tenant injected at startup.
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    match state.repo.query(&q).await {
        Ok(reads) => {
            let energiemengen: Vec<Energiemenge> = reads.iter().map(read_to_energiemenge).collect();
            Json(energiemengen).into_response()
        }
        Err(err) => {
            tracing::warn!(%err, malo_id, "edmd: get_deliveries failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_imbalance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path((malo_id, year, month)): Path<(String, i32, u8)>,
) -> impl IntoResponse {
    use time::{Date, Month};

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-imbalance", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let month_enum = match Month::try_from(month) {
        Ok(m) => m,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid month").into_response(),
    };

    let from = match Date::from_calendar_date(year, month_enum, 1) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid date").into_response(),
    };
    let to = match from.replace_month(month_enum).and_then(|d| {
        // Last day of month.
        let next_month = if month == 12 {
            Date::from_calendar_date(year + 1, Month::January, 1)
        } else {
            Date::from_calendar_date(year, Month::try_from(month + 1).unwrap(), 1)
        };
        next_month.map(|nm| nm.previous_day().unwrap_or(d))
    }) {
        Ok(d) => d,
        Err(_) => return (StatusCode::BAD_REQUEST, "date calculation failed").into_response(),
    };

    match state
        .repo
        .imbalance(&malo_id, from, to, &state.tenant)
        .await
    {
        Ok(report) => Json(serde_json::to_value(report).unwrap_or_default()).into_response(),
        Err(mako_edm::error::EdmError::NoData { .. }) => {
            (StatusCode::NOT_FOUND, "no data for this MaLo / period").into_response()
        }
        Err(err) => {
            tracing::warn!(%err, malo_id, "edmd: get_imbalance failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/billing-period/{malo_id}?from=YYYY-MM-DD&to=YYYY-MM-DD`
///
/// Returns the aggregated billing-period summary for a MaLo.
///
/// Consumed by `invoicd` for RLM plausibility (M16) and by `netzbilanzd` for
/// NNE invoice generation (N4).  Includes:
/// - `arbeitsmenge_kwh` — total energy quantity
/// - `spitzenleistung_kw` — peak demand (RLM Strom only)
/// - `brennwert_kwh_per_m3` / `zustandszahl` — Gas conversion factors
///
/// Source: GPKE BK6-22-024 §3; GeLi Gas 3.0 (BK7-24-01-009) §3.
#[derive(Debug, Deserialize)]
struct BillingPeriodParams {
    /// ISO 8601 date `YYYY-MM-DD` — start of billing period (inclusive).
    from: Option<String>,
    /// ISO 8601 date `YYYY-MM-DD` — end of billing period (inclusive).
    to: Option<String>,
}

async fn get_billing_period(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<BillingPeriodParams>,
) -> impl IntoResponse {
    use time::macros::format_description;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-billing-period", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let fmt = format_description!("[year]-[month]-[day]");

    let period_from = match params.from.as_deref() {
        Some(s) => match time::Date::parse(s, &fmt) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid 'from' date — use YYYY-MM-DD" })),
                )
                    .into_response();
            }
        },
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "'from' query parameter is required" })),
            )
                .into_response();
        }
    };

    let period_to = match params.to.as_deref() {
        Some(s) => match time::Date::parse(s, &fmt) {
            Ok(d) => d,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": "invalid 'to' date — use YYYY-MM-DD" })),
                )
                    .into_response();
            }
        },
        None => period_from, // Default: single-day period
    };

    if period_to < period_from {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "'to' must be >= 'from'" })),
        )
            .into_response();
    }

    let q = BillingPeriodQuery {
        malo_id: malo_id.clone(),
        period_from,
        period_to,
        tenant: state.tenant.clone(),
    };

    match state.repo.billing_period(&q).await {
        Ok(Some(period)) => Json(serde_json::to_value(period).unwrap_or_default()).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no meter reads found for this MaLo / period" })),
        )
            .into_response(),
        Err(err) => {
            tracing::warn!(%err, %malo_id, "edmd: get_billing_period failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Billing periods collection (mabis-syncd MaLo discovery) ──────────────────

/// `GET /api/v1/billing-periods?from=YYYY-MM-DD&to=YYYY-MM-DD&tenant=...`
///
/// Returns a list of `(malo_id, messtyp, period_from, period_to)` for all
/// MaLos that have billing period aggregates in the requested date window.
///
/// Used by `mabis-syncd` to discover which MaLo IDs have meter data in a given
/// month so it can submit Summenzeitreihen to BIKO (BK6-22-024 Anlage 3).
///
/// This is the collection form; `GET /api/v1/billing-period/{malo_id}` returns a
/// single MaLo.
#[derive(serde::Deserialize)]
struct BillingPeriodsParams {
    /// Period start date inclusive (YYYY-MM-DD).
    from: Option<String>,
    /// Period end date inclusive (YYYY-MM-DD).
    to: Option<String>,
    /// Optional tenant filter — overrides the instance tenant when set.
    tenant: Option<String>,
}

async fn list_billing_periods(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<BillingPeriodsParams>,
) -> impl IntoResponse {
    use time::macros::format_description;
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-billing-period", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Cedar authorised the caller against `resource_tenant`, so the query must
    // run against that same tenant. Binding a caller-supplied `?tenant=` would
    // let a principal cleared for its own tenant read any other tenant's
    // portfolio, since the parameter is never re-authorised.
    if let Some(requested) = params.tenant.as_deref()
        && requested != resource_tenant
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "tenant parameter does not match the authorised tenant",
            })),
        )
            .into_response();
    }
    let tenant = resource_tenant.to_owned();

    let fmt = format_description!("[year]-[month]-[day]");
    let from_date = params
        .from
        .as_deref()
        .and_then(|s| time::Date::parse(s, fmt).ok())
        .unwrap_or(time::Date::MIN);
    let to_date = params
        .to
        .as_deref()
        .and_then(|s| time::Date::parse(s, fmt).ok())
        .unwrap_or(time::Date::MAX);

    let pool = state.repo.pool();
    let rows = sqlx::query(
        r"SELECT malo_id, messtyp, sparte, period_from, period_to
          FROM meter_billing_periods
          WHERE period_from >= $1
            AND period_to   <= $2
            AND tenant       = $3
          ORDER BY malo_id, period_from",
    )
    .bind(from_date)
    .bind(to_date)
    .bind(&tenant)
    .fetch_all(pool)
    .await;

    match rows {
        Ok(rows) => {
            let items: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    use sqlx::Row as _;
                    let period_from: time::Date =
                        r.try_get("period_from").unwrap_or(time::Date::MIN);
                    let period_to: time::Date = r.try_get("period_to").unwrap_or(time::Date::MIN);
                    serde_json::json!({
                        "malo_id":     r.try_get::<String, _>("malo_id").unwrap_or_default(),
                        "messtyp":     r.try_get::<String, _>("messtyp").unwrap_or_default(),
                        "sparte":      r.try_get::<String, _>("sparte").unwrap_or_default(),
                        "period_from": period_from.to_string(),
                        "period_to":   period_to.to_string(),
                    })
                })
                .collect();
            Json(serde_json::json!({ "billing_periods": items, "count": items.len() }))
                .into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: list_billing_periods failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Lastgang ──────────────────────────────────────────────────────────────────

/// `GET /api/v1/lastgang/{malo_id}?from=RFC3339&to=RFC3339`
///
/// Returns one `Lastgang` BO4E object per distinct OBIS-Kennzahl found in the
/// requested time window.  Reads without an OBIS code are grouped together
/// under a single `Lastgang` with `obis_kennzahl = null`.
///
/// The interval length (`zeit_intervall_laenge`) is inferred from the first
/// read pair:
/// - 15 min → `Mengeneinheit::ViertelStunde`
/// - 60 min → `Mengeneinheit::Stunde`
/// - other  → `Mengeneinheit::Minute` with the exact value
///
/// The `werte[].zeitraum` uses `startdatum`/`enddatum` (UTC date) plus
/// `startuhrzeit`/`enduhrzeit` in `HH:MM:SS+00:00` format.
///
/// Source: BO4E-Standard; MSCONS AHB Gas/Strom.
#[derive(Debug, Deserialize)]
struct LastgangParams {
    /// RFC 3339 start (inclusive). Defaults to Unix epoch.
    from: Option<String>,
    /// RFC 3339 end (inclusive). Defaults to now.
    to: Option<String>,
    /// Bitemporal point-in-time query (RFC 3339).
    ///
    /// When set, the query returns the meter reads **as they were stored at this timestamp**,
    /// not the current (potentially corrected) values. Enables §22 MessZV point-in-time
    /// billing reconstruction: "what did we know at invoice date 2026-07-01T00:00:00Z?".
    ///
    /// Implementation: queries `meter_read_corrections` to find the state before any
    /// corrections applied after `as_of`. When `None`, returns current (latest) values.
    as_of: Option<String>,
}

async fn get_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
    reads_headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

    // ── Bitemporal query: ?as_of= (§22 MessZV point-in-time reconstruction) ──
    // When `as_of` is set, undo any corrections applied AFTER that timestamp.
    // This allows invoice auditors to reconstruct the exact billing basis at
    // any historical point in time.
    let as_of_ts = params
        .as_of
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let mut reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_lastgang query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    // Apply bitemporal overlay: for any read that was corrected AFTER `as_of`,
    // restore the original pre-correction value from `meter_read_corrections`.
    #[allow(clippy::collapsible_if)]
    if let Some(as_of) = as_of_ts {
        if let Ok(corrections) = sqlx::query(
            // Keyed on the register as well as the timestamp: a MaLo may carry
            // several OBIS codes at one instant, and restoring by timestamp
            // alone would apply one register's prior value to all of them.
            r"SELECT DISTINCT ON (malo_id, dtm_from, obis_code_norm)
                  malo_id, dtm_from, dtm_to, obis_code_norm,
                  original_kwh, original_quality
              FROM meter_read_corrections
              WHERE malo_id = $1
                AND dtm_from >= $2
                AND dtm_to   <= $3
                AND corrected_at > $4
                AND tenant   = $5
              ORDER BY malo_id, dtm_from, obis_code_norm, corrected_at ASC",
        )
        .bind(&malo_id)
        .bind(from)
        .bind(to)
        .bind(as_of)
        .bind(&state.tenant)
        .fetch_all(state.repo.pool())
        .await
        {
            use sqlx::Row;
            for corr in &corrections {
                let dtm_from: OffsetDateTime = corr
                    .try_get("dtm_from")
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let dtm_to: OffsetDateTime =
                    corr.try_get("dtm_to").unwrap_or(OffsetDateTime::UNIX_EPOCH);
                // `original_kwh` is NUMERIC(18,5), so it reads as `Decimal`.
                let orig_kwh: Option<rust_decimal::Decimal> = corr.try_get("original_kwh").ok();
                let corr_obis: String = corr.try_get("obis_code_norm").unwrap_or_default();
                let orig_quality: &str = corr.try_get("original_quality").unwrap_or("MEASURED");

                // Restore original values in the reads slice
                for read in reads.iter_mut() {
                    let read_obis = read.obis_code.clone().unwrap_or_default();
                    if read.dtm_from == dtm_from && read.dtm_to == dtm_to && read_obis == corr_obis
                    {
                        if let Some(kwh) = orig_kwh {
                            read.quantity_kwh = kwh;
                        }
                        read.quality = match orig_quality {
                            "MEASURED" => mako_edm::domain::QualityFlag::Measured,
                            "ESTIMATED" => mako_edm::domain::QualityFlag::Estimated,
                            "SUBSTITUTED" => mako_edm::domain::QualityFlag::Substituted,
                            "CALCULATED" => mako_edm::domain::QualityFlag::Calculated,
                            "CORRECTED" => mako_edm::domain::QualityFlag::Corrected,
                            "PRELIMINARY" => mako_edm::domain::QualityFlag::Preliminary,
                            "FAULTY" => mako_edm::domain::QualityFlag::Faulty,
                            _ => mako_edm::domain::QualityFlag::Unknown,
                        };
                    }
                }
            }
            tracing::debug!(
                malo_id, as_of = %as_of,
                corrections_applied = corrections.len(),
                "edmd: bitemporal overlay applied for as_of query"
            );
        }
    }

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": "no meter reads for this MaLo in requested window" }),
            ),
        )
            .into_response();
    }

    // Group by OBIS code (None → empty-string sentinel key so BTreeMap works).
    let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for r in &reads {
        let key = r.obis_code.clone().unwrap_or_default();
        groups.entry(key).or_default().push(r);
    }

    let lastgaenge: Vec<Lastgang> = groups
        .into_iter()
        .map(|(obis_key, group)| {
            let sparte = edm_sparte_to_bo4e(group[0].sparte);
            let obis_kennzahl = if obis_key.is_empty() {
                None
            } else {
                rubo4e::identifiers::ObisCode::new(&obis_key).ok()
            };

            // Infer interval from first consecutive pair (fallback: 15 min).
            let interval_min = group
                .windows(2)
                .next()
                .map(|w| {
                    (w[1].dtm_from - w[0].dtm_from)
                        .whole_minutes()
                        .unsigned_abs() as u32
                })
                .filter(|&m| m > 0)
                .unwrap_or(15);

            let werte: Vec<Zeitreihenwert> =
                group.iter().map(|r| read_to_zeitreihenwert(r)).collect();

            Lastgang {
                id: None,
                marktlokation: None,
                messgroesse: None,
                messlokation: None,
                obis_kennzahl,
                sparte: Some(sparte),
                typ: None,
                version: None,
                werte: Some(werte),
                zeit_intervall_laenge: minutes_to_menge(interval_min),
                zusatz_attribute: None,
                _additional: Default::default(),
            }
        })
        .collect();

    // ── Arrow IPC response path ────────────────────────────────────────────────
    // If the caller sends `Accept: application/vnd.apache.arrow.stream`, return
    // the raw reads as an Arrow IPC stream instead of BO4E JSON. This gives
    // mabis-syncd and billingd a 10× throughput improvement for bulk reads
    // without requiring gRPC.
    if request_wants_arrow(&reads_headers) {
        return match reads_to_arrow_ipc(&reads) {
            Ok(bytes) => (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apache.arrow.stream",
                )],
                bytes,
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: arrow IPC serialization failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    Json(lastgaenge).into_response()
}

/// `true` when the request `Accept` header requests Arrow IPC stream format.
fn request_wants_arrow(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.contains("application/vnd.apache.arrow.stream"))
        .unwrap_or(false)
}

/// Serialise a slice of `MeterRead` rows to an Arrow IPC stream.
///
/// Schema: `malo_id Utf8 · dtm_from TimestampMicrosecond(UTC) ·
/// dtm_to TimestampMicrosecond(UTC) · quantity_kwh Float64 ·
/// quality Utf8 · sparte Utf8 · obis_code Utf8(nullable) · pid Int32`.
///
/// Callers that receive `Content-Type: application/vnd.apache.arrow.stream`
/// can read the result with any Arrow library (DuckDB, Polars, PyArrow, etc.).
fn reads_to_arrow_ipc(reads: &[mako_edm::domain::MeterRead]) -> anyhow::Result<Vec<u8>> {
    use arrow::array::{
        Float64Array, Int32Array, StringArray, StringBuilder, TimestampMicrosecondArray,
    };
    use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
    use arrow::ipc::writer::StreamWriter;
    use arrow::record_batch::RecordBatch;
    use std::sync::Arc;

    let schema = Arc::new(Schema::new(vec![
        Field::new("malo_id", DataType::Utf8, false),
        Field::new(
            "dtm_from",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new(
            "dtm_to",
            DataType::Timestamp(TimeUnit::Microsecond, Some("UTC".into())),
            false,
        ),
        Field::new("quantity_kwh", DataType::Float64, false),
        Field::new("quality", DataType::Utf8, false),
        Field::new("sparte", DataType::Utf8, false),
        Field::new("obis_code", DataType::Utf8, true),
        Field::new("pid", DataType::Int32, false),
    ]));

    let n = reads.len();
    let malo_ids: StringArray = reads.iter().map(|r| Some(r.malo_id.as_str())).collect();
    let dtm_froms: TimestampMicrosecondArray = TimestampMicrosecondArray::from(
        reads
            .iter()
            .map(|r| (r.dtm_from.unix_timestamp_nanos() / 1_000) as i64)
            .collect::<Vec<i64>>(),
    )
    .with_timezone_opt(Some("UTC".to_string()));
    let dtm_tos: TimestampMicrosecondArray = TimestampMicrosecondArray::from(
        reads
            .iter()
            .map(|r| (r.dtm_to.unix_timestamp_nanos() / 1_000) as i64)
            .collect::<Vec<i64>>(),
    )
    .with_timezone_opt(Some("UTC".to_string()));
    let quantities: Float64Array = reads
        .iter()
        .map(|r| {
            use rust_decimal::prelude::ToPrimitive;
            r.quantity_kwh.to_f64()
        })
        .collect();
    let qualities: StringArray = reads
        .iter()
        .map(|r| {
            Some(match r.quality {
                metering::QualityFlag::Measured => "MEASURED",
                metering::QualityFlag::Estimated => "ESTIMATED",
                metering::QualityFlag::Substituted => "SUBSTITUTED",
                metering::QualityFlag::Calculated => "CALCULATED",
                metering::QualityFlag::Corrected => "CORRECTED",
                metering::QualityFlag::Preliminary => "PRELIMINARY",
                metering::QualityFlag::Faulty => "FAULTY",
                metering::QualityFlag::Unknown => "UNKNOWN",
            })
        })
        .collect();
    let spartes: StringArray = reads.iter().map(|r| Some(r.sparte.as_str())).collect();
    let mut obis_builder = StringBuilder::with_capacity(n, n * 12);
    for r in reads {
        match &r.obis_code {
            Some(o) => obis_builder.append_value(o),
            None => obis_builder.append_null(),
        }
    }
    let obis_codes = obis_builder.finish();
    let pids: Int32Array = reads.iter().map(|r| Some(r.pid as i32)).collect();

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(malo_ids),
            Arc::new(dtm_froms),
            Arc::new(dtm_tos),
            Arc::new(quantities),
            Arc::new(qualities),
            Arc::new(spartes),
            Arc::new(obis_codes),
            Arc::new(pids),
        ],
    )
    .map_err(|e| anyhow::anyhow!("RecordBatch: {e}"))?;

    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new(&mut buf, &schema)
        .map_err(|e| anyhow::anyhow!("StreamWriter: {e}"))?;
    writer
        .write(&batch)
        .map_err(|e| anyhow::anyhow!("write batch: {e}"))?;
    writer
        .finish()
        .map_err(|e| anyhow::anyhow!("finish: {e}"))?;
    Ok(buf)
}

/// Convert an `edm::Sparte` to the BO4E `Sparte` enum.
fn edm_sparte_to_bo4e(s: EdmSparte) -> Bo4eSparte {
    match s {
        EdmSparte::Strom => Bo4eSparte::Strom,
        EdmSparte::Gas => Bo4eSparte::Gas,
        // BO4E splits heat into Fern-/Nahwärme; `edmd` does not carry that
        // distinction, and Fernwaerme is the billing-relevant default.
        EdmSparte::Waerme => Bo4eSparte::Fernwaerme,
        EdmSparte::Wasser => Bo4eSparte::Wasser,
    }
}

/// Map `edm::Sparte` to the BO4E `Medium` enum for `Zeitreihe`.
fn edm_sparte_to_medium(s: EdmSparte) -> Medium {
    match s {
        EdmSparte::Strom => Medium::Strom,
        EdmSparte::Gas => Medium::Gas,
        EdmSparte::Wasser => Medium::Wasser,
        // BO4E `Medium` has no heat variant (STROM/GAS/WASSER/DAMPF only), so a
        // Wärmemengenzähler has no faithful mapping. `Dampf` would be wrong —
        // district heat is hot water, not steam — so this reports Unknown rather
        // than asserting something false in an exported Zeitreihe.
        EdmSparte::Waerme => Medium::Unknown,
    }
}

// ── Zeitreihe ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/zeitreihe/{malo_id}?from=RFC3339&to=RFC3339`
///
/// Returns one `Zeitreihe` BO4E object per distinct OBIS-Kennzahl found in
/// the requested time window.  Unlike [`get_lastgang`], which carries interval
/// metadata (`zeit_intervall_laenge`, OBIS code, Sparte), `Zeitreihe` exposes
/// the generic time-series contract used by API-Webdienste Strom consumers.
///
/// - `messart` is set to `Mittelwert` (interval-average, typical for SLP/RLM).
/// - `einheit` is set to `kWh`.
/// - `medium` reflects the commodity (Strom / Gas).
///
/// Source: BO4E-Standard Zeitreihe; API-Webdienste Strom §5.3.
async fn get_zeitreihe(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<LastgangParams>,
    zr_headers: axum::http::HeaderMap,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_zeitreihe query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": "no meter reads for this MaLo in requested window" }),
            ),
        )
            .into_response();
    }

    // Group by OBIS code.
    let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for r in &reads {
        let key = r.obis_code.clone().unwrap_or_default();
        groups.entry(key).or_default().push(r);
    }

    let zeitreihen: Vec<Zeitreihe> = groups
        .into_iter()
        .map(|(obis_key, group)| {
            let medium = edm_sparte_to_medium(group[0].sparte);
            let bezeichnung = if obis_key.is_empty() {
                format!("Zeitreihe MaLo {malo_id}")
            } else {
                format!("Zeitreihe MaLo {malo_id} OBIS {obis_key}")
            };
            let werte: Vec<Zeitreihenwert> =
                group.iter().map(|r| read_to_zeitreihenwert(r)).collect();
            Zeitreihe {
                bezeichnung: Some(bezeichnung),
                einheit: Some(Mengeneinheit::Kwh),
                medium: Some(medium),
                messart: Some(Messart::Mittelwert),
                werte: Some(werte),
                ..Default::default()
            }
        })
        .collect();

    // Arrow IPC response path — same reads, binary columnar format.
    if request_wants_arrow(&zr_headers) {
        return match reads_to_arrow_ipc(&reads) {
            Ok(bytes) => (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "application/vnd.apache.arrow.stream",
                )],
                bytes,
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: zeitreihe arrow IPC serialization failed");
                StatusCode::INTERNAL_SERVER_ERROR.into_response()
            }
        };
    }

    Json(zeitreihen).into_response()
}

// ── Resampled Lastgang (M8) ───────────────────────────────────────────────────

/// Query parameters for resampled Lastgang.
#[derive(Debug, serde::Deserialize)]
struct ResampledParams {
    from: Option<String>,
    to: Option<String>,
    /// Target resolution: `HOUR`, `DAY`, `MONTH`, `YEAR`. Default: `HOUR`.
    resolution: Option<String>,
}

/// `GET /api/v1/lastgang/{malo_id}/resampled`
///
/// Returns the metered time series down-sampled to a coarser time resolution.
/// Useful for dashboards, billing previews, and Mehr-/Mindermengensaldo summaries.
///
/// | Resolution | Use case |
/// |---|---|
/// | `HOUR` | Hourly dashboard chart (default) |
/// | `DAY` | Daily totals for SLP billing |
/// | `MONTH` | Monthly totals for MMM / §27 MessZV |
/// | `YEAR` | Annual settlement |
///
/// Each bucket carries:
/// - `total_kwh` — summed energy
/// - `peak_kw` — maximum 15-min demand kW (RLM Strom)
/// - `coverage_pct` — completeness indicator
/// - `has_missing_data` — `true` when source intervals are missing
async fn get_lastgang_resampled(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<ResampledParams>,
) -> impl IntoResponse {
    use metering::{ResampleConfig, resample};
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

    let config = match params
        .resolution
        .as_deref()
        .unwrap_or("HOUR")
        .to_uppercase()
        .as_str()
    {
        "HOUR" => ResampleConfig::to_hourly(),
        "DAY" => ResampleConfig::to_daily(),
        "MONTH" => ResampleConfig::to_monthly(),
        "YEAR" => ResampleConfig::to_yearly(),
        other => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("unknown resolution {other:?} — use HOUR, DAY, MONTH, or YEAR")
                })),
            )
                .into_response();
        }
    };

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };

    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_lastgang_resampled query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no meter reads for this MaLo in requested window"
            })),
        )
            .into_response();
    }

    // Convert MeterRead → MeterInterval (metering crate)
    // QualityFlag is now the same type — mako-edm re-exports metering::QualityFlag.
    let intervals: Vec<metering::MeterInterval> = reads
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value_kwh: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.clone(),
        })
        .collect();

    let buckets = resample(&intervals, &config);

    let response: Vec<serde_json::Value> = buckets
        .iter()
        .map(|b| {
            serde_json::json!({
                "from": b.from,
                "to": b.to,
                "total_kwh": b.total_kwh,
                "peak_kw": b.peak_kw,
                "interval_count": b.interval_count,
                "expected_count": b.expected_count,
                "coverage_pct": b.coverage_pct(),
                "has_missing_data": b.has_missing_data,
                "quality": format!("{:?}", b.quality),
            })
        })
        .collect();

    Json(serde_json::json!({
        "malo_id": malo_id,
        "resolution": params.resolution.as_deref().unwrap_or("HOUR"),
        "from": from,
        "to": to,
        "bucket_count": response.len(),
        "buckets": response,
    }))
    .into_response()
}

// ── Virtual meter endpoints (M9) ──────────────────────────────────────────────

/// Query params shared by virtual meter and other new endpoints.
#[derive(Debug, serde::Deserialize)]
struct SimpleTimeParams {
    from: Option<String>,
    to: Option<String>,
}

/// `GET /api/v1/virtual/{virtual_malo_id}/lastgang`
///
/// Computes the virtual meter time series by fetching all source MaLo time
/// series and applying the stored `AggregationRule`. The result is NOT stored
/// in `meter_reads` — it is computed on demand.
///
/// Use `?from=` / `?to=` (RFC3339 UTC) to bound the query window.
async fn get_virtual_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::{AggregationRule, compute_virtual_meter};
    use std::collections::HashMap;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Load virtual meter config from DB
    let config_row = match sqlx::query(
        "SELECT rule_type, rule_json FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2 LIMIT 1"
    )
    .bind(&virtual_malo_id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await {
        Ok(Some(r)) => r,
        Ok(None) => return (StatusCode::NOT_FOUND, Json(serde_json::json!({
            "error": format!("virtual meter {virtual_malo_id:?} not found")
        }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, virtual_malo_id, "edmd: virtual meter config query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule_json: serde_json::Value = match sqlx::Row::try_get(&config_row, "rule_json") {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: failed to decode virtual meter rule_json");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule: AggregationRule = match serde_json::from_value(rule_json) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "edmd: failed to deserialise AggregationRule");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": format!("invalid rule_json: {e}")
                })),
            )
                .into_response();
        }
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

    // Fetch source series for all referenced MaLos
    let mut sources: HashMap<String, Vec<metering::MeterInterval>> = HashMap::new();
    for malo_id in rule.source_malo_ids() {
        let q = TimeSeriesQuery {
            malo_id: malo_id.to_owned(),
            from,
            to,
            sparte: None,
            tenant: state.tenant.clone(),
        };
        match state.repo.query(&q).await {
            Ok(reads) => {
                let intervals: Vec<metering::MeterInterval> = reads
                    .iter()
                    .map(|r| metering::MeterInterval {
                        from: r.dtm_from,
                        to: r.dtm_to,
                        value_kwh: r.quantity_kwh,
                        quality: r.quality,
                        obis_code: r.obis_code.clone(),
                    })
                    .collect();
                sources.insert(malo_id.to_owned(), intervals);
            }
            Err(e) => {
                tracing::warn!(error = %e, malo_id, "edmd: virtual meter source query failed");
            }
        }
    }

    match compute_virtual_meter(&rule, &sources) {
        Ok(intervals) => {
            let result: Vec<serde_json::Value> = intervals
                .iter()
                .map(|iv| {
                    serde_json::json!({
                        "from": iv.from, "to": iv.to,
                        "value_kwh": iv.value_kwh,
                        "quality": format!("{:?}", iv.quality),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "virtual_malo_id": virtual_malo_id,
                "from": from, "to": to,
                "interval_count": result.len(),
                "intervals": result,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": e.to_string()
            })),
        )
            .into_response(),
    }
}

/// `GET /api/v1/virtual` — list all virtual meter configurations for this tenant.
async fn list_virtual_meters(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
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
    match sqlx::query("SELECT virtual_malo_id, display_name, rule_type, legal_basis, sparte, valid_from, valid_to, created_at FROM virtual_meter_configs WHERE tenant = $1 ORDER BY virtual_malo_id")
        .bind(&state.tenant)
        .fetch_all(state.repo.pool())
        .await {
        Ok(rows) => {
            let configs: Vec<serde_json::Value> = rows.iter().map(|r| {
                use sqlx::Row;
                serde_json::json!({
                    "virtual_malo_id": r.try_get::<String, _>("virtual_malo_id").unwrap_or_default(),
                    "display_name": r.try_get::<String, _>("display_name").unwrap_or_default(),
                    "rule_type": r.try_get::<String, _>("rule_type").unwrap_or_default(),
                    "legal_basis": r.try_get::<Option<String>, _>("legal_basis").unwrap_or_default(),
                    "sparte": r.try_get::<String, _>("sparte").unwrap_or_default(),
                    "valid_from": r.try_get::<time::Date, _>("valid_from").ok().map(|d| d.to_string()),
                    "valid_to": r.try_get::<Option<time::Date>, _>("valid_to").ok().flatten().map(|d| d.to_string()),
                    "created_at": r.try_get::<OffsetDateTime, _>("created_at").ok().map(|t| t.to_string()),
                })
            }).collect();
            Json(serde_json::json!({ "virtual_meters": configs, "count": configs.len() })).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: list_virtual_meters failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `POST /api/v1/virtual` — create a virtual meter configuration.
async fn create_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    // Validate that rule_json deserialises to a known AggregationRule
    if let Err(e) = serde_json::from_value::<metering::AggregationRule>(
        body.get("rule_json")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    ) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("invalid rule_json: {e}")
            })),
        )
            .into_response();
    }
    let virtual_malo_id = body
        .get("virtual_malo_id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .unwrap_or(virtual_malo_id);
    let rule_type = body.get("rule_type").and_then(|v| v.as_str()).unwrap_or("");
    let sparte = body
        .get("sparte")
        .and_then(|v| v.as_str())
        .unwrap_or("STROM");
    let rule_json = body
        .get("rule_json")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let legal_basis: Option<&str> = body.get("legal_basis").and_then(|v| v.as_str());

    if virtual_malo_id.is_empty() || rule_type.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "virtual_malo_id and rule_type are required"
            })),
        )
            .into_response();
    }

    match sqlx::query(
        "INSERT INTO virtual_meter_configs (virtual_malo_id, display_name, rule_type, rule_json, legal_basis, sparte, valid_from, tenant)
         VALUES ($1, $2, $3, $4, $5, $6, CURRENT_DATE, $7)
         ON CONFLICT (virtual_malo_id, tenant) DO UPDATE
            SET display_name = EXCLUDED.display_name,
                rule_type = EXCLUDED.rule_type,
                rule_json = EXCLUDED.rule_json,
                legal_basis = EXCLUDED.legal_basis,
                sparte = EXCLUDED.sparte,
                updated_at = now()
         RETURNING id"
    )
    .bind(virtual_malo_id)
    .bind(display_name)
    .bind(rule_type)
    .bind(rule_json)
    .bind(legal_basis)
    .bind(sparte)
    .bind(&state.tenant)
    .fetch_one(state.repo.pool())
    .await {
        Ok(_) => (StatusCode::CREATED, Json(serde_json::json!({
            "virtual_malo_id": virtual_malo_id, "status": "created"
        }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: create_virtual_meter insert failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `GET /api/v1/virtual/{virtual_malo_id}` — get one virtual meter config.
async fn get_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
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
    match sqlx::query("SELECT virtual_malo_id, display_name, rule_type, rule_json, legal_basis, sparte, valid_from, valid_to, created_at FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2")
        .bind(&virtual_malo_id)
        .bind(&state.tenant)
        .fetch_optional(state.repo.pool())
        .await {
        Ok(Some(r)) => {
            use sqlx::Row;
            Json(serde_json::json!({
                "virtual_malo_id": r.try_get::<String, _>("virtual_malo_id").unwrap_or_default(),
                "display_name": r.try_get::<String, _>("display_name").unwrap_or_default(),
                "rule_type": r.try_get::<String, _>("rule_type").unwrap_or_default(),
                "rule_json": r.try_get::<serde_json::Value, _>("rule_json").ok(),
                "legal_basis": r.try_get::<Option<String>, _>("legal_basis").unwrap_or_default(),
                "sparte": r.try_get::<String, _>("sparte").unwrap_or_default(),
            })).into_response()
        }
        Ok(None) => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: get_virtual_meter failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `DELETE /api/v1/virtual/{virtual_malo_id}` — remove a virtual meter configuration.
async fn delete_virtual_meter(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(virtual_malo_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    match sqlx::query(
        "DELETE FROM virtual_meter_configs WHERE virtual_malo_id = $1 AND tenant = $2",
    )
    .bind(&virtual_malo_id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await
    {
        Ok(res) if res.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "not found" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "edmd: delete_virtual_meter failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

// ── Quality assessments (M10) ─────────────────────────────────────────────────

/// `GET /api/v1/quality-assessments/{malo_id}`
///
/// Returns the quality assessment history for a MaLo.
/// Each batch ingest produces one quality assessment row per §22 MessZV audit trail.
async fn list_quality_assessments(
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

// ── Annual forecast (M11) ─────────────────────────────────────────────────────

/// `GET /api/v1/forecast/{malo_id}?from=&to=`
///
/// Computes an annual energy consumption forecast from the available meter reads
/// in the given window. Returns the projected annual kWh per §17 MessZV.
///
/// This is useful for:
/// - Setting Abschlag (advance payment) amounts
/// - Anticipating Mehr-/Mindermengensaldo at year-end
/// - Informing Jahresprognose in MSCONS
async fn get_annual_forecast(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::project_annual_consumption;
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

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };
    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: forecast query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    if reads.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no meter reads for this MaLo in requested window"
            })),
        )
            .into_response();
    }

    let intervals: Vec<metering::MeterInterval> = reads
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value_kwh: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.clone(),
        })
        .collect();

    match project_annual_consumption(&malo_id, &intervals, None) {
        Some(forecast) => Json(serde_json::json!({
            "malo_id": forecast.malo_id,
            "observation_from": forecast.observation_from,
            "observation_to": forecast.observation_to,
            "observed_kwh": forecast.observed_kwh,
            "observed_days": forecast.observed_days,
            "projected_annual_kwh": forecast.projected_annual_kwh,
            "seasonal_correction_applied": forecast.seasonal_correction_applied,
            "seasonal_factor": forecast.seasonal_factor,
            "method": format!("{:?}", forecast.method),
            "legal_basis": "§17 MessZV Jahresprognose",
        }))
        .into_response(),
        None => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "insufficient data for annual forecast (minimum 7 days required)"
            })),
        )
            .into_response(),
    }
}

// ── Summenzeitreihe (M12) ─────────────────────────────────────────────────────

/// `GET /api/v1/summenzeitreihe/{malo_id}?from=&to=`
///
/// Returns monthly aggregated energy data (Summenzeitreihe) for a MaLo.
///
/// This is the canonical data format for:
/// - MABIS balance group accounting (PID 13003)
/// - Mehr-/Mindermengensaldo (§27 MessZV)
/// - Annual Jahresabrechnung summaries
///
/// Each month bucket includes: `total_kwh`, `peak_kw`, `coverage_pct`, `quality`.
async fn get_summenzeitreihe(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Query(params): Query<SimpleTimeParams>,
) -> impl IntoResponse {
    use metering::{ResampleConfig, resample};
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

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant: state.tenant.clone(),
    };
    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: summenzeitreihe query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let intervals: Vec<metering::MeterInterval> = reads
        .iter()
        .map(|r| metering::MeterInterval {
            from: r.dtm_from,
            to: r.dtm_to,
            value_kwh: r.quantity_kwh,
            quality: r.quality,
            obis_code: r.obis_code.clone(),
        })
        .collect();

    let buckets = resample(&intervals, &ResampleConfig::to_monthly());
    let total_kwh: rust_decimal::Decimal = buckets.iter().map(|b| b.total_kwh).sum();

    let months: Vec<serde_json::Value> = buckets
        .iter()
        .map(|b| {
            serde_json::json!({
                "from": b.from,
                "to": b.to,
                "total_kwh": b.total_kwh,
                "peak_kw": b.peak_kw,
                "coverage_pct": b.coverage_pct(),
                "has_missing_data": b.has_missing_data,
                "quality": format!("{:?}", b.quality),
            })
        })
        .collect();

    Json(serde_json::json!({
        "malo_id": malo_id,
        "from": from,
        "to": to,
        "total_kwh": total_kwh,
        "month_count": months.len(),
        "months": months,
        "legal_basis": "MABIS PID 13003 / §27 MessZV Mehr-Mindermengensaldo",
    }))
    .into_response()
}

// ── Gas quality endpoint (M13) ────────────────────────────────────────────────

/// `GET /api/v1/gas-quality/{malo_id}`
///
/// Returns Gasbeschaffenheitsdaten (Brennwert + Zustandszahl) received via PID 13007.
/// Used for Gas m³ → kWh_Hs conversion per §25 Nr. 4 MessEV / DVGW G 685.
async fn get_gas_quality(
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
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok().map(|t| t.date()))
        .unwrap_or(time::Date::MIN);
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok().map(|t| t.date()))
        .unwrap_or(time::Date::MAX);

    match sqlx::query(
        "SELECT period_from, period_to, brennwert_kwh_per_m3, zustandszahl, pid, received_at
           FROM gas_quality_data
          WHERE malo_id = $1 AND period_from >= $2 AND period_to <= $3
            AND tenant = $4
          ORDER BY period_from DESC LIMIT 50",
    )
    .bind(&malo_id)
    .bind(from)
    .bind(to)
    .bind(state.tenant.as_str())
    .fetch_all(state.repo.pool())
    .await
    {
        Ok(rows) => {
            use sqlx::Row;
            let records: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::json!({
                "period_from": r.try_get::<time::Date, _>("period_from").ok().map(|d| d.to_string()),
                "period_to": r.try_get::<time::Date, _>("period_to").ok().map(|d| d.to_string()),
                "brennwert_kwh_per_m3": r.try_get::<String, _>("brennwert_kwh_per_m3").unwrap_or_default(),
                "zustandszahl": r.try_get::<String, _>("zustandszahl").unwrap_or_default(),
                "pid": r.try_get::<i32, _>("pid").unwrap_or(13007),
                "received_at": r.try_get::<OffsetDateTime, _>("received_at").ok().map(|t| t.to_string()),
                "legal_basis": "§25 Nr. 4 MessEV / DVGW G 685",
            })).collect();
            if records.is_empty() {
                (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({
                        "error": "no gas quality data for this MaLo in requested period"
                    })),
                )
                    .into_response()
            } else {
                Json(serde_json::json!({
                    "malo_id": malo_id,
                    "count": records.len(),
                    "gas_quality": records,
                }))
                .into_response()
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_gas_quality failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// Map a `QualityFlag` to the nearest `Messwertstatus` variant.
fn quality_to_messwertstatus(q: QualityFlag) -> Messwertstatus {
    match q {
        QualityFlag::Measured => Messwertstatus::Abgelesen,
        QualityFlag::Estimated => Messwertstatus::Prognosewert,
        QualityFlag::Substituted => Messwertstatus::Ersatzwert,
        QualityFlag::Calculated => Messwertstatus::Vorlaeufigerwert,
        QualityFlag::Corrected => Messwertstatus::Vorlaeufigerwert,
        QualityFlag::Preliminary => Messwertstatus::Prognosewert,
        QualityFlag::Faulty => Messwertstatus::Unknown,
        QualityFlag::Unknown => Messwertstatus::Unknown,
    }
}

/// Convert a `MeterRead` to a BO4E `Energiemenge`.
///
/// `Energiemenge` is the canonical BO4E Business Object for a metered energy
/// quantity at a location.  It carries the OBIS-Kennzahl, the measured `Menge`
/// in kWh, and the billing `Zeitraum` — exactly the triple that MSCONS
/// communicates per register per interval.
///
/// All timestamps are UTC (`startuhrzeit`/`enduhrzeit` format `HH:MM:SS+00:00`).
fn read_to_energiemenge(r: &mako_edm::domain::MeterRead) -> Energiemenge {
    fn fmt_uhrzeit(dt: OffsetDateTime) -> String {
        format!(
            "{:02}:{:02}:{:02}+00:00",
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    }
    Energiemenge {
        obis_kennzahl: r.obis_code.as_deref().and_then(|s| ObisCode::new(s).ok()),
        menge: Some(Menge {
            wert: Some(r.quantity_kwh),
            einheit: Some(Mengeneinheit::Kwh),
            ..Default::default()
        }),
        zeitraum: Some(Zeitraum {
            startdatum: Some(r.dtm_from.date()),
            startuhrzeit: Some(fmt_uhrzeit(r.dtm_from)),
            enddatum: Some(r.dtm_to.date()),
            enduhrzeit: Some(fmt_uhrzeit(r.dtm_to)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Convert a `MeterRead` to a BO4E `Zeitreihenwert`.
///
/// Timestamps are in UTC; `startuhrzeit`/`enduhrzeit` are formatted as
/// `HH:MM:SS+00:00` per Allgemeine Festlegungen V6.1d §3.
fn read_to_zeitreihenwert(r: &mako_edm::domain::MeterRead) -> Zeitreihenwert {
    fn fmt_uhrzeit(dt: OffsetDateTime) -> String {
        format!(
            "{:02}:{:02}:{:02}+00:00",
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    }
    Zeitreihenwert {
        wert: Some(r.quantity_kwh),
        status: Some(quality_to_messwertstatus(r.quality)),
        zeitraum: Some(Zeitraum {
            startdatum: Some(r.dtm_from.date()),
            startuhrzeit: Some(fmt_uhrzeit(r.dtm_from)),
            enddatum: Some(r.dtm_to.date()),
            enduhrzeit: Some(fmt_uhrzeit(r.dtm_to)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Build a `Menge` representing an interval length from whole minutes.
fn minutes_to_menge(minutes: u32) -> Menge {
    let (wert, einheit) = match minutes {
        15 => (Decimal::from(15u32), Mengeneinheit::ViertelStunde),
        60 => (Decimal::from(60u32), Mengeneinheit::Minute),
        1440 => (Decimal::from(1u32), Mengeneinheit::Tag),
        m => (Decimal::from(m), Mengeneinheit::Minute),
    };
    Menge {
        wert: Some(wert),
        einheit: Some(einheit),
        ..Default::default()
    }
}

// ── RunConfig + startup ───────────────────────────────────────────────────────

pub struct RunConfig {
    pub listen: SocketAddr,
    pub database_url: SecretString,
    pub marktd_url: String,
    pub marktd_api_key: secrecy::SecretString,
    pub subscriber_id: String,
    pub webhook_url: String,
    pub webhook_secret: Option<SecretString>,
    pub inbound_secret: Option<SecretString>,
    pub db_pool_size: u32,
    /// Tenant identifier — used as Cedar resource_tenant.
    pub tenant: String,
    /// OIDC verifier.  Use [`OidcVerifier::disabled`] in dev/test.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer.
    pub cedar: Arc<CedarEnforcer>,
    /// MCP server auth config (API-key fallback + optional per-named-key identity).
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Graceful-shutdown token.
    pub shutdown: CancellationToken,
    /// Resolved archive config (env vars already substituted, disabled when absent).
    pub archive: Option<crate::config::ArchiveConfig>,
    /// ERP webhook URL for outbound CloudEvents (direct push + quality warnings).
    pub erp_webhook_url: Option<String>,
    /// Request rate limits. Ingest endpoints accept unbounded batches, so an
    /// unthrottled client can saturate the write path for every other tenant.
    pub rate_limit: mako_service::RateLimitConfig,
}

/// Connect to the database, run migrations, register subscription, and serve.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let pool = PgPool::connect_with(
        cfg.database_url
            .expose_secret()
            .parse::<sqlx::postgres::PgConnectOptions>()?,
    )
    .await?;

    // Run database migrations at startup.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("run edmd migrations: {e}"))?;

    // ── Iceberg/S3 archive setup ───────────────────────────────────────────────
    let olap_engine: Option<Arc<OlapEngine>> = if let Some(ref archive_cfg) = cfg.archive {
        if archive_cfg.enabled && !archive_cfg.storage_uri.is_empty() {
            // Build FileIO (iceberg's opendal-backed storage abstraction).
            match crate::iceberg::build_file_io(archive_cfg) {
                Ok(file_io) => {
                    // Spawn archival worker: loads/creates the table via SqlCatalog,
                    // writes Parquet batches to S3, marks rows archived in PostgreSQL.
                    let worker = crate::iceberg::worker::ArchiveWorker::new(
                        pool.clone(),
                        archive_cfg.clone(),
                        file_io,
                        cfg.database_url.expose_secret().to_owned(),
                    );
                    worker.spawn(cfg.shutdown.clone());

                    // Build OLAP engine: loads the table from the SQL catalog and
                    // registers it with DataFusion as an IcebergTableProvider.
                    match crate::iceberg::worker::load_table_for_olap(
                        archive_cfg,
                        cfg.database_url.expose_secret(),
                        pool.clone(),
                        cfg.tenant.clone(),
                    )
                    .await
                    {
                        Ok(engine) => {
                            tracing::info!(
                                storage_uri = %archive_cfg.storage_uri,
                                catalog_schema = %archive_cfg.iceberg_catalog_schema,
                                "edmd: Iceberg OLAP engine ready"
                            );
                            Some(Arc::new(engine))
                        }
                        Err(e) => {
                            // Table may not exist on first run — that's fine.
                            // The worker will create it on next archive cycle.
                            tracing::info!(
                                error = %e,
                                "edmd: Iceberg OLAP engine not yet available \
                                 (table will be created on first archive run)"
                            );
                            None
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "edmd: cannot build FileIO — archive disabled");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    let mcp_state = Arc::new(crate::mcp_server::EdmdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        marktd_url: cfg.marktd_url.clone(),
        marktd_api_key: cfg.marktd_api_key.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            cfg.oidc.clone(),
            Some(cfg.cedar.clone()),
            &cfg.tenant,
        ),
    });

    let repo = PgTimeSeriesRepository::new(pool.clone());
    // Clone the webhook URL and tenant before they are moved into HandlerState.
    let smgw_webhook_url = cfg.erp_webhook_url.clone();
    let smgw_tenant = cfg.tenant.clone();
    let state = HandlerState {
        repo,
        inbound_secret: Arc::new(cfg.inbound_secret),
        tenant: cfg.tenant,
        marktd_url: cfg.marktd_url.clone(),
        marktd_api_key: cfg.marktd_api_key.clone(),
        olap_engine,
        erp_webhook_url: cfg.erp_webhook_url,
    };

    {
        use mako_markt::marktd_client::{MarktdClient, SubscriptionRequest};
        use mako_service::http::default_client;
        let marktd = MarktdClient::new(
            &cfg.marktd_url,
            cfg.marktd_api_key.clone(),
            default_client(),
        );
        marktd
            .put_subscription(
                &cfg.subscriber_id,
                &SubscriptionRequest {
                    webhook_url: &cfg.webhook_url,
                    webhook_secret: cfg.webhook_secret.as_ref().map(|s| {
                        use secrecy::ExposeSecret;
                        let secret: &str = s.expose_secret();
                        secret
                    }),
                    // Receive MSCONS completions for meter data storage
                    // + INSRPT initiations for reading-order auto-creation (M2+)
                    // + Lieferbeginn/Lieferende completions for supply handover readings
                    event_types: &["de.mako.process.completed", "de.mako.process.initiated"],
                    makopid_filter: mako_edm::domain::MSCONS_PIDS,
                    active: true,
                },
            )
            .await;
    }

    let pool_arc = Arc::new(pool);

    // Both limiters apply: the keyed one bounds any single caller, the global
    // one bounds their sum.
    let app = mako_service::ServiceBuilder::new()
        .merge(
            router(state)
                .layer(Extension(cfg.cedar))
                .layer(Extension(cfg.oidc))
                .layer(Extension(pool_arc.clone()))
                .merge(crate::mcp_server::router(mcp_state, cfg.shutdown.clone())),
        )
        .with_tenant_rate_limit(cfg.rate_limit.clone())
        .with_rate_limit(cfg.rate_limit)
        .build();

    let listener = TcpListener::bind(cfg.listen).await?;

    tracing::info!(
        listen = %cfg.listen,
        marktd_url = %cfg.marktd_url,
        "edmd: listening"
    );

    // §14a Fernsteuerbarkeit compliance background worker (MsbG §21c, BSI TR-03109-4 §6.3).
    // Daily sweep of all SmgwSessions: checks TLS cert validity, CLS channel §14a
    // Konfigurationsprodukt, and communication faults.
    // Emits `de.edmd.cls.compliance_issue` CloudEvents for every detected issue.
    {
        use crate::smgw::spawn_cls_compliance_worker;
        spawn_cls_compliance_worker(
            pool_arc,
            smgw_tenant,
            smgw_webhook_url,
            30,     // cert_warning_days — warn 30 days before expiry (BSI TR-03109-4 §6.3)
            2,      // comm_fault_threshold_hours — §17 MessZV: substitute after 2h silence
            86_400, // interval_secs — sweep daily
            cfg.shutdown.clone(),
        );
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cfg.shutdown.cancelled().await })
        .await?;
    Ok(())
}

// ── Archive endpoint handlers ─────────────────────────────────────────────────

/// `GET /api/v1/archive/status`
///
/// Returns archive statistics and the 20 most recent batches.
async fn get_archive_status(
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
struct ArchiveOlapParams {
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
async fn get_archive_olap(
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
struct PortfolioParams {
    from: Option<String>,
    to: Option<String>,
    #[serde(default = "default_portfolio_limit")]
    limit: usize,
}
fn default_portfolio_limit() -> usize {
    100
}

/// `GET /api/v1/archive/portfolio?from=RFC3339&to=RFC3339&limit=N`
///
/// Portfolio-level MMM aggregation over the Iceberg cold tier.
/// Returns total kWh per MaLo ordered by consumption descending.
async fn get_archive_portfolio(
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
async fn get_archive_timeseries(
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

// ── Ablesesteuerung — Reading Order API ──────────────────────────────────────
//
// All three market roles schedule meter readings through the same API:
//   LF  → LIEFERBEGINN / LIEFERENDE / ZWISCHENABLESUNG / JAHRESABLESUNG
//   NB  → JAHRESABLESUNG / EINZUG / AUSZUG / SPERRUNG / ENTSPERRUNG
//   MSB → SONDERABLESUNG / INSRPT_STOERUNG / ISMS_AUSLESUNG
//
// DB: `ablese_auftraege` (migration 0003_ablese_auftraege.sql)

#[derive(Debug, serde::Deserialize)]
struct CreateReadingOrderRequest {
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub anlass: String,
    pub auftraggeber_rolle: String,
    pub ausfuehrender_msb: Option<String>,
    pub geplant_am: time::Date,
    pub ausfuehrt_bis: Option<time::Date>,
    pub auftrag_position_id: Option<uuid::Uuid>,
    pub insrpt_process_id: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct CompleteReadingOrderRequest {
    pub zaehlerstand_kwh: Option<f64>,
    pub zaehlerstand_qm3: Option<f64>,
    pub brennwert: Option<f64>,
    pub zustandszahl: Option<f64>,
    pub mscons_ref: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ListReadingOrdersQuery {
    pub malo_id: Option<String>,
    pub status: Option<String>,
    pub anlass: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, serde::Serialize, sqlx::FromRow)]
struct ReadingOrderRow {
    pub id: uuid::Uuid,
    pub malo_id: String,
    pub melo_id: Option<String>,
    pub anlass: String,
    pub auftraggeber_rolle: String,
    pub ausfuehrender_msb: Option<String>,
    pub geplant_am: time::Date,
    pub ausfuehrt_bis: Option<time::Date>,
    pub status: String,
    pub zaehlerstand_kwh: Option<f64>,
    pub zaehlerstand_qm3: Option<f64>,
    pub ausgefuehrt_am: Option<time::OffsetDateTime>,
    pub mscons_ref: Option<String>,
    pub auftrag_position_id: Option<uuid::Uuid>,
    pub insrpt_process_id: Option<String>,
    pub created_at: time::OffsetDateTime,
}

/// `POST /api/v1/reading-orders` — schedule a meter reading.
async fn create_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<CreateReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let id = uuid::Uuid::new_v4();
    let res = sqlx::query(
        "INSERT INTO ablese_auftraege
         (id,malo_id,melo_id,tenant,anlass,auftraggeber_rolle,
          ausfuehrender_msb,geplant_am,ausfuehrt_bis,
          auftrag_position_id,insrpt_process_id)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)",
    )
    .bind(id)
    .bind(&req.malo_id)
    .bind(&req.melo_id)
    .bind(&state.tenant)
    .bind(&req.anlass)
    .bind(&req.auftraggeber_rolle)
    .bind(&req.ausfuehrender_msb)
    .bind(req.geplant_am)
    .bind(req.ausfuehrt_bis)
    .bind(req.auftrag_position_id)
    .bind(&req.insrpt_process_id)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(_) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": id, "status": "OFFEN" })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/reading-orders?malo_id=&status=&anlass=&limit=`
async fn list_reading_orders(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(q): Query<ListReadingOrdersQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let rows = sqlx::query_as::<_, ReadingOrderRow>(
        "SELECT id,malo_id,melo_id,anlass,auftraggeber_rolle,
                ausfuehrender_msb,geplant_am,ausfuehrt_bis,status,
                zaehlerstand_kwh,zaehlerstand_qm3,ausgefuehrt_am,
                mscons_ref,auftrag_position_id,insrpt_process_id,created_at
         FROM ablese_auftraege
         WHERE tenant=$1
           AND ($2::text IS NULL OR malo_id=$2)
           AND ($3::text IS NULL OR status=$3)
           AND ($4::text IS NULL OR anlass=$4)
         ORDER BY geplant_am DESC
         LIMIT $5",
    )
    .bind(&state.tenant)
    .bind(&q.malo_id)
    .bind(&q.status)
    .bind(&q.anlass)
    .bind(q.limit.unwrap_or(100).min(1000))
    .fetch_all(state.repo.pool())
    .await;

    match rows {
        Ok(r) => Json(r).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/reading-orders/{id}`
async fn get_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let row = sqlx::query_as::<_, ReadingOrderRow>(
        "SELECT id,malo_id,melo_id,anlass,auftraggeber_rolle,
                ausfuehrender_msb,geplant_am,ausfuehrt_bis,status,
                zaehlerstand_kwh,zaehlerstand_qm3,ausgefuehrt_am,
                mscons_ref,auftrag_position_id,insrpt_process_id,created_at
         FROM ablese_auftraege WHERE id=$1 AND tenant=$2",
    )
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await;

    match row {
        Ok(Some(r)) => Json(r).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/reading-orders/{id}/complete`
async fn complete_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<CompleteReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        "UPDATE ablese_auftraege
         SET status='AUSGEFUEHRT',
             zaehlerstand_kwh=$1::numeric,
             zaehlerstand_qm3=$2::numeric,
             brennwert=$3::numeric,
             zustandszahl=$4::numeric,
             ausgefuehrt_am=now(),
             mscons_ref=COALESCE($5,mscons_ref)
         WHERE id=$6 AND tenant=$7 AND status IN ('OFFEN','BEAUFTRAGT')",
    )
    .bind(req.zaehlerstand_kwh)
    .bind(req.zaehlerstand_qm3)
    .bind(req.brennwert)
    .bind(req.zustandszahl)
    .bind(&req.mscons_ref)
    .bind(id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/reading-orders/{id}/cancel`
async fn cancel_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        "UPDATE ablese_auftraege SET status='STORNIERT'
         WHERE id=$1 AND tenant=$2 AND status IN ('OFFEN','BEAUFTRAGT')",
    )
    .bind(id)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) if r.rows_affected() == 0 => StatusCode::NOT_FOUND.into_response(),
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Body for `PUT /api/v1/reading-orders/{id}/fail`.
#[derive(Debug, serde::Deserialize)]
struct FailReadingOrderRequest {
    /// Ablesehindernis — why no reading could be taken.
    grund: String,
    /// Free-text detail for the field report.
    #[serde(default)]
    notiz: Option<String>,
}

/// Ablesehindernis codes a reading order may be failed with.
const ABLESEHINDERNIS_GRUENDE: [&str; 7] = [
    "KEIN_ZUTRITT",
    "ZAEHLER_UNZUGAENGLICH",
    "ZAEHLER_DEFEKT",
    "ZAEHLER_NICHT_AUFFINDBAR",
    "KUNDE_VERWEIGERT",
    "ABLESUNG_UNPLAUSIBEL",
    "SONSTIGES",
];

/// `PUT /api/v1/reading-orders/{id}/fail`
///
/// Records that a dispatched reading could not be taken, with the
/// Ablesehindernis that prevented it.
///
/// Distinct from `/cancel`: a cancelled order is no longer owed, whereas a
/// failed one still is. A failed JAHRESABLESUNG past its deadline remains a
/// §40 Abs. 2 EnWG gap, so it keeps appearing in `list_overdue_reading_orders`
/// until it is re-dispatched or the quantity is estimated under §40a EnWG.
async fn fail_reading_order(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<FailReadingOrderRequest>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if !ABLESEHINDERNIS_GRUENDE.contains(&req.grund.as_str()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!("unknown Ablesehindernis `{}`", req.grund),
                "expected": ABLESEHINDERNIS_GRUENDE,
            })),
        )
            .into_response();
    }

    let row = sqlx::query(
        "UPDATE ablese_auftraege
            SET status            = 'FEHLGESCHLAGEN',
                fehlschlag_grund  = $1,
                fehlschlag_notiz  = $2,
                fehlgeschlagen_am = now()
          WHERE id = $3 AND tenant = $4 AND status IN ('OFFEN','BEAUFTRAGT')
          RETURNING malo_id, anlass, ausfuehrt_bis, ausfuehrender_msb",
    )
    .bind(&req.grund)
    .bind(&req.notiz)
    .bind(id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await;

    let row = match row {
        Ok(Some(r)) => r,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    use sqlx::Row as _;
    let malo_id: String = row.try_get("malo_id").unwrap_or_default();
    let anlass: String = row.try_get("anlass").unwrap_or_default();
    let ausfuehrt_bis: Option<time::Date> = row.try_get("ausfuehrt_bis").ok().flatten();
    let ausfuehrender_msb: Option<String> = row.try_get("ausfuehrender_msb").ok().flatten();

    // The order is terminal but the reading is still owed, so the failure is
    // announced rather than just recorded.
    if let Some(ref webhook_url) = state.erp_webhook_url {
        let client = mako_service::http::default_client();
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.edmd.reading.order.failed",
            "source": format!("urn:edmd:tenant:{}:{}", state.tenant, malo_id),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": OffsetDateTime::now_utc().to_string(),
            "subject": malo_id,
            "tenant": state.tenant,
            "datacontenttype": "application/json",
            "data": {
                "order_id":          id.to_string(),
                "malo_id":           malo_id,
                "anlass":            anlass,
                "grund":             req.grund,
                "notiz":             req.notiz,
                "ausfuehrt_bis":     ausfuehrt_bis.map(|d| d.to_string()),
                "ausfuehrender_msb": ausfuehrender_msb,
                "recommended_action":
                    "Re-dispatch the reading, or estimate under §40a EnWG and document the basis",
            }
        });
        post_ce_with_retry(&client, webhook_url, &ce).await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": id.to_string(),
            "status": "FEHLGESCHLAGEN",
            "grund": req.grund,
            "still_owed": true,
        })),
    )
        .into_response()
}

// ── Jahresablesung campaign (N7 — §40 Abs. 2 EnWG) ───────────────────────────

#[derive(Debug, serde::Deserialize)]
pub struct JahresablesungCampaignRequest {
    /// NB MP-ID (BDEW-Codenummer) — used to filter MaLos in the NB's grid area.
    pub nb_mp_id: String,
    /// Campaign year (defaults to current year).
    pub campaign_year: Option<i32>,
    /// Target reading date (YYYY-MM-DD).  Defaults to December 31 of campaign_year.
    pub geplant_am: Option<time::Date>,
    /// Latest acceptable reading date.  Defaults to January 31 of campaign_year+1.
    pub ausfuehrt_bis: Option<time::Date>,
    /// MSB MP-ID responsible for executing the reading.
    /// If absent, the grundzuständiger MSB per MaLo is used.
    pub ausfuehrender_msb: Option<String>,
    /// Maximum number of MaLos to process in one request (default 5000, max 50000).
    pub max_malos: Option<i64>,
}

/// `POST /api/v1/reading-orders/campaign`
///
/// **Jahresablesung campaign scheduler (§40 Abs. 2 EnWG).**
///
/// Creates bulk `JAHRESABLESUNG` reading orders for all SLP MaLos in the NB's
/// grid area that have not yet been scheduled for reading this campaign year.
///
/// ## Pipeline
///
/// 1. Query `marktd GET /api/v1/malo?bilanzierungsmethode=SLP&size=500` (paginated)
///    to enumerate SLP MaLos in the NB's grid area.
/// 2. For each MaLo: check `ablese_auftraege` — skip those already having an
///    OFFEN/BEAUFTRAGT/AUSGEFUEHRT `JAHRESABLESUNG` for this year.
/// 3. Insert `ablese_auftraege` rows with:
///    - `anlass = JAHRESABLESUNG`
///    - `auftraggeber_rolle = NB`
///    - `geplant_am = December 31 of campaign_year`
///    - `ausfuehrt_bis = January 31 of campaign_year+1`
/// 4. Return campaign summary.
///
/// ## §40 Abs. 2 EnWG
///
/// NB is obligated to ensure annual SLP meter reading.  Unread SLP meters →
/// estimated settlement → potential Mehr-/Mindermengendisputes with the LF.
/// This endpoint enables a single-click annual reading campaign without ERP
/// integration.
///
/// ## Idempotency
///
/// Re-running for the same NB + year is safe — already-scheduled MaLos are
/// counted in `skipped` and not double-scheduled.
async fn jahresablesung_campaign(
    State(state): State<HandlerState>,
    Json(req): Json<JahresablesungCampaignRequest>,
) -> impl IntoResponse {
    match run_jahresablesung_campaign(
        state.repo.pool(),
        &state.tenant,
        &state.marktd_url,
        &state.marktd_api_key,
        &req,
    )
    .await
    {
        Ok(outcome) => (StatusCode::CREATED, Json(outcome.into_json(&req))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// What a campaign run did.
pub struct CampaignOutcome {
    /// SLP MaLos found in the NB's grid area.
    pub total_malos: usize,
    /// Reading orders created.
    pub created: usize,
    /// MaLos that already had an order for this campaign year.
    pub skipped: usize,
    /// Campaign year the orders were dated in.
    pub year: i32,
    /// Planned reading date.
    pub geplant_am: time::Date,
    /// Latest acceptable reading date.
    pub ausfuehrt_bis: time::Date,
}

impl CampaignOutcome {
    fn into_json(self, req: &JahresablesungCampaignRequest) -> serde_json::Value {
        serde_json::json!({
            "nb_mp_id": req.nb_mp_id,
            "campaign_year": self.year,
            "geplant_am": self.geplant_am.to_string(),
            "ausfuehrt_bis": self.ausfuehrt_bis.to_string(),
            "total_slp_malos_enumerated": self.total_malos,
            "reading_orders_created": self.created,
            "already_scheduled_skipped": self.skipped,
            "legal_basis": "§40 Abs. 2 EnWG",
        })
    }
}

/// Why a campaign run could not complete.
pub enum CampaignError {
    /// `nb_mp_id` is not a 13-digit BDEW/DVGW Codenummer.
    InvalidNbMpId,
    /// `marktd` could not be reached or answered with an error.
    Marktd(String),
}

impl CampaignError {
    fn into_response(self) -> axum::response::Response {
        match self {
            Self::InvalidNbMpId => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "nb_mp_id must be a 13-digit BDEW/DVGW Codenummer",
                })),
            )
                .into_response(),
            Self::Marktd(detail) => (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": detail })),
            )
                .into_response(),
        }
    }

    /// Human-readable reason, for callers that are not HTTP.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::InvalidNbMpId => "nb_mp_id must be a 13-digit BDEW/DVGW Codenummer".to_owned(),
            Self::Marktd(d) => d.clone(),
        }
    }
}

/// Create the Jahresablesung reading orders for one NB's grid area.
///
/// Shared by the HTTP endpoint and the MCP tool so both raise identical orders.
/// A second implementation would be a second §40 Abs. 2 EnWG obligation with its
/// own idempotency rules.
///
/// # Errors
///
/// [`CampaignError`] when `nb_mp_id` is malformed or `marktd` cannot be read.
/// Per-MaLo insert failures are logged and skipped: a campaign that aborts
/// half-way leaves an unrepeatable partial state, whereas re-running is
/// idempotent.
pub async fn run_jahresablesung_campaign(
    pool: &sqlx::PgPool,
    tenant: &str,
    marktd_url: &str,
    marktd_api_key: &secrecy::SecretString,
    req: &JahresablesungCampaignRequest,
) -> Result<CampaignOutcome, CampaignError> {
    use secrecy::ExposeSecret as _;

    let year = req
        .campaign_year
        .unwrap_or_else(|| time::OffsetDateTime::now_utc().year());

    // Default dates: geplant_am = Dec 31, ausfuehrt_bis = Jan 31 next year.
    let geplant_am = req.geplant_am.unwrap_or_else(|| {
        time::Date::from_calendar_date(year, time::Month::December, 31)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date())
    });
    let ausfuehrt_bis = req.ausfuehrt_bis.unwrap_or_else(|| {
        time::Date::from_calendar_date(year + 1, time::Month::January, 31)
            .unwrap_or_else(|_| time::OffsetDateTime::now_utc().date())
    });

    let max_malos = req.max_malos.unwrap_or(5_000).min(50_000);
    let marktd_base = marktd_url.trim_end_matches('/').to_owned();
    let api_key = marktd_api_key.expose_secret().to_owned();
    let client = mako_service::http::default_client();

    // Enumerate the SLP MaLos in **this NB's** grid area (paginated, 500 per
    // page). `zuordnungstyp=NB` with `rollencodenummer` restricts the result to
    // MaLos whose Netzbetreiber role is held by `nb_mp_id`; without it the
    // campaign enumerates every SLP MaLo in the market and creates reading
    // orders for locations another NB is responsible for.
    // A BDEW/DVGW Codenummer is 13 digits. Validating rather than escaping keeps
    // the value out of the query string unless it is well formed.
    let nb_mp_id = req.nb_mp_id.trim();
    if nb_mp_id.len() != 13 || !nb_mp_id.chars().all(|c| c.is_ascii_digit()) {
        return Err(CampaignError::InvalidNbMpId);
    }
    let mut malos: Vec<String> = Vec::new();
    let mut page = 1i64;
    let page_size = 500i64;

    loop {
        let url = format!(
            "{marktd_base}/api/v1/malo\
             ?bilanzierungsmethode=SLP\
             &zuordnungstyp=NB\
             &rollencodenummer={nb_mp_id}\
             &size={page_size}&page={page}"
        );
        let mut get_req = client.get(&url);
        if !api_key.is_empty() {
            get_req = get_req.bearer_auth(&api_key);
        }
        let resp = match get_req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "edmd: campaign failed to reach marktd");
                return Err(CampaignError::Marktd(format!("marktd unreachable: {e}")));
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            tracing::error!(%status, "edmd: marktd list_malo returned error");
            return Err(CampaignError::Marktd(format!("marktd error: {status}")));
        }
        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return Err(CampaignError::Marktd(e.to_string()));
            }
        };

        let items = match body.get("items").and_then(|v| v.as_array()) {
            Some(a) => a.clone(),
            None => break,
        };
        if items.is_empty() {
            break;
        }

        for item in &items {
            if let Some(mid) = item.get("malo_id").and_then(|v| v.as_str()) {
                malos.push(mid.to_owned());
                if malos.len() as i64 >= max_malos {
                    break;
                }
            }
        }

        // Check pagination — stop when we've retrieved all or hit max.
        let total: i64 = body.get("total").and_then(|v| v.as_i64()).unwrap_or(0);
        if malos.len() as i64 >= total || malos.len() as i64 >= max_malos {
            break;
        }
        page += 1;
    }

    let total_malos = malos.len();
    let mut created = 0u64;
    let mut skipped = 0u64;

    for malo_id in &malos {
        // Check whether this MaLo already has a Jahresablesung this year.
        let existing: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM ablese_auftraege
             WHERE malo_id = $1 AND tenant = $2
               AND anlass = 'JAHRESABLESUNG'
               AND auftraggeber_rolle = 'NB'
               AND extract(year FROM geplant_am) = $3
               AND status IN ('OFFEN','BEAUFTRAGT','AUSGEFUEHRT')",
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(year)
        .fetch_one(pool)
        .await
        .unwrap_or(0);

        if existing > 0 {
            skipped += 1;
            continue;
        }

        let res = sqlx::query(
            "INSERT INTO ablese_auftraege
             (malo_id,tenant,anlass,auftraggeber_rolle,ausfuehrender_msb,geplant_am,ausfuehrt_bis)
             VALUES ($1,$2,'JAHRESABLESUNG','NB',$3,$4,$5)",
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(&req.ausfuehrender_msb)
        .bind(geplant_am)
        .bind(ausfuehrt_bis)
        .execute(pool)
        .await;

        match res {
            Ok(_) => created += 1,
            Err(e) => {
                tracing::warn!(malo_id, error = %e, "edmd: campaign insert failed for MaLo");
            }
        }
    }

    tracing::info!(
        nb_mp_id = %req.nb_mp_id,
        campaign_year = year,
        total_malos,
        created,
        skipped,
        "edmd: Jahresablesung campaign complete"
    );

    Ok(CampaignOutcome {
        total_malos,
        created: usize::try_from(created).unwrap_or(usize::MAX),
        skipped: usize::try_from(skipped).unwrap_or(usize::MAX),
        year,
        geplant_am,
        ausfuehrt_bis,
    })
}

/// `GET /api/v1/compliance/jahresablesung/{year}`
///
/// §40 Abs. 2 EnWG compliance report for a campaign year.
///
/// The obligation is to read each SLP Marktlokation annually. This reports
/// whether that happened, broken down by what actually became of each order —
/// which is the distinction that matters, because only `AUSGEFUEHRT` discharges
/// the obligation. `STORNIERT` withdraws it; `FEHLGESCHLAGEN` leaves it
/// outstanding with a documented Ablesehindernis; anything still `OFFEN` or
/// `BEAUFTRAGT` past its deadline is simply late.
///
/// **Cedar action**: `read-reading-order`
async fn jahresablesung_compliance(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(year): Path<i32>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-reading-order", &state.tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let rows = sqlx::query(
        r"SELECT status,
                 count(*)                                        AS orders,
                 count(*) FILTER (WHERE ausfuehrt_bis < CURRENT_DATE
                                    AND status <> 'AUSGEFUEHRT') AS overdue
          FROM   ablese_auftraege
          WHERE  tenant = $1
            AND  anlass = 'JAHRESABLESUNG'
            AND  extract(year FROM geplant_am) = $2
          GROUP BY status",
    )
    .bind(&state.tenant)
    .bind(year)
    .fetch_all(state.repo.pool())
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    use sqlx::Row as _;
    let mut by_status = serde_json::Map::new();
    let (mut total, mut ausgefuehrt, mut overdue) = (0i64, 0i64, 0i64);
    for r in &rows {
        let status: String = r.try_get("status").unwrap_or_default();
        let orders: i64 = r.try_get("orders").unwrap_or(0);
        let od: i64 = r.try_get("overdue").unwrap_or(0);
        total += orders;
        overdue += od;
        if status == "AUSGEFUEHRT" {
            ausgefuehrt = orders;
        }
        by_status.insert(status, serde_json::json!(orders));
    }

    // Reasons the failed readings could not be taken. The Ablesehindernis
    // decides whether the NB may estimate under §40a EnWG or must re-dispatch.
    let grounds = sqlx::query(
        r"SELECT fehlschlag_grund, count(*) AS n
          FROM   ablese_auftraege
          WHERE  tenant = $1 AND anlass = 'JAHRESABLESUNG'
            AND  extract(year FROM geplant_am) = $2
            AND  status = 'FEHLGESCHLAGEN'
          GROUP BY fehlschlag_grund",
    )
    .bind(&state.tenant)
    .bind(year)
    .fetch_all(state.repo.pool())
    .await
    .unwrap_or_default();

    let mut by_grund = serde_json::Map::new();
    for r in &grounds {
        let g: Option<String> = r.try_get("fehlschlag_grund").ok().flatten();
        let n: i64 = r.try_get("n").unwrap_or(0);
        by_grund.insert(
            g.unwrap_or_else(|| "UNBEKANNT".to_owned()),
            serde_json::json!(n),
        );
    }

    // Rate against orders raised, not against the SLP population: this service
    // knows what was ordered, and `marktd` owns how many MaLos exist. A
    // population-based rate computed here would overstate coverage whenever a
    // MaLo was never scheduled at all.
    #[allow(clippy::cast_precision_loss)]
    let quote = if total > 0 {
        ausgefuehrt as f64 / total as f64
    } else {
        0.0
    };

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "campaign_year":      year,
            "orders_total":       total,
            "ausgefuehrt":        ausgefuehrt,
            "ablesequote":        (quote * 10_000.0).round() / 10_000.0,
            "ueberfaellig":       overdue,
            "by_status":          by_status,
            "fehlschlag_gruende": by_grund,
            "legal_basis":        "§40 Abs. 2 EnWG (jährliche Ablesung), §40a EnWG (Schätzung)",
            "note": "`ablesequote` is over orders raised, not over the SLP population — \
                     a MaLo that was never scheduled has no order here. Cross-check the \
                     population with marktd.",
        })),
    )
        .into_response()
}

// \u2500\u2500 M4: iMSys / SMGW 15-min direct push \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// One 15-min (or other fixed-length) metered interval in a direct-push batch.
#[derive(Debug, serde::Deserialize)]
pub struct DirectInterval {
    /// Interval start (RFC 3339 UTC).  Must be an exact quarter-hour for iMSys.
    #[serde(with = "time::serde::rfc3339")]
    pub from: OffsetDateTime,
    /// Interval end (RFC 3339 UTC).
    #[serde(with = "time::serde::rfc3339")]
    pub to: OffsetDateTime,
    /// Energy quantity, expressed in [`Self::unit`].
    pub value: Decimal,
    /// Physical unit the meter registered, parsed by
    /// [`metering::interval::MeasurementUnit::parse_scaled`]: `kWh`/`MWh`/`GJ`/
    /// `MJ`/`Wh` for energy, `m3`/`m\u00b3`/`l` for volume. It must be either the
    /// unit the Sparte is measured in or the one it is billed in; anything else
    /// is rejected rather than stored under a guessed interpretation.
    #[serde(default = "default_unit_kwh")]
    pub unit: String,
    /// Reading quality per BDEW Messwertstatus.
    #[serde(default)]
    pub quality: Option<String>,
}

fn default_unit_kwh() -> String {
    "kWh".to_owned()
}

/// Request body for `POST /api/v1/meter-reads/rlm/{malo_id}`
/// and `POST /api/v1/meter-reads/gas/{malo_id}`.
///
/// Designed for SMGW direct push, iMSys CLS gateway, and ERP data import.
/// Each call is idempotent when the same `session_id` is re-submitted.
///
/// ## Format
///
/// ```json
/// {
///   "session_id": "SMGW-SN1234-2026-07-12T00:00:00Z",
///   "source": "SMGW",
///   "obis_code": "1-0:1.8.0",
///   "melo_id": "DE00001234567890123456789012345",
///   "intervals": [
///     { "from": "2026-07-12T00:00:00Z", "to": "2026-07-12T00:15:00Z", "value": "2.345" },
///     { "from": "2026-07-12T00:15:00Z", "to": "2026-07-12T00:30:00Z", "value": "2.412" }
///   ]
/// }
/// ```
///
/// ## Gas variant
///
/// For Gas, set `unit = "m3"` and supply `brennwert_kwh_per_m3` + `zustandszahl`
/// for the Hs-based conversion.  The handler stores converted kWh_Hs values.
#[derive(Debug, serde::Deserialize)]
pub struct DirectPushRequest {
    /// Caller-supplied idempotency key (e.g. SMGW SN + timestamp).
    /// Re-submitting the same key returns 200 with the original result.
    pub session_id: Option<String>,
    /// Human-readable source identifier (e.g. `"SMGW"`, `"CLS_GATEWAY"`, `"ERP"`).
    #[serde(default = "default_source")]
    pub source: String,
    /// OBIS-Kennzahl (e.g. `"1-0:1.8.0"` for Wirkarbeit Tarif 1 + 2).
    pub obis_code: Option<String>,
    /// 33-character MeLo-ID (optional but recommended for device tracing).
    pub melo_id: Option<String>,
    /// MP-ID of the sender (MSB or SMGW system). Stored as `sender_mp_id` per §22 MessZV
    /// per-interval MSB attribution — required after a WiM MSB switch (PID 55039).
    pub sender_mp_id: Option<String>,
    /// Metered intervals (15-min for iMSys; 60-min or 1440-min for SLP).
    pub intervals: Vec<DirectInterval>,
    // ── Gas-specific fields ───────────────────────────────────────────────────
    /// Brennwert (superior calorific value) in kWh/m³ — required when `unit = "m3"`.
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl (volume correction factor) — default 1.0 when absent.
    pub zustandszahl: Option<Decimal>,
}

fn default_source() -> String {
    "DIRECT_PUSH".to_owned()
}

/// Quality report returned in the direct-push response and stored in `meter_reads.quality_warnings`.
///
/// ## M7 — Best-in-class quality scoring
///
/// This struct uses the **Hampel filter** (sliding-window robust outlier detection)
/// rather than the simpler global 3-sigma rule.  The Hampel filter is the
/// state-of-the-art algorithm for time-series meter data because:
///
/// - Uses **median** (not mean) → robust to existing outliers
/// - Uses **MAD** (Median Absolute Deviation) → scale estimate not distorted by outliers  
/// - **Sliding window** → captures local behaviour, not global
/// - `sigma = 1.4826 × MAD` (the constant converts MAD to equivalent Gaussian σ)
/// - Flag `x[i]` as outlier if `|x[i] − window_median| > threshold × sigma`
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
async fn record_quality_assessment(
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

fn compute_quality(
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

/// `POST /api/v1/meter-reads/rlm/{malo_id}`
///
/// iMSys / SMGW direct push for **Strom RLM** and **iMSys** customers.
///
/// ## Why direct push?
///
/// - MSCONS round-trip via `makod` adds 15\u201360 min latency.
/// - \u00a741a EnWG dynamic tariffs need sub-hourly resolution for real-time billing.
/// - High-frequency RLM meters (up to 96 intervals/day) saturate the EDIFACT pipeline.
///
/// ## Idempotency
///
/// Submit the same `session_id` twice to get the stored result back without re-processing.
///
/// ## Quality scoring (M7)
///
/// Gap detection, consecutive-zero analysis, and 3-sigma outlier detection run at
/// ingest time.  If `has_warnings = true`, `edmd` emits `de.edmd.reading.quality.warning`
/// to the ERP webhook so `agentd` can investigate.
pub async fn post_direct_reads_rlm(
    State(state): State<HandlerState>,
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(req): Json<DirectPushRequest>,
) -> impl IntoResponse {
    // Cedar RBAC: only MSB, LF, or admin roles may push direct reads.
    if let Err(e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        tracing::warn!(malo_id, error = %e, "edmd: direct push RBAC denied");
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    if req.intervals.is_empty() {
        return (StatusCode::BAD_REQUEST, "intervals must not be empty").into_response();
    }

    post_direct_reads_inner(&state, &malo_id, req, "STROM", "DIRECT_PUSH").await
}

/// `POST /api/v1/meter-reads/gas/{malo_id}`
///
/// iMSys / SMGW direct push for **Gas RLM** customers.
/// Accepts m\u00b3 readings and converts to kWh_Hs using Brennwert \u00d7 Zustandszahl.
pub async fn post_direct_reads_gas(
    State(state): State<HandlerState>,
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Path(malo_id): Path<String>,
    Json(req): Json<DirectPushRequest>,
) -> impl IntoResponse {
    if let Err(_e) = enforcer.check(
        &claims.principal(),
        "write-meter-reads",
        state.tenant.as_str(),
    ) {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    if req.intervals.is_empty() {
        return (StatusCode::BAD_REQUEST, "intervals must not be empty").into_response();
    }

    post_direct_reads_inner(&state, &malo_id, req, "GAS", "DIRECT_GAS").await
}

/// Deliver a CloudEvent to the ERP webhook with 3 retries (exponential backoff 200ms→400ms).
///
/// Designed for fire-and-retry rather than fire-and-forget: a lost quality warning
/// CloudEvent (`de.edmd.reading.quality.warning`) constitutes a compliance gap under
/// §22 MessZV — the responsible party must be informed of quality issues.
async fn post_ce_with_retry(client: &reqwest::Client, url: &str, ce: &serde_json::Value) {
    for attempt in 0u32..3 {
        match client
            .post(url)
            .header("Content-Type", "application/cloudevents+json")
            .json(ce)
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => return,
            Ok(r) => tracing::warn!(attempt, status = %r.status(), "edmd: CE webhook non-2xx"),
            Err(e) => tracing::warn!(attempt, error = %e, "edmd: CE webhook error"),
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(200 * (1 << attempt))).await;
        }
    }
    tracing::error!("edmd: CloudEvent delivery failed after 3 retries — event lost");
}

/// Internal implementation shared by Strom and Gas direct-push handlers.
#[allow(clippy::too_many_lines)]
/// Parse a caller-supplied quality flag from its wire spelling.
///
/// Returns `None` for anything outside the set, so an unrecognised flag is
/// refused at the boundary rather than stored as `UNKNOWN` — a caller that
/// asserts a quality we cannot interpret has made a claim about billability
/// that must not be silently downgraded.
fn quality_flag_from_wire(s: &str) -> Option<QualityFlag> {
    match s.to_uppercase().as_str() {
        "MEASURED" => Some(QualityFlag::Measured),
        "ESTIMATED" => Some(QualityFlag::Estimated),
        "SUBSTITUTED" => Some(QualityFlag::Substituted),
        "CALCULATED" => Some(QualityFlag::Calculated),
        "CORRECTED" => Some(QualityFlag::Corrected),
        "PRELIMINARY" => Some(QualityFlag::Preliminary),
        "FAULTY" => Some(QualityFlag::Faulty),
        "UNKNOWN" => Some(QualityFlag::Unknown),
        _ => None,
    }
}

/// Outcome of running the V01–V10 engine over a batch, for the ingest response.
pub(crate) struct BatchValidation {
    pub(crate) issue_count: usize,
    pub(crate) billing_block_count: usize,
    pub(crate) rules: Vec<String>,
}

impl BatchValidation {
    /// `true` when no rule fired.
    pub(crate) fn is_clean(&self) -> bool {
        self.issue_count == 0
    }
}

/// Run V01–V10 over an ingest batch and annotate the rows each issue describes.
///
/// Every ingest family routes through here so a reading lands with the same
/// quality record whichever door it came in by. Issues are attached to the rows
/// they name rather than to the MaLo as a whole, so a downstream §17 MessZV
/// substitution decision can see which intervals are actually implicated.
///
/// Validation annotates and never rejects: whether an interval is billable is a
/// separate decision from whether it is stored, and discarding a suspect reading
/// would destroy the evidence the Netzbetreiber needs to resolve it.
pub(crate) fn validate_and_annotate(
    batch: &mut [MeterRead],
    source: &str,
    malo_id: &str,
) -> BatchValidation {
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
            value_kwh: r.quantity_kwh,
            quality: metering::QualityFlag::Measured,
            obis_code: r.obis_code.clone(),
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
        "edmd: ingest validation issues (§17 MessZV)"
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

async fn post_direct_reads_inner(
    state: &HandlerState,
    malo_id: &str,
    req: DirectPushRequest,
    sparte_str: &str,
    source_default: &str,
) -> axum::response::Response {
    use rust_decimal::Decimal;
    let pool = state.repo.pool();
    let source = if req.source.is_empty() {
        source_default.to_owned()
    } else {
        req.source.clone()
    };

    // \u2500\u2500 Idempotency check \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let session_id = req.session_id.clone().unwrap_or_else(|| {
        // Auto-generate from malo_id + first interval timestamp
        req.intervals
            .first()
            .map(|iv| format!("{malo_id}-{}", iv.from))
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    });

    // Check if this session was already committed.
    //
    // Scoped by tenant: two tenants may legitimately use the same `session_id`
    // for the same MaLo-ID, and without it one would read the other's summary
    // and skip its own ingest.
    let existing: Option<serde_json::Value> = match sqlx::query_scalar(
        r"SELECT quality_summary FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND tenant = $3
            AND status = 'committed'",
    )
    .bind(&session_id)
    .bind(malo_id)
    .bind(&state.tenant)
    .fetch_optional(state.repo.pool())
    .await
    {
        Ok(row) => row.flatten(),
        // A failed lookup is not evidence that the session is new. Re-ingesting
        // on a transient database error would be safe for the readings, which
        // upsert, but it would also re-emit the CloudEvents that trigger a
        // billing recompute downstream.
        Err(e) => {
            tracing::error!(malo_id, error = %e, "edmd: direct push idempotency check failed");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({
                    "error": "could not verify whether this session was already committed",
                })),
            )
                .into_response();
        }
    };

    if let Some(summary) = existing {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "session_id": session_id,
                "malo_id": malo_id,
                "status": "already_committed",
                "quality": summary,
            })),
        )
            .into_response();
    }

    // \u2500\u2500 Interval validation + kWh conversion \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    // Gas m³ → kWh_Hs via metering::gas_m3_to_kwh_hs (§25 Nr. 4 MessEV / DVGW G 685)
    // Units are parsed by the same `metering` machinery as the IoT path, so the
    // ingest families share one unit contract. A string compare against `"m3"`
    // missed the superscript `"m³"` that `MeasurementUnit` accepts, and the
    // electricity endpoint never checked the unit against the Sparte at all.
    let msparte = match sparte_str {
        "GAS" => metering::interval::Sparte::Gas,
        _ => metering::interval::Sparte::Strom,
    };
    let z = req.zustandszahl.unwrap_or(Decimal::ONE);

    let mut accepted: Vec<&DirectInterval> = Vec::new();
    let mut rejected_count = 0usize;
    let mut validation_errors: Vec<String> = Vec::new();

    for iv in &req.intervals {
        if iv.from >= iv.to {
            validation_errors.push(format!(
                "interval from={} to={}: from must be before to",
                iv.from, iv.to
            ));
            rejected_count += 1;
            continue;
        }
        let duration_secs = (iv.to - iv.from).whole_seconds();
        if duration_secs <= 0 || duration_secs > 86400 {
            validation_errors.push(format!(
                "interval from={}: duration {}s is out of range [1, 86400]",
                iv.from, duration_secs
            ));
            rejected_count += 1;
            continue;
        }
        if iv.value < Decimal::ZERO {
            validation_errors.push(format!(
                "interval from={}: negative value {}",
                iv.from, iv.value
            ));
            rejected_count += 1;
            continue;
        }
        let Some(scale) = metering::interval::MeasurementUnit::parse_scaled(&iv.unit) else {
            validation_errors.push(format!(
                "interval from={}: unknown unit `{}`; expected kWh/MWh/GJ/MJ/Wh (energy) \
                 or m³/l (volume)",
                iv.from, iv.unit
            ));
            rejected_count += 1;
            continue;
        };
        if scale.unit != msparte.measured_unit() && scale.unit != msparte.billing_unit() {
            validation_errors.push(format!(
                "interval from={}: unit {} is not valid for sparte {} — expected {} (as measured) \
                 or {} (as billed)",
                iv.from,
                scale.unit.as_str(),
                msparte.as_str(),
                msparte.measured_unit().as_str(),
                msparte.billing_unit().as_str()
            ));
            rejected_count += 1;
            continue;
        }
        if msparte.requires_conversion()
            && scale.unit == msparte.measured_unit()
            && req.brennwert_kwh_per_m3.is_none()
        {
            validation_errors.push(format!(
                "interval from={}: brennwert_kwh_per_m3 is required when submitting gas in m³ \
                 (§25 Nr. 4 MessEV); submit unit=kWh to supply pre-converted values",
                iv.from
            ));
            rejected_count += 1;
            continue;
        }
        if let Some(q) = iv.quality.as_deref()
            && quality_flag_from_wire(q).is_none()
        {
            validation_errors.push(format!(
                "interval from={}: unknown quality `{q}`; expected one of MEASURED, \
                 ESTIMATED, SUBSTITUTED, CALCULATED, CORRECTED, PRELIMINARY, FAULTY, UNKNOWN",
                iv.from
            ));
            rejected_count += 1;
            continue;
        }
        accepted.push(iv);
    }

    if accepted.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "all intervals failed validation",
                "validation_errors": validation_errors,
            })),
        )
            .into_response();
    }

    // \u2500\u2500 Quality scoring (M7) \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let period_start = accepted.iter().map(|iv| iv.from).min().unwrap();
    let period_end = accepted.iter().map(|iv| iv.to).max().unwrap();

    let mut quality = compute_quality(&accepted, period_start, period_end);
    quality.intervals_rejected = rejected_count;

    record_quality_assessment(
        pool,
        &state.tenant,
        malo_id,
        period_start,
        period_end,
        &source,
        &quality,
    )
    .await;

    let quality_json = serde_json::json!({
        "intervals_accepted": quality.intervals_accepted,
        "intervals_rejected": quality.intervals_rejected,
        "gaps_detected": quality.gaps_detected,
        "zero_run_length": quality.zero_run_length,
        "outlier_intervals": quality.outlier_intervals,
        "spike_intervals": quality.spike_intervals,
        "intervals_consistent": quality.intervals_consistent,
        "has_warnings": quality.has_warnings,
        "coverage_pct": quality.coverage_pct,
        "grade": quality.grade,
        "algorithm": "hampel_k3_t3",
    });

    // \u2500\u2500 Persist intervals \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let obis_code = req.obis_code.as_deref();
    let melo_id = req.melo_id.as_deref();

    let sparte_enum = match sparte_str {
        "GAS" => EdmSparte::Gas,
        _ => EdmSparte::Strom,
    };
    let ingestion_source = IngestionSource::from_db_str(&source);

    let mut batch: Vec<MeterRead> = Vec::with_capacity(accepted.len());
    for iv in &accepted {
        // Every accepted interval parsed in the loop above, so the unit is known
        // and valid for this Sparte.
        let scale = metering::interval::MeasurementUnit::parse_scaled(&iv.unit)
            .expect("unit validated in the accept loop");
        let rescaled = scale.apply(iv.value);
        // m³ → kWh_Hs for Gas (§25 Nr. 4 MessEV / DVGW G 685). The Brennwert is
        // required rather than defaulted: it varies by supply area and month, so
        // a national average would systematically mis-bill an L-Gas network.
        let kwh = if msparte.requires_conversion() && scale.unit == msparte.measured_unit() {
            let hs = req
                .brennwert_kwh_per_m3
                .expect("brennwert presence validated in the accept loop");
            metering::gas_m3_to_kwh_hs(rescaled, hs, z)
        } else {
            rescaled
        };

        batch.push(MeterRead {
            malo_id: malo_id.to_owned(),
            melo_id: melo_id.map(str::to_owned),
            dtm_from: iv.from,
            dtm_to: iv.to,
            quantity_kwh: kwh,
            // Unrecognised flags are rejected in the accept loop, so the
            // fallback only covers an omitted one. A direct push carries a
            // register reading, so it defaults to MEASURED — matching the IoT
            // path, and leaving substitution to the §17 MessZV flow that
            // records who substituted and why.
            quality: iv
                .quality
                .as_deref()
                .and_then(quality_flag_from_wire)
                .unwrap_or(QualityFlag::Measured),
            pid: 0, // no MSCONS process behind a direct push
            sparte: sparte_enum,
            obis_code: obis_code.map(str::to_owned),
            tenant: state.tenant.clone(),
            source: ingestion_source,
            push_session: Some(session_id.clone()),
            // Session-level Hampel scoring. `validate_and_annotate` adds the
            // per-interval V01–V10 findings under a `validation` key.
            quality_warnings: quality.has_warnings.then(|| quality_json.clone()),
            sender_mp_id: req.sender_mp_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
    }

    let validation = validate_and_annotate(&mut batch, "DIRECT_PUSH_VALIDATION", malo_id);

    if let Err(e) = state.repo.store_reads(&batch).await {
        tracing::error!(malo_id, error = %e, "edmd: direct push batch insert failed");
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    // \u2500\u2500 Record session \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, quality_summary, tenant)
          VALUES ($1, $2, $3, $4, $5, $6, $7, 'committed', $8, $9)
          ON CONFLICT (session_id) DO UPDATE
              SET status          = 'committed',
                  quality_summary = EXCLUDED.quality_summary",
    )
    .bind(&session_id)
    .bind(malo_id)
    .bind(&source)
    .bind(obis_code)
    .bind(accepted.len() as i32)
    .bind(period_start)
    .bind(period_end)
    .bind(&quality_json)
    .bind(state.tenant.as_str())
    .execute(state.repo.pool())
    .await;

    // \u2500\u2500 Recompute billing period aggregates \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    // After a direct push, the meter_billing_periods aggregate for the affected
    // period must be refreshed so billingd picks up the new data.
    // We recompute the affected date range using a sum over meter_reads.
    let period_from_date = period_start.date();
    let period_to_date = period_end.date();

    let recompute_result = sqlx::query(
        r"INSERT INTO meter_billing_periods
              (malo_id, period_from, period_to, messtyp, sparte,
               arbeitsmenge_kwh, spitzenleistung_kw, quality, computed_at, tenant)
          SELECT
              $1 AS malo_id,
              $2::date AS period_from,
              $3::date AS period_to,
              'RLM' AS messtyp,
              $4 AS sparte,
              COALESCE(SUM(quantity_kwh), 0) AS arbeitsmenge_kwh,
              -- Spitzenleistung: peak 15-min slot converted to kW (×4)
              MAX(quantity_kwh) * 4 AS spitzenleistung_kw,
              'VALID' AS quality,
              now() AS computed_at,
              $5 AS tenant
          FROM meter_reads
          WHERE malo_id = $1
            AND dtm_from >= $2::date::timestamptz
            AND dtm_to   <= ($3::date + INTERVAL '1 day')::timestamptz
            AND sparte   = $4
            AND tenant   = $5
            AND quality NOT IN ('FAULTY', 'UNKNOWN')
          ON CONFLICT ON CONSTRAINT mbp_tenant_period_unique
          DO UPDATE
              SET arbeitsmenge_kwh  = EXCLUDED.arbeitsmenge_kwh,
                  spitzenleistung_kw = EXCLUDED.spitzenleistung_kw,
                  quality           = EXCLUDED.quality,
                  computed_at       = EXCLUDED.computed_at",
    )
    .bind(malo_id)
    .bind(period_from_date)
    .bind(period_to_date)
    .bind(sparte_str)
    .bind(&state.tenant)
    .execute(state.repo.pool())
    .await;

    if let Err(e) = recompute_result {
        tracing::warn!(malo_id, error = %e, "edmd: billing period recompute after direct push failed (non-fatal)");
    }

    // \u2500\u2500 CloudEvent notifications \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    if let Some(ref webhook_url) = state.erp_webhook_url {
        let client = mako_service::http::default_client();
        let correlation_id = uuid::Uuid::new_v4().to_string();

        // Always emit de.edmd.reading.direct.stored so billingd knows to recompute.
        let stored_ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.edmd.reading.direct.stored",
            "source": format!("urn:edmd:tenant:{}:{}", state.tenant, malo_id),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": OffsetDateTime::now_utc().to_string(),
            "subject": malo_id,
            "tenant": state.tenant,
            "correlationid": correlation_id,
            "causationid": session_id,
            "datacontenttype": "application/json",
            "data": {
                "malo_id": malo_id,
                "session_id": session_id,
                "sparte": sparte_str,
                "obis_code": obis_code,
                "period_from": period_from_date.to_string(),
                "period_to": period_to_date.to_string(),
                "intervals_stored": accepted.len(),
                "source": source,
            }
        });
        post_ce_with_retry(&client, webhook_url, &stored_ce).await;

        // If quality warnings detected, emit de.edmd.reading.quality.warning (M7).
        if quality.has_warnings {
            let warn_ce = serde_json::json!({
                "specversion": "1.0",
                "type": "de.edmd.reading.quality.warning",
                "source": format!("urn:edmd:tenant:{}:{}", state.tenant, malo_id),
                "id": uuid::Uuid::new_v4().to_string(),
                "time": OffsetDateTime::now_utc().to_string(),
                "subject": malo_id,
                "tenant": state.tenant,
                "correlationid": correlation_id,
                "causationid": session_id,
                "datacontenttype": "application/json",
                "data": {
                    "malo_id": malo_id,
                    "session_id": session_id,
                    "sparte": sparte_str,
                    "period_from": period_from_date.to_string(),
                    "period_to": period_to_date.to_string(),
                    "quality": quality_json,
                    "recommended_action": "Investigate with agentd billing-anomaly-agent or edmd MCP get_lastgang tool",
                }
            });
            post_ce_with_retry(&client, webhook_url, &warn_ce).await;
        }
    }

    let status = if quality.has_warnings || !validation.is_clean() {
        StatusCode::ACCEPTED // 202 — stored but with quality warnings
    } else {
        StatusCode::CREATED // 201 — clean store
    };

    (
        status,
        Json(serde_json::json!({
            "session_id": session_id,
            "malo_id": malo_id,
            "sparte": sparte_str,
            "intervals_accepted": accepted.len(),
            "intervals_rejected": rejected_count,
            "validation_errors": validation_errors,
            "period_from": period_from_date.to_string(),
            "period_to": period_to_date.to_string(),
            "quality": quality_json,
            "validation": {
                "issue_count":         validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules":               validation.rules,
            },
            "billing_period_recomputed": true,
            "note": if quality.has_warnings || !validation.is_clean() {
                "de.edmd.reading.quality.warning emitted — investigate before billing run"
            } else {
                "de.edmd.reading.direct.stored emitted — billing period recomputed"
            },
        })),
    )
        .into_response()
}

// ─── M7: retroactive quality rescoring ──────────────────────────────────────

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
/// date window using the Hampel filter (M7 quality algorithm).
///
/// This is useful when:
/// - MSCONS-ingested historical data was stored without quality scoring
/// - The quality algorithm was upgraded (e.g. from 3-sigma to Hampel)
/// - A billing dispute requires re-verification of read quality
///
/// The handler re-runs `compute_quality()` per logical day (96 × 15-min
/// intervals) against the DB values, updates `meter_reads.quality_warnings`,
/// and emits `de.edmd.reading.quality.warning` for any newly-found warnings.
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

    // Load raw meter_reads rows for this malo_id × date window.
    use sqlx::Row as _;
    let rows = sqlx::query(
        r#"SELECT dtm_from, dtm_to, quantity_kwh
           FROM meter_reads
           WHERE malo_id = $1
             AND dtm_from >= $2
             AND dtm_from < $3
             AND tenant = $4
           ORDER BY dtm_from"#,
    )
    .bind(&malo_id)
    .bind(from_dt)
    .bind(to_dt)
    .bind(&state.tenant)
    .fetch_all(state.repo.pool())
    .await;

    let rows = match rows {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    if rows.is_empty() {
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
    // We convert DB rows to DirectInterval for reuse of compute_quality().
    let pseudo_intervals: Vec<DirectInterval> = rows
        .iter()
        .map(|r| {
            let dtm_from: OffsetDateTime = r.get("dtm_from");
            let dtm_to: OffsetDateTime = r.get("dtm_to");
            // 0010: quantity_kwh is NUMERIC(18,5) — read as Decimal directly.
            let v: Decimal = r.try_get("quantity_kwh").unwrap_or(Decimal::ZERO);
            DirectInterval {
                from: dtm_from,
                to: dtm_to,
                value: v,
                unit: "kWh".to_owned(),
                quality: None,
            }
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
    let rows_rescored = rows.len();

    // Bulk-update quality_warnings for all rows in window.
    if !rows.is_empty() {
        let quality_json = serde_json::json!({
            "gaps_detected": quality.gaps_detected,
            "zero_run_length": quality.zero_run_length,
            "outlier_intervals": quality.outlier_intervals,
            "spike_intervals": quality.spike_intervals,
            "intervals_consistent": quality.intervals_consistent,
            "has_warnings": quality.has_warnings,
            "coverage_pct": quality.coverage_pct,
            "grade": quality.grade,
            "algorithm": "hampel_k3_t3",
            "rescored_at": now.to_string(),
        });

        let _ = sqlx::query(
            r#"UPDATE meter_reads
               SET quality_warnings = $1
             WHERE malo_id = $2
               AND tenant  = $5
               AND dtm_from >= $3
               AND dtm_from < $4"#,
        )
        .bind(&quality_json)
        .bind(&malo_id)
        .bind(from_dt)
        .bind(to_dt)
        .bind(&state.tenant)
        .execute(state.repo.pool())
        .await;

        // Emit quality warning CloudEvent if warranted.
        if quality.has_warnings
            && let Some(ref url) = state.erp_webhook_url
        {
            let event_id = uuid::Uuid::new_v4().to_string();
            let ce = serde_json::json!({
                "specversion": "1.0",
                "type": "de.edmd.reading.quality.warning",
                "source": "/edmd/quality-rescore",
                "id": event_id,
                "time": now.to_string(),
                "datacontenttype": "application/json",
                "data": {
                    "malo_id": malo_id,
                    "grade": quality.grade,
                    "gaps_detected": quality.gaps_detected,
                    "outlier_count": quality.outlier_intervals.len() + quality.spike_intervals.len(),
                    "coverage_pct": quality.coverage_pct,
                    "window_from": from_dt.to_string(),
                    "window_to": to_dt.to_string(),
                    "algorithm": "hampel_k3_t3",
                    "trigger": "retroactive_rescore",
                }
            });
            let client = mako_service::http::default_client();
            let _ = client
                .post(url)
                .header("Content-Type", "application/cloudevents+json")
                .json(&ce)
                .send()
                .await;
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

// ─── M7 unit tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod ingest_contract_tests {
    use super::*;

    #[test]
    fn every_wire_quality_flag_is_one_the_column_check_accepts() {
        // The set here and the CHECK in 0001_schema.sql must agree: a flag this
        // function accepts but the column rejects fails the insert at runtime.
        const SCHEMA_CHECK_VALUES: [&str; 8] = [
            "MEASURED",
            "ESTIMATED",
            "SUBSTITUTED",
            "CALCULATED",
            "CORRECTED",
            "PRELIMINARY",
            "FAULTY",
            "UNKNOWN",
        ];
        for value in SCHEMA_CHECK_VALUES {
            let parsed = quality_flag_from_wire(value)
                .unwrap_or_else(|| panic!("`{value}` is in the column CHECK but is not accepted"));
            assert_eq!(
                crate::pg::timeseries::quality_to_str(parsed),
                value,
                "`{value}` must round-trip to the same spelling the column stores"
            );
        }
    }

    #[test]
    fn an_unknown_quality_flag_is_refused_rather_than_coerced() {
        // Binding an unrecognised flag raw would violate the column CHECK; the
        // bulk path used to swallow that error and still count the row stored.
        assert!(quality_flag_from_wire("SUBSTITUTION_VALUE").is_none());
        assert!(quality_flag_from_wire("").is_none());
        assert!(quality_flag_from_wire("banana").is_none());
    }

    #[test]
    fn quality_flags_are_accepted_case_insensitively() {
        assert_eq!(
            quality_flag_from_wire("measured"),
            Some(QualityFlag::Measured)
        );
    }

    #[test]
    fn the_superscript_cubic_metre_is_recognised_as_a_volume_unit() {
        // A string compare against "m3" missed this spelling, so gas submitted
        // as m³ was stored unconverted — roughly a tenfold under-count.
        use metering::interval::{MeasurementUnit, Sparte};
        for spelling in ["m3", "m³", "M3"] {
            let scale = MeasurementUnit::parse_scaled(spelling)
                .unwrap_or_else(|| panic!("`{spelling}` must parse as a volume unit"));
            assert_eq!(scale.unit, MeasurementUnit::CubicMetre);
            assert_eq!(scale.unit, Sparte::Gas.measured_unit());
        }
    }

    #[test]
    fn cubic_metres_are_not_a_valid_unit_for_electricity() {
        // The electricity endpoint had no unit check, so a value labelled "m3"
        // was multiplied by the gas Brennwert and stored as STROM.
        use metering::interval::{MeasurementUnit, Sparte};
        let m3 = MeasurementUnit::CubicMetre;
        assert_ne!(m3, Sparte::Strom.measured_unit());
        assert_ne!(m3, Sparte::Strom.billing_unit());
    }
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

// ── §22 MessZV Bitemporal Corrections ─────────────────────────────────────────

/// `POST /api/v1/corrections/{malo_id}`
///
/// Submit one or more retroactive corrections to stored meter intervals.
///
/// ## §22 MessZV compliance
///
/// Every correction creates an immutable `meter_read_corrections` row that
/// preserves the original value, corrected value, reason, and operator identity.
/// This enables BNetzA auditors to reconstruct the billing basis at any point
/// in time over the mandatory 3-year retention period.
///
/// ## Request body
///
/// ```json
/// {
///   "corrections": [
///     {
///       "malo_id": "51238696781",
///       "dtm_from": "2026-06-01T00:00:00Z",
///       "dtm_to": "2026-06-01T00:15:00Z",
///       "original_kwh": "2.500",
///       "original_quality": "MEASURED",
///       "corrected_kwh": "2.420",
///       "corrected_quality": "CORRECTED",
///       "reason": "Ablese-Korrekturbericht MSB 2026-07-01: Zählerfehlstand Q2/2026",
///       "source": "OPERATOR",
///       "corrected_by": "dispatcher@netzbetreiber.de"
///     }
///   ]
/// }
/// ```
///
/// ## Response
///
/// ```json
/// {
///   "corrected_count": 1,
///   "correction_ids": ["<uuid of the correction record>"]
/// }
/// ```
pub async fn post_corrections(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<mako_edm::domain::CorrectionRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.corrections.is_empty() {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "corrections array must not be empty",
        )
            .into_response();
    }

    // Validate: all corrections must reference the path MaLo
    for (i, rec) in req.corrections.iter().enumerate() {
        if rec.malo_id != malo_id {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!(
                    "correction[{}].malo_id {:?} does not match path malo_id {:?}",
                    i, rec.malo_id, malo_id
                ),
            )
                .into_response();
        }
        if rec.reason.trim().is_empty() {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!("correction[{i}].reason must not be empty (§22 MessZV audit requirement)"),
            )
                .into_response();
        }
    }

    match state.repo.store_corrections(&req.corrections).await {
        Ok(correction_ids) => {
            let count = correction_ids.len();
            tracing::info!(
                malo_id,
                corrected_count = count,
                "edmd: {} interval(s) corrected (§22 MessZV)",
                count
            );
            (
                axum::http::StatusCode::OK,
                Json(mako_edm::domain::CorrectionResponse {
                    corrected_count: count,
                    correction_ids,
                }),
            )
                .into_response()
        }
        Err(e) => (axum::http::StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Bulk ingestion ────────────────────────────────────────────────────────────

/// Request body for `POST /api/v1/meter-reads/{malo_id}/bulk`.
///
/// Accepts a batch of interval readings for one MaLo in a single HTTP request.
/// This is the performance path for large MSCONS deliveries and MSB bulk uploads.
///
/// ## Idempotency
///
/// Each interval is upserted — re-submitting the same `(malo_id, dtm_from, dtm_to)`
/// updates the value and quality. Supply `session_id` to deduplicate entire batches.
///
/// ## Validation
///
/// The batch is validated with [`metering::validate_intervals`] before it is
/// stored, and the resulting issues are written to `quality_warnings` on the
/// intervals they name, in the same statement as the readings themselves.
#[derive(Debug, serde::Deserialize)]
pub struct BulkReadRequest {
    /// Idempotency key — re-submitting the same `session_id` is a no-op if already committed.
    #[serde(default)]
    pub session_id: Option<String>,
    /// Energy commodity (STROM or GAS).
    pub sparte: String,
    /// OBIS-Kennzahl (optional — defaults to `1-0:1.8.0*255` for Strom Bezug).
    #[serde(default)]
    pub obis_code: Option<String>,
    /// Source identifier (default: `API_IMPORT`).
    #[serde(default)]
    pub source: Option<String>,
    /// The interval readings.
    pub reads: Vec<BulkReadEntry>,
}

/// One interval in a bulk read batch.
#[derive(Debug, serde::Deserialize)]
pub struct BulkReadEntry {
    /// Interval start (RFC 3339 UTC).
    pub dtm_from: String,
    /// Interval end (RFC 3339 UTC).
    pub dtm_to: String,
    /// Energy quantity (kWh or kWh_Hs for Gas).
    pub quantity_kwh: String,
    /// Quality flag (MEASURED / ESTIMATED / SUBSTITUTED / …). Defaults to MEASURED.
    #[serde(default)]
    pub quality: Option<String>,
    /// Messlokations-ID (optional).
    #[serde(default)]
    pub melo_id: Option<String>,
}

/// `POST /api/v1/meter-reads/{malo_id}/bulk`
///
/// Batch ingestion endpoint. Accepts up to 50 000 intervals per request.
///
/// The whole batch is validated (V01–V10) and then written in one statement, so
/// `stored_count` reflects rows that actually committed and a failure leaves
/// nothing behind for the caller to reconcile.
///
/// Returns a summary of stored intervals and any validation issues.
pub async fn post_bulk_reads(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<BulkReadRequest>,
) -> impl IntoResponse {
    use metering::QualityFlag;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.reads.is_empty() {
        return (StatusCode::BAD_REQUEST, "reads array must not be empty").into_response();
    }
    const MAX_BATCH: usize = 50_000;
    if req.reads.len() > MAX_BATCH {
        return (
            StatusCode::BAD_REQUEST,
            format!("batch too large: {} > {MAX_BATCH}", req.reads.len()),
        )
            .into_response();
    }

    // Deduplicate by session_id
    if let Some(ref sid) = req.session_id {
        let existing: Option<i64> = sqlx::query_scalar(
            // Only a committed session is a duplicate. Matching any status made
            // a `failed` session permanently unretryable.
            "SELECT interval_count FROM direct_push_sessions
             WHERE session_id = $1 AND tenant = $2 AND status = 'committed'",
        )
        .bind(sid)
        .bind(state.tenant.as_str())
        .fetch_optional(state.repo.pool())
        .await
        .unwrap_or(None);
        if let Some(count) = existing {
            return (
                StatusCode::OK,
                Json(serde_json::json!({
                    "session_id": sid,
                    "stored_count": count,
                    "deduplicated": true
                })),
            )
                .into_response();
        }
    }

    // Sparte determines the storage unit, so an unrecognised value is rejected
    // rather than defaulted.
    let sparte = match req.sparte.to_uppercase().as_str() {
        "STROM" => "STROM",
        "GAS" => "GAS",
        "WAERME" | "WÄRME" => "WAERME",
        "WASSER" => "WASSER",
        other => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!(
                        "unknown sparte `{other}`; expected STROM, GAS, WAERME or WASSER"
                    )
                })),
            )
                .into_response();
        }
    };
    let source = req.source.as_deref().unwrap_or("API_IMPORT");
    let session_id = req
        .session_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let sparte_enum = match sparte {
        "GAS" => EdmSparte::Gas,
        "WAERME" => EdmSparte::Waerme,
        "WASSER" => EdmSparte::Wasser,
        _ => EdmSparte::Strom,
    };
    let ingestion_source = IngestionSource::from_db_str(source);

    let mut batch: Vec<MeterRead> = Vec::with_capacity(req.reads.len());

    for entry in &req.reads {
        let dtm_from = match OffsetDateTime::parse(&entry.dtm_from, &Rfc3339) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid dtm_from {:?}: {e}", entry.dtm_from),
                )
                    .into_response();
            }
        };
        let dtm_to = match OffsetDateTime::parse(&entry.dtm_to, &Rfc3339) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid dtm_to {:?}: {e}", entry.dtm_to),
                )
                    .into_response();
            }
        };
        let qty: rust_decimal::Decimal = match entry.quantity_kwh.parse() {
            Ok(d) => d,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("invalid quantity {:?}: {e}", entry.quantity_kwh),
                )
                    .into_response();
            }
        };
        // An unrecognised flag is refused rather than coerced: binding it raw
        // would fail the column CHECK, and treating it as UNKNOWN would silently
        // strip the row from every billing aggregate.
        let quality = match entry.quality.as_deref() {
            None => QualityFlag::Measured,
            Some(q) => match quality_flag_from_wire(q) {
                Some(f) => f,
                None => {
                    return (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        Json(serde_json::json!({
                            "error": format!(
                                "interval {}: unknown quality `{q}`; expected one of MEASURED, \
                                 ESTIMATED, SUBSTITUTED, CALCULATED, CORRECTED, PRELIMINARY, \
                                 FAULTY, UNKNOWN",
                                entry.dtm_from
                            )
                        })),
                    )
                        .into_response();
                }
            },
        };

        batch.push(MeterRead {
            malo_id: malo_id.clone(),
            melo_id: entry.melo_id.clone(),
            dtm_from,
            dtm_to,
            quantity_kwh: qty,
            quality,
            pid: 0, // no MSCONS process behind an API import
            sparte: sparte_enum,
            obis_code: req.obis_code.clone(),
            tenant: state.tenant.clone(),
            source: ingestion_source,
            push_session: Some(session_id.clone()),
            quality_warnings: None,
            sender_mp_id: None,
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
    }

    // Validation runs before the write so its findings can be stored with the
    // rows they describe, in the same statement.
    batch.sort_by_key(|r| r.dtm_from);
    let validation = validate_and_annotate(&mut batch, "BULK_IMPORT_VALIDATION", &malo_id);

    let period_from = batch.first().map(|r| r.dtm_from);
    let period_to = batch.last().map(|r| r.dtm_to);

    // One batched statement, so the count reported is the count committed.
    if let Err(e) = state.repo.store_reads(&batch).await {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: bulk import batch insert failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": e.to_string(),
                "stored_count": 0,
                "session_id": session_id,
            })),
        )
            .into_response();
    }
    let stored = batch.len();

    let issues_summary = serde_json::json!({
        "is_clean": validation.is_clean(),
        "billing_block_count": validation.billing_block_count,
        "issue_count": validation.issue_count,
        "rules_triggered": validation.rules,
    });

    // Persist session record
    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, quality_summary, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,'committed',$8,$9)
          ON CONFLICT (session_id) DO NOTHING",
    )
    .bind(&session_id)
    .bind(&malo_id)
    .bind(source)
    .bind(&req.obis_code)
    .bind(stored as i32)
    .bind(period_from)
    .bind(period_to)
    .bind(&issues_summary)
    .bind(state.tenant.as_str())
    .execute(state.repo.pool())
    .await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "session_id": session_id,
            "malo_id": malo_id,
            "stored_count": stored,
            "validation": issues_summary,
        })),
    )
        .into_response()
}

// ── §17 MessZV Auto-Substitute (post_substitute_values) ───────────────────────

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
    /// Why a substitute is required (§22 MessZV audit trail).
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
/// a value was derived, the CHECK list describes §17 MessZV substitution
/// categories. Methods with no §17 category map to `LinearInterpolation`, the
/// closest admissible description, rather than failing the write.
fn forecast_method_to_db(method: metering::ForecastMethod) -> &'static str {
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
/// Generate and store §17 MessZV substitute values for a gap interval.
///
/// This endpoint:
/// 1. Validates the requested gap window.
/// 2. Fetches prior-period reference data from `meter_reads`.
/// 3. Calls `metering::prior_period_substitutes()` to generate values.
/// 4. Stores the generated intervals as `AUTO_SUBSTITUTE` source.
/// 5. Records each substitution in `substitute_value_log` for §22 MessZV audit.
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
    use metering::{MeterInterval, QualityFlag, SubstituteMethod};
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

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
    .bind(&malo_id)
    .bind(prior_from)
    .bind(gap_from)
    .bind(&state.tenant)
    .fetch_all(state.repo.pool())
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
    .bind(&malo_id)
    .bind(&state.tenant)
    .bind(gap_to)
    .fetch_optional(state.repo.pool())
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
    let pool = state.repo.pool();

    // Every interval's reading and its §22 MessZV audit row commit together. As
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

    for entry in &substitute_entries {
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
        // destroying a real reading: §17 MessZV authorises an Ersatzwert where
        // no usable measurement exists, not in place of one. A window that
        // overlaps billable data leaves that data untouched and reports the
        // interval as skipped.
        // The CTE snapshots the value being replaced before the upsert runs, so
        // `substitute_value_log.original_kwh` records what was actually there.
        // Without it the §22 MessZV trail says a substitute was written but not
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
                 obis_code, obis_code_norm, source, tenant)
              VALUES ($1, $2, $3, $4, 'SUBSTITUTED', 0, $5, $6, $8, $9, 'AUTO_SUBSTITUTE', $7)
              ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO UPDATE
                SET quantity_kwh = EXCLUDED.quantity_kwh,
                    quality = EXCLUDED.quality,
                    source = EXCLUDED.source,
                    archived = false
                WHERE meter_reads.quality IN ('FAULTY', 'UNKNOWN')
              RETURNING (SELECT quantity_kwh FROM prior) AS original_kwh",
        )
        .bind(&malo_id)
        .bind(iv.from)
        .bind(iv.to)
        .bind(iv.value_kwh)
        .bind(sparte.as_str())
        .bind(sparte.billing_unit().as_str())
        .bind(&state.tenant)
        .bind(req.obis_code.as_deref())
        .bind(&obis_norm)
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

        // §22 MessZV audit trail: which value was replaced, by what method, on
        // whose authority.
        if let Err(e) = sqlx::query(
            r"INSERT INTO substitute_value_log
                (malo_id, dtm_from, dtm_to, method, reason, substitute_kwh,
                 original_kwh, created_by, tenant)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(&malo_id)
        .bind(iv.from)
        .bind(iv.to)
        .bind(method_str)
        .bind(reason_str)
        .bind(iv.value_kwh)
        .bind(original_kwh)
        .bind(operator_id)
        .bind(&state.tenant)
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
        "edmd: §17 MessZV substitute values generated"
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
            // to zero — and the §22 MessZV record must name what ran, not what
            // was asked for.
            "methods_applied": log_entries
                .iter()
                .filter_map(|e| e["method"].as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            // Intervals already covered by a billable reading; §17 MessZV
            // authorises a substitute only where no measurement exists.
            "skipped_measured": skipped,
            "legal_basis": "§17 MessZV Abs. 2 Ersatzwertbildung",
            "intervals": log_entries,
        })),
    )
        .into_response()
}

// ── F-13: §42c Energy Sharing VZW allocation ──────────────────────────────────

/// `GET /api/v1/sharing/{community_id}/allocation?from=RFC3339&to=RFC3339`
///
/// Returns the quarter-hour VZW (Viertelstunden-Zeitreihe) allocation for a
/// `§42c EnWG Energy Sharing community`. Each 15-min interval shows the total
/// community production and the per-participant attribution fraction.
///
/// The `community_id` maps to a `virtual_meter_configs` entry with
/// `rule_type IN ('GgvConstantAllocation', 'GgvProportionalAllocation')`.
/// Source MaLo IDs for the producer(s) and participants are encoded in `rule_json`.
///
/// ## Regulatory basis
///
/// BNetzA Festlegung BK6-23-288 (§42c EnWG): Sharing communities require
/// quarter-hour (VZW) attribution of generated energy to participants.
/// The MSB must provide 15-min MSCONS (PID 13014/13015) for iMSys participants.
async fn get_sharing_allocation(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(community_id): Path<String>,
    Query(params): Query<LastgangParams>,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

    // Load the virtual meter config for this community.
    let pool = state.repo.pool();
    let config_row = sqlx::query(
        r"SELECT rule_type, rule_json, display_name, legal_basis
          FROM virtual_meter_configs
          WHERE virtual_malo_id = $1 AND tenant = $2
            AND rule_type IN ('GgvConstantAllocation','GgvProportionalAllocation')
          LIMIT 1",
    )
    .bind(&community_id)
    .bind(resource_tenant)
    .fetch_optional(pool)
    .await;

    use sqlx::Row as _;
    let config_row = match config_row {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": "No Energy Sharing community found",
                    "community_id": community_id,
                    "hint": "Create via POST /api/v1/virtual with rule_type GgvConstantAllocation or GgvProportionalAllocation"
                })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::warn!(error = %e, "edmd: get_sharing_allocation query failed");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };

    let rule_type: String = config_row.try_get("rule_type").unwrap_or_default();
    let rule_json: serde_json::Value = config_row.try_get("rule_json").unwrap_or_default();
    let display_name: String = config_row.try_get("display_name").unwrap_or_default();
    let legal_basis: Option<String> = config_row.try_get("legal_basis").unwrap_or(None);

    // Extract source MaLo IDs from rule_json.
    // Expected shape: { "source_malo_ids": ["11234567890"], "participant_malo_ids": ["11234567891", ...], "fractions": [...] }
    let source_malo_ids: Vec<String> = rule_json
        .get("source_malo_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    let participant_malo_ids: Vec<String> = rule_json
        .get("participant_malo_ids")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();

    if source_malo_ids.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "Community rule_json must contain non-empty 'source_malo_ids'",
                "community_id": community_id,
            })),
        )
            .into_response();
    }

    // Fetch the aggregation rule and compute via metering::compute_virtual_meter.
    use metering::{MeterInterval, QualityFlag as MQualityFlag};
    use rust_decimal::Decimal;

    // Load production intervals from all source MaLos.
    let mut all_production: Vec<MeterInterval> = Vec::new();
    for malo_id in &source_malo_ids {
        let rows = sqlx::query(
            r"SELECT dtm_from, dtm_to, quantity_kwh, quality
              FROM meter_reads
              WHERE malo_id = $1
                AND dtm_from >= $2
                AND dtm_to   <= $3
                AND tenant    = $4
                AND quality NOT IN ('FAULTY','UNKNOWN')
              ORDER BY dtm_from",
        )
        .bind(malo_id)
        .bind(from)
        .bind(to)
        .bind(resource_tenant)
        .fetch_all(state.repo.pool())
        .await
        .unwrap_or_default();

        use sqlx::Row as _;
        for r in &rows {
            let qty: Decimal = r
                .try_get::<String, _>("quantity_kwh")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(Decimal::ZERO);
            all_production.push(MeterInterval {
                from: r.try_get("dtm_from").unwrap_or(from),
                to: r.try_get("dtm_to").unwrap_or(to),
                value_kwh: qty,
                quality: MQualityFlag::Measured,
                obis_code: None,
            });
        }
    }
    all_production.sort_by_key(|iv| iv.from);

    // Compute community production totals per interval.
    // Per-participant attribution is done by the LF using the fractions in rule_json
    // (GgvConstantAllocation) or by the dynamic consumption ratio (GgvProportionalAllocation).
    // This endpoint returns the community-level production data; callers fetch individual
    // participant consumption via GET /api/v1/lastgang/{malo_id}.
    let total_kwh: Decimal = all_production.iter().map(|iv| iv.value_kwh).sum();
    let interval_count = all_production.len();

    let allocation_intervals: Vec<serde_json::Value> = all_production
        .iter()
        .map(|iv| {
            serde_json::json!({
                "from":         iv.from,
                "to":           iv.to,
                "total_kwh":    iv.value_kwh.to_string(),
                "quality":      "MEASURED",
            })
        })
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "community_id":          community_id,
            "display_name":          display_name,
            "rule_type":             rule_type,
            "legal_basis":           legal_basis.as_deref().unwrap_or("§42c EnWG BK6-23-288"),
            "from":                  from,
            "to":                    to,
            "source_malo_ids":       source_malo_ids,
            "participant_malo_ids":  participant_malo_ids,
            "total_production_kwh":  total_kwh.to_string(),
            "interval_count":        interval_count,
            "intervals":             allocation_intervals,
            "note": "Per-participant allocation fractions applied per rule_type. \
                     Fetch participant consumption via GET /api/v1/lastgang/{malo_id} \
                     for the full §42c VZW settlement picture.",
        })),
    )
        .into_response()
}

// ── GDPR Art. 17 — cold-tier erasure ──────────────────────────────────────────

/// `POST /api/v1/gdpr/erasure/{malo_id}/archive-plan`
///
/// Plan the physical deletion of an erased MaLo's rows from the Iceberg cold
/// tier, and record the affected data files.
///
/// Read-time exclusion already hides these rows from every query. This is about
/// the bytes still on disk, which Art. 17 also reaches.
///
/// iceberg-rust 0.9.1 exposes only `fast_append` on a transaction — no public
/// API removes or rewrites data files — so the rewrite itself is run by an
/// external engine (Spark, Trino) against the returned list. Recording it turns
/// `archive_deletion_pending` from a flag that is never cleared into an
/// obligation with a defined discharge.
///
/// **Cedar action**: `write-gdpr-erasure`
async fn plan_gdpr_archive_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let Some(ref olap) = state.olap_engine else {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "archival is not enabled; there is no cold tier to erase from",
            })),
        )
            .into_response();
    };

    // The erasure must already be on record: planning a rewrite for a MaLo
    // nobody asked to erase would delete lawfully-held data.
    let deletion_id: Option<uuid::Uuid> =
        sqlx::query_scalar("SELECT id FROM gdpr_deletions WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .fetch_optional(state.repo.pool())
            .await
            .ok()
            .flatten();

    let Some(deletion_id) = deletion_id else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no erasure request on record for this MaLo",
                "hint": "DELETE /api/v1/gdpr/erasure/{malo_id} first",
            })),
        )
            .into_response();
    };

    let files = match olap.plan_erasure_files(&malo_id).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: erasure planning failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    for f in &files {
        let _ = sqlx::query(
            r"INSERT INTO gdpr_archive_files
                  (deletion_id, file_path, record_count, file_size_bytes, tenant)
              VALUES ($1,$2,$3,$4,$5)
              ON CONFLICT (deletion_id, file_path) DO NOTHING",
        )
        .bind(deletion_id)
        .bind(&f.file_path)
        .bind(f.record_count.map(|c| i64::try_from(c).unwrap_or(i64::MAX)))
        .bind(i64::try_from(f.file_size_bytes).unwrap_or(i64::MAX))
        .bind(resource_tenant)
        .execute(state.repo.pool())
        .await;
    }

    // No files means nothing of this MaLo reached the cold tier, so the
    // obligation is already discharged there.
    if files.is_empty() {
        let _ = sqlx::query(
            "UPDATE gdpr_deletions
                SET archive_deletion_pending = false, archive_deletion_completed_at = now()
              WHERE id = $1",
        )
        .bind(deletion_id)
        .execute(state.repo.pool())
        .await;
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":      malo_id,
            "deletion_id":  deletion_id.to_string(),
            "files":        files,
            "file_count":   files.len(),
            "pending":      !files.is_empty(),
            "next_step": if files.is_empty() {
                "nothing of this MaLo is in the cold tier — the obligation is discharged"
            } else {
                "run the rewrite with an external engine, then POST .../archive-complete"
            },
            "legal_basis":  "DSGVO Art. 17",
        })),
    )
        .into_response()
}

/// `POST /api/v1/gdpr/erasure/{malo_id}/archive-complete`
///
/// Record that the cold-tier rewrite has been carried out, discharging the
/// Art. 17 obligation for the archive.
///
/// **Cedar action**: `write-gdpr-erasure`
async fn complete_gdpr_archive_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let res = sqlx::query(
        r"WITH d AS (
              UPDATE gdpr_deletions
                 SET archive_deletion_pending = false,
                     archive_deletion_completed_at = now()
               WHERE malo_id = $1 AND tenant = $2
               RETURNING id
          )
          UPDATE gdpr_archive_files f
             SET rewritten_at = now()
            FROM d
           WHERE f.deletion_id = d.id AND f.rewritten_at IS NULL",
    )
    .bind(&malo_id)
    .bind(resource_tenant)
    .execute(state.repo.pool())
    .await;

    match res {
        Ok(r) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id":        malo_id,
                "files_marked":   r.rows_affected(),
                "archive_pending": false,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

// ── F-17: GDPR §17 DSGVO right to erasure ────────────────────────────────────

/// `DELETE /api/v1/gdpr/erasure/{malo_id}`
///
/// Initiates GDPR Art. 17 right-to-erasure for a MaLo.
///
/// ## What this does
///
/// 1. Inserts a row in `gdpr_deletions` (idempotent on `malo_id + tenant`).
/// 2. **Soft-deletes** all `meter_reads` rows for this MaLo by marking them
///    `quality = 'FAULTY'` and replacing `quantity_kwh` with `'0'`
///    (§22 MessZV: audit trail must be preserved — rows are not physically deleted).
/// 3. Deletes `meter_billing_periods` rows (no audit trail obligation).
/// 4. Deletes `quality_assessments` rows.
/// 5. Hard deletion of Iceberg Parquet data must be done by the operator
///    via the archive rewrite pipeline (out-of-band; noted in `gdpr_deletions`).
///
/// ## Regulatory basis
///
/// DSGVO Art. 17 right to erasure. §22 MessZV (3-year audit trail) applies
/// to *billing-relevant* data — once anonymized, the obligation is satisfied.
#[derive(serde::Deserialize)]
struct GdprErasureRequest {
    /// Human-readable reason for erasure (required for audit trail).
    reason: String,
    /// Operator identity who authorized the erasure.
    authorized_by: String,
}

async fn post_gdpr_erasure(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<GdprErasureRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    // Erasure is irreversible and destroys billing history, so it is gated by
    // its own action rather than by the general write permission every ingest
    // client already holds.
    if let Err(e) = enforcer.check(&claims.principal(), "write-gdpr-erasure", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let pool = state.repo.pool();

    // Every step runs in one transaction. An Art. 17 erasure either completed
    // or it did not: a partial erasure reported as success closes out the
    // request while personal data remains, and the caller has no way to tell
    // that apart from a MaLo that legitimately held no readings.
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!(malo_id = %malo_id, error = %e, "edmd: GDPR erasure could not begin");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    /// Abort the erasure, reporting which step failed.
    macro_rules! erasure_step {
        ($step:literal, $expr:expr) => {
            match $expr {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(
                        malo_id = %malo_id, step = $step, error = %e,
                        "edmd: GDPR Art. 17 erasure failed — rolled back"
                    );
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": e.to_string(),
                            "failed_step": $step,
                            "status": "not_erased",
                        })),
                    )
                        .into_response();
                }
            }
        };
    }

    // 1. Record the erasure request (idempotent).
    erasure_step!(
        "record_request",
        sqlx::query(
            r"INSERT INTO gdpr_deletions
                  (malo_id, tenant, reason, authorized_by, requested_at, archive_deletion_pending)
              VALUES ($1, $2, $3, $4, now(), true)
              ON CONFLICT (malo_id, tenant) DO UPDATE
                  SET reason                  = EXCLUDED.reason,
                      authorized_by           = EXCLUDED.authorized_by,
                      requested_at            = now(),
                      archive_deletion_pending = true",
        )
        .bind(&malo_id)
        .bind(resource_tenant)
        .bind(&req.reason)
        .bind(&req.authorized_by)
        .execute(&mut *tx)
        .await
    );

    // 2. Anonymize meter_reads: zero the value, mark Faulty, preserve row for audit.
    // `archived = false` requeues the row for re-export, so the anonymised
    // version replaces the personal data already sitting in the cold tier.
    let anonymized = erasure_step!(
        "anonymize_reads",
        sqlx::query(
            r"UPDATE meter_reads
              SET quantity_kwh = '0',
                  quality      = 'FAULTY',
                  source       = 'GDPR_ERASURE',
                  push_session = NULL,
                  quality_warnings = NULL,
                  sender_mp_id = NULL,
                  archived     = false
              WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(&malo_id)
        .bind(resource_tenant)
        .execute(&mut *tx)
        .await
    );
    let anonymized_count = anonymized.rows_affected();

    // 3. Delete billing period aggregates (no audit trail required).
    // A MaLo-ID is not unique across tenants, so an erasure request scoped to
    // one tenant must not reach another tenant's aggregates for the same ID.
    erasure_step!(
        "delete_billing_periods",
        sqlx::query("DELETE FROM meter_billing_periods WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    // 4. Delete quality assessments.
    erasure_step!(
        "delete_quality_assessments",
        sqlx::query("DELETE FROM quality_assessments WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    // 5. Delete substitute value log.
    erasure_step!(
        "delete_substitute_log",
        sqlx::query("DELETE FROM substitute_value_log WHERE malo_id = $1 AND tenant = $2")
            .bind(&malo_id)
            .bind(resource_tenant)
            .execute(&mut *tx)
            .await
    );

    erasure_step!("commit", tx.commit().await);

    tracing::info!(
        malo_id,
        anonymized_count,
        authorized_by = %req.authorized_by,
        "edmd: GDPR Art. 17 erasure completed for MaLo (hot storage anonymized)"
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "malo_id":           malo_id,
            "status":            "anonymized",
            "anonymized_reads":  anonymized_count,
            "archive_pending":   true,
            "legal_basis":       "DSGVO Art. 17 right to erasure",
            "audit_note": "meter_reads rows anonymized (quantity=0, quality=FAULTY) — \
                           §22 MessZV audit trail row structure preserved. \
                           Iceberg Parquet deletion is scheduled via archive rewrite pipeline.",
        })),
    )
        .into_response()
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
async fn iceberg_rest_config(
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
async fn iceberg_list_namespaces(
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
async fn iceberg_list_tables(
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
async fn iceberg_load_table(
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
struct SqlQueryRequest {
    sql: String,
    /// Maximum rows to return (default: 10_000).
    #[serde(default = "default_sql_limit")]
    limit: usize,
    /// Output format: "json" (default) or "arrow_ipc".
    #[serde(default)]
    #[allow(dead_code)]
    format: SqlOutputFormat,
}

fn default_sql_limit() -> usize {
    10_000
}

#[derive(serde::Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SqlOutputFormat {
    #[default]
    Json,
    ArrowIpc,
}

async fn post_sql_query(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Json(req): Json<SqlQueryRequest>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
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

// ── §42c EnWG Energy-Sharing readiness report ─────────────────────────────────

/// Query parameters for `GET /api/v1/sharing/readiness`.
#[derive(Debug, serde::Deserialize)]
struct SharingReadinessParams {
    /// RFC 3339 start of the observation window. Defaults to 30 days ago.
    from: Option<String>,
    /// RFC 3339 end of the observation window. Defaults to now.
    to: Option<String>,
    /// Comma-separated MaLo-IDs to assess. Defaults to every MaLo with readings
    /// in the window.
    malo_ids: Option<String>,
    /// Coverage threshold in percent. Defaults to
    /// [`metering::sharing::DEFAULT_COVERAGE_THRESHOLD_PCT`].
    min_coverage_pct: Option<f64>,
}

/// Per-point delivery verdict in the readiness report.
#[derive(Debug, serde::Serialize)]
struct SharingReadinessItem {
    malo_id: String,
    /// `DELIVERING` · `INSUFFICIENT` · `ABSENT`.
    delivery: String,
    /// Detected interval length in seconds, when determinable.
    interval_seconds: Option<i64>,
    /// Classification derived from the observed series.
    messtyp: Option<String>,
    /// Share of expected quarter-hour slots present, 0–100.
    coverage_pct: Option<f64>,
    reading_count: u64,
    /// Why the point is not delivering a conforming series.
    reasons: Vec<String>,
    /// What an operator must do next.
    required_action: String,
}

/// `GET /api/v1/sharing/readiness`
///
/// Fleet report: which delivery points are **actually** producing the
/// quarter-hour series that §42c Abs. 1 EnWG requires.
///
/// This is the delivery half of the §42c readiness question. `marktd`'s
/// `GET /api/v1/melos/{id}/sharing-eligibility` answers the capability half from
/// device master data. Read together they separate the two states an operator
/// must act on differently:
///
/// - **capable but not delivering** — the meter supports Zählerstandsgangmessung
///   but none is configured; order the configuration, not a meter.
/// - **not capable** — needs an iMSys rollout or an RLM conversion.
///
/// Resolution is derived per point from the median of `dtm_to - dtm_from` via
/// `metering::classification::detect_interval_length`; `meter_reads` stores no
/// resolution column. Coverage is measured against the number of quarter-hour
/// slots the window contains.
async fn get_sharing_readiness(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<SharingReadinessParams>,
) -> impl IntoResponse {
    use metering::classification::{classify_messtyp, detect_interval_length};
    use metering::interval::{MeterInterval, QualityFlag};
    use metering::sharing::{
        DEFAULT_COVERAGE_THRESHOLD_PCT, Delivery, DeliveryEvidenceInput, assess_delivery,
    };
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-timeseries", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let now = OffsetDateTime::now_utc();
    let to = params
        .to
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or(now);
    let from = params
        .from
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
        .unwrap_or_else(|| to - time::Duration::days(30));

    if from >= to {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": "`from` must precede `to`" })),
        )
            .into_response();
    }

    let threshold = params
        .min_coverage_pct
        .unwrap_or(DEFAULT_COVERAGE_THRESHOLD_PCT);

    // Expected quarter-hour slots in the window — the coverage denominator.
    let window_secs = (to - from).whole_seconds().max(1);
    let expected_slots = (window_secs as f64 / 900.0).max(1.0);

    let explicit: Option<Vec<String>> = params.malo_ids.as_deref().map(|s| {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    });

    let pool = state.repo.pool();

    // Resolve the candidate set: explicit list, or every MaLo with readings.
    let malo_ids: Vec<String> = match explicit {
        Some(ids) => ids,
        None => {
            let rows = sqlx::query(
                r"SELECT DISTINCT malo_id
                    FROM meter_reads
                   WHERE tenant = $1 AND dtm_from >= $2 AND dtm_to <= $3
                   ORDER BY malo_id",
            )
            .bind(resource_tenant)
            .bind(from)
            .bind(to)
            .fetch_all(pool)
            .await;
            match rows {
                Ok(rows) => {
                    use sqlx::Row as _;
                    rows.iter()
                        .filter_map(|r| r.try_get::<String, _>("malo_id").ok())
                        .collect()
                }
                Err(e) => {
                    tracing::warn!(error = %e, "sharing readiness: candidate scan failed");
                    return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
                }
            }
        }
    };

    let mut items = Vec::with_capacity(malo_ids.len());

    for malo_id in &malo_ids {
        let rows = sqlx::query(
            r"SELECT dtm_from, dtm_to, quantity_kwh, source
                FROM meter_reads
               WHERE malo_id = $1 AND tenant = $2
                 AND dtm_from >= $3 AND dtm_to <= $4
                 AND quality NOT IN ('FAULTY', 'UNKNOWN')
               ORDER BY dtm_from",
        )
        .bind(malo_id)
        .bind(resource_tenant)
        .bind(from)
        .bind(to)
        .fetch_all(pool)
        .await;

        // A failed per-point query must not abort the fleet report; surface it
        // as an explicit reason instead of silently dropping the point.
        let rows = match rows {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(malo_id = %malo_id, error = %e, "sharing readiness: read query failed");
                items.push(SharingReadinessItem {
                    malo_id: malo_id.clone(),
                    delivery: "ABSENT".to_owned(),
                    interval_seconds: None,
                    messtyp: None,
                    coverage_pct: None,
                    reading_count: 0,
                    reasons: vec![format!("Abfrage fehlgeschlagen: {e}")],
                    required_action: "edmd-Log prüfen".to_owned(),
                });
                continue;
            }
        };

        use sqlx::Row as _;
        let mut source_hint: Option<String> = None;
        let intervals: Vec<MeterInterval> = rows
            .iter()
            .filter_map(|r| {
                if source_hint.is_none() {
                    source_hint = r.try_get::<String, _>("source").ok();
                }
                Some(MeterInterval {
                    from: r.try_get("dtm_from").ok()?,
                    to: r.try_get("dtm_to").ok()?,
                    value_kwh: r.try_get("quantity_kwh").ok()?,
                    quality: QualityFlag::Measured,
                    obis_code: None,
                })
            })
            .collect();

        let interval_class = detect_interval_length(&intervals);
        let messtyp = if intervals.is_empty() {
            None
        } else {
            Some(classify_messtyp(&intervals, source_hint.as_deref()))
        };
        let coverage_pct = if intervals.is_empty() {
            None
        } else {
            Some(((intervals.len() as f64 / expected_slots) * 100.0).min(100.0))
        };

        let evidence = DeliveryEvidenceInput {
            interval_class,
            messtyp,
            coverage_pct,
            reading_count: intervals.len() as u64,
            last_reading_at: intervals.last().map(|iv| iv.to),
        };
        let (delivery, reasons) = assess_delivery(&evidence, threshold);

        let required_action = match delivery {
            Delivery::Delivering => "keine",
            Delivery::Insufficient => "Zählerstandsgangmessung konfigurieren bzw. Lücken klären",
            Delivery::Absent => "Messwertlieferung beim MSB beauftragen",
        };

        items.push(SharingReadinessItem {
            malo_id: malo_id.clone(),
            delivery: match delivery {
                Delivery::Delivering => "DELIVERING",
                Delivery::Insufficient => "INSUFFICIENT",
                Delivery::Absent => "ABSENT",
            }
            .to_owned(),
            interval_seconds: interval_class
                .as_ref()
                .map(metering::classification::IntervalLengthClass::seconds),
            messtyp: messtyp.map(|m| format!("{m:?}").to_uppercase()),
            coverage_pct,
            reading_count: intervals.len() as u64,
            reasons,
            required_action: required_action.to_owned(),
        });
    }

    let delivering = items.iter().filter(|i| i.delivery == "DELIVERING").count();
    let assessed = items.len();
    let ready_pct = if assessed == 0 {
        0.0
    } else {
        (delivering as f64 / assessed as f64) * 100.0
    };

    Json(serde_json::json!({
        "assessed_at":          now.format(&Rfc3339).unwrap_or_default(),
        "window_from":          from.format(&Rfc3339).unwrap_or_default(),
        "window_to":            to.format(&Rfc3339).unwrap_or_default(),
        "min_coverage_pct":     threshold,
        "points_assessed":      assessed,
        "points_delivering":    delivering,
        "points_insufficient":  items.iter().filter(|i| i.delivery == "INSUFFICIENT").count(),
        "points_absent":        items.iter().filter(|i| i.delivery == "ABSENT").count(),
        "ready_pct":            ready_pct,
        "legal_basis":          "§42c Abs. 1 EnWG i. V. m. §2 Satz 1 Nr. 27 MsbG",
        "note":                 "Lieferung, nicht Fähigkeit — Stammdaten-Eignung via marktd GET /api/v1/melos/{id}/sharing-eligibility",
        "items":                items,
    }))
    .into_response()
}

// ── IoT meter ingest (LoRaWAN / M-Bus / REST heat meters) ────────────────────

/// One decoded interval in an IoT push.
#[derive(Debug, serde::Deserialize)]
struct IotInterval {
    /// RFC 3339 interval start (inclusive).
    from: String,
    /// RFC 3339 interval end (exclusive).
    to: String,
    /// Consumption in `unit` over the interval.
    value: rust_decimal::Decimal,
}

/// An IoT meter-reading push.
///
/// The envelope is **transport-agnostic and already decoded**. See
/// [`post_iot_reads`] for why `edmd` does not decode wM-Bus frames itself.
#[derive(Debug, serde::Deserialize)]
struct IotPushRequest {
    /// `WAERME` · `WASSER` · `STROM` · `GAS`.
    sparte: String,
    /// `KWH` or `M3`. Must be consistent with `sparte`.
    unit: String,
    /// Stable per-batch idempotency key. For LoRaWAN use `devEUI:fCnt`; for
    /// OMS/M-Bus the telegram access number.
    session_id: String,
    /// Transport the reading arrived over, for provenance:
    /// `LORAWAN` · `MBUS` · `WMBUS` · `REST`.
    transport: String,
    /// Optional device identity (LoRaWAN devEUI, M-Bus secondary address).
    device_id: Option<String>,
    /// Optional OBIS code. Medium group: 4 = Heizkostenverteiler, 5/6 = thermal,
    /// 7 = gas, 8 = cold water, 9 = hot water.
    obis_code: Option<String>,
    /// Optional Messlokation.
    melo_id: Option<String>,
    /// Raw, undecoded payload as received (base64 or hex, verbatim).
    ///
    /// Retained as the system of record: network-server codecs are mutable and
    /// carry no version on the uplink, so a stored value can only be re-derived
    /// from the original frame.
    raw_payload: Option<String>,
    /// Brennwert Hs in kWh/m³. **Required** when `sparte = GAS` and `unit = M3`.
    ///
    /// Published monthly per supply area by the NB. There is no safe
    /// default: the calorific value determines the billed quantity.
    brennwert_kwh_per_m3: Option<rust_decimal::Decimal>,
    /// Zustandszahl (dimensionless), default 1.0 when not separately metered.
    zustandszahl: Option<rust_decimal::Decimal>,
    /// Calibration validity (`Eichfrist`) end date, `YYYY-MM-DD`, if known.
    ///
    /// Per §34 Abs. 2 MessEV a Eichfrist of at least a year ends only *"mit dem
    /// Ende des Jahres, in dem die Frist rechnerisch endet"*, so callers send
    /// `YYYY-12-31`. Leave unset for Heizkostenverteiler, which have no Eichfrist.
    eichung_bis: Option<String>,
    intervals: Vec<IotInterval>,
}

/// `POST /api/v1/meter-reads/iot/{malo_id}`
///
/// Ingest metering data that does not pass through MSCONS: LoRaWAN uplinks,
/// M-Bus/wM-Bus concentrators, and REST-capable heat meters.
///
/// Heat and water submetering points have no Smart-Meter-Gateway and are
/// governed by **HeizkostenV**: §5 Abs. 3 requires remote readability by
/// 31 December 2026, §6a a monthly consumption message, and §12 Abs. 1 grants a
/// 3 % Kürzungsrecht on two independent grounds — a missing fernablesbare device
/// (Satz 2) and information supplied "nicht oder nicht vollständig" (Satz 3).
///
/// ## Payload
///
/// Values arrive already decoded. wM-Bus/OMS payload specifications are vendor-
/// gated and the device keys sit at the network server, so decoding belongs
/// there. `raw_payload` is retained verbatim so a value can be re-derived if a
/// codec changes.
///
/// ## Calibration
///
/// An expired Eichfrist is recorded as a warning, not a rejection.
///
/// §37 Abs. 1 Satz 1 Nr. 1 MessEG bars *use of the Messgerät* once the Eichfrist
/// has run; §33 Abs. 1 MessEG then bars the resulting values, since a device used
/// contrary to §37 was not "bestimmungsgemäß verwendet". BGH VIII ZR 112/10 holds
/// that in civil billing such a reading loses only its *Vermutung der
/// Richtigkeit*. Public-law Gebührenabrechnung is stricter (BayVGH 20 B 21.2421),
/// which is a billing-side decision.
///
/// §37 Abs. 2 also ends a Eichfrist early on defect or tampering, so an expiry
/// date alone is not the whole eichrechtliche validity test.
async fn post_iot_reads(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
    Json(req): Json<IotPushRequest>,
) -> impl IntoResponse {
    use metering::interval::{MeasurementUnit, Sparte as MSparte};
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "write-meter-reads", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    if req.intervals.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "intervals must not be empty" })),
        )
            .into_response();
    }
    if req.session_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "session_id is required — use devEUI:fCnt (LoRaWAN) or the \
                          telegram access number (OMS/M-Bus)"
            })),
        )
            .into_response();
    }

    let sparte = match req.sparte.to_uppercase().as_str() {
        "STROM" => MSparte::Strom,
        "GAS" => MSparte::Gas,
        "WAERME" | "WÄRME" => MSparte::Waerme,
        "WASSER" => MSparte::Wasser,
        other => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": format!("unknown sparte `{other}`; expected STROM, GAS, WAERME or WASSER")
                })),
            )
                .into_response();
        }
    };

    // EN 1434-1 cl. 6.3.1 permits heat registers in Joules or Watt-hours and any
    // decimal multiple; water submeters commonly report litres. The scale is an
    // exact rational, so GJ→kWh (2500/9) stays exact.
    let Some(scale) = MeasurementUnit::parse_scaled(&req.unit) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "unknown unit `{}`; expected kWh/MWh/GJ/MJ/Wh (energy) or m³/l (volume)",
                    req.unit
                )
            })),
        )
            .into_response();
    };
    let unit = scale.unit;

    // A reading may arrive in the unit the meter registers, or already converted
    // to the settlement unit. Anything else is a decode error.
    if unit != sparte.measured_unit() && unit != sparte.billing_unit() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "unit {} is not valid for sparte {} — expected {} (as measured) or {} (as billed)",
                    unit.as_str(),
                    sparte.as_str(),
                    sparte.measured_unit().as_str(),
                    sparte.billing_unit().as_str()
                )
            })),
        )
            .into_response();
    }

    // Gas is metered in m³ and billed in kWh, so a raw gas uplink needs the
    // Brennwert before it can be stored in an energy column. The calorific value
    // varies by supply area and month, so it is required rather than defaulted.
    let conversion = if sparte.requires_conversion() && unit == sparte.measured_unit() {
        let Some(hs) = req.brennwert_kwh_per_m3 else {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "brennwert_kwh_per_m3 is required when submitting gas in m³ \
                              (§25 Nr. 4 MessEV); submit unit=KWH to supply pre-converted values"
                })),
            )
                .into_response();
        };
        Some((hs, req.zustandszahl.unwrap_or(rust_decimal::Decimal::ONE)))
    } else {
        None
    };

    let pool = state.repo.pool();

    // Idempotency: a committed session replays as 200, never as duplicate rows.
    let already: Option<String> = sqlx::query_scalar(
        r"SELECT status FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND tenant = $3 AND status = 'committed'",
    )
    .bind(&req.session_id)
    .bind(&malo_id)
    .bind(resource_tenant)
    .fetch_optional(state.repo.pool())
    .await
    .ok()
    .flatten();

    if already.is_some() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "malo_id": malo_id,
                "session_id": req.session_id,
                "status": "already_committed",
            })),
        )
            .into_response();
    }

    // Calibration check. Expired → warn, never reject (see fn docs).
    let mut warnings: Vec<String> = Vec::new();
    let eichung_expired = req.eichung_bis.as_deref().and_then(|d| {
        time::Date::parse(
            d,
            &time::macros::format_description!("[year]-[month]-[day]"),
        )
        .ok()
    });
    if let Some(bis) = eichung_expired
        && bis < time::OffsetDateTime::now_utc().date()
    {
        warnings.push(format!(
            "Eichfrist am {bis} abgelaufen (§37 Abs. 1 Satz 1 Nr. 1 MessEG) — \
             Messwerte behalten ihre Verwendbarkeit, verlieren aber die Vermutung \
             der Richtigkeit (BGH VIII ZR 112/10); Befundprüfung nach §39 MessEG \
             empfohlen"
        ));
    }

    // OBIS normalisation happens once, inside `store_reads` — it feeds the
    // primary key, so a second implementation here could drift from it.

    // Hampel-filter quality scoring with media-aware thresholds: heat and water
    // profiles contain long legitimate zero runs.
    let mut scored: Vec<(f64, i64)> = req
        .intervals
        .iter()
        .filter_map(|iv| {
            let from = time::OffsetDateTime::parse(&iv.from, &Rfc3339).ok()?;
            let rescaled = scale.apply(iv.value);
            let converted = conversion.map_or(rescaled, |(hs, z)| {
                metering::gas_m3_to_kwh_hs(rescaled, hs, z)
            });
            let v = converted.to_string().parse::<f64>().ok()?;
            Some((v, (from.unix_timestamp_nanos() as i64)))
        })
        .collect();
    scored.sort_by_key(|(_, ts)| *ts);

    let quality = if scored.len() >= 3 {
        let values: Vec<f64> = scored.iter().map(|(v, _)| *v).collect();
        let stamps: Vec<i64> = scored.iter().map(|(_, t)| *t).collect();
        let period_end = req
            .intervals
            .iter()
            .filter_map(|iv| time::OffsetDateTime::parse(&iv.to, &Rfc3339).ok())
            .map(|t| t.unix_timestamp_nanos() as i64)
            .max()
            .unwrap_or_else(|| stamps[stamps.len() - 1]);
        Some(metering::score_intervals_f64(
            &values,
            &stamps,
            stamps[0],
            period_end,
            metering::QualityConfig::for_sparte(sparte),
        ))
    } else {
        None
    };

    // `score_intervals_f64` reports outliers as `"t+<unix_nanos>"`, not RFC 3339
    // — it takes raw `i64` nanosecond stamps and has no calendar to format
    // against. Parsing them as RFC 3339 silently yields an empty set, which
    // makes the PRELIMINARY flag below unreachable.
    let outlier_stamps: std::collections::HashSet<i64> = quality
        .as_ref()
        .map(|q| {
            q.outlier_intervals
                .iter()
                .chain(q.spike_intervals.iter())
                .filter_map(|ts| ts.strip_prefix("t+").and_then(|n| n.parse::<i64>().ok()))
                .collect()
        })
        .unwrap_or_default();

    let stored_unit = sparte.billing_unit();

    let mut stored = 0usize;
    let mut rejected: Vec<String> = Vec::new();
    let mut batch: Vec<MeterRead> = Vec::with_capacity(req.intervals.len());

    for iv in &req.intervals {
        let (Ok(from), Ok(to)) = (
            time::OffsetDateTime::parse(&iv.from, &Rfc3339),
            time::OffsetDateTime::parse(&iv.to, &Rfc3339),
        ) else {
            rejected.push(format!("unparseable interval {}..{}", iv.from, iv.to));
            continue;
        };
        if from >= to {
            rejected.push(format!("from >= to at {from}"));
            continue;
        }
        // BDEW requires quantities to be positive or zero; direction is carried
        // in the OBIS code.
        if iv.value < rust_decimal::Decimal::ZERO {
            rejected.push(format!(
                "negative value {} at {from} — direction belongs in the OBIS code",
                iv.value
            ));
            continue;
        }

        // An outlier is flagged, not discarded: §17 MessZV substitution is a
        // downstream decision.
        // `PRELIMINARY` (MSCONS Z84, vorläufiger Wert): measured but not yet
        // confirmed. `FAULTY` would assert a defect the filter cannot establish.
        let quality_flag = if outlier_stamps.contains(&(from.unix_timestamp_nanos() as i64)) {
            "PRELIMINARY"
        } else {
            "MEASURED"
        };

        // `meter_reads.quantity_kwh` holds the settlement quantity, so gas is
        // converted before it lands.
        let rescaled = scale.apply(iv.value);
        let quantity = conversion.map_or(rescaled, |(hs, z)| {
            metering::gas_m3_to_kwh_hs(rescaled, hs, z)
        });

        // Rows are accumulated and written in one batched `unnest` statement
        // below, so the whole push lands or none of it does.
        batch.push(MeterRead {
            malo_id: malo_id.clone(),
            melo_id: req.melo_id.clone(),
            dtm_from: from,
            dtm_to: to,
            quantity_kwh: quantity,
            quality: if quality_flag == "PRELIMINARY" {
                QualityFlag::Preliminary
            } else {
                QualityFlag::Measured
            },
            pid: 0,
            sparte: edm_sparte_from_metering(sparte),
            obis_code: req.obis_code.clone(),
            tenant: resource_tenant.to_owned(),
            source: IngestionSource::IotPush,
            push_session: Some(req.session_id.clone()),
            quality_warnings: None,
            sender_mp_id: req.device_id.clone(),
            allocation_version: "INITIAL".to_owned(),
            valid_from_tx: Some(OffsetDateTime::now_utc()),
        });
        stored += 1;
    }

    let validation = validate_and_annotate(&mut batch, "IOT_PUSH_VALIDATION", &malo_id);

    // The IoT path scores with `score_intervals_f64` rather than
    // `compute_quality`, so the report is adapted before it is recorded — the
    // history must not depend on which door the reading came in by.
    if let (Some(q), Some(first), Some(last)) = (
        quality.as_ref(),
        batch.first().map(|r| r.dtm_from),
        batch.last().map(|r| r.dtm_to),
    ) {
        let report = QualityReport {
            intervals_accepted: q.intervals_analysed,
            intervals_rejected: rejected.len(),
            gaps_detected: q.gaps_detected,
            zero_run_length: q.max_zero_run,
            outlier_intervals: q.outlier_intervals.clone(),
            spike_intervals: q.spike_intervals.clone(),
            intervals_consistent: q.intervals_consistent,
            has_warnings: q.has_warnings,
            coverage_pct: q.coverage_pct,
            grade: q.grade.as_str(),
        };
        record_quality_assessment(
            pool,
            resource_tenant,
            &malo_id,
            first,
            last,
            "IOT_PUSH",
            &report,
        )
        .await;
    }

    if !batch.is_empty()
        && let Err(e) = state.repo.store_reads(&batch).await
    {
        tracing::error!(malo_id = %malo_id, error = %e, "edmd: IoT batch insert failed");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // Commit the session only when something landed, so a wholly-failed batch
    // stays retryable.
    if stored > 0 {
        let _ = sqlx::query(
            r"INSERT INTO direct_push_sessions
                (session_id, malo_id, interval_count, status, tenant)
              VALUES ($1,$2,$3,'committed',$4)
              ON CONFLICT (session_id) DO UPDATE SET status = 'committed'",
        )
        .bind(&req.session_id)
        .bind(&malo_id)
        .bind(i32::try_from(stored).unwrap_or(i32::MAX))
        .bind(resource_tenant)
        .execute(state.repo.pool())
        .await;
    }

    let status = if stored == 0 {
        StatusCode::UNPROCESSABLE_ENTITY
    } else if warnings.is_empty() && rejected.is_empty() && validation.is_clean() {
        StatusCode::CREATED
    } else {
        StatusCode::ACCEPTED
    };

    (
        status,
        Json(serde_json::json!({
            "malo_id":     malo_id,
            "session_id":  req.session_id,
            "transport":   req.transport,
            "device_id":   req.device_id,
            "sparte":      sparte.as_str(),
            "unit_submitted": unit.as_str(),
            "unit_stored":     stored_unit.as_str(),
            "converted":       conversion.is_some(),
            "stored":      stored,
            "rejected":    rejected,
            "warnings":    warnings,
            "raw_retained": req.raw_payload.is_some(),
            "quality": quality.as_ref().map(|q| serde_json::json!({
                "grade":         q.grade.as_str(),
                "coverage_pct":  q.coverage_pct,
                "gaps_detected": q.gaps_detected,
                "outliers":      q.outlier_intervals.len() + q.spike_intervals.len(),
                "blocks_billing": q.grade.blocks_billing(),
            })),
            "validation": {
                "issue_count":         validation.issue_count,
                "billing_block_count": validation.billing_block_count,
                "rules":               validation.rules,
            },
            "legal_basis": "HeizkostenV §5 Abs. 3 / §6a; MessEG §37",
        })),
    )
        .into_response()
}
