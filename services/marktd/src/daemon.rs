//! The `marktd` daemon — the [`Daemon`] impl `mako_service::run` starts.
//!
//! `mako_service::run` owns the lifecycle — tracing, the tuned pool, migrations,
//! real DB-ping readiness, `/health/*`, `/metrics`, the HTTP trace layer, and
//! graceful SIGINT **and SIGTERM** shutdown. This module supplies only what is
//! marktd's own: the repositories, the domain router, and the two background
//! workers.
//!
//! The full endpoint surface is documented in the crate README and in
//! the published service documentation; `/api/v1/openapi.json` serves the
//! OpenAPI document live.

use std::sync::Arc;

use crate::state::AppState;
use anyhow::Context as _;
use axum::{Extension, Router, routing::post};
use mako_service::{Daemon, ServiceContext, cedar::CedarEnforcer};
use sqlx::PgPool;
use tracing::info;

use crate::{
    config::{self, Config},
    handlers::{
        Tenant,
        event_ingest::{InboundWebhookSecret, ingest_event},
    },
    pg,
};

/// The `marktd` daemon.
pub struct Marktd;

impl Daemon for Marktd {
    type Config = Config;
    const NAME: &'static str = "marktd";

    async fn migrate(_cfg: &Self::Config, pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run marktd migrations")?;
        // `event_outbox` carries marktd's events, fanned out by the
        // subscription module, and the commands owed to makod (the ORDERS
        // 17008 Abbestellung of an ended consent).
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")
    }

    /// Fail closed: without `[oidc]` every request is admitted with dev claims,
    /// and without `webhook.inbound_secret` the inbound events endpoint accepts
    /// unsigned events that mutate VersorgungsStatus and the device registry.
    /// Either gap must be asked for by name, never reached by omission; each is
    /// its own finding.
    fn validate(cfg: &Config) -> Vec<mako_service::Befund> {
        use mako_service::befund::{Befund, Bereich};
        let mut out = cfg.subscriptions.befunde();
        if cfg.allow_insecure_no_auth {
            return out;
        }
        if cfg.oidc.is_none() {
            out.push(
                Befund::blockiert(
                    "oidc",
                    "no [oidc] section configured — every request would be admitted with dev \
                     claims. Configure [oidc], or set allow_insecure_no_auth = true.",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        if cfg.webhook.inbound_secret.is_none() {
            out.push(
                Befund::blockiert(
                    "webhook.inbound_secret",
                    "no webhook.inbound_secret configured — the inbound events endpoint would \
                     accept unsigned events that mutate master data. Configure it, or set \
                     allow_insecure_no_auth = true.",
                )
                .bereich(Bereich::Zugriff),
            );
        }
        out
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        let pool = ctx.pool().clone();
        let shutdown = ctx.shutdown.clone();
        let http = ctx.http.clone();
        let tenant = cfg.markt.tenant.clone();

        // The auth posture is refused in `validate`, before `build` runs.
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "marktd: allow_insecure_no_auth is set — dev claims accepted and unsigned \
                 inbound events accepted"
            );
        }

        let makod_api_key =
            config::resolve_env_secret(&cfg.makod.api_key).context("resolve makod.api_key")?;
        let inbound_secret = cfg
            .webhook
            .inbound_secret
            .as_deref()
            .map(config::resolve_env)
            .transpose()
            .context("resolve webhook.inbound_secret")?;
        // `None` means "accept unsigned events", a posture
        // `allow_insecure_no_auth` has to admit by name. An *empty* secret is
        // neither posture: verification stays on, keyed with nothing, so every
        // signature fails and the sender sees 401 on every event.
        if inbound_secret
            .as_deref()
            .is_some_and(|s| s.trim().is_empty())
        {
            anyhow::bail!(
                "webhook.inbound_secret is set but empty. Leave it unset to accept \
                 unsigned events (together with allow_insecure_no_auth), or give it \
                 the same shared secret the sender signs with — an empty key \
                 verifies nothing and rejects every inbound event with 401."
            );
        }

        let verifier = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &http,
            &tenant,
            shutdown.clone(),
        )
        .await?;

        let cedar = Arc::new(
            CedarEnforcer::from_policy_str(include_str!("../policies/marktd.cedar"))
                .context("load Cedar policies from policies/marktd.cedar")?,
        );

        let makod_client = Arc::new(mako_markt::makod_client::MakodClient::new(
            &cfg.makod.base_url,
            makod_api_key.clone(),
        ));
        // The commands owed to makod, persisted with the business write that
        // owes them, drained with retry and a dead-letter budget.
        tokio::spawn(
            mako_service::outbox::OutboxWorker::commands(
                pool.clone(),
                crate::consent_lifecycle::MAKOD_UPSTREAM,
                cfg.makod.base_url.clone(),
                Some(makod_api_key),
            )
            .run(shutdown.clone()),
        );

        let state = Arc::new(AppState {
            malo_repo: pg::PgMaloRepository::new(pool.clone()),
            melo_repo: pg::PgMeloRepository::new(pool.clone(), tenant.clone()),
            correlation_index: pg::PgCorrelationIndex::new(pool.clone()),
            partner_repo: pg::PgPartnerRepository::new(pool.clone()),
            tenant: tenant.clone(),
        });

        // Every event is enqueued to `event_outbox` with its business write
        // (`crate::outbox::enqueue`); the module fans it out to the in-cluster
        // receivers of `[[subscriptions.endpoints]]` and to every endpoint an
        // integrator registers. marktd names no ERP URL of its own.
        let subs = mako_service::subscriptions::Subscriptions::new(
            pool.clone(),
            Self::NAME,
            tenant.clone(),
            cfg.subscriptions.clone(),
        );
        subs.spawn(shutdown.clone());

        spawn_workers(&cfg, &pool, &http, &shutdown, &tenant);

        let mcp = crate::mcp_server::router(
            Arc::new(crate::mcp_server::MdmdMcpState {
                pool: pool.clone(),
                tenant: tenant.clone(),
                auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                    &cfg.mcp,
                    verifier.clone(),
                    Some(Arc::clone(&cedar)),
                    &tenant,
                ),
                policy: ctx.einrichtung.policy.clone(),
                subscriptions: subs.clone(),
            }),
            shutdown,
        );

