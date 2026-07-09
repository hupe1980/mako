//! Axum router and startup logic for `obsd`.

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
use uuid::Uuid;

use crate::{
    handler::{HandlerState, handle_webhook},
    pg::PgProcessProjectionRepository,
};
use mako_obs::{domain::ObsQuery, repository::ProcessProjectionRepository};

// ── Router ────────────────────────────────────────────────────────────────────

pub fn router(state: HandlerState) -> Router {
    Router::new()
        .route("/webhook", post(handle_webhook))
        .route("/obs/processes", get(get_processes))
        .route("/obs/processes/{process_id}", get(get_process))
        .route("/obs/kpis", get(get_kpis))
        .route("/obs/overdue", get(get_overdue))
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
            tracing::warn!(error = %e, "obsd: readiness probe: DB unreachable");
            StatusCode::SERVICE_UNAVAILABLE
        }
    }
}

/// `GET /metrics` — Prometheus-compatible operational metrics.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    let total_processes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM process_projections")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let open_processes: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM process_projections WHERE state = 'Open'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);
    let overdue_processes: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM process_projections \
         WHERE state = 'Open' AND deadline_at < now()",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let pool_size = pool.size();
    let pool_idle = pool.num_idle();

    out.push_str("# HELP obsd_process_projections_total Total ProcessProjection records.\n");
    out.push_str("# TYPE obsd_process_projections_total gauge\n");
    out.push_str(&format!(
        "obsd_process_projections_total {total_processes}\n"
    ));
    out.push_str("# HELP obsd_open_processes_total Open (in-progress) MaKo processes.\n");
    out.push_str("# TYPE obsd_open_processes_total gauge\n");
    out.push_str(&format!("obsd_open_processes_total {open_processes}\n"));
    out.push_str("# HELP obsd_overdue_processes_total Processes past their regulatory deadline.\n");
    out.push_str("# TYPE obsd_overdue_processes_total gauge\n");
    out.push_str(&format!(
        "obsd_overdue_processes_total {overdue_processes}\n"
    ));
    out.push_str("# HELP obsd_db_pool_size Current PostgreSQL connection pool size.\n");
    out.push_str("# TYPE obsd_db_pool_size gauge\n");
    out.push_str(&format!("obsd_db_pool_size {pool_size}\n"));
    out.push_str("# HELP obsd_db_pool_idle Idle PostgreSQL connections.\n");
    out.push_str("# TYPE obsd_db_pool_idle gauge\n");
    out.push_str(&format!("obsd_db_pool_idle {pool_idle}\n"));

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
struct ProcessQueryParams {
    state: Option<String>,
    pid: Option<u32>,
    partner_mp_id: Option<String>,
    mdm_role: Option<String>,
    since: Option<String>,
    limit: Option<u32>,
}

async fn get_processes(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<ProcessQueryParams>,
) -> impl IntoResponse {
    use mako_obs::domain::ProcessState;
    use time::format_description::well_known::Rfc3339;

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-process", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let obs_state = params.state.as_deref().and_then(|s| match s {
        "initiated" => Some(ProcessState::Initiated),
        "running" => Some(ProcessState::Running),
        "aperak_timeout" => Some(ProcessState::AperakTimeout),
        "completed" => Some(ProcessState::Completed),
        "rejected" => Some(ProcessState::Rejected),
        "cancelled" => Some(ProcessState::Cancelled),
        _ => ProcessState::from_ce_type(&format!("de.mako.process.{s}")),
    });

    let since = params
        .since
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

    let q = ObsQuery {
        state: obs_state,
        pid: params.pid,
        partner_mp_id: params.partner_mp_id,
        mdm_role: params.mdm_role,
        since,
        tenant: Some(state.tenant.clone()),
        limit: params.limit.unwrap_or(100).min(1000),
    };

    match state.repo.query(&q).await {
        Ok(processes) => Json(serde_json::to_value(processes).unwrap_or_default()).into_response(),
        Err(err) => {
            tracing::warn!(%err, "obsd: get_processes failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_process(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Path(process_id_str): Path<String>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-process", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let process_id: Uuid = match process_id_str.parse() {
        Ok(id) => id,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid UUID").into_response(),
    };

    match state.repo.get(process_id).await {
        Ok(Some(p)) => Json(serde_json::to_value(p).unwrap_or_default()).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "process not found").into_response(),
        Err(err) => {
            tracing::warn!(%err, %process_id, "obsd: get_process failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct KpiQueryParams {
    pid: u32,
    period: Option<String>, // "YYYY-MM" format
}

async fn get_kpis(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<KpiQueryParams>,
) -> impl IntoResponse {
    use time::{Date, Month};

    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-kpi", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let (from, to) = if let Some(period) = params.period.as_deref() {
        let parts: Vec<&str> = period.split('-').collect();
        if parts.len() != 2 {
            return (StatusCode::BAD_REQUEST, "period must be YYYY-MM").into_response();
        }
        let year: i32 = match parts[0].parse() {
            Ok(y) => y,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid year").into_response(),
        };
        let month: u8 = match parts[1].parse() {
            Ok(m) => m,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid month").into_response(),
        };
        let month_enum = match Month::try_from(month) {
            Ok(m) => m,
            Err(_) => return (StatusCode::BAD_REQUEST, "month out of range").into_response(),
        };
        let from = match Date::from_calendar_date(year, month_enum, 1) {
            Ok(d) => d,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid date").into_response(),
        };
        let to = {
            let next_year = if month == 12 { year + 1 } else { year };
            let next_month_u8 = if month == 12 { 1 } else { month + 1 };
            let next_month = Month::try_from(next_month_u8).unwrap();
            Date::from_calendar_date(next_year, next_month, 1)
                .map(|d| d.previous_day().unwrap_or(d))
                .unwrap_or(from)
        };
        (from, to)
    } else {
        let today = OffsetDateTime::now_utc().date();
        let from = Date::from_calendar_date(today.year(), today.month(), 1).unwrap();
        (from, today)
    };

    match state
        .repo
        .kpi_report(params.pid, from, to, &state.tenant)
        .await
    {
        Ok(report) => Json(serde_json::to_value(report).unwrap_or_default()).into_response(),
        Err(mako_obs::error::ObsError::NoKpiData { .. }) => {
            (StatusCode::NOT_FOUND, "no data for this PID / period").into_response()
        }
        Err(err) => {
            tracing::warn!(%err, pid = params.pid, "obsd: get_kpis failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn get_overdue(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-overdue", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let now = OffsetDateTime::now_utc();
    match state.repo.overdue_processes(now, &state.tenant).await {
        Ok(processes) => Json(serde_json::to_value(processes).unwrap_or_default()).into_response(),
        Err(err) => {
            tracing::warn!(%err, "obsd: get_overdue failed");
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
    /// OIDC verifier.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer.
    pub cedar: Arc<CedarEnforcer>,
    /// Graceful-shutdown token.
    pub shutdown: CancellationToken,
}

pub async fn run(cfg: RunConfig) -> anyhow::Result<()> {
    let pool = PgPool::connect_with(
        cfg.database_url
            .expose_secret()
            .parse::<sqlx::postgres::PgConnectOptions>()?,
    )
    .await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let mcp_state = Arc::new(crate::mcp_server::ObsdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        oidc: cfg.oidc.clone(),
        cedar: cfg.cedar.clone(),
    });

    let repo = PgProcessProjectionRepository::new(pool);
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
                    makopid_filter: &[],
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
        "obsd: listening"
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(async move { cfg.shutdown.cancelled().await })
        .await?;
    Ok(())
}
