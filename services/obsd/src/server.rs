//! Axum router and startup logic for `obsd`.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use mako_service::ServiceContext;
use mako_service::cedar::CedarEnforcer;
use mako_service::oidc::Claims;
use secrecy::ExposeSecret;
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::config::Config;
use crate::pg::projection::TERMINAL_STATE_SQL;
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
        .route(
            "/api/v1/audit/gleichbehandlung",
            get(get_gleichbehandlung_report),
        )
        .route("/obs/metrics", get(metrics))
        .with_state(state)
}

// ── REST handlers ─────────────────────────────────────────────────────────────

/// `GET /obs/metrics` — Prometheus-compatible operational metrics.
///
/// obsd-specific business gauges (process counts + pool stats); the runner's
/// generic `/metrics` (request counters) is mounted separately by `run`.
/// No authentication required; restrict network access at the ingress layer.
async fn metrics(State(state): State<HandlerState>) -> impl IntoResponse {
    let mut out = String::with_capacity(512);
    let pool = state.repo.pool();

    let total_processes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM process_projections")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    let open_processes: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM process_projections WHERE state NOT IN ({TERMINAL_STATE_SQL})"
    ))
    .fetch_one(pool)
    .await
    .inspect_err(|e| tracing::warn!(%e, "obsd: open-process gauge query failed"))
    .unwrap_or(0);
    let overdue_processes: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM process_projections \
         WHERE state NOT IN ({TERMINAL_STATE_SQL}) AND deadline_at < now()"
    ))
    .fetch_one(pool)
    .await
    .inspect_err(|e| tracing::warn!(%e, "obsd: overdue-process gauge query failed"))
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
    family: Option<String>,
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

    // An unparseable `state` is a client error, not "no filter": silently
    // dropping it returns every row and reads as a much larger result set than
    // the caller asked for.
    let obs_state = match params.state.as_deref() {
        None => None,
        Some(s) => match ProcessState::from_str_exact(s) {
            Some(st) => Some(st),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": format!("unknown state `{s}`"),
                        "known": ProcessState::ALL.map(ProcessState::as_str),
                    })),
                )
                    .into_response();
            }
        },
    };

    let since = params
        .since
        .as_deref()
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

    let q = ObsQuery {
        state: obs_state,
        pid: params.pid,
        family: params.family,
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
        let today = mako_fristen::heute();
        let from = Date::from_calendar_date(today.year(), today.month(), 1).unwrap();
        (from, today)
    };

    match state
        .repo
        .kpi_report(params.pid, from, to, &state.tenant)
        .await
    {
        // The report's rates are `Option`, so an unclosed or unmeasurable
        // bucket serialises as `null` without this layer patching anything —
        // which is what stops the MCP surface answering 0.0 to the same query.
        Ok(report) => Json(report).into_response(),
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

// ── Domain router assembly ────────────────────────────────────────────────────

/// Build obsd's domain [`Router`]: resolve config secrets, build the OIDC
/// verifier + Cedar enforcer, register the `marktd` subscription, spawn the
/// `de.obs.*` sweep producers on `ctx.shutdown`, and wire the MCP server.
///
/// The runner ([`mako_service::run`]) owns the pool, migrations, the health /
/// metrics infra routes, bind and graceful serve — none of those live here.
pub async fn build_router(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
    let oidc = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &ctx.http,
        &cfg.identity.tenant,
        ctx.shutdown.clone(),
    )
    .await?;
    let cedar = Arc::new(
        CedarEnforcer::from_policy_str(include_str!("../policies/obsd.cedar"))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let marktd_api_key =
        crate::config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
    let inbound_secret = cfg
        .webhook
        .inbound_secret
        .as_deref()
        .map(crate::config::resolve_env_secret)
        .transpose()
        .context("webhook.inbound_secret")?;
    let webhook_secret = inbound_secret.clone();
    let outbound_secret = cfg
        .webhook
        .outbound_secret
        .as_deref()
        .map(crate::config::resolve_env_secret)
        .transpose()
        .context("webhook.outbound_secret")?;
    let outbound_url = cfg
        .webhook
        .outbound_url
        .as_deref()
        .map(crate::config::resolve_env)
        .transpose()
        .context("webhook.outbound_url")?;

    let tenant = cfg.identity.tenant.clone();
    let pool = ctx.pool().clone();

    let mcp_state = Arc::new(crate::mcp_server::ObsdMcpState {
        pool: pool.clone(),
        tenant: tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
            &cfg.mcp,
            oidc.clone(),
            Some(cedar.clone()),
            &tenant,
        ),
    });

    let worker_pool = pool.clone();
    let repo = PgProcessProjectionRepository::new(pool);

    // § 7a Abs. 5 EnWG: build the affiliate-detection set from own_mp_ids.
    // Fall back to tenant alone for single-MP-ID deployments.
    let own_mp_ids: std::collections::HashSet<String> = if cfg.identity.own_mp_ids.is_empty() {
        std::iter::once(tenant.clone()).collect()
    } else {
        cfg.identity.own_mp_ids.iter().cloned().collect()
    };

    let state = HandlerState {
        repo,
        inbound_secret: Arc::new(inbound_secret),
        tenant: tenant.clone(),
        own_mp_ids: Arc::new(own_mp_ids),
    };

    {
        use mako_markt::marktd_client::{MarktdClient, SubscriptionRequest};
        let marktd = MarktdClient::new(&cfg.marktd.url, marktd_api_key, ctx.http.clone());
        // Subscribe to the full configured event set — obsd needs
        // `process.initiated` (to create the projection + register the
        // deadline the sweep worker watches), not only `process.completed`.
        let event_types: Vec<&str> = cfg
            .subscription
            .event_types
            .iter()
            .map(String::as_str)
            .collect();
        marktd
            .put_subscription(
                &cfg.subscription.subscriber_id,
                &SubscriptionRequest {
                    webhook_url: &cfg.subscription.webhook_url,
                    webhook_secret: webhook_secret.as_ref().map(|s| {
                        let secret: &str = s.expose_secret();
                        secret
                    }),
                    event_types: &event_types,
                    makopid_filter: &[],
                    active: true,
                },
            )
            .await;
    }

    // ── Background: de.obs.* producers (only when an outbound target is set) ──
    if let Some(outbound_url) = outbound_url {
        let rt = crate::worker::WorkerRuntime {
            pool: worker_pool,
            client: Arc::new(ctx.http.clone()),
            outbound_url: Arc::new(outbound_url),
            outbound_secret: outbound_secret.map(|s| Arc::new(s.expose_secret().to_owned())),
            tenant: tenant.clone(),
            deadline_sweep_secs: cfg.worker.deadline_sweep_secs,
            deadline_warn_hours: cfg.worker.deadline_warn_hours,
            parity_sweep_secs: cfg.worker.parity_sweep_secs,
            parity_threshold_pp: cfg.worker.parity_threshold_pp,
            parity_window_days: cfg.worker.parity_window_days,
        };
        crate::worker::spawn_deadline_sweep(rt.clone(), ctx.shutdown.clone());
        crate::worker::spawn_parity_sweep(rt, ctx.shutdown.clone());
        tracing::info!("obsd: de.obs.* sweep producers started");
    } else {
        tracing::warn!(
            "obsd: webhook.outbound_url not set — de.obs.deadline.approaching and \
             de.obs.stp.parity.alert producers are disabled"
        );
    }

    Ok(router(state)
        .layer(Extension(cedar))
        .layer(Extension(oidc))
        .merge(crate::mcp_server::router(mcp_state, ctx.shutdown.clone())))
}

