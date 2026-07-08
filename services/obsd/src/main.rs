#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio_util::sync::CancellationToken;

use obsd::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    let otel_cfg = config
        .otel_endpoint
        .as_deref()
        .map(|ep| mako_service::OtelConfig {
            endpoint: ep.to_owned(),
            service_name: "obsd".to_owned(),
        });
    let _otel_guard = mako_service::init_tracing(
        "obsd",
        config.log_level.as_deref().unwrap_or("info"),
        otel_cfg.as_ref(),
    );

    // ── Graceful shutdown ────────────────────────────────────────────────────
    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("obsd: shutdown signal received");
            shutdown.cancel();
        });
    }

    // ── OIDC ─────────────────────────────────────────────────────────────────
    let http = mako_service::http::default_client();
    let oidc = if let Some(ref issuer) = config.oidc_issuer {
        let audience = config
            .oidc_audience
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--oidc-audience required when --oidc-issuer is set"))?;
        let verifier = mako_service::oidc::OidcVerifier::new(issuer, audience, &http).await?;
        verifier.clone().spawn_refresh_task(
            http.clone(),
            config.oidc_jwks_refresh_secs,
            shutdown.clone(),
        );
        verifier
    } else {
        tracing::warn!("obsd: OIDC disabled — all requests accepted without authentication");
        mako_service::oidc::OidcVerifier::disabled(&config.tenant)
    };

    // ── Cedar ABAC ───────────────────────────────────────────────────────────
    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!("../policies/obsd.cedar"))
            .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    // ── Server ───────────────────────────────────────────────────────────────
    let listen: SocketAddr = config
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --listen '{}': {e}", config.listen))?;

    let inbound_secret = config.effective_inbound_secret().cloned();

    obsd::server::run(obsd::server::RunConfig {
        listen,
        database_url: config.database_url,
        marktd_url: config.marktd_url,
        marktd_api_key: config.marktd_api_key,
        subscriber_id: config.subscriber_id,
        webhook_url: config.webhook_url,
        webhook_secret: config.webhook_secret,
        inbound_secret,
        db_pool_size: config.db_pool_size,
        tenant: config.tenant,
        oidc,
        cedar,
        shutdown,
    })
    .await
}
