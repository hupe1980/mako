//! `invoicd` configuration — loaded from `invoicd.toml` + `env:` substitution.
//!
//! # Minimal `invoicd.toml`
//!
//! ```toml
//! [http]
//! addr = "0.0.0.0:8280"
//!
//! [database]
//! url = "env:DATABASE_URL"
//!
//! [identity]
//! tenant = "9900357000004"
//!
//! [makod]
//! url     = "http://makod:8080"
//! api_key = "env:INVOICD_MAKOD_API_KEY"
//!
//! [marktd]
//! url     = "http://marktd:8180"
//! api_key = "env:INVOICD_MARKTD_API_KEY"
//!
//! [webhook]
//! inbound_secret = "env:INVOICD_INBOUND_SECRET"
//!
//! [subscription]
//! webhook_url   = "http://invoicd:8280/webhook"
//! subscriber_id = "invoicd"
//!
//! [check]
//! # All tolerances are relative (0.01 = 1 %)
//! arithmetic_tolerance = 0.01
//! total_tolerance      = 0.01
//! tariff_tolerance     = 0.03
//! require_tariff       = false
//! # EUR above which a Warn outcome triggers a Dispute (0.0 = never)
//! auto_dispute_threshold_eur = 0.0
//!
//! # [oidc]
//! # issuer   = "https://login.microsoftonline.com/{tenant-id}/v2.0"
//! # audience = "api://mako-invoicd"
//! # [otel]
//! # endpoint = "http://otel-collector:4317"
//! ```

use invoic_checker::CheckConfig;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    /// Required for §22 MessZV / §41 EnWG compliance (3-year receipt retention).
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub makod: MakodConfig,
    pub marktd: MarktdConfig,
    #[serde(default)]
    pub webhook: WebhookConfig,
    #[serde(default)]
    pub subscription: SubscriptionConfig,
    #[serde(default)]
    pub check: CheckSectionConfig,
    /// Optional `edmd` connection — required for PID 31006 selbstausstellen.
    #[serde(default)]
    pub edmd: EdmdConfig,
    /// Optional ERP webhook for outbound payment CloudEvents (A8).
    #[serde(default)]
    pub erp: ErpConfig,
    #[serde(default)]
    pub oidc: Option<OidcConfig>,
    #[serde(default)]
    pub otel: OtelConfig,
}

impl Config {
    #[must_use]
    pub fn check_config(&self) -> CheckConfig {
        CheckConfig {
            arithmetic_tolerance_ppm: (self.check.arithmetic_tolerance * 1_000_000.0).round()
                as u32,
            total_tolerance_ppm: (self.check.total_tolerance * 1_000_000.0).round() as u32,
            tariff_tolerance_ppm: (self.check.tariff_tolerance * 1_000_000.0).round() as u32,
            require_tariff: self.check.require_tariff,
            max_zahlungsziel_days: self.check.max_zahlungsziel_days,
        }
    }

