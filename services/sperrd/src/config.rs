//! Configuration for `sperrd`.

use serde::Deserialize;

/// Newtype for the tenant string injected as an Axum `Extension`.
///
/// Using a newtype avoids accidental collisions with other `Extension<String>` values.
#[derive(Clone, Debug)]
pub struct Tenant(pub String);

#[derive(Debug, Deserialize)]
pub struct SperrdConfig {
    pub database_url: String,
    pub port: Option<u16>,
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    /// `makod` base URL — used to dispatch IFTSTA 21039 on execution confirmation.
    pub makod_url: String,
    pub makod_api_key: String,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:SPERRD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}
