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

use serde::Deserialize;
use std::path::Path;

// ── Archive config (formerly in mako-edm::archive) ───────────────────────────

/// Iceberg/S3 archival configuration.
///
/// Set via the `[archive]` section of `edmd.toml`.
///
/// ```toml
/// [archive]
/// enabled                = true
/// storage_uri            = "s3://my-bucket/edmd/meter_reads"
/// access_key_id          = "env:AWS_ACCESS_KEY_ID"
/// secret_access_key      = "env:AWS_SECRET_ACCESS_KEY"
/// region                 = "eu-central-1"
/// retention_months       = 12
/// batch_size             = 100000
/// interval_secs          = 3600
/// ```
#[derive(Debug, Clone, serde::Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveConfig {
    /// Enable archival worker.  Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Root URI for Parquet/Iceberg data files.
    /// Supported schemes: `s3://`, `gs://`, `abfss://`, `file://`.
    pub storage_uri: String,
    /// Months of hot-tier retention.  Reads older than this are archived.
    #[serde(default = "archive_default_retention_months")]
    pub retention_months: u32,
    /// Maximum rows per archival batch.
    #[serde(default = "archive_default_batch_size")]
    pub batch_size: u32,
    /// Archival worker interval in seconds.
    #[serde(default = "archive_default_interval_secs")]
    pub interval_secs: u64,
    /// PostgreSQL schema for the Iceberg SQL-catalog tables.
    #[serde(default = "archive_default_catalog_schema")]
    pub iceberg_catalog_schema: String,
    /// Logical catalog name registered in the SQL catalog.
    #[serde(default = "archive_default_catalog_name")]
    pub iceberg_catalog_name: String,
    /// AWS access key ID.  Use `"env:AWS_ACCESS_KEY_ID"`.
    pub access_key_id: Option<String>,
    /// AWS secret access key.  Use `"env:AWS_SECRET_ACCESS_KEY"`.
    pub secret_access_key: Option<String>,
    /// AWS region.
    #[serde(default = "archive_default_aws_region")]
    pub region: String,
    /// S3-compatible endpoint override (MinIO, LocalStack, Ceph RGW).
    pub endpoint_url: Option<String>,
}

fn archive_default_retention_months() -> u32 {
    12
}
fn archive_default_batch_size() -> u32 {
    100_000
}
fn archive_default_interval_secs() -> u64 {
    3_600
}
fn archive_default_catalog_schema() -> String {
    "iceberg_catalog".to_owned()
}
fn archive_default_catalog_name() -> String {
    "edmd".to_owned()
}
fn archive_default_aws_region() -> String {
    "eu-central-1".to_owned()
}

impl Default for ArchiveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            storage_uri: String::new(),
            retention_months: archive_default_retention_months(),
            batch_size: archive_default_batch_size(),
            interval_secs: archive_default_interval_secs(),
            iceberg_catalog_schema: archive_default_catalog_schema(),
            iceberg_catalog_name: archive_default_catalog_name(),
            access_key_id: None,
            secret_access_key: None,
            region: archive_default_aws_region(),
            endpoint_url: None,
        }
    }
}

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
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:EDMD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Iceberg/S3 archival configuration.  Disabled by default.
    #[serde(default)]
    pub archive: ArchiveConfig,
    /// Request rate limits, global and per tenant. See `[rate_limit]` in TOML.
    #[serde(default)]
    pub rate_limit: mako_service::RateLimitConfig,
    /// Kafka ingest consumer. Disabled unless the section is present with
    /// `enabled = true`. See [`KafkaIngestConfig`].
    #[serde(default)]
    pub kafka_ingest: Option<KafkaIngestConfig>,
    /// Start without token verification.
    ///
    /// With `[oidc]` absent the verifier admits every request as `dev-admin`
    /// holding every market role, which satisfies every Cedar policy — including
    /// GDPR erasure and the SQL query endpoint. That posture must be asked for
    /// by name rather than reached by leaving a section out.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

/// `[kafka_ingest]` — high-throughput meter-reading intake from a Kafka topic.
///
/// ```toml
/// [kafka_ingest]
/// enabled           = true
/// bootstrap_servers = "kafka-1:9092,kafka-2:9092"
/// topic             = "edmd.meter-reads"
/// group_id          = "edmd-ingest"
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KafkaIngestConfig {
    /// Enable the consumer. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Comma-separated bootstrap servers.
    pub bootstrap_servers: String,
    /// Topic carrying the JSON batch documents.
    #[serde(default = "kafka_default_topic")]
    pub topic: String,
    /// Consumer group id.
    #[serde(default = "kafka_default_group")]
    pub group_id: String,
    /// Poll timeout in milliseconds.
    #[serde(default = "kafka_default_poll_ms")]
    pub poll_ms: u64,
}

fn kafka_default_topic() -> String {
    "edmd.meter-reads".to_owned()
}
fn kafka_default_group() -> String {
    "edmd-ingest".to_owned()
}
fn kafka_default_poll_ms() -> u64 {
    500
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

/// PostgreSQL config — shared struct from `mako-service`.
pub use mako_service::config::DatabaseConfig;

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

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

/// OpenTelemetry config — shared struct from `mako-service`.
pub use mako_service::telemetry::OtelConfig;

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
