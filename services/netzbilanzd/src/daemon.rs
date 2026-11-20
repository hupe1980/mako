//! The [`Netzbilanzd`] daemon: what `mako_service::run` starts — the
//! migrations, the domain router and the background workers.

use std::sync::Arc;

use crate::{ausfallarbeit_api, autorun, config, handlers, kostenblatt, mabis, mcp_server};
use anyhow::Context as _;
use axum::{Extension, Router};
use mako_markt::{makod_client::MakodClient, marktd_client::MarktdClient};
use mako_service::subscriptions::Subscriptions;
use mako_service::{Daemon, ServiceContext};
use secrecy::SecretString;
use sqlx::PgPool;
use tracing::info;
use utoipa::OpenApi as _;
use utoipa_axum::{router::OpenApiRouter, routes};

use config::NetzbilanzConfig;

/// The daemon. `mako_service::run` owns the lifecycle — tracing, the tuned pool,
/// real DB-ping readiness, graceful shutdown; this supplies the migrations, the
/// domain router and the background workers.
pub struct Netzbilanzd;

impl Daemon for Netzbilanzd {
    type Config = NetzbilanzConfig;
    const NAME: &'static str = "netzbilanzd";

    async fn migrate(_cfg: &Self::Config, pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run netzbilanzd migrations")?;
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure outbox schema")?;
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")?;
        Ok(())
    }

    /// Fail closed before anything is bound. Every route moves money or
    /// discloses the § 147 AO record, and the REMADV webhook closes a
    /// receivable on an inbound event, so an unauthenticated deployment is a
    /// posture the operator has to ask for by name. An unimplemented submission
    /// target and a Bilanzierungsgebiet that is not one are refused while the
    /// window is still open, and so is a missing or too-short Zahlungsziel.
    fn validate(cfg: &NetzbilanzConfig) -> Vec<mako_service::Befund> {
        let mut out = cfg.mabis_befunde();
        out.extend(cfg.zahlungsziel_befunde());
        if let Err(e) = cfg.check_auth_posture() {
            out.push(
                mako_service::Befund::aus_fehler("oidc", e)
                    .bereich(mako_service::befund::Bereich::Zugriff),
            );
        }
        out.extend(cfg.subscriptions.befunde());
        out
    }

    async fn build(cfg: Arc<NetzbilanzConfig>, ctx: ServiceContext) -> anyhow::Result<Router> {
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

        // ── Webhook subscriptions on the outbox ──────────────────────────────
        // Every CloudEvent netzbilanzd commits fans out to the endpoints an
        // operator registers through `/api/v1/webhooks/endpoints`, and to the
        // `default` endpoint `erp_webhook_url` names.
        let subs = Subscriptions::new(
            pool.clone(),
            Self::NAME,
            &cfg.tenant,
            cfg.subscriptions.clone(),
        )
        .with_default_endpoint(
            cfg.erp_webhook_url.clone(),
            cfg.erp_webhook_secret.clone().map(SecretString::from),
        );
        subs.spawn(shutdown.clone());

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
            policy: ctx.einrichtung.policy.clone(),
            subscriptions: subs.clone(),
        });

        spawn_workers(&pool, &cfg, &shutdown);

        // The MaBiS routes are documented always and served with `[mabis]`.
        let (api, doc) = api(&subs).split_for_parts();
        let mut app = Router::new().merge(api);
        if let Some(state) = mabis_state(&pool, &cfg, &shutdown) {
            app = app.merge(mabis_api().split_for_parts().0.with_state(state));
        }
        let app = app
            .merge(mako_service::openapi::serve(openapi_document(doc)))
            // Read-only: GET /admin/einrichtung (Cedar `read-config`) and
            // GET /api/v1/policy (Cedar `read-policy`), from the boot report.
            .merge(mako_service::einrichtung::router(ctx.einrichtung.clone()))
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

