//! `marktd` configuration — loaded from `marktd.toml` + environment overrides.
//!
//! Values can be resolved from the environment by using `"env:VAR_NAME"` as
//! the string value in the TOML file.  Call [`resolve_env_secret`] for
//! sensitive fields before use.
//!
//! # Example `marktd.toml`
//!
//! ```toml
//! [storage.postgres]
//! url = "env:DATABASE_URL"
//!
//! [http]
//! addr = "0.0.0.0:8180"
//!
//! [oidc]
//! issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! audience = "api://mako-markt"
//!
//! [makod]
//! base_url  = "http://makod:8080"
//! api_key   = "env:MAKOD_API_KEY"
//! tenant   = "9900357000004"
//!
//! [webhook]
//! inbound_path   = "/api/v1/mako/events"
//! inbound_secret = "env:MAKOD_WEBHOOK_SECRET"
//! delivery_timeout_secs = 10
//! max_retry_attempts    = 3
//!
//! [otel]
//! endpoint     = "http://otel-collector:4317"
//! service_name = "marktd"
//!
//! [mcp]
//! path = "/mcp"
//! ```

use serde::Deserialize;
use std::path::Path;

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub storage: StorageConfig,
    pub http: HttpConfig,
    /// OIDC configuration.  When omitted, authentication is **disabled** and
    /// all API requests are accepted with synthetic dev-admin claims.
    /// **Never omit this in production.**
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    pub makod: MakodConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    /// B12: Automated monthly MMMA Gas / MMM Strom price import.
    /// When omitted or `enabled = false`, prices must be imported via
    /// `PUT /api/v1/mmma-preise/gas/{year}/{month}` by the ERP.
    #[serde(default)]
    pub mmma_import: MmmaImportConfig,
    /// Start without token verification AND without inbound webhook signing.
    ///
    /// Without `[oidc]` every request is admitted with synthetic dev claims,
    /// and without `webhook.inbound_secret` the `POST /events` endpoint accepts
    /// unsigned events that mutate VersorgungsStatus and the device registry.
    /// Both postures must be asked for by name — `main` refuses to start when
    /// either is missing unless this flag is set.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

// ── Storage ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StorageConfig {
    pub postgres: PostgresConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PostgresConfig {
    /// PostgreSQL URL, e.g. `postgres://marktd:secret@postgres:5432/marktd`.
    /// Use `"env:DATABASE_URL"` to defer to the `DATABASE_URL` environment variable.
    pub url: String,
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
    #[serde(default = "default_min_connections")]
    pub min_connections: u32,
    #[serde(default = "default_acquire_timeout_secs")]
    pub acquire_timeout_secs: u64,
}

fn default_max_connections() -> u32 {
    20
}
fn default_min_connections() -> u32 {
    2
}
fn default_acquire_timeout_secs() -> u64 {
    5
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8180".to_owned()
}

// ── OIDC ──────────────────────────────────────────────────────────────────────

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

// ── MaKod client ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// Base URL of the `makod` admin API.  Example: `http://makod:8080`.
    pub base_url: String,
    /// Bearer token / API key.  Use `"env:MAKOD_API_KEY"` for env-var resolution.
    pub api_key: String,
    /// Tenant identifier — the operator's primary MP-ID string
    /// (BDEW-Codenummer starting with 99). Used as the `tenant_gln` in
    /// outbound CloudEvents source URNs.
    pub tenant: String,
}

// ── Inbound webhooks (from makod) ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// URL path on which marktd listens for CloudEvents from makod.
    #[serde(default = "default_inbound_path")]
    pub inbound_path: String,
    /// HMAC-SHA256 shared secret.  Use `"env:MAKOD_WEBHOOK_SECRET"`.
    pub inbound_secret: Option<String>,
    #[serde(default = "default_delivery_timeout_secs")]
    pub delivery_timeout_secs: u64,
    #[serde(default = "default_max_retry_attempts")]
    pub max_retry_attempts: u32,
}

