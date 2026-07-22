//! `netzbilanzd` — NNE/KA/MMM billing daemon.
//!
//! Generates INVOIC 31001/31002/31005/31009/31011 invoices (NB → LF/MSB/LFG) from
//! meter readings and tariff data.  Integrates with `marktd` (tariffs) and `edmd`
//! (billing data).
//!
//! Port: `:8680`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/run` | Generate invoice drafts for a billing period |
//! | `GET`  | `/api/v1/billing/drafts` | List drafts (`?status=&malo_id=&limit=`) |
//! | `GET`  | `/api/v1/billing/drafts/{id}` | Fetch single draft with Rechnung JSONB |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/dispatch` | Validate + dispatch via `makod` |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/reject` | Reject with reason |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-paid` | REMADV 33001: payment confirmed |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-disputed` | REMADV 33002: dispute received |
//! | `POST` | `/api/v1/billing/mmm-run/{malo_id}` | MMM auto-run (edmd auto-fetch) |
//! | `POST` | `/api/v1/billing/ggv-nne/{ggv_malo_id}` | §42a GGV NNE NB-side billing |
//! | `POST` | `/api/v1/billing/drafts/dispatch-batch` | Batch dispatch |
//! | `POST` | `/api/v1/billing/drafts/{id}/correction` | Stornorechnung / Korrekturrechnung |
//! | `GET`  | `/api/v1/billing/malo/{malo_id}` | Billing history per MaLo (lightweight) |
//! | `GET`  | `/api/v1/billing/summary` | Monthly billing totals by PID and status |
//! | `GET`  | `/api/v1/billing/audit` | § 147 AO / GoBD BNetzA audit export |
//! | `POST` | `/api/v1/webhooks/remadv` | REMADV CloudEvent ingest (status update) |
//! | `GET/PUT` | `/api/v1/redispatch/kostenblatt/{activation_id}` | Redispatch 2.0 Kostenblatt |
//! | `POST` | `/api/v1/redispatch/kostenblatt/{activation_id}/compute` | Auto-compute from edmd Lastgang (15-min sum) |
//! | `GET`  | `/api/v1/redispatch/kostenblatt/gaps/{year}/{month}` | Activations without dispatch_kwh data |
//! | `POST` | `/api/v1/redispatch/kostenblatt/submit/{year}/{month}` | Submit pending |
//! | `GET/PUT` | `/api/v1/billing/fremdkosten/{draft_id}` | Fremdkosten (§ 147 AO / GoBD) |
//! | `GET`  | `/health` | Liveness check |
//! | `GET`  | `/health/ready` | Readiness check |
//! | `POST` | `/mcp` | MCP server (13 tools, 6 prompts) — NB billing AI tooling |

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_markt::makod_client::MakodClient;
use mako_markt::marktd_client::MarktdClient;
use mako_service::{health::health_routes, load_config};
use netzbilanzd::{config, handlers, mcp_server, pg};
use secrecy::SecretString;
use sqlx::PgPool;
use std::sync::Arc;
// CancellationToken is used via mako_service::shutdown
use tracing::info;

