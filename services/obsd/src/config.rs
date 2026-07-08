//! Configuration for `obsd`.

use clap::Parser;
use secrecy::SecretString;

/// Business-process observability daemon.
#[derive(Debug, Parser)]
#[command(name = "obsd", about, version)]
pub struct Config {
    #[arg(long = "listen", env = "OBSD_LISTEN", default_value = "0.0.0.0:8480")]
    pub listen: String,

    #[arg(long = "database-url", env = "OBSD_DATABASE_URL")]
    pub database_url: SecretString,

    #[arg(
        long = "marktd-url",
        env = "OBSD_MARKTD_URL",
        default_value = "http://localhost:8180"
    )]
    pub marktd_url: String,
    /// Bearer token for `marktd` machine-to-machine auth.
    #[arg(long = "marktd-api-key", env = "OBSD_MARKTD_API_KEY")]
    pub marktd_api_key: secrecy::SecretString,
    #[arg(
        long = "subscriber-id",
        env = "OBSD_SUBSCRIBER_ID",
        default_value = "obsd"
    )]
    pub subscriber_id: String,

    #[arg(long = "webhook-url", env = "OBSD_WEBHOOK_URL")]
    pub webhook_url: String,

    #[arg(long = "webhook-secret", env = "OBSD_WEBHOOK_SECRET")]
    pub webhook_secret: Option<SecretString>,

    #[arg(long = "inbound-secret", env = "OBSD_INBOUND_SECRET")]
    pub inbound_secret: Option<SecretString>,

    #[arg(long = "db-pool-size", env = "OBSD_DB_POOL_SIZE", default_value_t = 10)]
    pub db_pool_size: u32,

    /// Log level (e.g. `"info"`, `"debug"`). Overridden by `RUST_LOG`.
    #[arg(long = "log-level", env = "OBSD_LOG_LEVEL")]
    pub log_level: Option<String>,

    /// OpenTelemetry OTLP gRPC endpoint (e.g. `http://otel-collector:4317`).
    #[arg(long = "otel-endpoint", env = "OBSD_OTEL_ENDPOINT")]
    pub otel_endpoint: Option<String>,

    // ── Tenant ────────────────────────────────────────────────────────────────
    /// Tenant identifier for Cedar resource checks.
    #[arg(long = "tenant", env = "OBSD_TENANT", default_value = "default")]
    pub tenant: String,

    // ── OIDC ──────────────────────────────────────────────────────────────────
    /// OIDC issuer URL.  When absent, auth is disabled (dev mode only).
    #[arg(long = "oidc-issuer", env = "OBSD_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// JWT `aud` claim expected value.  Required when `--oidc-issuer` is set.
    #[arg(long = "oidc-audience", env = "OBSD_OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    /// Seconds between JWKS background refreshes.
    #[arg(
        long = "oidc-jwks-refresh-secs",
        env = "OBSD_OIDC_JWKS_REFRESH_SECS",
        default_value_t = 3600u64
    )]
    pub oidc_jwks_refresh_secs: u64,
}

impl Config {
    #[must_use]
    pub fn effective_inbound_secret(&self) -> Option<&SecretString> {
        self.inbound_secret
            .as_ref()
            .or(self.webhook_secret.as_ref())
    }
}
