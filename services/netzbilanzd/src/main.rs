//! `netzbilanzd` — the Netzbetreiber's outbound billing daemon.
//!
//! Port: `:8680`
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |---|---|---|
//! | `POST` | `/api/v1/billing/run` | Settle, check and store invoices for a period |
//! | `GET`  | `/api/v1/billing/drafts` | List invoices — filter by Sparte, page by cursor |
//! | `GET`  | `/api/v1/billing/drafts/{id}` | One invoice in full |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/dispatch` | Merge Fremdkosten, re-check, send via `makod` |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/reject` | Discard a draft and reopen the period |
//! | `POST` | `/api/v1/billing/drafts/dispatch-batch` | Dispatch many, each in its own transaction |
//! | `POST` | `/api/v1/billing/drafts/{id}/storno` | Stornorechnung — recomputed and negated |
//! | `POST` | `/api/v1/billing/drafts/{id}/korrektur` | Korrekturrechnung from corrected inputs |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-paid` | REMADV 33001 |
//! | `PUT`  | `/api/v1/billing/drafts/{id}/mark-disputed` | REMADV 33002/33003/33004 |
//! | `POST` | `/api/v1/billing/mmm-run/{malo_id}` | Monthly MMM saldo, measured half from `edmd` |
//! | `POST` | `/api/v1/billing/ggv-nne/{ggv_malo_id}` | §42b EnWG per-tenant NNE |
//! | `GET`  | `/api/v1/billing/malo/{malo_id}` | Billing history for one MaLo |
//! | `GET`  | `/api/v1/billing/summary` | Monthly totals by PID, Sparte, status, Rechnungsart |
//! | `GET`  | `/api/v1/billing/audit` | § 147 AO / § 14b UStG export |
//! | `GET/PUT` | `/api/v1/billing/fremdkosten/{draft_id}` | Typed external-cost pass-through |
//! | `POST` | `/api/v1/webhooks/remadv` | REMADV CloudEvent ingest |
//! | `GET/PUT` | `/api/v1/redispatch/kostenblatt/{activation_id}` | Redispatch 2.0 cost sheet |
//! | `POST` | `/api/v1/redispatch/kostenblatt/{activation_id}/compute` | Quantify from `edmd` Lastgang |
//! | `GET`  | `/api/v1/redispatch/kostenblatt` | List a month's records |
//! | `GET`  | `/api/v1/redispatch/kostenblatt/gaps/{year}/{month}` | Unquantified activations |
//! | `POST` | `/api/v1/redispatch/kostenblatt/submit/{year}/{month}` | Submit the month |
//! | `POST` | `/api/v1/redispatch/verguetung/{activation_id}/compute` | §13a Abs. 2 EnWG |
//! | `POST` | `/api/v1/redispatch/ausfallarbeit/*` | BilAReM Kap. 3 compute surface |
//! | `POST` | `/mcp` | Read-only MCP tooling |
//!
//! Liveness and readiness (`/health`, `/health/ready`) are mounted by
//! `mako_service::run`.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};
use mako_service::{Daemon, ServiceContext};
use netzbilanzd::{ausfallarbeit_api, autorun, config, handlers, kostenblatt, mcp_server};
use secrecy::SecretString;
use sqlx::PgPool;
use tracing::info;

pub use config::NetzbilanzConfig;

/// The daemon. `mako_service::run` owns the lifecycle — tracing, the tuned pool,
/// real DB-ping readiness, graceful shutdown; this supplies the migrations, the
/// domain router and the background workers.
struct Netzbilanzd;

impl Daemon for Netzbilanzd {
    type Config = NetzbilanzConfig;
    const NAME: &'static str = "netzbilanzd";

    async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run netzbilanzd migrations")?;
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        Ok(())
    }

    async fn build(cfg: Arc<NetzbilanzConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let pool = ctx.pool().clone();
        let shutdown = ctx.shutdown.clone();
        let http = Arc::new(ctx.http.clone());

        let makod = Arc::new(MakodClient::new(
            &cfg.makod_url,
            SecretString::from(cfg.makod_api_key.clone()),
        ));
        let marktd = Arc::new(MarktdClient::new(
            &cfg.marktd_url,
            SecretString::from(cfg.marktd_api_key.clone()),
            (*http).clone(),
        ));

        let mcp_state = Arc::new(mcp_server::NetzbilanzMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        });

        spawn_workers(&pool, &cfg, &shutdown);

        let app = Router::new()
            .nest("/api/v1/billing", billing_routes())
            .nest("/api/v1/redispatch", redispatch_routes())
            .route(
                "/api/v1/webhooks/remadv",
                post(handlers::post_remadv_webhook),
            )
            .merge(mcp_server::router(mcp_state, shutdown))
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(makod))
            .layer(Extension(marktd))
            .layer(Extension(http))
            .layer(Extension(pool));

        info!(tenant = %cfg.tenant, "netzbilanzd router built");
        Ok(app)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Netzbilanzd>().await
}

