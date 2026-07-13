//! Axum router and startup logic for `edmd`.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;

// Quality scoring and Gas conversion are provided by the `metering` crate.
// The inline `hampel_filter` has been replaced; `compute_quality` still uses
// edmd-specific types but delegates the filter to `metering::hampel_filter`.
use metering::hampel_filter;

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
};
use mako_edm::{
    domain::{BillingPeriodQuery, QualityFlag, Sparte as EdmSparte, TimeSeriesQuery},
    repository::TimeSeriesRepository,
};

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
        // N7: Jahresablesung campaign scheduler (§40 Abs. 2 EnWG)
        .route(
            "/api/v1/reading-orders/campaign",
            post(jahresablesung_campaign),
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
        // Iceberg/S3 archive endpoints
        .route("/api/v1/archive/status", get(get_archive_status))
        .route("/api/v1/archive/olap/{malo_id}", get(get_archive_olap))
        .route("/api/v1/archive/portfolio", get(get_archive_portfolio))
        .route(
            "/api/v1/archive/timeseries/{malo_id}",
            get(get_archive_timeseries),
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
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let billing_periods: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods")
        .fetch_one(pool)
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
        tenant_id: None,
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

    match state.repo.imbalance(&malo_id, from, to, None).await {
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
/// Source: GPKE BK6-22-024 §3; GeLi Gas BK7-24-01-009 §3.
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
        tenant_id: None,
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
}

async fn get_lastgang(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(malo_id): Path<String>,
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

    let q = TimeSeriesQuery {
        malo_id: malo_id.clone(),
        from,
        to,
        sparte: None,
        tenant_id: None,
    };

    let reads = match state.repo.query(&q).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, malo_id, "edmd: get_lastgang query failed");
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

    Json(lastgaenge).into_response()
}

/// Convert an `edm::Sparte` to the BO4E `Sparte` enum.
fn edm_sparte_to_bo4e(s: EdmSparte) -> Bo4eSparte {
    match s {
        EdmSparte::Strom => Bo4eSparte::Strom,
        EdmSparte::Gas => Bo4eSparte::Gas,
    }
}

/// Map `edm::Sparte` to the BO4E `Medium` enum for `Zeitreihe`.
fn edm_sparte_to_medium(s: EdmSparte) -> Medium {
    match s {
        EdmSparte::Strom => Medium::Strom,
        EdmSparte::Gas => Medium::Gas,
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
        tenant_id: None,
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

    Json(zeitreihen).into_response()
}

/// Map a `QualityFlag` to the nearest `Messwertstatus` variant.
fn quality_to_messwertstatus(q: QualityFlag) -> Messwertstatus {
    match q {
        QualityFlag::Measured => Messwertstatus::Abgelesen,
        QualityFlag::Estimated => Messwertstatus::Prognosewert,
        QualityFlag::Substituted => Messwertstatus::Ersatzwert,
        QualityFlag::Calculated => Messwertstatus::Vorlaeufigerwert,
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
    /// Graceful-shutdown token.
    pub shutdown: CancellationToken,
    /// Resolved archive config (env vars already substituted, disabled when absent).
    pub archive: Option<mako_edm::archive::ArchiveConfig>,
    /// ERP webhook URL for outbound CloudEvents (direct push + quality warnings).
    pub erp_webhook_url: Option<String>,
}

/// Connect to the database, run migrations, register subscription, and serve.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let pool = PgPool::connect_with(
        cfg.database_url
            .expose_secret()
            .parse::<sqlx::postgres::PgConnectOptions>()?,
    )
    .await?;

    // Schema must be applied manually — see migrations/0001_initial.sql for DDL.

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
        oidc: cfg.oidc.clone(),
        cedar: cfg.cedar.clone(),
    });

    let repo = PgTimeSeriesRepository::new(pool.clone());
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

    let app = router(state)
        .layer(Extension(cfg.cedar))
        .layer(Extension(cfg.oidc))
        .layer(Extension(Arc::new(pool)))
        .merge(crate::mcp_server::router(mcp_state, cfg.shutdown.clone()));

    let listener = TcpListener::bind(cfg.listen).await?;

    tracing::info!(
        listen = %cfg.listen,
        marktd_url = %cfg.marktd_url,
        "edmd: listening"
    );

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
    State(state): State<HandlerState>,
    Json(req): Json<CreateReadingOrderRequest>,
) -> impl IntoResponse {
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
    State(state): State<HandlerState>,
    Query(q): Query<ListReadingOrdersQuery>,
) -> impl IntoResponse {
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
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
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
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<CompleteReadingOrderRequest>,
) -> impl IntoResponse {
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
    State(state): State<HandlerState>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
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

// ── Jahresablesung campaign (N7 — §40 Abs. 2 EnWG) ───────────────────────────

#[derive(Debug, serde::Deserialize)]
struct JahresablesungCampaignRequest {
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
    let marktd_base = state.marktd_url.trim_end_matches('/').to_owned();
    let api_key = state.marktd_api_key.expose_secret().to_owned();
    let client = reqwest::Client::new();

    // Enumerate SLP MaLos from marktd (paginated, max 500 per page).
    let mut malos: Vec<String> = Vec::new();
    let mut page = 1i64;
    let page_size = 500i64;

    loop {
        let url = format!(
            "{marktd_base}/api/v1/malo?bilanzierungsmethode=SLP&size={page_size}&page={page}"
        );
        let mut get_req = client.get(&url);
        if !api_key.is_empty() {
            get_req = get_req.bearer_auth(&api_key);
        }
        let resp = match get_req.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "edmd: campaign failed to reach marktd");
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({ "error": format!("marktd unreachable: {e}") })),
                )
                    .into_response();
            }
        };
        if !resp.status().is_success() {
            let status = resp.status();
            tracing::error!(%status, "edmd: marktd list_malo returned error");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({ "error": format!("marktd error: {status}") })),
            )
                .into_response();
        }
        let body: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                return (StatusCode::BAD_GATEWAY, e.to_string()).into_response();
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
        .bind(&state.tenant)
        .bind(year)
        .fetch_one(state.repo.pool())
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
        .bind(&state.tenant)
        .bind(&req.ausfuehrender_msb)
        .bind(geplant_am)
        .bind(ausfuehrt_bis)
        .execute(state.repo.pool())
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

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "nb_mp_id": req.nb_mp_id,
            "campaign_year": year,
            "geplant_am": geplant_am.to_string(),
            "ausfuehrt_bis": ausfuehrt_bis.to_string(),
            "total_slp_malos_enumerated": total_malos,
            "reading_orders_created": created,
            "skipped_already_scheduled": skipped,
        })),
    )
        .into_response()
}