impl Default for WebhookConfig {
    fn default() -> Self {
        Self {
            inbound_path: default_inbound_path(),
            inbound_secret: None,
            delivery_timeout_secs: default_delivery_timeout_secs(),
            max_retry_attempts: default_max_retry_attempts(),
        }
    }
}

fn default_inbound_path() -> String {
    "/api/v1/mako/events".to_owned()
}
fn default_delivery_timeout_secs() -> u64 {
    10
}
fn default_max_retry_attempts() -> u32 {
    3
}

// ── OpenTelemetry ─────────────────────────────────────────────────────────────

/// Re-export `mako-service` OTel config so the rest of the crate only imports
/// from `config`.
pub use mako_service::telemetry::OtelConfig;

// ── MCP server ────────────────────────────────────────────────────────────────

/// Re-export so that `[mcp]` in `marktd.toml` maps to the shared `McpAuthConfig`.
/// Supports `api_key` (Bearer token for agentd) and optional named keys.
pub use mako_service::mcp_auth::McpAuthConfig as McpConfig;

// ── Loader + env resolution ───────────────────────────────────────────────────

/// Load configuration from a TOML file.
///
/// # Errors
///
/// Returns an error if the file cannot be read or the TOML is invalid.
pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read config file {}: {e}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("config parse error in {}: {e}", path.display()))?;
    Ok(cfg)
}

// ── MMMA/MMM price import (B12) ──────────────────────────────────────────────

/// Configuration for the automated monthly MMMA Gas / MMM Strom price import.
///
/// The import worker runs on the 1st of each month at `check_hour_utc`
/// (default 06:00 UTC, after THE publishes the monthly prices) and fetches
/// from the configured URLs.
///
/// Both Gas and Strom import URLs support:
/// - `http(s)://...` — HTTP fetch; response body must be CSV or JSON
/// - `file:///...`   — local file (for testing / CSV drop-in)
/// - Empty string    — skip this commodity
///
/// ## CSV format (THE Gas MMMA monthly file)
///
/// ```csv
/// year,month,marktgebiet,mehr_ct_kwh,minder_ct_kwh
/// 2026,7,THE,1.23,0.87
/// ```
///
/// ## JSON format
///
/// ```json
/// { "mehr_ct_kwh": "1.23", "minder_ct_kwh": "0.87", "marktgebiet": "THE" }
/// ```
///
/// A CloudEvent `de.markt.mmma.import.success` or `de.markt.mmma.import.failed`
/// is emitted to the EventBus fan-out on each run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmmaImportConfig {
    /// Whether the automated import worker is active.  Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// URL of the THE Gas MMMA CSV/JSON file.  Leave empty to skip Gas import.
    /// Example: `https://www.the-group.de/gas/market/market-area-manager/mmma`
    #[serde(default)]
    pub gas_url: String,
    /// URL of the VNB Strom MMM CSV/JSON file.  Leave empty to skip Strom import.
    /// Example: `https://www.netztransparenz.de/de-de/strommarkt/mmm`
    #[serde(default)]
    pub strom_url: String,
    /// UTC hour (0–23) at which the import runs on the 1st of each month.
    /// Default: 6 (06:00 UTC — after THE typically publishes around 05:00 UTC).
    #[serde(default = "default_mmma_check_hour")]
    pub check_hour_utc: u8,
    /// ERP webhook URL for import success/failure CloudEvents.
    /// If empty, the EventBus fan-out is used instead.
    #[serde(default)]
    pub erp_webhook_url: String,
}

fn default_mmma_check_hour() -> u8 {
    6
}

impl Default for MmmaImportConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            gas_url: String::new(),
            strom_url: String::new(),
            check_hour_utc: default_mmma_check_hour(),
            erp_webhook_url: String::new(),
        }
    }
}

/// If the value starts with `"env:"`, the remainder is looked up in the
/// environment.  Otherwise the value is returned as-is.
///
/// # Errors
///
/// Returns an error if the `env:` reference is not set in the environment.
pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in marktd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

/// Like [`resolve_env`] but wraps the result in `secrecy::SecretString`.
pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
