//! `edmd` configuration — loaded from `edmd.toml` + `env:` substitution.
//!
//! # Minimal `edmd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8380"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant = "9900357000004"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:EDMD_MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:EDMD_INBOUND_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://edmd:8380/webhook"
//! subscriber_id = "edmd"
//!
//! # Iceberg/S3 archival — offloads meter_reads > retention_months to Parquet
//! # [archive]
//! # enabled           = true
//! # storage_uri       = "s3://my-bucket/edmd/meter_reads"
//! # access_key_id     = "env:AWS_ACCESS_KEY_ID"
//! # secret_access_key = "env:AWS_SECRET_ACCESS_KEY"
//! # region            = "eu-central-1"
//! # retention_months  = 12
//! # batch_size        = 100000
//! # interval_secs     = 3600
//! # # Optional: register with Nessie/Polaris/AWS Glue REST catalog
//! # iceberg_catalog_url = "http://nessie:19120/iceberg/v1"
//!
//! # [oidc]
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-edmd"
//! # [otel]
//! # endpoint = "http://otel-collector:4317"
//! ```

use mako_edm::archive::ArchiveConfig;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub marktd: MarktdConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
    /// Iceberg/S3 archival configuration.  Disabled by default.
    #[serde(default)]
    pub archive: ArchiveConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8380".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_http_addr(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatabaseConfig {
    /// PostgreSQL URL.  Use `"env:DATABASE_URL"` to defer to the environment.
    pub url: String,
    #[serde(default = "default_pool_size")]
    pub pool_size: u32,
}

fn default_pool_size() -> u32 {
    10
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Tenant identifier written to every DB row and used in Cedar resource checks.
    pub tenant: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token / API key.  Use `"env:EDMD_MARKTD_API_KEY"`.
    pub api_key: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Use `"env:EDMD_INBOUND_SECRET"`.
    pub inbound_secret: Option<String>,
    /// ERP webhook URL for outbound CloudEvents (`de.edmd.reading.direct.stored`,
    /// `de.edmd.reading.quality.warning`). Omit to disable outbound notifications.
    pub erp_webhook_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST events to.
    pub webhook_url: String,
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
    /// Comma-separated CloudEvent types.
    #[serde(default = "default_event_types")]
    pub event_types: Vec<String>,
}

fn default_subscriber_id() -> String {
    "edmd".to_owned()
}
fn default_event_types() -> Vec<String> {
    vec![
        "de.mako.process.initiated".to_owned(),
        "de.mako.process.completed".to_owned(),
        "de.mako.edifact.inbound".to_owned(),
    ]
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: "http://edmd:8380/webhook".to_owned(),
            subscriber_id: default_subscriber_id(),
            event_types: default_event_types(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    #[serde(default = "default_jwks_refresh_secs")]
    pub jwks_refresh_secs: u64,
}

fn default_jwks_refresh_secs() -> u64 {
    300
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct OtelConfig {
    pub endpoint: Option<String>,
}

pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read config file {}: {e}", path.display()))?;
    toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("config parse error in {}: {e}", path.display()))
}

pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in edmd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
