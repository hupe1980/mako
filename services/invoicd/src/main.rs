#![deny(unsafe_code)]

use std::net::SocketAddr;

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt};

use invoicd::config::Config;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // ── Logging ───────────────────────────────────────────────────────────────
    fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .init();

    // ── Config ────────────────────────────────────────────────────────────────
    let config = Config::parse();

    let listen: SocketAddr = config
        .listen
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid --listen address '{}': {e}", config.listen))?;

    let check_config = config.check_config();
    let auto_dispute_threshold_eur_cents =
        (config.auto_dispute_threshold_eur * 100.0_f64).round() as i64;
    let inbound_secret = config.effective_inbound_secret().cloned();

    invoicd::server::run(invoicd::server::RunConfig {
        listen,
        makod_url: config.makod_url,
        mdmd_url: config.mdmd_url,
        subscriber_id: config.subscriber_id,
        webhook_url: config.webhook_url,
        webhook_secret: config.webhook_secret,
        inbound_secret,
        check_config,
        auto_dispute_threshold_eur_cents,
    })
    .await
}