pub use config::NetzbilanzConfig;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _guard = mako_service::init_tracing_from_env("netzbilanzd");

    let cfg: NetzbilanzConfig = load_config("netzbilanzd").context("load config")?;
    let cfg = Arc::new(cfg);

    let pool = PgPool::connect(&cfg.database_url)
        .await
        .context("connect PostgreSQL")?;

    // Run migrations.
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("run migrations")?;

    let makod = Arc::new(MakodClient::new(
        &cfg.makod_url,
        SecretString::from(cfg.makod_api_key.clone()),
    ));

    // Shared HTTP client with timeout — used by all handlers that call edmd/marktd REST APIs.
    // Do NOT create per-request clients; that wastes connection pool slots.
    let http_client: Arc<reqwest::Client> = Arc::new(
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build HTTP client")?,
    );

    let marktd = Arc::new(MarktdClient::new(
        &cfg.marktd_url,
        secrecy::SecretString::from(cfg.marktd_api_key.clone()),
        (*http_client).clone(),
    ));

    // MCP server setup.
    let shutdown = mako_service::shutdown::token();
    let mcp_state = Arc::new(mcp_server::NetzbilanzMcpState {
        pool: pool.clone(),
        tenant: cfg.tenant.clone(),
        auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
    });

    // ── Background workers ────────────────────────────────────────────────────
    //
    // Worker 1: Undispatched draft alert.
    //   Emits `de.netzbilanz.invoic.dispatch_overdue` CloudEvent for each draft
    //   older than 48 h that is still in 'draft' status.  Runs hourly by default.
    //
    // Worker 2: Kostenblatt 15th-of-month deadline alert.
    //   Emits `de.netzbilanz.kostenblatt.deadline_approaching` when the 15th is
    //   ≤5 days away and pending Kostenblatt records exist.  Runs daily by default.

    if cfg.erp_webhook_url.is_some() {
        let alert_pool = pool.clone();
        let alert_cfg = Arc::clone(&cfg);
        let alert_client = Arc::clone(&http_client);
        let interval_secs = cfg.dispatch_alert_interval_secs.unwrap_or(3_600);

        if interval_secs > 0 {
            tokio::spawn(async move {
                let mut ticker =
                    tokio::time::interval(std::time::Duration::from_secs(interval_secs));
                ticker.tick().await; // skip first immediate tick
                loop {
                    ticker.tick().await;
                    spawn_dispatch_alert(&alert_pool, &alert_cfg, &alert_client).await;
                }
            });
            info!(
                interval_secs,
                "netzbilanzd: undispatched-draft alert worker started"
            );
        }

        let kb_pool = pool.clone();
        let kb_cfg = Arc::clone(&cfg);
        let kb_client = Arc::clone(&http_client);
        let kb_interval = cfg.kostenblatt_alert_interval_secs.unwrap_or(86_400);

        if kb_interval > 0 {
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(kb_interval));
                ticker.tick().await; // skip first immediate tick
                loop {
                    ticker.tick().await;
                    spawn_kostenblatt_alert(&kb_pool, &kb_cfg, &kb_client).await;
                }
            });
            info!(
                kb_interval,
                "netzbilanzd: Kostenblatt 15th deadline alert worker started"
            );
        }
    }

    let app = Router::new()
        .merge(health_routes(|| async { true }))
        .nest("/api/v1/billing", billing_routes())
        // N4: Kostenblatt REST API (Redispatch 2.0, BK6-20-061)
        .route(
            "/api/v1/redispatch/kostenblatt",
            get(handlers::list_kostenblatt_handler),
        )
        .route(
            "/api/v1/redispatch/kostenblatt/:activation_id",
            put(handlers::put_kostenblatt).get(handlers::get_kostenblatt),
        )
        // N5: Kostenblatt edmd auto-compute (BK6-20-061 §4.2)
        // Lastgang 15-min sum primary; billing-period fallback; manual override.
        .route(
            "/api/v1/redispatch/kostenblatt/:activation_id/compute",
            post(handlers::post_kostenblatt_compute),
        )
        // §13a Abs. 2 EnWG — angemessene Vergütung for one activation
        .route(
            "/api/v1/redispatch/verguetung/:activation_id/compute",
            post(handlers::post_verguetung_compute),
        )
        // N5a: Kostenblatt gap detection — activations without dispatch_kwh data.
        .route(
            "/api/v1/redispatch/kostenblatt/gaps/:year/:month",
            get(handlers::get_kostenblatt_gaps),
        )
        // Fremdkosten typed BO4E REST (§ 147 AO / GoBD external cost pass-through)
        .route(
            "/api/v1/billing/fremdkosten/:draft_id",
            put(handlers::put_fremdkosten).get(handlers::get_fremdkosten),
        )
        .route(
            "/api/v1/redispatch/kostenblatt/submit/:year/:month",
            post(handlers::post_submit_kostenblatt),
        )
        // REMADV CloudEvent ingest (status update webhook)
        .route(
            "/api/v1/webhooks/remadv",
            post(handlers::post_remadv_webhook),
        )
        // MCP server at /mcp — NB billing AI tooling
        .merge(mcp_server::router(mcp_state, shutdown.clone()))
        .layer(Extension(Arc::clone(&cfg)))
        .layer(Extension(makod))
        .layer(Extension(marktd))
        .layer(Extension(Arc::clone(&http_client)))
        .layer(Extension(pool));

    let addr = format!("0.0.0.0:{}", cfg.port.unwrap_or(8680));
    info!(%addr, tenant = %cfg.tenant, "netzbilanzd starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .context("bind TCP")?;
    mako_service::shutdown::serve(listener, app, shutdown).await
}

