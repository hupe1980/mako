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

    async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run migrations")?;
        // Transactional outbox for customer-facing CloudEvents.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        Ok(())
    }

    fn validate(cfg: &config::VertragdConfig) -> anyhow::Result<()> {
        cfg.check_auth_posture()
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

        let mcp_state = Arc::new(mcp_server::VertragdMcpState {
            pool: pool.clone(),
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

        let app = handlers::router(handler_ctx, enforcer)
            .merge(mcp_server::router(mcp_state, shutdown.clone()))
            // Operator policy: GET /api/v1/policy (read-only; Cedar `read-policy`).
            .merge(mako_service::policy::router(cfg.policy_report()))
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc));

        // ── Durable outbound calls (processd / edmd / productd / accountingd) ──
        tokio::spawn(
            outbound::OutboundWorker::new(pool.clone(), Arc::clone(&cfg), http.clone())
                .run(shutdown.clone()),
        );

        // ── Customer-facing CloudEvents to the ERP ───────────────────────────
        // Only spawned with a webhook configured; without one the events stay
        // in the outbox, which is where an operator can still find them.
        if let Some(url) = cfg.erp_webhook_url.clone() {
            tokio::spawn(
                mako_service::outbox::OutboxWorker::new(
                    pool.clone(),
                    url,
                    cfg.erp_hmac_secret.clone().map(Into::into),
                )
                .run(shutdown.clone()),
            );
        } else {
            tracing::warn!(
                "vertragd: no erp_webhook_url — statutory notices accumulate in event_outbox \
                 undelivered"
            );
        }

        // ── Daily contract-lifecycle workers ─────────────────────────────────
        workers::spawn_all(pool, Arc::clone(&cfg), shutdown);

        tracing::info!("vertragd: router and workers ready");
        Ok(app)
    }
}
