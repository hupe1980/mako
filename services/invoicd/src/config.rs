//! Configuration for `invoicd`.

use clap::Parser;
use invoic_checker::CheckConfig;
use secrecy::SecretString;

/// INVOIC plausibility-check daemon.
#[derive(Debug, Parser)]
#[command(name = "invoicd", about, version)]
pub struct Config {
    /// Bind address for the HTTP server.
    #[arg(
        long = "listen",
        env = "INVOICD_LISTEN",
        default_value = "0.0.0.0:8280"
    )]
    pub listen: String,

    /// Base URL of the `makod` command API.
    #[arg(
        long = "makod-url",
        env = "INVOICD_MAKOD_URL",
        default_value = "http://localhost:8080"
    )]
    pub makod_url: String,

    /// Named API key for `makod`'s command endpoint.
    ///
    /// Must match a key provisioned on `makod` with `--auth-key invoicd=<token>`.
    /// When absent, requests are sent without authentication (development only).
    #[arg(long = "makod-api-key", env = "INVOICD_MAKOD_API_KEY")]
    pub makod_api_key: Option<SecretString>,

    /// Base URL of the `marktd` subscription API.
    #[arg(
        long = "marktd-url",
        env = "INVOICD_MARKTD_URL",
        default_value = "http://localhost:9180"
    )]
    pub marktd_url: String,

    /// Bearer token for authenticating with `marktd` APIs.
    ///
    /// Required when `invoicd` calls `GET /api/v1/preisblaetter/{nb_mp_id}`.
    #[arg(long = "marktd-api-key", env = "INVOICD_MARKTD_API_KEY")]
    pub marktd_api_key: secrecy::SecretString,

    /// CloudEvents subscriber ID registered with `marktd`.
    #[arg(
        long = "subscriber-id",
        env = "INVOICD_SUBSCRIBER_ID",
        default_value = "invoicd"
    )]
    pub subscriber_id: String,

    /// Public webhook URL that `marktd` will POST events to.
    #[arg(long = "webhook-url", env = "INVOICD_WEBHOOK_URL")]
    pub webhook_url: String,

    /// HMAC-SHA256 secret that `marktd` will use when signing outbound payloads.
    ///
    /// When set, `invoicd` registers this secret with `marktd` and verifies
    /// the `X-Mako-Signature` header on every inbound webhook request.
    #[arg(long = "webhook-secret", env = "INVOICD_WEBHOOK_SECRET")]
    pub webhook_secret: Option<SecretString>,

    /// HMAC secret for verifying inbound `X-Mako-Signature` headers.
    ///
    /// Defaults to `--webhook-secret` when not set explicitly.
    #[arg(long = "inbound-secret", env = "INVOICD_INBOUND_SECRET")]
    pub inbound_secret: Option<SecretString>,

    /// Relative tolerance for per-line arithmetic checks (0.01 = 1 %).
    #[arg(
        long = "arithmetic-tolerance",
        env = "INVOICD_ARITHMETIC_TOLERANCE",
        default_value_t = 0.01
    )]
    pub arithmetic_tolerance: f64,

    /// Relative tolerance for total-amount checks (0.01 = 1 %).
    #[arg(
        long = "total-tolerance",
        env = "INVOICD_TOTAL_TOLERANCE",
        default_value_t = 0.01
    )]
    pub total_tolerance: f64,

    /// Relative tolerance for tariff unit-price checks (0.03 = 3 %).
    #[arg(
        long = "tariff-tolerance",
        env = "INVOICD_TARIFF_TOLERANCE",
        default_value_t = 0.03
    )]
    pub tariff_tolerance: f64,

    /// Require a tariff entry; a missing tariff escalates to `Dispute`.
    ///
    /// Default `false`: missing tariffs generate a `Warn` finding, not a dispute.
    #[arg(
        long = "require-tariff",
        env = "INVOICD_REQUIRE_TARIFF",
        default_value_t = false
    )]
    pub require_tariff: bool,

    /// INVOIC net-amount (EUR) above which a `Warn` outcome triggers a dispute
    /// instead of automatic approval.
    ///
    /// `0.0` (default) means `Warn` outcomes are always approved automatically.
    #[arg(
        long = "auto-dispute-threshold",
        env = "INVOICD_AUTO_DISPUTE_THRESHOLD",
        default_value_t = 0.0_f64
    )]
    pub auto_dispute_threshold_eur: f64,

    /// PostgreSQL connection URL for persisting INVOIC receipts.
    ///
    /// **Required for §22 MessZV / §41 EnWG compliance** (3-year retention).
    /// When not set `invoicd` runs in development mode — receipts are NOT
    /// persisted and a warning is emitted on every handled event.
    ///
    /// Example: `postgres://invoicd:secret@postgres:5432/invoicd`
    #[arg(long = "database-url", env = "DATABASE_URL")]
    pub database_url: Option<String>,

    /// Maximum number of PostgreSQL connections in the pool.
    #[arg(
        long = "db-max-connections",
        env = "INVOICD_DB_MAX_CONNECTIONS",
        default_value_t = 5u32
    )]
    pub db_max_connections: u32,

    /// Operator-configured tenant identifier written to every receipt row.
    ///
    /// Allows a shared `invoicd` instance to partition receipts by tenant.
    /// Defaults to `"default"` for single-tenant deployments.
    #[arg(long = "tenant", env = "INVOICD_TENANT", default_value = "default")]
    pub tenant: String,

    /// Log level (e.g. `"info"`, `"debug"`). Overridden by `RUST_LOG`.
    #[arg(long = "log-level", env = "INVOICD_LOG_LEVEL")]
    pub log_level: Option<String>,

    /// OpenTelemetry OTLP gRPC endpoint (e.g. `http://otel-collector:4317`).
    /// When absent, tracing is local-only (no spans exported).
    #[arg(long = "otel-endpoint", env = "INVOICD_OTEL_ENDPOINT")]
    pub otel_endpoint: Option<String>,

    // ── OIDC ──────────────────────────────────────────────────────────────────
    /// OIDC issuer URL.  When absent, auth is disabled (dev mode only).
    ///
    /// Example: `https://login.microsoftonline.com/{tenant-id}/v2.0`
    #[arg(long = "oidc-issuer", env = "INVOICD_OIDC_ISSUER")]
    pub oidc_issuer: Option<String>,

    /// JWT `aud` claim expected value.  Required when `--oidc-issuer` is set.
    #[arg(long = "oidc-audience", env = "INVOICD_OIDC_AUDIENCE")]
    pub oidc_audience: Option<String>,

    /// Seconds between JWKS background refreshes.
    #[arg(
        long = "oidc-jwks-refresh-secs",
        env = "INVOICD_OIDC_JWKS_REFRESH_SECS",
        default_value_t = 3600u64
    )]
    pub oidc_jwks_refresh_secs: u64,
}

impl Config {
    /// Build a [`CheckConfig`] from the relevant tolerance fields.
    #[must_use]
    pub fn check_config(&self) -> CheckConfig {
        CheckConfig {
            arithmetic_tolerance: self.arithmetic_tolerance,
            total_tolerance: self.total_tolerance,
            tariff_tolerance: self.tariff_tolerance,
            require_tariff: self.require_tariff,
        }
    }

    /// Return the effective inbound secret (falls back to `--webhook-secret`).
    #[must_use]
    pub fn effective_inbound_secret(&self) -> Option<&SecretString> {
        self.inbound_secret
            .as_ref()
            .or(self.webhook_secret.as_ref())
    }
}
