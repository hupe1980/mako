//! Configuration for `netzbilanzd`.

use serde::Deserialize;

/// Top-level configuration loaded from `netzbilanzd.toml` / env vars.
#[derive(Debug, Deserialize)]
pub struct NetzbilanzConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,
    /// HTTP port (default 8680).
    pub port: Option<u16>,
    /// Tenant identifier for multi-tenant deployments. Defaults to `"default"`.
    #[serde(default = "default_tenant")]
    pub tenant: String,
    /// `marktd` base URL for tariff lookups.
    pub marktd_url: String,
    /// `marktd` API key.
    pub marktd_api_key: String,
    /// `makod` base URL for command dispatch.
    pub makod_url: String,
    /// `makod` API key.
    pub makod_api_key: String,
    /// `edmd` base URL — auto-fetches imbalance data for MMM auto-run (N6).
    pub edmd_url: Option<String>,
    /// `edmd` bearer token.
    pub edmd_api_key: Option<String>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:NETZBILANZD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Optional ERP webhook URL — receives CloudEvents
    /// `de.netzbilanz.invoic.drafted` and `de.netzbilanz.invoic.dispatched`.
    pub erp_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing the outbound ERP webhook CloudEvents
    /// (`X-Mako-Signature: sha256=<hex>`). Use `env:VAR_NAME`. Leave unset only
    /// in dev — a receiver verifying the signature rejects unsigned events.
    pub erp_webhook_secret: Option<String>,
    /// HMAC-SHA256 secret for verifying INBOUND REMADV CloudEvents on
    /// `POST /webhooks/remadv`. When unset the endpoint accepts unsigned bodies
    /// (dev mode); set it in production so a forged REMADV cannot mark a
    /// Bilanzkreis INVOIC paid/disputed.
    #[serde(default)]
    pub inbound_secret: Option<String>,
    /// VNB MP-ID used to auto-fetch this operator's published Strom MMM
    /// (Mehr-/Mindermengen) settlement prices from `marktd` when not explicitly
    /// supplied in a billing run request.
    ///
    /// Required for `billing_type = "mmm_strom"` auto-fetch path.
    /// Identify your ÜNB from BDEW Codenummernbericht or
    /// `marktd GET /api/v1/partners` (rol: ÜNB).
    pub vnb_mp_id: Option<String>,
    /// How often (seconds) to check for undispatched drafts older than 48 h.
    /// Default: 3600 (1 hour). Set to 0 to disable.
    pub dispatch_alert_interval_secs: Option<u64>,
    /// How often (seconds) to check for pending Kostenblatt near the 15th-of-month deadline.
    /// Default: 86400 (1 day). Set to 0 to disable.
    pub kostenblatt_alert_interval_secs: Option<u64>,
}

fn default_tenant() -> String {
    "default".to_owned()
}

impl mako_service::ServiceConfig for NetzbilanzConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(8680))
    }
}