// ── § 7a Abs. 5 EnWG Gleichbehandlungsbericht evidence ───────────────────────

/// Per-PID parity counts for one **calendar year**, bucketed by `started_at`.
///
/// **The bucket is `started_at`, not `updated_at`.** Grouping by `updated_at`
/// moved rows between report years as later events touched them, so re-running
/// last year's filing produced different numbers — the one property an annual
/// regulatory filing must not have. A process that started on 30 December and
/// completed in January belongs to the year it started, in every re-run.
///
/// The `state` literals are the ones `ProcessState::as_str` writes — lowercase.
/// They were `'Completed'` / `'Rejected'` here, which matches nothing, so every
/// PID reported perfect parity and unequal treatment was invisible in the
/// artefact the filing rests on. `report_state_literals_match_projection` pins
/// them.
const GLEICHBEHANDLUNG_SQL: &str = r"SELECT
      pid::int,
      initiator_is_affiliate,
      COUNT(*)                                      AS total,
      COUNT(*) FILTER (WHERE state = 'completed')   AS completed,
      COUNT(*) FILTER (WHERE state = 'rejected')    AS rejected,
      COUNT(*) FILTER (
          WHERE deadline_at IS NOT NULL
            AND deadline_at < COALESCE(completed_at, now())
      )                                             AS frist_breached
  FROM process_projections
  WHERE EXTRACT(YEAR FROM started_at)::int = $1
    AND tenant = $2
  GROUP BY pid, initiator_is_affiliate
  ORDER BY pid, initiator_is_affiliate";

