//! The [`Vertragd`] daemon: what `mako_service::run` starts.

use std::sync::Arc;

use crate::{config, handlers, mcp_server, outbound, workers};
use anyhow::Context as _;
use axum::{Extension, Router};
use mako_service::{Daemon, ServiceContext};
use sqlx::PgPool;

pub struct Vertragd;

impl Daemon for Vertragd {
    type Config = config::VertragdConfig;
    const NAME: &'static str = "vertragd";

    async fn migrate(_cfg: &Self::Config, pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run migrations")?;
        // Transactional outbox for customer-facing CloudEvents, and the
        // webhook endpoints and deliveries that fan them out.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        mako_service::subscriptions::ensure_schema(pool)
            .await
            .context("ensure webhook subscription schema")?;
        Ok(())
    }

    fn validate(cfg: &config::VertragdConfig) -> Vec<mako_service::Befund> {
        let mut befunde = cfg.auth_befunde();
        befunde.extend(cfg.bestellung_befunde());
        befunde.extend(cfg.subscription_befunde());
        befunde
    }

    fn policy(cfg: &config::VertragdConfig) -> mako_service::policy::PolicyReport {
        cfg.policy_report()
    }

    async fn build(
        cfg: Arc<config::VertragdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        let pool = ctx.pool().clone();
        let http = ctx.http.clone();
        let shutdown = ctx.shutdown.clone();

        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &http,
            &cfg.tenant,
            shutdown.clone(),
        )
        .await
        .context("OIDC setup")?;

        // Authorization. `Claims` proves who is calling and that the token was
        // issued for this deployment; the policy decides what that principal
        // may do — and in particular separates the customer token `portald`
        // forwards for the portal check from the operator identities that may
        // read a customer's record or change a contract.
        let enforcer = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/vertragd.cedar"
            ))
            .context("vertragd.cedar must parse at startup")?,
        );

        // ── Customer-facing CloudEvents: the webhook subscriptions ──────────
        // `erp_webhook_url` is the endpoint `default`; integrators register
        // more through `/api/v1/webhooks/endpoints`. Without any endpoint the
        // events stay in the outbox, readable on `GET /api/v1/events`.
        if cfg.erp_webhook_url.is_none() {
            tracing::warn!(
                "vertragd: no erp_webhook_url — statutory notices reach only registered \
                 webhook endpoints and the event feed"
            );
        }
        let subs = subscriptions(&cfg, pool.clone());
        subs.spawn(shutdown.clone());

        let mcp_state = Arc::new(mcp_server::VertragdMcpState {
            pool: pool.clone(),
            subscriptions: subs.clone(),
            tenant: cfg.tenant.clone(),
            // `use-mcp` gates the whole MCP surface, which reads customer
            // profiles and bank details across the tenant. Without it any token
            // valid for this deployment reaches those tools, and a portal
            // customer's token is one.
            auth: mako_service::mcp_auth::McpAuth::from_auth_config_oidc(
                &cfg.mcp,
                oidc.clone(),
                Some(Arc::clone(&enforcer)),
                &cfg.tenant,
            ),
            policy: Arc::new(cfg.policy_report()),
        });

        let handler_ctx = Arc::new(handlers::Ctx {
            pool: pool.clone(),
            cfg: Arc::clone(&cfg),
            http: http.clone(),
        });

        let (routes, doc) = handlers::documented(&subs);
        let app = handlers::layered(
            routes.merge(mako_service::openapi::serve(doc)),
            handler_ctx,
            enforcer,
        )
        .merge(mcp_server::router(mcp_state, shutdown.clone()))
        // Readiness and operator policy: GET /admin/einrichtung (Cedar
        // `read-config`) and GET /api/v1/policy (Cedar `read-policy`).
        .merge(mako_service::einrichtung::router(Arc::clone(
            &ctx.einrichtung,
        )))
        .layer(Extension(mako_service::oidc::ExpectedTenant(
            cfg.tenant.clone(),
        )))
        .layer(Extension(oidc));

        // ── Durable outbound calls (processd / edmd / accountingd) ───────────
        for worker in outbound::workers(&pool, &cfg) {
            tokio::spawn(worker.run(shutdown.clone()));
        }

        // ── Daily contract-lifecycle workers ─────────────────────────────────
        workers::spawn_all(pool, Arc::clone(&cfg), shutdown);

        tracing::info!("vertragd: router and workers ready");
        Ok(app)
    }
}

/// The webhook subscription module over vertragd's outbox: `erp_webhook_url`,
/// signed with `erp_hmac_secret`, is the endpoint `default`.
#[must_use]
pub fn subscriptions(
    cfg: &config::VertragdConfig,
    pool: PgPool,
) -> mako_service::subscriptions::Subscriptions {
    mako_service::subscriptions::Subscriptions::new(
        pool,
        <Vertragd as Daemon>::NAME,
        cfg.tenant.clone(),
        cfg.subscriptions.clone(),
    )
    .with_default_endpoint(
        cfg.erp_webhook_url.clone(),
        cfg.erp_hmac_secret.clone().map(Into::into),
    )
}