        let (documented, doc) = crate::openapi::documented(&subs);
        let app = Router::new()
            .merge(documented)
            .route(&cfg.webhook.inbound_path, post(ingest_event))
            .merge(mako_service::openapi::serve(doc))
            .with_state(state)
            // Read-only: GET /admin/einrichtung (Cedar `read-config`) and
            // GET /api/v1/policy (Cedar `read-policy`), from the boot report.
            .merge(mako_service::einrichtung::router(ctx.einrichtung.clone()))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.markt.tenant.clone(),
            )))
            .layer(Extension(verifier))
            .layer(Extension(InboundWebhookSecret(inbound_secret)))
            .layer(Extension(cedar))
            .layer(Extension(Tenant(tenant.clone())))
            .layer(Extension(makod_client))
            .layer(Extension(http))
            .layer(Extension(Arc::new(cfg.mmma_import.clone())))
            .layer(Extension(pool.clone()))
            // Bound request bodies so an accidental bulk upload cannot exhaust
            // memory. The largest legitimate body is a BO4E price sheet.
            .layer(axum::extract::DefaultBodyLimit::max(2 * 1024 * 1024))
            .merge(mcp);

        let app = repository_layers(app, &pool);

        info!(%tenant, "marktd router built");
        Ok(app)
    }
}

// ── Wiring ────────────────────────────────────────────────────────────────────

/// Attach the repository handles that handlers pull as `Extension<Arc<Pg…>>`.
///
/// Kept in one function rather than trailing the router so that adding a
/// repository is a one-line edit in a list, not another link in a 30-deep
/// `.layer()` chain whose order carries no meaning.
fn repository_layers(app: Router, pool: &PgPool) -> Router {
    macro_rules! repos {
        ($app:expr, $($repo:ident),+ $(,)?) => {{
            let app = $app;
            $( let app = app.layer(Extension(Arc::new(pg::$repo::new(pool.clone())))); )+
            app
        }};
    }

    repos!(
        app,
        PgPreisblattRepository,
        PgPreisblattMessungRepository,
        PgPreisblattKaRepository,
        PgPreisblattDienstleistungRepository,
        PgPreisblattHardwareRepository,
        PgMmmaPreisGasRepository,
        PgMmmPreisStromRepository,
        PgPriCatRepository,
        PgNbContractRepository,
        PgVersorgungsStatusRepository,
        PgNeLoRepository,
        PgTrancheRepository,
        PgMaloGridRepository,
        PgMabisZpRepository,
        PgGrundversorgerRepository,
        PgMeloMsbRepository,
        PgBilanzierungRepository,
        PgSteuerbareRessourceRepository,
        PgTechnischeRessourceRepository,
        PgLokationszuordnungRepository,
        PgDeviceRepository,
        PgEinwilligungRepository,
        PgNetzzugangRepository,
        PgMsbRahmenvertragRepository,
        PgZaehlzeitRepository,
        PgUtilmdStammdatenRepository,
        PgLokationsbuendelRepository,
        PgZuordnungsermaechtigungRepository,
    )
}

/// Start the MMMA/MMM price import worker, the
/// ESA consent-expiry sweep, and the `processed_events` retention sweep.
fn spawn_workers(
    cfg: &Arc<Config>,
    pool: &PgPool,
    http: &reqwest::Client,
    shutdown: &tokio_util::sync::CancellationToken,
    tenant: &str,
) {
    crate::mmma_worker::spawn_mmma_worker(
        Arc::new(cfg.mmma_import.clone()),
        http.clone(),
        Arc::new(pg::PgMmmaPreisGasRepository::new(pool.clone())),
        Arc::new(pg::PgMmmPreisStromRepository::new(pool.clone())),
        cfg.markt.tenant.clone(),
        pool.clone(),
        shutdown.clone(),
    );

    // §49 Abs. 2 Nr. 9 MsbG: an expired Einwilligung is no lawful basis, and the
    // only way to stop a running delivery is the ORDERS 17008 the ESA sends.
    // Nothing in the market stops on its own.
    crate::consent_lifecycle::spawn_expiry_sweep(
        Arc::new(pg::PgEinwilligungRepository::new(pool.clone())),
        pool.clone(),
        tenant.to_owned(),
        shutdown.clone(),
    );

    crate::retention::spawn_processed_events_sweep(pool.clone(), shutdown.clone());
    crate::metrics::spawn_sampler(pool.clone(), shutdown.clone());
}
