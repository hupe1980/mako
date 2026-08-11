//! Configuration for `sperrd`.

use serde::Deserialize;

/// Newtype for the tenant string injected as an Axum `Extension`.
///
/// Using a newtype avoids accidental collisions with other `Extension<String>` values.
#[derive(Clone, Debug)]
pub struct Tenant(pub String);

#[derive(Debug, Deserialize)]
pub struct SperrdConfig {
    pub database: mako_service::config::DatabaseConfig,
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
    /// OIDC token verification for the REST API.
    ///
    /// Required: `execute` and `fail` each dispatch a real IFTSTA 21039 into the
    /// market, and `create` schedules a physical disconnection. The service does
    /// not start without it unless `allow_insecure_no_auth` is set explicitly.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,
    /// Start without token verification.
    ///
    /// Intended for local development. It must be named in the config rather
    /// than reached by omitting a section, so that running unauthenticated is
    /// always a decision someone wrote down.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl mako_service::ServiceConfig for SperrdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(8780))
    }
}