fn billing_routes() -> Router {
    Router::new()
        .route("/run", post(handlers::run_billing))
        .route("/drafts", get(handlers::list_drafts))
        // Registered before `/drafts/{id}` so the literal segment is not
        // captured as a UUID path parameter.
        .route(
            "/drafts/dispatch-batch",
            post(handlers::post_dispatch_batch),
        )
        .route("/drafts/{id}", get(handlers::get_draft))
        .route("/drafts/{id}/dispatch", put(handlers::dispatch_draft))
        .route("/drafts/{id}/reject", put(handlers::reject_draft))
        .route("/drafts/{id}/storno", post(handlers::post_storno))
        .route("/drafts/{id}/korrektur", post(handlers::post_korrektur))
        .route("/drafts/{id}/mark-paid", put(handlers::mark_paid))
        .route("/drafts/{id}/mark-disputed", put(handlers::mark_disputed))
        .route("/mmm-run/{malo_id}", post(autorun::post_mmm_run))
        .route("/ggv-nne/{ggv_malo_id}", post(autorun::post_ggv_nne))
        .route("/malo/{malo_id}", get(handlers::get_malo_billing_history))
        .route("/summary", get(handlers::get_billing_summary))
        .route("/audit", get(handlers::get_billing_audit))
        .route(
            "/fremdkosten/{draft_id}",
            put(handlers::put_fremdkosten).get(handlers::get_fremdkosten),
        )
}

fn redispatch_routes() -> Router {
    Router::new()
        .route("/kostenblatt", get(kostenblatt::list_kostenblatt))
        // Both literals precede `/kostenblatt/{activation_id}`.
        .route(
            "/kostenblatt/gaps/{year}/{month}",
            get(kostenblatt::get_kostenblatt_gaps),
        )
        .route(
            "/kostenblatt/submit/{year}/{month}",
            post(kostenblatt::post_submit_kostenblatt),
        )
        .route(
            "/kostenblatt/{activation_id}",
            put(kostenblatt::put_kostenblatt).get(kostenblatt::get_kostenblatt),
        )
        .route(
            "/kostenblatt/{activation_id}/compute",
            post(kostenblatt::post_compute),
        )
        // §13a Abs. 2 EnWG — the compensation owed to a curtailed operator.
        .route(
            "/verguetung/{activation_id}/compute",
            post(kostenblatt::post_verguetung),
        )
        // BilAReM Kap. 3 (BK6-23-241) — the stateless Ausfallarbeit engine.
        .route(
            "/ausfallarbeit/compute",
            post(ausfallarbeit_api::post_ausfallarbeit_compute),
        )
        .route(
            "/ausfallarbeit/ueberbauung",
            post(ausfallarbeit_api::post_ausfallarbeit_ueberbauung),
        )
        .route(
            "/ausfallarbeit/kf-bin",
            post(ausfallarbeit_api::post_ausfallarbeit_kf_bin),
        )
        .route(
            "/ausfallarbeit/malo-split",
            post(ausfallarbeit_api::post_ausfallarbeit_malo_split),
        )
}

/// Start the outbox drain and the two alert workers.
///
/// All three are gated on `erp_webhook_url`: without a delivery target the
/// outbox would grow unbounded and the alerts would have nowhere to go. The
/// alerts enqueue on that same outbox rather than posting for themselves, so
/// every `de.netzbilanz.*` event takes one path with one retry policy.
fn spawn_workers(
    pool: &PgPool,
    cfg: &Arc<NetzbilanzConfig>,
    shutdown: &tokio_util::sync::CancellationToken,
) {
    let Some(url) = cfg.erp_webhook_url.clone() else {
        info!("netzbilanzd: no erp_webhook_url — outbox and alert workers not started");
        return;
    };

    let worker = mako_service::outbox::OutboxWorker::new(
        pool.clone(),
        url,
        cfg.erp_webhook_secret.clone().map(Into::into),
    );
    tokio::spawn(worker.run(shutdown.clone()));
    info!("netzbilanzd: transactional outbox worker started");

    let stale_hours = cfg.dispatch_stale_hours.unwrap_or(48);
    spawn_ticker(
        cfg.dispatch_alert_interval_secs.unwrap_or(3_600),
        "undispatched-draft alert",
        shutdown.clone(),
        {
            let (pool, cfg) = (pool.clone(), Arc::clone(cfg));
            move || {
                let (pool, cfg) = (pool.clone(), Arc::clone(&cfg));
                async move {
                    autorun::dispatch_overdue_alert(&pool, &cfg, stale_hours).await;
                }
            }
        },
    );

    spawn_ticker(
        cfg.kostenblatt_alert_interval_secs.unwrap_or(86_400),
        "Kostenblatt 15th-of-month alert",
        shutdown.clone(),
        {
            let (pool, cfg) = (pool.clone(), Arc::clone(cfg));
            move || {
                let (pool, cfg) = (pool.clone(), Arc::clone(&cfg));
                async move {
                    autorun::kostenblatt_deadline_alert(&pool, &cfg).await;
                }
            }
        },
    );
}

/// Run `tick` every `interval_secs`, until shutdown. `0` disables the worker.
///
/// The loop selects on the shutdown token as well as the ticker, so a daily
/// worker does not hold the process open until its next tick.
fn spawn_ticker<F, Fut>(
    interval_secs: u64,
    name: &'static str,
    shutdown: tokio_util::sync::CancellationToken,
    tick: F,
) where
    F: Fn() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send,
{
    if interval_secs == 0 {
        info!(name, "netzbilanzd: worker disabled by configuration");
        return;
    }
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        ticker.tick().await; // the first tick is immediate; skip it
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    info!(name, "netzbilanzd: worker stopping");
                    return;
                }
                _ = ticker.tick() => tick().await,
            }
        }
    });
    info!(name, interval_secs, "netzbilanzd: worker started");
}