    /// EUR-cents threshold (converted from the TOML EUR value).
    #[must_use]
    pub fn auto_dispute_threshold_eur_cents(&self) -> i64 {
        (self.check.auto_dispute_threshold_eur * 100.0_f64).round() as i64
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_http_addr")]
    pub addr: String,
}

fn default_http_addr() -> String {
    "0.0.0.0:8280".to_owned()
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
    #[serde(default = "default_max_connections")]
    pub max_connections: u32,
}

fn default_max_connections() -> u32 {
    5
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Tenant identifier written to every receipt row.
    pub tenant: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL.  Example: `http://makod:8080`
    pub url: String,
    /// API key for the `makod` command endpoint.  Use `"env:INVOICD_MAKOD_API_KEY"`.
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL.  Example: `http://marktd:8180`
    pub url: String,
    /// Bearer token.  Use `"env:INVOICD_MARKTD_API_KEY"`.
    pub api_key: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
/// Optional `edmd` connection — required for `POST /api/v1/selbstausstellen` (PID 31006).
///
/// When omitted, selbstausstellen returns 503 (edmd not configured).
pub struct EdmdConfig {
    /// `edmd` base URL.  Example: `http://edmd:8380`
    #[serde(default)]
    pub url: Option<String>,
    /// Bearer token.  Use `"env:INVOICD_EDMD_API_KEY"`.
    #[serde(default)]
    pub api_key: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// HMAC-SHA256 secret for verifying inbound webhooks from `marktd`.
    /// Use `"env:INVOICD_INBOUND_SECRET"`.
    pub inbound_secret: Option<String>,
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
    "invoicd".to_owned()
}
fn default_event_types() -> Vec<String> {
    vec![
        "de.mako.process.initiated".to_owned(),
        "de.mako.process.completed".to_owned(),
    ]
}

impl Default for SubscriptionConfig {
    fn default() -> Self {
        Self {
            webhook_url: "http://invoicd:8280/webhook".to_owned(),
            subscriber_id: default_subscriber_id(),
            event_types: default_event_types(),
        }
    }
}

/// `invoic-checker` plausibility tolerances and dispute policy.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckSectionConfig {
    /// Per-line arithmetic tolerance as a fraction (0.01 = 1 %).
    /// Converted to ppm when building `CheckConfig`.
    #[serde(default = "default_arithmetic_tolerance")]
    pub arithmetic_tolerance: f64,
    /// Document-total tolerance as a fraction (0.01 = 1 %).
    #[serde(default = "default_total_tolerance")]
    pub total_tolerance: f64,
    /// Tariff unit-price tolerance as a fraction (0.03 = 3 %).
    #[serde(default = "default_tariff_tolerance")]
    pub tariff_tolerance: f64,
    /// When `true`, a missing PRICAT entry escalates from `Warn` to `Dispute`.
    #[serde(default)]
    pub require_tariff: bool,
    /// INVOIC net-amount (EUR) above which a `Warn` outcome becomes a `Dispute`.
    /// `0.0` (default) means `Warn` is always auto-approved.
    #[serde(default)]
    pub auto_dispute_threshold_eur: f64,
    /// Maximum Zahlungsziel (payment term) in days from `rechnungsdatum` to
    /// `faelligkeitsdatum` (DTM+92). Set to `0` to disable this check.
    /// Default: `30` days per §7 Allgemeine Festlegungen V6.1d.
    #[serde(default = "default_max_zahlungsziel_days")]
    pub max_zahlungsziel_days: u16,
}

fn default_arithmetic_tolerance() -> f64 {
    0.01
}
fn default_total_tolerance() -> f64 {
    0.01
}
fn default_tariff_tolerance() -> f64 {
    0.03
}
fn default_max_zahlungsziel_days() -> u16 {
    30
}

impl Default for CheckSectionConfig {
    fn default() -> Self {
        Self {
            arithmetic_tolerance: default_arithmetic_tolerance(),
            total_tolerance: default_total_tolerance(),
            tariff_tolerance: default_tariff_tolerance(),
            require_tariff: false,
            auto_dispute_threshold_eur: 0.0,
            max_zahlungsziel_days: default_max_zahlungsziel_days(),
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

/// ERP integration config — outbound payment CloudEvents.
///
/// When `webhook_url` is set, `invoicd` POSTs a `de.invoic.receipt.settled`
/// or `de.invoic.receipt.disputed` CloudEvent after each validated INVOIC.
///
/// Example TOML:
/// ```toml
/// [erp]
/// webhook_url = "https://erp.example.com/webhooks/invoicd"
/// hmac_secret = "env:INVOICD_ERP_HMAC_SECRET"
/// ```
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ErpConfig {
    /// ERP webhook URL for `de.invoic.receipt.*` CloudEvents.
    pub webhook_url: Option<String>,
    /// Optional HMAC-SHA256 secret for `X-Mako-Signature` on outbound requests.
    /// Use `"env:INVOICD_ERP_HMAC_SECRET"` to load from environment.
    pub hmac_secret: Option<String>,
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
            anyhow::anyhow!("environment variable {var:?} is not set (referenced in invoicd.toml)")
        })
    } else {
        Ok(value.to_owned())
    }
}

pub fn resolve_env_secret(value: &str) -> anyhow::Result<secrecy::SecretString> {
    resolve_env(value).map(secrecy::SecretString::from)
}
