#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use edmd::config::{self, Config};

#[derive(Debug, Parser)]
#[command(name = "edmd", about = "Energy Data Management daemon")]
struct Cli {
    #[arg(short = 'c', long, default_value = "edmd.toml", env = "EDMD_CONFIG")]
    config: std::path::PathBuf,
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,
    /// Validate configuration and database connectivity, then exit 0.
    #[arg(long, env = "EDMD_CHECK", default_value_t = false)]
    check: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let cfg: Config = config::load_from_file(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    let _otel_guard = mako_service::init_tracing(
        "edmd",
        &cli.log_level,
        if cfg.otel.is_enabled() {
            Some(&cfg.otel)
        } else {
            None
        },
    );

    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("edmd: shutdown signal received");
            shutdown.cancel();
        });
    }

    let http = mako_service::http::default_client();
    let oidc = mako_service::oidc::OidcConfig::build_verifier(
        cfg.oidc.as_ref(),
        &http,
        &cfg.identity.tenant,
        shutdown.clone(),
    )
    .await?;

    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!("../policies/edmd.cedar"))
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
            .context("edmd --check: connecting to PostgreSQL")?;
        tracing::info!("edmd: check mode — config and database connectivity verified");
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

    edmd::server::run(edmd::server::RunConfig {
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
        oidc,
        cedar,
        mcp: cfg.mcp,
        shutdown,
        erp_webhook_url: cfg.webhook.erp_webhook_url,
        archive: if cfg.archive.enabled {
            // Resolve env references in archive credentials.
            let mut archive = cfg.archive;
            if let Some(key) = archive.access_key_id.as_deref() {
                archive.access_key_id = config::resolve_env(key).ok();
            }
            if let Some(secret) = archive.secret_access_key.as_deref() {
                archive.secret_access_key = config::resolve_env(secret).ok();
            }
            Some(archive)
        } else {
            None
        },
    })
    .await
}
