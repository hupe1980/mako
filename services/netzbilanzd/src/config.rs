//! Configuration for `netzbilanzd`.

use serde::Deserialize;

/// Top-level configuration loaded from `netzbilanzd.toml` / env vars.
#[derive(Debug, Deserialize)]
pub struct NetzbilanzConfig {
    /// PostgreSQL connection URL.
    pub database_url: String,
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
    /// ÜNB MP-ID for this NB's Regelzone — used to auto-fetch Strom MMM
    /// (Mehr-/Mindermengen) settlement prices from `marktd` when not explicitly
    /// supplied in a billing run request.
    ///
    /// Required for `billing_type = "mmm_strom"` auto-fetch path.
    /// Identify your ÜNB from BDEW Codenummernbericht or
    /// `marktd GET /api/v1/partners` (rol: ÜNB).
    pub unb_mp_id: Option<String>,
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
