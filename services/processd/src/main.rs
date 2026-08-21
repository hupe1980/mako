#![deny(unsafe_code)]

//! `processd` — Process decision engine for the German energy market
//! (LF E_0624 auto-response + NB Anmeldung STP + MSB REQOTE + EoG gap closure).
//!
//! `mako_service::run` owns the lifecycle (tracing, tuned pool with
//! `application_name`, real DB-ping readiness, graceful shutdown, `--check`);
//! this only supplies the migrations and the domain router + background workers
//! via [`processd::server::build_router`].

use secrecy::SecretString;
use std::sync::Arc;

use anyhow::Context as _;
use mako_service::{Daemon, ServiceContext};

use processd::config::{self, Config};

/// The `processd` daemon.
struct Processd;

impl Daemon for Processd {
    type Config = Config;
    const NAME: &'static str = "processd";

    async fn migrate(pool: &sqlx::PgPool) -> anyhow::Result<()> {
        sqlx::migrate!("./migrations")
            .run(pool)
            .await
            .context("run processd migrations")?;
        Ok(())
    }

    async fn build(cfg: Arc<Config>, ctx: ServiceContext) -> anyhow::Result<axum::Router> {
        // ── Resolve env-var references ────────────────────────────────────────
        let makod_api_key =
            config::resolve_env_secret(&cfg.makod.api_key).context("makod.api_key")?;
        let marktd_api_key =
            config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
        let inbound_secret = cfg
            .webhook
            .inbound_secret
            .as_deref()
            .map(config::resolve_env_secret)
            .transpose()
            .context("webhook.inbound_secret")?;
        // Resolved, not passed through: the field documents `"env:VAR_NAME"`
        // support, and an unresolved reference would be used as the HMAC key
        // itself — signing every §38 expiry notification with the ASCII bytes
        // `env:VAR_NAME`, which any verifying receiver rejects.
        let eog_notify_webhook_secret = cfg
            .eog
            .notify_webhook_secret
            .as_deref()
            .map(config::resolve_env)
            .transpose()
            .context("eog.notify_webhook_secret")?;

        let tenant = if cfg.identity.tenant.is_empty() {
            cfg.identity.own_mp_id.clone()
        } else {
            cfg.identity.tenant.clone()
        };

        // ── OIDC ──────────────────────────────────────────────────────────────
        let oidc = mako_service::oidc::OidcConfig::build_verifier(
            cfg.oidc.as_ref(),
            &ctx.http,
            &tenant,
            ctx.shutdown.clone(),
        )
        .await?;

        // ── Cedar ABAC ────────────────────────────────────────────────────────
        let cedar = Arc::new(
            mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
                "../policies/processd.cedar"
            ))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
        );

        processd::server::build_router(
            processd::server::RunConfig {
                inbound_secret,
                makod_url: cfg.makod.url.clone(),
                makod_api_key,
                marktd_url: cfg.marktd.url.clone(),
                marktd_api_key,
                own_mp_id: cfg.identity.own_mp_id.clone(),
                tenant,
                nb_auto_accept: cfg.nb.auto_accept,
                nb_gas_bearbeitungsfrist_wt: cfg.nb.gas_bearbeitungsfrist_wt,
                nb_einsd_url: cfg.nb.einsd_url.clone(),
                nb_einsd_api_key: cfg.nb.einsd_api_key.clone().map(SecretString::from),
                lf_auto_respond: cfg.lf.auto_respond,
                lf_vertragd_url: cfg.lf.vertragd_url.clone(),
                lf_vertragd_api_key: cfg.lf.vertragd_api_key.clone().map(SecretString::from),
                msb_auto_accept: cfg.msb.auto_accept,
                msb_auto_preisanfrage: cfg.msb.auto_preisanfrage,
                eog_auto_activate: cfg.eog.auto_activate,
                eog_default_transaktionsgrund: cfg.eog.default_transaktionsgrund.clone(),
                eog_warn_days_before_expiry: cfg.eog.warn_days_before_expiry,
                eog_notify_webhook_url: cfg.eog.notify_webhook_url.clone(),
                eog_notify_webhook_secret,
                self_register_webhook_url: cfg.subscription.webhook_url.clone(),
                subscriber_id: cfg.subscription.subscriber_id.clone(),
                subscriber_event_types: cfg.subscription.event_types.clone(),
                oidc,
                cedar,
                mcp: cfg.mcp.clone(),
            },
            ctx,
        )
        .await
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    mako_service::run::<Processd>().await
}
