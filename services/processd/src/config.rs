//! `processd` configuration — loaded from `processd.toml` + `env:` substitution.
//!
//! All secrets can be deferred to environment variables by writing `"env:VAR_NAME"`
//! as the value in the TOML file.  Call [`resolve_env`] / [`resolve_env_secret`]
//! before use.
//!
//! # Minimal `processd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8580"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! own_mp_id = "9900357000004"
//!
//! [makod]
//! url     = "http://makod:8080"
//! api_key = "env:MAKOD_API_KEY"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:INBOUND_WEBHOOK_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://processd:8580/webhook"
//! subscriber_id = "processd"
//!
//! [nb]
//! auto_accept = false   # true: dispatch bestaetigen automatically on Accept
//!
//! [lf]
//! auto_respond   = true
//! queue_ttl_secs = 2700  # 45 min — LFW24 E_0624 window
//!
//! # [oidc]                # omit to disable auth (dev mode only)
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-processd"
//! #
//! # [otel]                # omit to disable tracing
//! # endpoint = "http://otel-collector:4317"
//! ```

use serde::Deserialize;
use std::path::Path;

// ── Top-level ─────────────────────────────────────────────────────────────────

/// Full `processd.toml` configuration.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub makod: MakodConfig,
    pub marktd: MarktdConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub nb: NbConfig,
    #[serde(default)]
    pub lf: LfConfig,
    #[serde(default)]
    pub msb: MsbConfig,
    /// OIDC configuration.  When omitted, authentication is **disabled** and
    /// all API requests are accepted with synthetic dev-admin claims.
    /// **Never omit this in production.**
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
    /// MCP server authentication. Supports OIDC + API-key fallback, or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:PROCESSD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

// ── HTTP ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8580".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_http_addr(),
        }
    }
}

// ── Database ──────────────────────────────────────────────────────────────────

/// PostgreSQL config — shared struct from `mako-service`.
pub use mako_service::config::DatabaseConfig;

// ── Identity ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Operator primary MP-ID (BDEW-Codenummer starting with `99`, or DVGW `98`).
    ///
    /// Must match `makod.toml` `[[party]] primary = true`.
    /// Used for `initiator_is_affiliate` §20 EnWG parity reporting.
    pub own_mp_id: String,
    /// Tenant identifier written to every DB row.  Defaults to `own_mp_id`.
    #[serde(default)]
    pub tenant: String,
}

// ── makod connection ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL.  Example: `http://makod:8080`
    pub url: String,
    /// Bearer token / API key.  Use `"env:MAKOD_API_KEY"`.
    pub api_key: String,
}

// ── marktd connection ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token / API key.  Use `"env:MARKTD_API_KEY"`.
    pub api_key: String,
}

// ── Inbound webhook ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Must match `marktd`'s subscription `webhook_secret`.
    /// Leave unset to disable signature verification (dev only).
    /// Use `"env:INBOUND_WEBHOOK_SECRET"`.
    pub inbound_secret: Option<String>,
}

// ── Self-registration with marktd ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubscriptionConfig {
    /// URL that `marktd` will POST `de.mako.process.initiated` events to.
    ///
    /// When set, `processd` calls `PUT {marktd.url}/api/v1/subscriptions/{subscriber_id}`
    /// on startup and self-registers as a subscriber.  Retries for up to 30 s to
    /// tolerate `marktd` startup ordering.
    ///
    /// Typical value: `http://<processd-service-dns>:8580/webhook`
    pub webhook_url: Option<String>,
    /// Unique subscription ID for this deployment.  Used as the path segment in
    /// `PUT /api/v1/subscriptions/:id` — idempotent upsert.
    #[serde(default = "default_subscriber_id")]
    pub subscriber_id: String,
    /// Comma-separated CloudEvent types to subscribe to.
    #[serde(default = "default_event_types")]
    pub event_types: String,
}

