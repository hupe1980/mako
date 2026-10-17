//! The [`Netzbilanzd`] daemon: what `mako_service::run` starts — the
//! migrations, the domain router and the background workers.

use std::sync::Arc;

use crate::{ausfallarbeit_api, autorun, config, handlers, kostenblatt, mabis, mcp_server};
use anyhow::Context as _;
use axum::{
    Extension, Router,
    routing::{get, post, put},
};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use sqlx::PgPool;
use tracing::info;

use config::NetzbilanzConfig;

/// The daemon. `mako_service::run` owns the lifecycle — tracing, the tuned pool,
/// real DB-ping readiness, graceful shutdown; this supplies the migrations, the
/// domain router and the background workers.
pub struct Netzbilanzd;

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
        // Fail closed before anything is bound. Every route below moves money
        // or discloses the § 147 AO record, and the REMADV webhook closes a
        // receivable on an inbound event, so an unauthenticated deployment is
        // a posture the operator has to ask for by name.
        cfg.check_auth_posture()?;
        // Refuses an unimplemented submission target and a Bilanzierungsgebiet
        // that is not one, while the window is still open.
        cfg.check_mabis()?;

        let pool = ctx.pool().clone();
        let shutdown = ctx.shutdown.clone();
        let http = Arc::new(ctx.http.clone());

        // ── OIDC/JWT authentication ──────────────────────────────────────────
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.tenant,
            shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        // ── Cedar ABAC ───────────────────────────────────────────────────────
        // Authentication says who is calling; this says what they may do. The
        // two are separate decisions: an auditor reads the § 147 AO export with
        // the same token shape an operator dispatches invoices with, and only
        // the policy tells those apart.
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/netzbilanzd.cedar"
            ))
            .context("netzbilanzd.cedar must parse at startup")?,
        );

        let makod = Arc::new(MakodClient::new(
            &cfg.makod_url,
            SecretString::from(cfg.makod_api_key.clone()),
        ));
        let marktd = Arc::new(MarktdClient::new(
            &cfg.marktd_url,
            SecretString::from(cfg.marktd_api_key.clone()),
            (*http).clone(),
        ));

        // The MCP tools read the whole invoice register, so they are gated by
        // the same verifier and the same policy as the REST surface: a JWT is
        // verified and checked for `use-mcp`, and a configured `[mcp]` key
        // stays accepted for agent clients that mint no OIDC token.
        let mcp_state = Arc::new(mcp_server::NetzbilanzMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc.clone(),
                Some(Arc::clone(&cedar)),
                &cfg.tenant,
            ),
        });

        spawn_workers(&pool, &cfg, &shutdown);

        let mut app = Router::new()
            .nest("/api/v1/billing", billing_routes())
            .nest("/api/v1/redispatch", redispatch_routes());
        if let Some(state) = mabis_state(&pool, &cfg, &shutdown) {
            app = app.nest("/api/v1/mabis", mabis_routes(state));
        }
        let app = app
            .route(
                "/api/v1/webhooks/remadv",
                post(handlers::post_remadv_webhook),
            )
            .merge(mcp_server::router(mcp_state, shutdown))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc))
            .layer(Extension(cedar))
            .layer(Extension(Arc::clone(&cfg)))
            .layer(Extension(makod))
            .layer(Extension(marktd))
            .layer(Extension(http))
            .layer(Extension(pool));

        info!(tenant = %cfg.tenant, "netzbilanzd router built");
        Ok(app)
    }
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

/// The MaBiS module's state, and its scheduler started — `None` without `[mabis]`.
///
/// The engine reads `edmd` with a longer request timeout than the shared
/// default, because one Lastgang fetch carries a month of quarter-hourly
/// values; `marktd` and `makod` keep the default.
fn mabis_state(
    pool: &PgPool,
    cfg: &Arc<NetzbilanzConfig>,
    shutdown: &tokio_util::sync::CancellationToken,
) -> Option<mabis::server::ServerState> {
    let section = cfg.mabis.clone()?;
    let settings = Arc::new(mabis::config::Settings {
        tenant: cfg.tenant.clone(),
        mabis: section,
    });
    let default = mako_service::http::default_client();
    let slow = mako_service::http::default_client_with(std::time::Duration::from_secs(120));
    // `check_mabis` refused a `[mabis]` without `edmd_url` before this runs.
    let edmd = cfg.edmd(slow)?;
    let upstream = |name: &'static str, url: &str, key: &str| {
        mako_service::http::Upstream::new(
            name,
            url,
            Some(SecretString::from(key.to_owned())),
            default.clone(),
        )
    };
    let engine = Arc::new(mabis::sync_engine::SyncEngine::new(
        pool.clone(),
        Arc::clone(&settings),
        edmd,
        upstream("marktd", &cfg.marktd_url, &cfg.marktd_api_key),
        upstream("makod", &cfg.makod_url, &cfg.makod_api_key),
    ));
    mabis::spawn_scheduler(
        Arc::clone(&engine),
        Arc::clone(&settings),
        pool.clone(),
        shutdown.clone(),
    );
    info!(
        bilanzierungsgebiet_id = %settings.mabis.bilanzierungsgebiet_id,
        "netzbilanzd: MaBiS Summenzeitreihe scheduler started"
    );
    Some(mabis::server::ServerState {
        pool: pool.clone(),
        engine,
        cfg: settings,
    })
}

fn mabis_routes(state: mabis::server::ServerState) -> Router {
    Router::new()
        .route("/sync", post(mabis::server::trigger_sync))
        .route("/runs", get(mabis::server::list_runs))
        .route("/runs/{id}", get(mabis::server::get_run))
        .route("/runs/{id}/retry", put(mabis::server::retry_run))
        .route("/datenstatus", post(mabis::server::post_datenstatus))
        .route(
            "/pruefmitteilung",
            post(mabis::server::post_pruefmitteilung),
        )
        .route("/korrekturbedarf", get(mabis::server::list_korrekturbedarf))
        .with_state(state)
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
        .route(
            "/ausfallarbeit/vergleichstag",
            post(ausfallarbeit_api::post_ausfallarbeit_vergleichstag),
        )
        .route(
            "/ausfallarbeit/vergleichszeitraum",
            post(ausfallarbeit_api::post_ausfallarbeit_vergleichszeitraum),
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
