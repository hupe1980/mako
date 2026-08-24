//! Configuration for `productd`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ProductdConfig {
    /// PostgreSQL connection + pool tuning (`[database]` block).
    pub database: mako_service::config::DatabaseConfig,

    /// HTTP listen port.  Defaults to `9080`.
    pub port: Option<u16>,

    /// Tenant identifier — the data-isolation key written to every row and
    /// enforced on every token.
    pub tenant: String,

    /// The Lieferant market-partner ID this catalogue's products are sold
    /// under, and the default for requests that do not name one.
    ///
    /// Distinct from `tenant`: the isolation key and the market identity are
    /// the same string in a single-mandant install and different in a shared
    /// one. Using `tenant` for both files every MaLo→product assignment under a
    /// market party that does not trade, where `billingd`'s lookup by the real
    /// MP-ID finds nothing. Defaults to `tenant` so a single-mandant deployment
    /// need not repeat itself.
    #[serde(default, rename = "lf_mp_id")]
    lf_mp_id_override: Option<String>,

    /// OIDC/JWT authentication configuration.
    ///
    /// When absent, auth is **disabled** (dev mode only).  Must be set in
    /// production.  All REST management endpoints require a valid Bearer
    /// token; only the public comparison feed is unauthenticated.
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// ERP webhook URL for `de.tarif.product.updated` notifications.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for outbound webhook signing.
    /// When absent, the signature headers are omitted from outbound
    /// CloudEvent webhooks.  Required in production.
    pub erp_hmac_secret: Option<String>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:PRODUCTD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

impl ProductdConfig {
    /// The market identity products are sold under.
    #[must_use]
    pub fn lf_mp_id(&self) -> &str {
        self.lf_mp_id_override.as_deref().unwrap_or(&self.tenant)
    }
}

impl mako_service::ServiceConfig for ProductdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9080))
    }
}