fn default_subscriber_id() -> String {
    "processd".to_owned()
}
fn default_event_types() -> String {
    "de.mako.process.initiated".to_owned()
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: None,
            subscriber_id: default_subscriber_id(),
            event_types: default_event_types(),
        }
    }
}

// ── NB module ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct NbConfig {
    /// When `true`, `processd` dispatches `bestaetigen` automatically on `Accept`.
    ///
    /// When `false` (default), decisions are written to `anmeldung_decisions` but
    /// `bestaetigen` is NOT dispatched — operator must approve via
    /// `PUT /api/v1/queue/{id}/approve`.  Activate only after verifying grid
    /// record and partner coverage (STP target ≥ 95 %).
    #[serde(default)]
    pub auto_accept: bool,
}

// ── LF module ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LfConfig {
    /// When `true` (default), `processd` automatically dispatches
    /// `einwilligung` / `ablehnen` for E_0624 queries without ERP involvement.
    #[serde(default = "default_lf_auto_respond")]
    pub auto_respond: bool,
    /// Approval queue entry TTL in seconds.  Default: 2700 (= 45 min, LFW24 window).
    ///
    /// Entries older than this are auto-expired (status = `Expired`).
    #[serde(default = "default_queue_ttl_secs")]
    pub queue_ttl_secs: u64,
}

fn default_lf_auto_respond() -> bool {
    true
}
fn default_queue_ttl_secs() -> u64 {
    2700
}

impl Default for LfConfig {
    fn default() -> Self {
        Self {
            auto_respond: default_lf_auto_respond(),
            queue_ttl_secs: default_queue_ttl_secs(),
        }
    }
}

// ── MSB module ─────────────────────────────────────────────────────────────────

/// MSB process automation configuration.
///
/// When `auto_preisanfrage = true` (default), `processd` automatically dispatches
/// a QUOTES response when a REQOTE Preisanfrage (PIDs 35001–35005) arrives,
/// sourcing prices from the current `PreisblattMessung` in `marktd`.
///
/// If no active `PreisblattMessung` exists for the aMSB MP-ID, the auto-response
/// is skipped and the REQOTE is escalated to the operator.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MsbConfig {
    /// When `true` (default), dispatch QUOTES automatically from `PreisblattMessung`.
    /// Set `false` to require manual QUOTES dispatch via ERP.
    #[serde(default = "default_msb_auto_preisanfrage")]
    pub auto_preisanfrage: bool,
}

fn default_msb_auto_preisanfrage() -> bool {
    true
}

impl Default for MsbConfig {
    fn default() -> Self {
        Self {
            auto_preisanfrage: default_msb_auto_preisanfrage(),
        }
    }
}

// ── OIDC ──────────────────────────────────────────────────────────────────────

/// OIDC configuration — re-exported from `mako-service` (shared across all daemons).
pub use mako_service::oidc::OidcConfig;

// ── OpenTelemetry ─────────────────────────────────────────────────────────────

/// OpenTelemetry config — shared struct from `mako-service`.
pub use mako_service::telemetry::OtelConfig;

// ── Loader + env resolution ───────────────────────────────────────────────────

/// Load configuration from a TOML file.
///
/// # Errors
///
/// Returns an error if the file cannot be read or the TOML is malformed.
pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read config file {}: {e}", path.display()))?;
    let cfg: Config = toml::from_str(&text)
        .map_err(|e| anyhow::anyhow!("config parse error in {}: {e}", path.display()))?;
    Ok(cfg)
}

/// Resolve an `"env:VAR_NAME"` reference or return the value as-is.
///
/// # Errors
///
/// Returns an error if the `env:` variable is not set.
pub fn resolve_env(value: &str) -> anyhow::Result<String> {
    if let Some(var) = value.strip_prefix("env:") {
        std::env::var(var).map_err(|_| {
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in processd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

/// Like [`resolve_env`] but wraps the result in `secrecy::SecretString`.
pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
