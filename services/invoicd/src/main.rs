#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use invoicd::config::{self, Config};

#[derive(Debug, Parser)]
#[command(name = "invoicd", about = "INVOIC plausibility-check daemon (LF role)")]
struct Cli {
    #[arg(
        short = 'c',
        long,
        default_value = "invoicd.toml",
        env = "INVOICD_CONFIG"
    )]
    config: std::path::PathBuf,
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,
    /// Validate configuration and database connectivity, then exit 0.
    #[arg(long, env = "INVOICD_CHECK", default_value_t = false)]
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
            service_name: "invoicd".to_owned(),
        });
    let _otel_guard = mako_service::init_tracing("invoicd", &cli.log_level, otel_cfg.as_ref());

    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("invoicd: shutdown signal received");
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
        tracing::warn!(
            "invoicd: OIDC disabled — all requests accepted without authentication (not for production)"
        );
        mako_service::oidc::OidcVerifier::disabled(&cfg.identity.tenant)
    };

    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../policies/invoicd.cedar"
        ))
        .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let listen: SocketAddr = cfg
        .http
        .addr
        .parse()
        .with_context(|| format!("invalid http.addr '{}'", cfg.http.addr))?;

    let database_url = config::resolve_env(&cfg.database.url)
        .context("database.url")
        .ok();

    // ── --check mode early exit ───────────────────────────────────────────────
    if cli.check {
        if let Some(ref url) = database_url {
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(1)
                .connect(url)
                .await
                .context("invoicd --check: connecting to PostgreSQL")?;
        }
        tracing::info!("invoicd: check mode — config and database connectivity verified");
        return Ok(());
    }
    let makod_api_key = cfg
        .makod
        .api_key
        .as_deref()
        .map(config::resolve_env_secret)
        .transpose()
        .context("makod.api_key")?;
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

    let check_config = cfg.check_config();
    let auto_dispute_threshold_eur_cents = cfg.auto_dispute_threshold_eur_cents();
    let makod_url = cfg.makod.url.clone();
    let marktd_url = cfg.marktd.url.clone();
    let subscriber_id = cfg.subscription.subscriber_id.clone();
    let webhook_url = cfg.subscription.webhook_url.clone();
    let tenant = cfg.identity.tenant.clone();
    let db_max_connections = cfg.database.max_connections;

    invoicd::server::run(invoicd::server::RunConfig {
        listen,
        makod_url,
        makod_api_key,
        marktd_url,
        marktd_api_key,
        subscriber_id,
        webhook_url,
        webhook_secret,
        inbound_secret,
        check_config,
        auto_dispute_threshold_eur_cents,
        database_url,
        db_max_connections,
        tenant,
        oidc,
        cedar,
        shutdown,
    })
    .await
}
