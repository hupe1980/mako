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
        .route("/api/v1/audit/bnetza-report", get(get_bnetza_report))
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
    /// All operator MP-IDs for §20 EnWG `initiator_is_affiliate` detection.
    /// Includes Strom (BDEW 99…) and Gas (DVGW 98…) codes.
    /// Falls back to `[tenant]` when empty.
    pub own_mp_ids: Vec<String>,
    /// OIDC verifier.
    pub oidc: OidcVerifier,
    /// Cedar ABAC enforcer.
    pub cedar: Arc<CedarEnforcer>,
    /// MCP server auth config (API-key fallback + optional per-named-key identity).
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
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

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("run obsd migrations: {e}"))?;

    let mcp_state = Arc::new(crate::mcp_server::ObsdMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            cfg.oidc.clone(),
            Some(cfg.cedar.clone()),
            &cfg.tenant,
        ),
    });

    let repo = PgProcessProjectionRepository::new(pool);

    // §20 EnWG: build the affiliate-detection set from configured own_mp_ids.
    // Fall back to tenant alone for single-MP-ID deployments.
    let own_mp_ids: std::collections::HashSet<String> = if cfg.own_mp_ids.is_empty() {
        std::iter::once(cfg.tenant.clone()).collect()
    } else {
        cfg.own_mp_ids.into_iter().collect()
    };

    let state = HandlerState {
        repo,
        inbound_secret: Arc::new(cfg.inbound_secret),
        tenant: cfg.tenant,
        own_mp_ids: Arc::new(own_mp_ids),
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

// ── BNetzA §20 Diskriminierungsbericht ───────────────────────────────────────

/// Query parameters for `GET /api/v1/audit/bnetza-report`.
#[derive(Debug, Deserialize)]
struct BnetzaReportQuery {
    /// Calendar year to report (default: current year).
    year: Option<i32>,
    /// Output format: `json` (default) or `csv`.
    format: Option<String>,
}

/// `GET /api/v1/audit/bnetza-report?year=YYYY[&format=csv|json]`
///
/// Generate a BNetzA §20 Abs. 1 EnWG Diskriminierungsbericht for the given
/// calendar year.  The report shows affiliate vs. non-affiliate process
/// statistics per PID, enabling the NB to demonstrate non-discriminatory
/// treatment at BNetzA audits.
///
/// Response shape (JSON):
/// ```json
/// {
///   "year": 2026,
///   "tenant": "9903000...",
///   "generated_at": "2026-07-12T...",
///   "by_pid": [
///     {
///       "pid": 55001,
///       "affiliate":     { "total": 10, "completed": 9, "rejected": 1, "completion_rate": 0.9  },
///       "non_affiliate": { "total": 50, "completed": 48, "rejected": 2, "completion_rate": 0.96 },
///       "parity_gap_pct": -6.0
///     }
///   ]
/// }
/// ```
///
/// `parity_gap_pct` = (affiliate completion_rate − non_affiliate completion_rate) × 100.
/// Negative values indicate the NB resolves affiliate processes FASTER (potential bias).
/// BNetzA threshold: |parity_gap_pct| > 5 triggers scrutiny.
async fn get_bnetza_report(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<BnetzaReportQuery>,
) -> impl IntoResponse {
    let resource_tenant = state.tenant.as_str();
    if let Err(e) = enforcer.check(&claims.principal(), "read-kpi", resource_tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let year = params
        .year
        .unwrap_or_else(|| OffsetDateTime::now_utc().year());
    let format = params.format.as_deref().unwrap_or("json");

    // Query parity stats: affiliate vs. non-affiliate per PID for the year.
    let rows: Vec<(i32, bool, i64, i64, i64)> =
        match sqlx::query_as::<_, (i32, bool, i64, i64, i64)>(
            r"SELECT
              pid::int,
              initiator_is_affiliate,
              COUNT(*)                                              AS total,
              COUNT(*) FILTER (WHERE state = 'Completed')          AS completed,
              COUNT(*) FILTER (WHERE state = 'Rejected')           AS rejected
          FROM process_projections
          WHERE EXTRACT(YEAR FROM updated_at)::int = $1
            AND tenant = $2
          GROUP BY pid, initiator_is_affiliate
          ORDER BY pid, initiator_is_affiliate",
        )
        .bind(year)
        .bind(&state.tenant)
        .fetch_all(state.repo.pool())
        .await
        {
            Ok(r) => r,
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
            }
        };

    // Collate into per-PID affiliate / non-affiliate pairs.
    use std::collections::HashMap;
    #[derive(Default)]
    struct PidStats {
        affiliate_total: i64,
        affiliate_completed: i64,
        affiliate_rejected: i64,
        non_affiliate_total: i64,
        non_affiliate_completed: i64,
        non_affiliate_rejected: i64,
    }
    let mut by_pid: HashMap<i32, PidStats> = HashMap::new();
    for (pid, is_affiliate, total, completed, rejected) in rows {
        let entry = by_pid.entry(pid).or_default();
        if is_affiliate {
            entry.affiliate_total += total;
            entry.affiliate_completed += completed;
            entry.affiliate_rejected += rejected;
        } else {
            entry.non_affiliate_total += total;
            entry.non_affiliate_completed += completed;
            entry.non_affiliate_rejected += rejected;
        }
    }

    fn completion_rate(completed: i64, total: i64) -> f64 {
        if total == 0 {
            0.0
        } else {
            completed as f64 / total as f64
        }
    }

    let mut pid_list: Vec<i32> = by_pid.keys().copied().collect();
    pid_list.sort_unstable();

    if format == "csv" {
        // CSV output for BNetzA tabular reporting.
        let mut csv = String::from(
            "pid,affiliate_total,affiliate_completed,affiliate_rejected,affiliate_completion_rate,\
             non_affiliate_total,non_affiliate_completed,non_affiliate_rejected,non_affiliate_completion_rate,\
             parity_gap_pct\n",
        );
        for pid in &pid_list {
            let s = &by_pid[pid];
            let aff_rate = completion_rate(s.affiliate_completed, s.affiliate_total);
            let non_rate = completion_rate(s.non_affiliate_completed, s.non_affiliate_total);
            let gap = (aff_rate - non_rate) * 100.0;
            csv.push_str(&format!(
                "{},{},{},{},{:.4},{},{},{},{:.4},{:.2}\n",
                pid,
                s.affiliate_total,
                s.affiliate_completed,
                s.affiliate_rejected,
                aff_rate,
                s.non_affiliate_total,
                s.non_affiliate_completed,
                s.non_affiliate_rejected,
                non_rate,
                gap,
            ));
        }
        return (
            StatusCode::OK,
            [("content-type", "text/csv; charset=utf-8")],
            csv,
        )
            .into_response();
    }

    // JSON output.
    let by_pid_json: Vec<serde_json::Value> = pid_list
        .iter()
        .map(|pid| {
            let s = &by_pid[pid];
            let aff_rate = completion_rate(s.affiliate_completed, s.affiliate_total);
            let non_rate = completion_rate(s.non_affiliate_completed, s.non_affiliate_total);
            let gap = (aff_rate - non_rate) * 100.0;
            serde_json::json!({
                "pid": pid,
                "affiliate": {
                    "total": s.affiliate_total,
                    "completed": s.affiliate_completed,
                    "rejected": s.affiliate_rejected,
                    "completion_rate": (aff_rate * 1000.0).round() / 1000.0,
                },
                "non_affiliate": {
                    "total": s.non_affiliate_total,
                    "completed": s.non_affiliate_completed,
                    "rejected": s.non_affiliate_rejected,
                    "completion_rate": (non_rate * 1000.0).round() / 1000.0,
                },
                "parity_gap_pct": (gap * 100.0).round() / 100.0,
            })
        })
        .collect();

    Json(serde_json::json!({
        "year": year,
        "tenant": state.tenant,
        "generated_at": OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "note": "parity_gap_pct = (affiliate_completion_rate - non_affiliate_completion_rate) * 100. BNetzA §20 scrutiny threshold: |gap| > 5.0.",
        "by_pid": by_pid_json,
    }))
    .into_response()
}
