//! `obsd` configuration — loaded from `obsd.toml` + `env:` substitution.
//!
//! # Minimal `obsd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8480"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant     = "9900357000004"
//! # All operator MP-IDs for §20 EnWG affiliate detection.
//! # Include both Strom (BDEW 99…) and Gas (DVGW 98…) codes
//! # when running an integrated NB+GNB deployment.
//! own_mp_ids = ["9900357000004", "9800357000004"]
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:OBSD_MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:OBSD_INBOUND_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://obsd:8480/webhook"
//! subscriber_id = "obsd"
//!
//! # [oidc]
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-obsd"
//! # [otel]
//! # endpoint = "http://otel-collector:4317"
//! ```

use serde::Deserialize;
use std::path::Path;

// NOTE: no `deny_unknown_fields` on the top-level struct — `mako_service::run`
// loads config via `load_config`, whose env layer (`OBSD_*`) also surfaces the
// `OBSD_CONFIG` path variable as a stray `config` key. Rejecting unknown fields
// here would make every deployment that points at its config via that variable
// fail to start. Nested blocks keep `deny_unknown_fields`.
#[derive(Debug, Deserialize)]
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
    pub worker: WorkerConfig,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:OBSD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

impl mako_service::ServiceConfig for Config {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8480".to_owned()
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
    /// Tenant identifier used in Cedar resource checks.
    pub tenant: String,
    /// All operator MP-IDs used by this deployment.
    ///
    /// Include every MP-ID the operator acts under:
    /// - NB Strom BDEW-Codenummer (`99…`)
    /// - GNB Gas  DVGW-Codenummer (`98…`)
    /// - MSB / nMSB codes if applicable
    ///
    /// Used to compute `initiator_is_affiliate` for §20 EnWG parity reporting:
    /// when `data.new_supplier` matches **any** of these MP-IDs, the initiating
    /// LF is a subsidiary of the operator (vertically integrated utility).
    ///
    /// Defaults to `[tenant]` when absent (single-MP-ID deployments).
    #[serde(default)]
    pub own_mp_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token.  Use `"env:OBSD_MARKTD_API_KEY"`.
    pub api_key: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Use `"env:OBSD_INBOUND_SECRET"`.
    pub inbound_secret: Option<String>,
    /// Outbound CloudEvent target for the `de.obs.*` events obsd produces
    /// (deadline.approaching, stp.parity.alert). In production this is the
    /// `marktd` event-ingest endpoint (`…/api/v1/mako/events`), whose fan-out
    /// delivers to the `agentd` subscribers. When `None` the sweep workers do
    /// not run. Use `"env:OBSD_OUTBOUND_URL"`.
    pub outbound_url: Option<String>,
    /// HMAC-SHA256 secret for signing outbound `de.obs.*` CloudEvents
    /// (`X-Mako-Signature: sha256=…`). Must match the target's inbound secret.
    /// Use `"env:OBSD_OUTBOUND_SECRET"`.
    pub outbound_secret: Option<String>,
}

/// Background sweep-worker tuning for the `de.obs.*` producers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerConfig {
    /// Interval between deadline sweeps, seconds. Default 900 (15 min).
    #[serde(default = "default_deadline_sweep_secs")]
    pub deadline_sweep_secs: u64,
    /// A process is "approaching" its deadline when `deadline_at` is within this
    /// many hours of now. Default 24 (the Amber window).
    #[serde(default = "default_deadline_warn_hours")]
    pub deadline_warn_hours: i64,
    /// Interval between §20 EnWG parity sweeps, seconds. Default 86400 (daily).
    #[serde(default = "default_parity_sweep_secs")]
    pub parity_sweep_secs: u64,
    /// Parity-gap threshold in percentage points above which
    /// `de.obs.stp.parity.alert` fires. Default 5.0 (BNetzA scrutiny threshold).
    #[serde(default = "default_parity_threshold_pp")]
    pub parity_threshold_pp: f64,
    /// Look-back window for the parity computation, days. Default 90.
    #[serde(default = "default_parity_window_days")]
    pub parity_window_days: i32,
}

fn default_deadline_sweep_secs() -> u64 {
    900
}
fn default_deadline_warn_hours() -> i64 {
    24
}
fn default_parity_sweep_secs() -> u64 {
    86_400
}
fn default_parity_threshold_pp() -> f64 {
    5.0
}
fn default_parity_window_days() -> i32 {
    90
}

impl Default for WorkerConfig {
    fn default() -> Self {
        Self {
            deadline_sweep_secs: default_deadline_sweep_secs(),
            deadline_warn_hours: default_deadline_warn_hours(),
            parity_sweep_secs: default_parity_sweep_secs(),
            parity_threshold_pp: default_parity_threshold_pp(),
            parity_window_days: default_parity_window_days(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST events to.
    pub webhook_url: String,
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
    #[serde(default = "default_event_types")]
    pub event_types: Vec<String>,
}

fn default_subscriber_id() -> String {
    "obsd".to_owned()
}
fn default_event_types() -> Vec<String> {
    vec![
        mako_events::mako::PROCESS_INITIATED.to_owned(),
        mako_events::mako::PROCESS_COMPLETED.to_owned(),
        mako_events::mako::APERAK_ACCEPTED.to_owned(),
        mako_events::mako::APERAK_TIMEOUT.to_owned(),
        mako_events::mako::PROCESS_FAILED.to_owned(),
        mako_events::mako::APERAK_REJECTED.to_owned(),
    ]
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: "http://obsd:8480/webhook".to_owned(),
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
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in obsd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
