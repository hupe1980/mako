//! Configuration for `tarifbd`.

use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct TarifbdConfig {
    pub database_url: String,

    /// HTTP listen port.  Defaults to `9080`.
    pub port: Option<u16>,

    /// Operator BDEW-Codenummer used as the default `lf_mp_id` when not
    /// supplied in the request.
    pub tenant: String,

    /// ERP webhook URL for `de.tarifd.product.updated` notifications.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for outbound webhook signing.
    pub erp_hmac_secret: Option<String>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:TARIFBD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}
