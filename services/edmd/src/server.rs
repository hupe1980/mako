//! Axum router and startup logic for `edmd`.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::{Claims, OidcVerifier};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sqlx::PgPool;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::{
    handler::{HandlerState, handle_webhook},
    pg::PgTimeSeriesRepository,
};
use mako_edm::{domain::BillingPeriodQuery, repository::TimeSeriesRepository};

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

    match state.repo.receipts(&malo_id, from, to, None).await {
        Ok(receipts) => Json(serde_json::to_value(receipts).unwrap_or_default()).into_response(),
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
}

/// Connect to the database, run migrations, register subscription, and serve.
pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let pool = PgPool::connect_with(
        cfg.database_url
            .expose_secret()
            .parse::<sqlx::postgres::PgConnectOptions>()?,
    )
    .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let mcp_state = Arc::new(crate::mcp_server::EdmdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        oidc: cfg.oidc.clone(),
        cedar: cfg.cedar.clone(),
    });

    let repo = PgTimeSeriesRepository::new(pool);
    let state = HandlerState {
        repo,
        inbound_secret: Arc::new(cfg.inbound_secret),
        tenant: cfg.tenant,
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
                    event_types: &["de.mako.process.completed"],
                    makopid_filter: mako_edm::domain::MSCONS_PIDS,
                    active: true,
                },
            )
            .await;
    }

    let app = router(state)
        .layer(Extension(cfg.cedar))
        .layer(Extension(cfg.oidc))
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
