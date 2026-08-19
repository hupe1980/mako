//! `marktd` configuration.
//!
//! Loaded by [`mako_service::load_config`], so the layering is the platform's:
//! `marktd.toml` (path overridable with `MARKTD_CONFIG`) as the base, then
//! `MARKTD_*` environment variables with `__` as the section separator, then
//! `*_FILE` variables read from a file for Kubernetes/Swarm secrets. A
//! container therefore needs no config file at all — every key can arrive as an
//! environment variable:
//!
//! ```text
//! MARKTD_DATABASE__URL=postgres://…            # [database] url
//! MARKTD_MARKT__TENANT=9900357000004           # [markt] tenant
//! MARKTD_MAKOD__API_KEY_FILE=/run/secrets/key  # [makod] api_key, from a file
//! ```
//!
//! # Example `marktd.toml`
//!
//! ```toml
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [http]
//! addr = "0.0.0.0:8180"
//!
//! [markt]
//! tenant = "9900357000004"
//!
//! [oidc]
//! issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! audience = "api://mako-markt"
//!
//! [makod]
//! base_url = "http://makod:8080"
//! api_key  = "env:MAKOD_API_KEY"
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

pub use mako_service::config::{DatabaseConfig, HttpConfig, resolve_env, resolve_env_secret};
use mako_service::service::ServiceConfig;

// ── Top-level config ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `[database]` — connection URL plus the shared pool tuning knobs.
    pub database: DatabaseConfig,
    #[serde(default = "default_http")]
    pub http: HttpConfig,
    /// This deployment's own identity.
    pub markt: MarktConfig,
    /// OIDC configuration. When omitted, authentication is **disabled** and all
    /// API requests are accepted with synthetic dev-admin claims — which is why
    /// `allow_insecure_no_auth` must then be set explicitly.
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    pub makod: MakodConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    /// Automated monthly MMMA Gas / MMM Strom price import. When omitted or
    /// `enabled = false`, prices are imported by the ERP via
    /// `PUT /api/v1/mmma-preise/gas/{year}/{month}`.
    #[serde(default)]
    pub mmma_import: MmmaImportConfig,
    /// Start without token verification AND without inbound webhook signing.
    ///
    /// Without `[oidc]` every request is admitted with synthetic dev claims,
    /// and without `webhook.inbound_secret` the inbound events endpoint accepts
    /// unsigned events that mutate VersorgungsStatus and the device registry.
    /// Both postures must be asked for by name — startup refuses when either is
    /// missing unless this flag is set.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl ServiceConfig for Config {
    fn database(&self) -> Option<&DatabaseConfig> {
        Some(&self.database)
    }

    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

fn default_http() -> HttpConfig {
    HttpConfig {
        addr: "0.0.0.0:8180".to_owned(),
    }
}

// ── Identity ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktConfig {
    /// Tenant identifier — this deployment's own operator identity. Typically
    /// the BDEW- or DVGW-Codenummer. It is the `resource_tenant` every Cedar
    /// check compares the caller's `mako_tenant` claim against, the `tenant`
    /// column written on tenant-scoped rows, and the source URN of every
    /// outbound CloudEvent.
    pub tenant: String,
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

// ── MMMA/MMM price import ────────────────────────────────────────────────────

/// Configuration for the automated monthly MMMA Gas / MMM Strom price import.
///
/// The import worker runs on the 1st of each month at `check_hour_utc`
/// (default 06:00 UTC, after THE publishes the monthly prices) and fetches
/// from the configured URLs.
///
/// Both import URLs support:
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
/// is emitted to the durable fan-out on each run.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MmmaImportConfig {
    /// Whether the automated import worker is active.  Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// URL of the THE Gas MMMA CSV/JSON file.  Leave empty to skip Gas import.
    #[serde(default)]
    pub gas_url: String,
    /// URL of the VNB Strom MMM CSV/JSON file.  Leave empty to skip Strom import.
    #[serde(default)]
    pub strom_url: String,
    /// UTC hour (0–23) at which the import runs on the 1st of each month.
    /// Default: 6 (06:00 UTC — after THE typically publishes around 05:00 UTC).
    #[serde(default = "default_mmma_check_hour")]
    pub check_hour_utc: u8,
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
        }
    }
}