/// Query parameters for `GET /api/v1/audit/gleichbehandlung`.
#[derive(Debug, Deserialize)]
struct GleichbehandlungQuery {
    /// Calendar year to report (default: current year).
    year: Option<i32>,
    /// Output format: `json` (default) or `csv`.
    format: Option<String>,
}

/// `GET /api/v1/audit/gleichbehandlung?year=YYYY[&format=csv|json]`
///
/// Per-PID evidence for the **Gleichbehandlungsbericht** a vertically
/// integrated undertaking's Gleichbehandlungsbeauftragte files with the
/// Bundesnetzagentur by **31 March** for the preceding calendar year
/// (§ 7a Abs. 5 EnWG), and publishes in non-personalised form.
/// Lieferantenwechsel is one of the areas those reports examine.
///
/// It compares how the operator's network arm treated Lieferanten inside its
/// own undertaking against third-party Lieferanten, over the processes the
/// network arm actually answers (`handler::is_nb_answered_lieferanten_process`).
/// The underlying duties are § 6a EnWG (informatorische Entflechtung) and
/// § 20 Abs. 1 Satz 1 EnWG (diskriminierungsfreier Netzzugang).
///
/// **`gap_pp` is `affiliate − third_party`, in percentage points.** A positive
/// gap means the affiliate fared better, which is the concern. This convention
/// is `ParityComparison`'s and is shared with the `de.obs.stp.parity.alert`
/// CloudEvent and the MCP tool: three surfaces read one sign, because a sign
/// computed per surface is a sign that can disagree about which side was
/// favoured.
///
/// **No threshold is asserted.** The Bundesnetzagentur publishes no numeric
/// parity limit for this figure; what counts as a gap worth explaining is the
/// operator's judgement, and the escalation threshold lives in
/// `[worker] parity_threshold_pp`. A gap over a group smaller than
/// `PARITY_MIN_SAMPLE` is reported as `null`, not as a large number.
async fn get_gleichbehandlung_report(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    State(state): State<HandlerState>,
    Query(params): Query<GleichbehandlungQuery>,
) -> impl IntoResponse {
    use mako_obs::domain::{ParityComparison, ParityGroup};

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

    let rows: Vec<(i32, bool, i64, i64, i64, i64)> =
        match sqlx::query_as::<_, (i32, bool, i64, i64, i64, i64)>(GLEICHBEHANDLUNG_SQL)
            .bind(year)
            .bind(&state.tenant)
            .fetch_all(state.repo.pool())
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(%e, year, "obsd: Gleichbehandlung report query failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

    // Collate into per-PID affiliate / third-party pairs.
    use std::collections::BTreeMap;
    let mut by_pid: BTreeMap<i32, (ParityGroup, ParityGroup)> = BTreeMap::new();
    for (pid, is_affiliate, total, completed, rejected, frist_breached) in rows {
        let entry = by_pid.entry(pid).or_default();
        let group = if is_affiliate {
            &mut entry.0
        } else {
            &mut entry.1
        };
        group.total += total;
        group.completed += completed;
        group.rejected += rejected;
        group.frist_breached += frist_breached;
    }

    let comparisons: Vec<(i32, ParityComparison)> = by_pid
        .into_iter()
        .map(|(pid, (affiliate, third_party))| (pid, ParityComparison::new(affiliate, third_party)))
        .collect();

    if format == "csv" {
        let mut csv = String::from(
            "pid,affiliate_total,affiliate_completed,affiliate_rejected,affiliate_frist_breached,             affiliate_completion_rate,third_party_total,third_party_completed,             third_party_rejected,third_party_frist_breached,third_party_completion_rate,gap_pp
",
        );
        for (pid, c) in &comparisons {
            // An unstatable rate is an empty CSV cell, never a 0 that reads as a
            // measured "completed none of them".
            let rate = |g: &ParityGroup| {
                g.completion_rate()
                    .map_or_else(String::new, |r| format!("{r:.4}"))
            };
            let gap = c.gap_pp.map_or_else(String::new, |g| format!("{g:.1}"));
            csv.push_str(&format!(
                "{pid},{},{},{},{},{},{},{},{},{},{},{gap}
",
                c.affiliate.total,
                c.affiliate.completed,
                c.affiliate.rejected,
                c.affiliate.frist_breached,
                rate(&c.affiliate),
                c.third_party.total,
                c.third_party.completed,
                c.third_party.rejected,
                c.third_party.frist_breached,
                rate(&c.third_party),
            ));
        }
        return (
            StatusCode::OK,
            [("content-type", "text/csv; charset=utf-8")],
            csv,
        )
            .into_response();
    }

    let by_pid_json: Vec<serde_json::Value> = comparisons
        .iter()
        .map(|(pid, c)| {
            serde_json::json!({
                "pid": pid,
                "process": mako_fristen::antwort::antwort_obligation(pid.unsigned_abs())
                    .map(|o| o.name),
                "affiliate": c.affiliate,
                "third_party": c.third_party,
                "gap_pp": c.gap_pp,
                "favours": c.favours(),
            })
        })
        .collect();

    Json(serde_json::json!({
        "year": year,
        "tenant": state.tenant,
        "generated_at": OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        "basis": "§ 7a Abs. 5 EnWG — Gleichbehandlungsbericht, filed by 31 March for the \
                  preceding calendar year; duties under § 6a EnWG (informatorische \
                  Entflechtung) and § 20 Abs. 1 Satz 1 EnWG (diskriminierungsfreier Netzzugang)",
        "gap_convention": "gap_pp = (affiliate completion rate − third-party completion rate) \
                           × 100. Positive means the affiliate fared better. `null` when either \
                           group is below the minimum sample size — an unstatable gap, not a \
                           zero one.",
        "min_sample": mako_obs::domain::PARITY_MIN_SAMPLE,
        "threshold": "none published. The Bundesnetzagentur sets no numeric parity limit for \
                      this figure; the operator's escalation threshold is [worker] \
                      parity_threshold_pp and is an internal policy, not a regulatory one.",
        "by_pid": by_pid_json,
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::{GLEICHBEHANDLUNG_SQL, TERMINAL_STATE_SQL};
    use mako_obs::domain::ProcessState;

    /// The Gleichbehandlungsbericht must count the states the projection
    /// actually writes. Casing drift here is silent: every PID reports
    /// completed=0/rejected=0 and therefore perfect parity — in the artefact
    /// the § 7a Abs. 5 filing rests on.
    #[test]
    fn report_state_literals_match_projection() {
        for s in [ProcessState::Completed, ProcessState::Rejected] {
            assert!(
                GLEICHBEHANDLUNG_SQL.contains(&format!("state = '{}'", s.as_str())),
                "the report does not filter on the stored literal for {s:?}"
            );
        }
        assert!(
            !GLEICHBEHANDLUNG_SQL.contains("'Completed'")
                && !GLEICHBEHANDLUNG_SQL.contains("'Rejected'"),
            "the report still carries the PascalCase literals"
        );
    }

    /// An annual filing must reproduce when re-run.
    ///
    /// Bucketing by `updated_at` migrated rows between report years every time a
    /// later event touched them, so last year's numbers changed after it was
    /// filed. `started_at` is fixed once the process begins.
    #[test]
    fn the_report_year_is_the_year_the_process_started() {
        assert!(
            GLEICHBEHANDLUNG_SQL.contains("EXTRACT(YEAR FROM started_at)"),
            "the report year must be the process's own, not the row's last touch"
        );
        assert!(
            !GLEICHBEHANDLUNG_SQL.contains("updated_at"),
            "a filing bucketed on updated_at does not reproduce"
        );
    }

    /// The metrics gauges count non-terminal rows; the literal list they
    /// exclude is the projection's terminal set.
    #[test]
    fn open_process_gauge_excludes_exactly_the_terminal_states() {
        for s in ProcessState::ALL {
            assert_eq!(
                TERMINAL_STATE_SQL.contains(&format!("'{}'", s.as_str())),
                s.is_terminal(),
                "{s:?} is on the wrong side of the open-process gauge filter"
            );
        }
    }
}