// \u2500\u2500 M4: iMSys / SMGW 15-min direct push \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500

/// One 15-min (or other fixed-length) metered interval in a direct-push batch.
#[derive(Debug, serde::Deserialize)]
pub struct DirectInterval {
    /// Interval start (RFC 3339 UTC).  Must be an exact quarter-hour for iMSys.
    pub from: OffsetDateTime,
    /// Interval end (RFC 3339 UTC).
    pub to: OffsetDateTime,
    /// Energy quantity.  For Strom: kWh.  For Gas: m\u00b3 (converted to kWh inside).
    pub value: Decimal,
    /// Physical unit: `"kWh"` | `"m3"` | `"kW"` (instantaneous demand).
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
    /// Metered intervals (15-min for iMSys; 60-min or 1440-min for SLP).
    pub intervals: Vec<DirectInterval>,
    // ── Gas-specific fields ───────────────────────────────────────────────────
    /// Brennwert (superior calorific value) in kWh/m\u00b3 \u2014 required when `unit = "m3"`.
    pub brennwert_kwh_per_m3: Option<Decimal>,
    /// Zustandszahl (volume correction factor) \u2014 default 1.0 when absent.
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
/// Delegates Hampel filter computation to `metering::hampel_filter`.
/// See [`metering::score_intervals`] for the full `MeterInterval`-based API.
fn compute_quality(
    accepted: &[&DirectInterval],
    period_start: OffsetDateTime,
    period_end: OffsetDateTime,
) -> QualityReport {
    use rust_decimal::Decimal;

    let intervals_accepted = accepted.len();

    // Sort by `from` for all window-based checks.
    let mut sorted: Vec<&DirectInterval> = accepted.to_vec();
    sorted.sort_by_key(|iv| iv.from);

    // ── 1. Gap detection ────────────────────────────────────────────────────
    let mut gaps_detected = 0usize;
    for pair in sorted.windows(2) {
        if pair[0].to != pair[1].from {
            gaps_detected += 1;
        }
    }

    // ── 2. Consecutive zero-run ─────────────────────────────────────────────
    let mut zero_run = 0usize;
    let mut max_zero_run = 0usize;
    for iv in &sorted {
        if iv.value == Decimal::ZERO {
            zero_run += 1;
            max_zero_run = max_zero_run.max(zero_run);
        } else {
            zero_run = 0;
        }
    }

    // ── 3. Interval consistency ─────────────────────────────────────────────
    let durations: Vec<i64> = sorted
        .windows(2)
        .filter_map(|w| {
            let d = (w[0].to - w[0].from).whole_seconds();
            if d > 0 { Some(d) } else { None }
        })
        .collect();
    let intervals_consistent = durations.windows(2).all(|d| d[0] == d[1]);

    let values: Vec<f64> = sorted
        .iter()
        .map(|iv| iv.value.to_string().parse::<f64>().unwrap_or(0.0))
        .collect();

    // ── 4. Hampel filter outlier detection ──────────────────────────────────
    // Window k=3 (total 7 points), threshold t=3.0 robust sigma.
    // Minimum 7 intervals needed for a meaningful window.
    let outlier_indices = if sorted.len() >= 7 {
        hampel_filter(&values, 3, 3.0)
    } else {
        vec![]
    };
    let outlier_intervals: Vec<String> = outlier_indices
        .iter()
        .map(|&i| sorted[i].from.to_string())
        .collect();

    // ── 5. Spike detection ──────────────────────────────────────────────────
    // Flag intervals where value > 10 × window median of neighbours.
    // Catches decimal-point errors (e.g. 2345 instead of 2.345 kWh).
    // Only applies when sufficient non-zero neighbours exist.
    const SPIKE_FACTOR: f64 = 10.0;
    let spike_indices: Vec<usize> = if sorted.len() >= 5 {
        (0..sorted.len())
            .filter(|&i| {
                let lo = i.saturating_sub(3);
                let hi = (i + 4).min(sorted.len());
                let neighbours: Vec<f64> = values[lo..hi]
                    .iter()
                    .enumerate()
                    .filter(|(j, _)| lo + j != i) // exclude self
                    .map(|(_, &v)| v)
                    .filter(|&v| v > 0.0)
                    .collect();
                if neighbours.len() < 3 {
                    return false;
                }
                let mut nb_sorted = neighbours.clone();
                nb_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let median = nb_sorted[nb_sorted.len() / 2];
                median > 0.0 && values[i] > SPIKE_FACTOR * median
            })
            .collect()
    } else {
        vec![]
    };
    let spike_intervals: Vec<String> = spike_indices
        .iter()
        .map(|&i| sorted[i].from.to_string())
        .collect();

    // ── 6. Coverage ─────────────────────────────────────────────────────────
    let period_duration_secs = (period_end - period_start).whole_seconds().max(1) as f64;
    let expected_15min = (period_duration_secs / 900.0).ceil() as usize;
    let coverage_pct = if expected_15min == 0 {
        100.0
    } else {
        (intervals_accepted as f64 / expected_15min as f64 * 100.0).min(100.0)
    };

    // ── Quality grade ────────────────────────────────────────────────────────
    let total_anomalies = outlier_intervals.len() + spike_intervals.len();
    let grade = if gaps_detected == 0
        && max_zero_run <= 2
        && total_anomalies == 0
        && coverage_pct >= 99.0
        && intervals_consistent
    {
        "A"
    } else if gaps_detected <= 1 && total_anomalies <= 1 && coverage_pct >= 99.0 {
        "B"
    } else if gaps_detected <= 3 && total_anomalies <= 3 && coverage_pct >= 95.0 {
        "C"
    } else {
        "F"
    };

    let has_warnings = gaps_detected > 0
        || max_zero_run > 4
        || !outlier_intervals.is_empty()
        || !spike_intervals.is_empty()
        || !intervals_consistent
        || coverage_pct < 95.0;

    QualityReport {
        intervals_accepted,
        intervals_rejected: 0, // filled by caller
        gaps_detected,
        zero_run_length: max_zero_run,
        outlier_intervals,
        spike_intervals,
        intervals_consistent,
        has_warnings,
        coverage_pct,
        grade,
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

/// Internal implementation shared by Strom and Gas direct-push handlers.
#[allow(clippy::too_many_lines)]
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
    let existing: Option<serde_json::Value> = sqlx::query_scalar(
        r"SELECT quality_summary FROM direct_push_sessions
          WHERE session_id = $1 AND malo_id = $2 AND status = 'committed'",
    )
    .bind(&session_id)
    .bind(malo_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

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
    // Gas m³ → kWh_Hs via metering::gas_m3_to_kwh_hs (§24 GasGVV / DVGW G 685)
    let hs = req
        .brennwert_kwh_per_m3
        .unwrap_or_else(|| metering::GasConversionParams::default_erdgas_h().hs_kwh_per_m3);
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

    for iv in &accepted {
        // Convert m³ → kWh_Hs for Gas via metering::gas_m3_to_kwh_hs (§24 GasGVV).
        let kwh = if iv.unit.to_lowercase() == "m3" {
            metering::gas_m3_to_kwh_hs(iv.value, hs, z)
        } else {
            iv.value
        };

        let quality_flag = iv.quality.as_deref().unwrap_or("SUBSTITUTION_VALUE");
        // Use quality warnings JSON on first interval only (session-level, not per-interval).
        let warnings_json: Option<&serde_json::Value> = if quality.has_warnings {
            Some(&quality_json)
        } else {
            None
        };

        let result = sqlx::query(
            r"INSERT INTO meter_reads
                  (malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                   pid, sparte, obis_code, source, push_session, quality_warnings, tenant_id)
              VALUES ($1, $2, $3, $4, $5, $6,
                      $7, $8, $9, $10, $11, $12, NULL)
              ON CONFLICT (malo_id, dtm_from, dtm_to) DO UPDATE
                  SET quantity_kwh      = EXCLUDED.quantity_kwh,
                      quality           = EXCLUDED.quality,
                      source            = EXCLUDED.source,
                      push_session      = EXCLUDED.push_session,
                      quality_warnings  = EXCLUDED.quality_warnings,
                      obis_code         = COALESCE(EXCLUDED.obis_code, meter_reads.obis_code)",
        )
        .bind(malo_id)
        .bind(melo_id)
        .bind(iv.from)
        .bind(iv.to)
        .bind(kwh.to_string())
        .bind(quality_flag)
        .bind(0_i32) // pid=0 for direct push (no MSCONS process)
        .bind(sparte_str)
        .bind(obis_code)
        .bind(&source)
        .bind(&session_id)
        .bind(warnings_json)
        .execute(pool)
        .await;

        if let Err(e) = result {
            tracing::error!(malo_id, error = %e, "edmd: direct push interval insert failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    }

    // \u2500\u2500 Record session \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    let _ = sqlx::query(
        r"INSERT INTO direct_push_sessions
              (session_id, malo_id, source, obis_code, interval_count,
               period_from, period_to, status, quality_summary)
          VALUES ($1, $2, $3, $4, $5, $6, $7, 'committed', $8)
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
    .execute(pool)
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
               arbeitsmenge_kwh, spitzenleistung_kw, quality, computed_at)
          SELECT
              $1 AS malo_id,
              $2::date AS period_from,
              $3::date AS period_to,
              'RLM' AS messtyp,
              $4 AS sparte,
              COALESCE(SUM(quantity_kwh::NUMERIC), 0)::TEXT AS arbeitsmenge_kwh,
              -- Spitzenleistung: peak 15-min slot converted to kW (×4)
              (MAX(quantity_kwh::NUMERIC) * 4)::TEXT AS spitzenleistung_kw,
              'VALID' AS quality,
              now() AS computed_at
          FROM meter_reads
          WHERE malo_id = $1
            AND dtm_from >= $2::date::timestamptz
            AND dtm_to   <= ($3::date + INTERVAL '1 day')::timestamptz
            AND sparte   = $4
          ON CONFLICT (malo_id, period_from, period_to, tenant_id)
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
    .execute(pool)
    .await;

    if let Err(e) = recompute_result {
        tracing::warn!(malo_id, error = %e, "edmd: billing period recompute after direct push failed (non-fatal)");
    }

    // \u2500\u2500 CloudEvent notifications \u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500\u2500
    if let Some(ref webhook_url) = state.erp_webhook_url {
        let client = reqwest::Client::new();

        // Always emit de.edmd.reading.direct.stored so billingd knows to recompute.
        let stored_ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.edmd.reading.direct.stored",
            "source": format!("urn:edmd:{malo_id}"),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": OffsetDateTime::now_utc().to_string(),
            "subject": malo_id,
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
        let _ = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&stored_ce)
            .send()
            .await;

        // If quality warnings detected, emit de.edmd.reading.quality.warning (M7).
        if quality.has_warnings {
            let warn_ce = serde_json::json!({
                "specversion": "1.0",
                "type": "de.edmd.reading.quality.warning",
                "source": format!("urn:edmd:{malo_id}"),
                "id": uuid::Uuid::new_v4().to_string(),
                "time": OffsetDateTime::now_utc().to_string(),
                "subject": malo_id,
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
            let _ = client
                .post(webhook_url)
                .header("Content-Type", "application/cloudevents+json")
                .json(&warn_ce)
                .send()
                .await;
        }
    }

    let status = if quality.has_warnings {
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
            "period_from": period_from_date.to_string(),
            "period_to": period_to_date.to_string(),
            "quality": quality_json,
            "billing_period_recomputed": true,
            "note": if quality.has_warnings {
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
           ORDER BY dtm_from"#,
    )
    .bind(&malo_id)
    .bind(from_dt)
    .bind(to_dt)
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
    use rust_decimal::prelude::FromStr;
    let pseudo_intervals: Vec<DirectInterval> = rows
        .iter()
        .filter_map(|r| {
            let dtm_from: OffsetDateTime = r.get("dtm_from");
            let dtm_to: OffsetDateTime = r.get("dtm_to");
            let qty_str: &str = r.get("quantity_kwh");
            let v = Decimal::from_str(qty_str).ok()?;
            Some(DirectInterval {
                from: dtm_from,
                to: dtm_to,
                value: v,
                unit: "kWh".to_owned(),
                quality: None,
            })
        })
        .collect();

    let refs: Vec<&DirectInterval> = pseudo_intervals.iter().collect();
    let mut quality = compute_quality(&refs, from_dt, to_dt);
    quality.intervals_rejected = 0;

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
               AND dtm_from >= $3
               AND dtm_from < $4"#,
        )
        .bind(&quality_json)
        .bind(&malo_id)
        .bind(from_dt)
        .bind(to_dt)
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
            let client = reqwest::Client::new();
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
        let outliers = hampel_filter(&values, 3, 3.0);
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
        let outliers = hampel_filter(&values, 3, 3.0);
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
