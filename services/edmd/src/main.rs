#![deny(unsafe_code)]

//! `edmd` — Energy Data Management daemon.
//!
//! `mako_service::run` owns the lifecycle: tracing/OTel, config loading, the
//! tuned pool (tagged `edmd` in `pg_stat_activity`), migrations, a real DB-ping
//! readiness probe, the generic `/metrics`, and graceful shutdown on **`SIGINT`
//! and `SIGTERM`**. This module supplies only what is edmd's: the fail-closed
//! auth gate, the resolved secrets, and the domain router via
//! [`edmd::server::build`].
//!
//! Port: `:8380`.

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use mako_service::{Daemon, ServiceContext};

use edmd::config::{self, Config};

/// The `edmd` daemon.
struct Edmd;

impl Daemon for Edmd {
    type Config = Config;
    const NAME: &'static str = "edmd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run edmd migrations")
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
        // `OidcVerifier::disabled` admits every caller as `dev-admin` with all
        // roles, so an omitted `[oidc]` section leaves the whole API — GDPR
        // erasure and `POST /api/v1/query/sql` included — open to anyone who can
        // reach the port. Refuse to start rather than let a missing section be
        // the difference.
        if cfg.oidc.is_none() && !cfg.allow_insecure_no_auth {
            anyhow::bail!(
                "no [oidc] section configured. Without it every request is admitted as \
                 `dev-admin` with all market roles and every Cedar check passes. \
                 Configure [oidc], or set allow_insecure_no_auth = true to accept an \
                 unauthenticated deployment."
            );
        }
        if cfg.allow_insecure_no_auth {
            tracing::warn!(
                "edmd: allow_insecure_no_auth is set — every request is admitted as \
                 dev-admin with all market roles"
            );
        }

        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &cfg.identity.tenant,
            ctx.shutdown.clone(),
        )
        .await?;

        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/edmd.cedar"
            ))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
        );

        let database_url = config::resolve_env_secret(&cfg.database.url).context("database.url")?;
        let marktd_api_key =
            config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
        let inbound_secret = cfg
            .webhook
            .inbound_secret
            .as_deref()
            .map(config::resolve_env_secret)
            .transpose()
            .context("webhook.inbound_secret")?;

        // Resolve `env:` references in the S3 credentials before they reach the
        // warehouse builder.
        let archive = cfg.archive.enabled.then(|| {
            let mut archive = cfg.archive.clone();
            if let Some(key) = archive.access_key_id.as_deref() {
                archive.access_key_id = config::resolve_env(key).ok();
            }
            if let Some(secret) = archive.secret_access_key.as_deref() {
                archive.secret_access_key = config::resolve_env(secret).ok();
            }
            archive
        });

        edmd::server::build(edmd::server::RunConfig {
            pool: ctx.pool().clone(),
            smgw: cfg.smgw.clone(),
            surveillance: cfg.surveillance.clone(),
            database_url,
            marktd_url: cfg.marktd.url.clone(),
            marktd_api_key,
            subscriber_id: cfg.subscription.subscriber_id.clone(),
            webhook_url: cfg.subscription.webhook_url.clone(),
            webhook_secret: inbound_secret.clone(),
            inbound_secret,
            tenant: cfg.identity.tenant.clone(),
            oidc,
            cedar,
            mcp: cfg.mcp.clone(),
            shutdown: ctx.shutdown.clone(),
            erp_webhook_url: cfg.webhook.erp_webhook_url.clone(),
            erp_webhook_secret: cfg.webhook.erp_webhook_secret.clone(),
            rate_limit: cfg.rate_limit.clone(),
            kafka_ingest: cfg.kafka_ingest.clone(),
            confirmation: cfg.confirmation.clone(),
            archive,
        })
        .await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Edmd>().await
}
