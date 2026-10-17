//! The [`Edmd`] daemon: what `mako_service::run` starts.
//!
//! `mako_service::run` owns the lifecycle: tracing/OTel, config loading, the
//! tuned pool (tagged `edmd` in `pg_stat_activity`), migrations, a real DB-ping
//! readiness probe, the generic `/metrics`, and graceful shutdown on **`SIGINT`
//! and `SIGTERM`**. This module supplies only what is edmd's: the fail-closed
//! auth gate, the resolved secrets, and the domain router via
//! [`crate::server::build`].

use std::sync::Arc;

use anyhow::Context as _;
use axum::Router;
use mako_service::{Daemon, ServiceContext};

use crate::config::{self, Config};

/// The `edmd` daemon.
pub struct Edmd;

impl Daemon for Edmd {
    type Config = Config;
    const NAME: &'static str = "edmd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run edmd migrations")?;
        // Transactional outbox for the CloudEvents edmd publishes.
        //
        // A metering event is not an optimisation: `de.messwert.reading.direct.stored`
        // is what tells `billingd` to recompute, so an RLM push that is stored and
        // never announced is consumption that is never billed. Three attempts
        // inside the emitting request survive a network blip and not a receiver
        // restart, which is exactly when a fleet-wide push arrives.
        mako_service::outbox::ensure_schema(pool)
            .await
            .context("ensure event_outbox schema")?;
        Ok(())
    }

    fn validate(cfg: &Config) -> anyhow::Result<()> {
        cfg.validate()
    }

    fn policy(cfg: &Config) -> mako_service::policy::PolicyReport {
        cfg.policy_report()
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<Router> {
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

        // The erasure suppression key ring. Resolved before the server is built:
        // a half-finished rotation must stop startup, not surface later as
        // erasures that record nothing.
        let erasure_keys = config::erasure_key_ring(&cfg.privacy)?;

        crate::server::build(crate::server::RunConfig {
            pool: ctx.pool().clone(),
            smgw: cfg.smgw.clone(),
            surveillance: cfg.surveillance.clone(),
            database_url,
            marktd_url: cfg.marktd.url.clone(),
            marktd_api_key,
            own_mp_id: cfg.identity.own_mp_id.clone(),
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
            erasure_keys,
            policy: cfg.policy_report(),
            physische_loeschung: cfg.policy.physische_loeschung.triple(),
        })
        .await
    }
}
