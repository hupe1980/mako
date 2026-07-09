#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use tokio_util::sync::CancellationToken;

use processd::config::{self, Config};

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Parser)]
#[command(
    name = "processd",
    about = "Process decision engine for German energy market (LF E_0624 auto-response + NB Anmeldung STP)"
)]
struct Cli {
    /// Path to the `processd.toml` configuration file.
    #[arg(
        short = 'c',
        long,
        default_value = "processd.toml",
        env = "PROCESSD_CONFIG"
    )]
    config: std::path::PathBuf,

    /// Log level override (RUST_LOG syntax: `info`, `debug`, `processd=trace`).
    #[arg(long, default_value = "info", env = "RUST_LOG")]
    log_level: String,
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // ── Config ────────────────────────────────────────────────────────────────
    let cfg: Config = config::load_from_file(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    // ── Logging + OpenTelemetry ───────────────────────────────────────────────
    let otel_cfg = cfg
        .otel
        .endpoint
        .as_deref()
        .map(|ep| mako_service::OtelConfig {
            endpoint: ep.to_owned(),
            service_name: "processd".to_owned(),
        });
    let _otel_guard = mako_service::init_tracing("processd", &cli.log_level, otel_cfg.as_ref());

    // ── Graceful shutdown ─────────────────────────────────────────────────────
    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("processd: shutdown signal received");
            shutdown.cancel();
        });
    }

    // ── Resolve env-var references ───────────────────────────────────────────
    let database_url = config::resolve_env(&cfg.database.url).context("database.url")?;
    let makod_api_key = config::resolve_env_secret(&cfg.makod.api_key).context("makod.api_key")?;
    let marktd_api_key =
        config::resolve_env_secret(&cfg.marktd.api_key).context("marktd.api_key")?;
    let inbound_secret = cfg
        .webhook
        .inbound_secret
        .as_deref()
        .map(config::resolve_env_secret)
        .transpose()
        .context("webhook.inbound_secret")?;

    // ── OIDC ─────────────────────────────────────────────────────────────────
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
            "processd: OIDC disabled — all requests accepted without authentication (not for production)"
        );
        let tenant = if cfg.identity.tenant.is_empty() {
            &cfg.identity.own_mp_id
        } else {
            &cfg.identity.tenant
        };
        mako_service::oidc::OidcVerifier::disabled(tenant)
    };

    // ── Cedar ABAC ───────────────────────────────────────────────────────────
    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../policies/processd.cedar"
        ))
        .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let listen: SocketAddr = cfg
        .http
        .addr
        .parse()
        .with_context(|| format!("invalid http.addr '{}'", cfg.http.addr))?;

    let tenant = if cfg.identity.tenant.is_empty() {
        cfg.identity.own_mp_id.clone()
    } else {
        cfg.identity.tenant.clone()
    };

    processd::server::run(processd::server::RunConfig {
        listen,
        database_url,
        db_pool_size: cfg.database.pool_size,
        inbound_secret,
        makod_url: cfg.makod.url,
        makod_api_key,
        marktd_url: cfg.marktd.url,
        marktd_api_key,
        own_mp_id: cfg.identity.own_mp_id,
        tenant,
        nb_auto_accept: cfg.nb.auto_accept,
        lf_auto_respond: cfg.lf.auto_respond,
        lf_queue_ttl_secs: cfg.lf.queue_ttl_secs,
        self_register_webhook_url: cfg.subscription.webhook_url,
        subscriber_id: cfg.subscription.subscriber_id,
        subscriber_event_types: cfg.subscription.event_types,
        oidc,
        cedar,
        shutdown,
    })
    .await
}
