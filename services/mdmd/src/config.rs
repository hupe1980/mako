//! `mdmd` configuration — loaded from `mdmd.toml` + environment overrides.
//!
//! Values can be resolved from the environment by using `"env:VAR_NAME"` as
//! the string value in the TOML file.  Call [`resolve_env_secret`] for
//! sensitive fields before use.
//!
//! # Example `mdmd.toml`
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
//! audience = "api://mako-mdm"
//!
//! [makod]
//! base_url  = "http://makod:8080"
//! api_key   = "env:MAKOD_API_KEY"
//! tenant_id = "9900357000004"
//!
//! [webhook]
//! inbound_path   = "/api/v1/mako/events"
//! inbound_secret = "env:MAKOD_WEBHOOK_SECRET"
//! delivery_timeout_secs = 10
//! max_retry_attempts    = 3
//!
//! [otel]
//! endpoint     = "http://otel-collector:4317"
//! service_name = "mdmd"
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
    pub oidc: OidcConfig,
    pub makod: MakodConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub mcp: McpConfig,
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
    /// PostgreSQL URL, e.g. `postgres://mdmd:secret@postgres:5432/mdmd`.
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    /// OIDC issuer URL (without trailing slash).
    /// Example: `https://login.microsoftonline.com/{tenant-id}/v2.0`
    pub issuer: String,
    /// JWT `aud` claim expected value.
    /// Example: `api://mako-mdm`
    pub audience: String,
    #[serde(default = "default_jwks_refresh_secs")]
    pub jwks_refresh_secs: u64,
}

fn default_jwks_refresh_secs() -> u64 {
    300
}

// ── MaKod client ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// Base URL of the `makod` admin API.  Example: `http://makod:8080`.
    pub base_url: String,
    /// Bearer token / API key.  Use `"env:MAKOD_API_KEY"` for env-var resolution.
    pub api_key: String,
    /// Primary GLN of the tenant (BDEW-Codenummer starting with 99).
    /// Used as the `tenant_gln` in outbound CloudEvents source URN.
    pub tenant_id: String,
}

// ── Inbound webhooks (from makod) ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// URL path on which mdmd listens for CloudEvents from makod.
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

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OtelConfig {
    #[serde(default = "default_otel_endpoint")]
    pub endpoint: Option<String>,
    #[serde(default = "default_service_name")]
    pub service_name: String,
}

fn default_otel_endpoint() -> Option<String> {
    None
}
fn default_service_name() -> String {
    "mdmd".to_owned()
}

// ── MCP server ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpConfig {
    #[serde(default = "default_mcp_path")]
    pub path: String,
    #[serde(default)]
    pub enabled: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            path: default_mcp_path(),
            enabled: false,
        }
    }
}

fn default_mcp_path() -> String {
    "/mcp".to_owned()
}

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

/// Resolve a string value that may contain an `"env:VAR_NAME"` reference.
///
/// If the value starts with `"env:"`, the remainder is looked up in the
/// environment.  Otherwise the value is returned as-is.
///
/// # Errors
///
/// Returns an error if the `env:` reference is not set in the environment.
pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in mdmd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

/// Like [`resolve_env`] but wraps the result in `secrecy::SecretString`.
pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
