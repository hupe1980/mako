#![deny(unsafe_code)]

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tokio_util::sync::CancellationToken;

use processd::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::parse();

    // ── Logging + OpenTelemetry ───────────────────────────────────────────────
    let otel_cfg = config
        .otel_endpoint
        .as_deref()
        .map(|ep| mako_service::OtelConfig {
            endpoint: ep.to_owned(),
            service_name: "processd".to_owned(),
        });
    let _otel_guard = mako_service::init_tracing("processd", &config.log_level, otel_cfg.as_ref());

    // ── Graceful shutdown ────────────────────────────────────────────────────
    let shutdown = CancellationToken::new();
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("processd: shutdown signal received");
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
        tracing::warn!(
            "processd: OIDC disabled — all requests accepted without authentication (not for production)"
        );
        mako_service::oidc::OidcVerifier::disabled(&config.tenant)
    };

    // ── Cedar ABAC ───────────────────────────────────────────────────────────
    let cedar = Arc::new(
        mako_service::cedar::CedarEnforcer::from_policy_str(include_str!(
            "../policies/processd.cedar"
        ))
        .map_err(|e| anyhow::anyhow!("Cedar policy error: {e}"))?,
    );

    let listen: SocketAddr = config
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --listen '{}': {e}", config.listen))?;

    let inbound_secret = config.inbound_secret;

    let tenant = if config.tenant.is_empty() {
        config.own_mp_id.clone()
    } else {
        config.tenant.clone()
    };

    processd::server::run(processd::server::RunConfig {
        listen,
        database_url: config.database_url,
        db_pool_size: config.db_pool_size,
        inbound_secret,
        makod_url: config.makod_url,
        makod_api_key: config.makod_api_key,
        marktd_url: config.marktd_url,
        marktd_api_key: config.marktd_api_key,
        own_mp_id: config.own_mp_id,
        tenant,
        nb_auto_accept: config.nb_auto_accept,
        lf_auto_respond: config.lf_auto_respond,
        lf_queue_ttl_secs: config.lf_queue_ttl_secs,
        self_register_webhook_url: config.self_register_webhook_url,
        subscriber_id: config.subscriber_id,
        subscriber_event_types: config.subscriber_event_types,
        oidc,
        cedar,
        shutdown,
    })
    .await
}
