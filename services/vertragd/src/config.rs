//! Configuration for `vertragd`.

use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct VertragdConfig {
    pub database_url: String,
    pub port: Option<u16>,
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    pub lf_mp_id: String,
    /// `processd` — triggers Lieferbeginn/Lieferende per Vertragskomponente.
    pub processd_url: String,
    pub processd_api_key: Option<String>,
    /// `tarifbd` — product assignment after MaKo confirmation.
    pub tarifbd_url: String,
    pub tarifbd_api_key: Option<String>,
    /// `accountingd` — provision billing account on Vertrag AKTIV.
    pub accountingd_url: String,
    pub accountingd_api_key: Option<String>,
    /// `edmd` — trigger Ablesesteuerung reading orders.
    pub edmd_url: String,
    pub edmd_api_key: Option<String>,
    /// ERP webhook — receives `de.vertrag.*` CloudEvents.
    pub erp_webhook_url: Option<String>,
    pub erp_hmac_secret: Option<String>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:VERTRAGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Operator escalation after N Werktage without MaKo response.
    pub mako_timeout_werktage: Option<u32>,
}
