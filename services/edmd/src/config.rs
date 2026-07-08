//! Configuration for `edmd`.

use clap::Parser;
use secrecy::SecretString;

/// Energy Data Management daemon.
#[derive(Debug, Parser)]
#[command(name = "edmd", about, version)]
pub struct Config {
    /// Bind address for the HTTP server.
    #[arg(long = "listen", env = "EDMD_LISTEN", default_value = "0.0.0.0:8380")]
    pub listen: String,

    /// PostgreSQL/TimescaleDB connection URL.
    #[arg(long = "database-url", env = "EDMD_DATABASE_URL")]
    pub database_url: SecretString,

    /// Base URL of the `marktd` subscription API.
    #[arg(
        long = "marktd-url",
        env = "EDMD_MARKTD_URL",
        default_value = "http://localhost:8180"
    )]
    pub marktd_url: String,

    /// Bearer token for `marktd` machine-to-machine auth.
    #[arg(long = "marktd-api-key", env = "EDMD_MARKTD_API_KEY")]
    pub marktd_api_key: secrecy::SecretString,

    /// CloudEvents subscriber ID registered with `marktd`.
    #[arg(
        long = "subscriber-id",
        env = "EDMD_SUBSCRIBER_ID",
        default_value = "edmd"
    )]
    pub subscriber_id: String,

    /// Public webhook URL that `marktd` will POST events to.
    #[arg(long = "webhook-url", env = "EDMD_WEBHOOK_URL")]
    pub webhook_url: String,

    /// HMAC-SHA256 secret that `marktd` uses when signing outbound payloads.
    #[arg(long = "webhook-secret", env = "EDMD_WEBHOOK_SECRET")]
    pub webhook_secret: Option<SecretString>,

    /// HMAC secret for verifying inbound `X-Mako-Signature` headers.
    ///
    /// Defaults to `--webhook-secret` when not set explicitly.
    #[arg(long = "inbound-secret", env = "EDMD_INBOUND_SECRET")]
    pub inbound_secret: Option<SecretString>,

    /// Maximum number of database connections in the pool.
    #[arg(long = "db-pool-size", env = "EDMD_DB_POOL_SIZE", default_value_t = 10)]
    pub db_pool_size: u32,

    /// Log level (e.g. `"info"`, `"debug"`). Overridden by `RUST_LOG`.
    #[arg(long = "log-level", env = "EDMD_LOG_LEVEL")]
    pub log_level: Option<String>,

    /// OpenTelemetry OTLP gRPC endpoint (e.g. `http://otel-collector:4317`).
    #[arg(long = "otel-endpoint", env = "EDMD_OTEL_ENDPOINT")]
    pub otel_endpoint: Option<String>,

    // ── Tenant ────────────────────────────────────────────────────────────────
    /// Tenant identifier for Cedar resource checks.
    #[arg(long = "tenant", env = "EDMD_TENANT", default_value = "default")]
    pub tenant: String,

    // ── OIDC ──────────────────────────────────────────────────────────────────
    /// OIDC issuer URL.  When absent, auth is disabled (dev mode only).
    #[arg(long = "oidc-issuer", env = "EDMD_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// JWT `aud` claim expected value.  Required when `--oidc-issuer` is set.
    #[arg(long = "oidc-audience", env = "EDMD_OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    /// Seconds between JWKS background refreshes.
    #[arg(
        long = "oidc-jwks-refresh-secs",
        env = "EDMD_OIDC_JWKS_REFRESH_SECS",
        default_value_t = 3600u64
    )]
    pub oidc_jwks_refresh_secs: u64,
}

impl Config {
    /// Return the effective inbound secret (falls back to `--webhook-secret`).
    #[must_use]
    pub fn effective_inbound_secret(&self) -> Option<&SecretString> {
        self.inbound_secret
            .as_ref()
            .or(self.webhook_secret.as_ref())
    }
}