/// Every `/api/v1` route netzbilanzd serves outside MaBiS, each with its
/// document entry, and the subscription routes of `subs`.
pub fn api(subs: &Subscriptions) -> OpenApiRouter {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        // ── Netznutzungs- and MMM-Abrechnung ──────────────────────────────────
        .routes(routes!(handlers::run_billing))
        .routes(routes!(handlers::list_drafts))
        .routes(routes!(handlers::post_dispatch_batch))
        .routes(routes!(handlers::get_draft))
        .routes(routes!(handlers::dispatch_draft))
        .routes(routes!(handlers::reject_draft))
        .routes(routes!(handlers::post_storno))
        .routes(routes!(handlers::post_korrektur))
        .routes(routes!(handlers::mark_paid))
        .routes(routes!(handlers::mark_disputed))
        .routes(routes!(autorun::post_mmm_run))
        .routes(routes!(autorun::post_ggv_nne))
        .routes(routes!(handlers::get_malo_billing_history))
        .routes(routes!(handlers::get_billing_summary))
        .routes(routes!(handlers::get_billing_audit))
        .routes(routes!(
            handlers::put_fremdkosten,
            handlers::get_fremdkosten
        ))
        // ── Redispatch 2.0: Kostenblatt, Vergütung, Ausfallarbeit ─────────────
        .routes(routes!(kostenblatt::list_kostenblatt))
        .routes(routes!(kostenblatt::get_kostenblatt_gaps))
        .routes(routes!(kostenblatt::post_submit_kostenblatt))
        .routes(routes!(
            kostenblatt::put_kostenblatt,
            kostenblatt::get_kostenblatt
        ))
        .routes(routes!(kostenblatt::post_compute))
        .routes(routes!(kostenblatt::post_verguetung))
        .routes(routes!(ausfallarbeit_api::post_ausfallarbeit_compute))
        .routes(routes!(ausfallarbeit_api::post_ausfallarbeit_ueberbauung))
        .routes(routes!(ausfallarbeit_api::post_ausfallarbeit_kf_bin))
        .routes(routes!(ausfallarbeit_api::post_ausfallarbeit_malo_split))
        .routes(routes!(ausfallarbeit_api::post_ausfallarbeit_vergleichstag))
        .routes(routes!(
            ausfallarbeit_api::post_ausfallarbeit_vergleichszeitraum
        ))
        // ── The REMADV webhook (HMAC, no bearer) ──────────────────────────────
        .routes(routes!(handlers::post_remadv_webhook))
        .merge(subs.router())
}

/// The MaBiS Summenzeitreihe routes, served only with `[mabis]`.
pub fn mabis_api() -> OpenApiRouter<mabis::server::ServerState> {
    OpenApiRouter::new()
        .routes(routes!(mabis::server::trigger_sync))
        .routes(routes!(mabis::server::list_runs))
        .routes(routes!(mabis::server::get_run))
        .routes(routes!(mabis::server::retry_run))
        .routes(routes!(mabis::server::post_datenstatus))
        .routes(routes!(mabis::server::post_pruefmitteilung))
        .routes(routes!(mabis::server::list_korrekturbedarf))
}

/// The document netzbilanzd serves: `doc` (from [`api`]) with the MaBiS
/// paths, which it documents whether or not `[mabis]` mounts them.
#[must_use]
pub fn openapi_document(mut doc: utoipa::openapi::OpenApi) -> utoipa::openapi::OpenApi {
    doc.merge(mabis_api().split_for_parts().1);
    doc
}

/// The document's header; the paths come from [`api`] and [`mabis_api`].
#[derive(utoipa::OpenApi)]
#[openapi(
    info(
        title = "netzbilanzd — Netzbilanzierung and Netzabrechnung",
        description = "The Netzbetreiber's invoicing (Netznutzung, MMM, GGV), Redispatch 2.0 Kostenblätter and Ausfallarbeit, MaBiS Summenzeitreihen (served with `[mabis]`), the inbound REMADV webhook, and webhook subscriptions on netzbilanzd's events."
    ),
    tags(
        (name = "billing", description = "INVOIC drafts, dispatch, Storno and Korrektur"),
        (name = "redispatch", description = "Redispatch 2.0 Kostenblatt, Vergütung and Ausfallarbeit"),
        (name = "mabis", description = "MaBiS Summenzeitreihen — served only with `[mabis]`"),
        (name = "webhooks-inbound", description = "Signed CloudEvents netzbilanzd receives"),
        (name = "webhooks", description = "Webhook endpoints, deliveries and the event feed"),
    )
)]
pub struct ApiDoc;

/// Start the two alert workers.
///
/// The alerts enqueue on the outbox rather than posting for themselves, so
/// every `de.mako.netzbilanz.*` event takes one path — the subscription fan-out —
/// with one retry policy.
fn spawn_workers(
    pool: &PgPool,
    cfg: &Arc<NetzbilanzConfig>,
    shutdown: &tokio_util::sync::CancellationToken,
) {
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
