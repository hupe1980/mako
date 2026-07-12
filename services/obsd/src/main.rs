#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use obsd::config::{self, Config};

#[derive(Debug, Parser)]
#[command(name = "obsd", about = "Business-process observability daemon")]
struct Cli {
    #[arg(short = 'c', long, default_value = "obsd.toml", env = "OBSD_CONFIG")]
    config: std::path::PathBuf,
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,
    /// Validate configuration and database connectivity, then exit 0.
    #[arg(long, env = "OBSD_CHECK", default_value_t = false)]
    check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let cfg: Config = config::load_from_file(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    let otel_cfg = cfg
        .otel
        .endpoint
        .as_deref()
        .map(|ep| mako_service::OtelConfig {
            endpoint: ep.to_owned(),
            service_name: "obsd".to_owned(),
        });
    let _otel_guard = mako_service::init_tracing("obsd", &cli.log_level, otel_cfg.as_ref());

    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("obsd: shutdown signal received");
            shutdown.cancel();
        });
    }

    let http = mako_service::http::default_client();
    let oidc = if let Some(ref oidc_cfg) = cfg.oidc {
        let verifier =
            mako_service::oidc::OidcVerifier::new(&oidc_cfg.issuer, &oidc_cfg.audience, &http)
                .await
                .context("OIDC discovery")?;
        verifier.clone().spawn_refresh_task(
            http.clone(),
            oidc_cfg.jwks_refresh_secs,
            shutdown.clone(),
        );
        verifier
    } else {
        tracing::warn!("obsd: OIDC disabled — all requests accepted without authentication");
        mako_service::oidc::OidcVerifier::disabled(&cfg.identity.tenant)
    };

    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!("../policies/obsd.cedar"))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let listen: SocketAddr = cfg
        .http
        .addr
        .parse()
        .with_context(|| format!("invalid http.addr '{}'", cfg.http.addr))?;

    let database_url = config::resolve_env_secret(&cfg.database.url).context("database.url")?;

    // ── --check mode early exit ────────────────────────────────────────────────
    if cli.check {
        use secrecy::ExposeSecret as _;
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(database_url.expose_secret())
            .await
            .context("obsd --check: connecting to PostgreSQL")?;
        tracing::info!("obsd: check mode — config and database connectivity verified");
        return Ok(());
    }

    let marktd_api_key =
        config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
    let inbound_secret = cfg
        .webhook
        .inbound_secret
        .as_deref()
        .map(config::resolve_env_secret)
        .transpose()
        .context("webhook.inbound_secret")?;
    let webhook_secret = inbound_secret.clone();

    obsd::server::run(obsd::server::RunConfig {
        listen,
        database_url,
        marktd_url: cfg.marktd.url,
        marktd_api_key,
        subscriber_id: cfg.subscription.subscriber_id,
        webhook_url: cfg.subscription.webhook_url,
        webhook_secret,
        inbound_secret,
        db_pool_size: cfg.database.pool_size,
        tenant: cfg.identity.tenant,
        // §20 EnWG: if own_mp_ids is empty the server falls back to [tenant].
        own_mp_ids: cfg.identity.own_mp_ids,
        oidc,
        cedar,
        shutdown,
    })
    .await
}