fn billing_routes() -> Router {
    Router::new()
        .route("/run", post(handlers::run_billing))
        // N6: MMM auto-run — auto-fetches profil_kwh from edmd (GPKE (BK6-24-174) Teil 1 Kap. 8.4)
        .route("/mmm-run/:malo_id", post(handlers::post_mmm_auto_run))
        // N8: §42a GGV NNE NB-side billing — N × NNE per tenant from Lokationszuordnung
        .route("/ggv-nne/:ggv_malo_id", post(handlers::post_ggv_nne))
        .route("/drafts", get(handlers::list_drafts))
        .route("/drafts/:id", get(handlers::get_draft))
        .route("/drafts/:id/dispatch", put(handlers::dispatch_draft))
        .route("/drafts/:id/reject", put(handlers::reject_draft))
        // REMADV payment lifecycle
        .route("/drafts/:id/mark-paid", put(handlers::mark_paid))
        .route("/drafts/:id/mark-disputed", put(handlers::mark_disputed))
        // Batch dispatch — dispatch all approved drafts at once
        .route(
            "/drafts/dispatch-batch",
            post(handlers::post_dispatch_batch),
        )
        // Korrekturrechnung / Stornorechnung (§ 147 AO / GoBD audit trail)
        .route(
            "/drafts/:id/correction",
            post(handlers::post_draft_correction),
        )
        // Billing history per MaLo (lightweight, no Rechnung JSONB)
        .route("/malo/:malo_id", get(handlers::get_malo_billing_history))
        // Monthly billing summary (REST equivalent of get_billing_summary MCP tool)
        .route("/summary", get(handlers::get_billing_summary_rest))
        // § 147 AO / GoBD BNetzA audit export
        .route("/audit", get(handlers::get_billing_audit))
}

// ── Background worker helpers ─────────────────────────────────────────────────

/// Emit `de.netzbilanz.invoic.dispatch_overdue` for drafts stuck in 'draft' > 48 h.
async fn spawn_dispatch_alert(
    pool: &PgPool,
    cfg: &NetzbilanzConfig,
    client: &Arc<reqwest::Client>,
) {
    let webhook_url = match &cfg.erp_webhook_url {
        Some(u) => u.clone(),
        None => return,
    };
    let rows = match pg::list_undispatched_stale(pool, &cfg.tenant, 48, 100).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "dispatch alert: DB query failed");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    tracing::warn!(
        count = rows.len(),
        "netzbilanzd: undispatched drafts > 48h, emitting alert"
    );
    let payload = serde_json::json!({
        "tenant": cfg.tenant,
        "undispatched_count": rows.len(),
        "draft_ids": rows.iter().map(|r| &r.id).collect::<Vec<_>>(),
        "hint": "Drafts are approaching Zahlungsziel. Dispatch or reject via PUT /api/v1/billing/drafts/{id}/dispatch.",
    });
    let body = serde_json::json!({
        "specversion": "1.0",
        "type": "de.netzbilanz.invoic.dispatch_overdue",
        "source": "netzbilanzd",
        "id": uuid::Uuid::new_v4().to_string(),
        "time": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
        "data": payload,
    });
    let _ = client
        .post(&webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&body)
        .send()
        .await;
}

/// Emit `de.netzbilanz.kostenblatt.deadline_approaching` when the 15th-of-month
/// Redispatch 2.0 Kostenblatt deadline is ≤5 days away and pending records exist.
async fn spawn_kostenblatt_alert(
    pool: &PgPool,
    cfg: &NetzbilanzConfig,
    client: &Arc<reqwest::Client>,
) {
    let webhook_url = match &cfg.erp_webhook_url {
        Some(u) => u.clone(),
        None => return,
    };
    let now = time::OffsetDateTime::now_utc();
    let day = now.day();
    // Alert window: day 10–14 of month (15th is the submission deadline)
    if !(10..=14).contains(&day) {
        return;
    }

    let days_left = 15u8.saturating_sub(day);
    // Check PREVIOUS month (deadline is for submissions of prior month's activations)
    let (year, month) = if now.month() as u8 > 1 {
        (now.year(), now.month() as u8 - 1)
    } else {
        (now.year() - 1, 12u8)
    };

    let pending = match pg::list_kostenblatt(
        pool,
        &cfg.tenant,
        year as i16,
        month as i16,
        Some("pending"),
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "kostenblatt alert: DB query failed");
            return;
        }
    };
    if pending.is_empty() {
        return;
    }

    tracing::warn!(
        count = pending.len(),
        year,
        month,
        days_left,
        "netzbilanzd: pending Kostenblatt — 15th deadline in {days_left} days"
    );
    let body = serde_json::json!({
        "specversion": "1.0",
        "type": "de.netzbilanz.kostenblatt.deadline_approaching",
        "source": "netzbilanzd",
        "id": uuid::Uuid::new_v4().to_string(),
        "time": time::OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
        "data": {
            "tenant": cfg.tenant,
            "period_year": year,
            "period_month": month,
            "pending_count": pending.len(),
            "days_until_deadline": days_left,
            "deadline": format!("{year}-{:02}-15", month),
            "action": format!("POST /api/v1/redispatch/kostenblatt/submit/{year}/{month}"),
        },
    });
    let _ = client
        .post(&webhook_url)
        .header("Content-Type", "application/cloudevents+json")
        .json(&body)
        .send()
        .await;
}
