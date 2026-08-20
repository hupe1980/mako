//! Configuration for `sperrd`.

use secrecy::SecretString;
use serde::Deserialize;

/// Newtype for the tenant string injected as an Axum `Extension`.
///
/// Using a newtype avoids accidental collisions with other `Extension<String>`
/// values.
#[derive(Clone, Debug)]
pub struct Tenant(pub String);

#[derive(Debug, Deserialize)]
pub struct SperrdConfig {
    pub database: mako_service::config::DatabaseConfig,
    pub port: Option<u16>,
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique
    /// string is valid.
    pub tenant: String,
    /// `makod` base URL — where IFTSTA 21039 is dispatched.
    pub makod_url: String,
    pub makod_api_key: SecretString,
    /// HMAC secret verifying the inbound `/webhook`, where ORDERS 17115/17117
    /// arrive as `de.mako.process.initiated`.
    ///
    /// Absent → the webhook accepts unsigned events with a startup warning. That
    /// is a dev-mode setting: the webhook queues physical disconnections.
    pub inbound_hmac_secret: Option<SecretString>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// OIDC token verification for the REST API.
    ///
    /// Required: `execute` and `fail` each put a real IFTSTA 21039 on the
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
