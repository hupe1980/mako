//! `vertragd` — B2C and B2B contract & customer management (LF role), `:9780`.
//!
//! See the crate documentation in `lib.rs` for the data model and the two
//! durability rails. This binary owns only the wiring: fail-closed
//! authentication checks, the router, and the background workers.

use std::sync::Arc;

use anyhow::Context as _;
use axum::{Extension, Router};
use mako_service::{Daemon, ServiceContext};
use sqlx::PgPool;
use vertragd::{config, handlers, mcp_server, outbound, workers};

struct Vertragd;

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

    async fn build(
        cfg: Arc<config::VertragdConfig>,
        ctx: ServiceContext,
    ) -> anyhow::Result<Router> {
        cfg.check_auth_posture()?;

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

        let mcp_state = Arc::new(mcp_server::VertragdMcpState {
            pool: pool.clone(),
            tenant: cfg.tenant.clone(),
            auth: mako_service::mcp_auth::McpAuth::from_auth_config(&cfg.mcp, &cfg.tenant),
        });

        let handler_ctx = Arc::new(handlers::Ctx {
            pool: pool.clone(),
            cfg: Arc::clone(&cfg),
            http: http.clone(),
        });

        let app = handlers::router(handler_ctx)
            .merge(mcp_server::router(mcp_state, shutdown.clone()))
            // Every authenticated handler extracts `Claims`, and the extractor
            // rejects a token whose `mako_tenant` is not this deployment's. A
            // route added later cannot forget the check without also dropping
            // authentication — the omission is not silent.
            .layer(Extension(mako_service::oidc::ExpectedTenant(
                cfg.tenant.clone(),
            )))
            .layer(Extension(oidc));

        // ── Durable outbound calls (processd / edmd / tarifbd / accountingd) ──
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Vertragd>().await
}
