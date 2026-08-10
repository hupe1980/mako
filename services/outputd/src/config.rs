//! Configuration for `outputd`: TOML (`outputd.toml`) + `OUTPUTD_` env
//! overrides, loaded by `mako_service`.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct OutputdConfig {
    pub database: mako_service::config::DatabaseConfig,
    pub port: Option<u16>,
    /// The operator's MP-ID — the tenant every template row is scoped to.
    pub tenant: String,
    /// OIDC verification for the HTTP API. Fail closed: without it, anyone can
    /// publish the layout every customer document renders with, and render
    /// arbitrary documents under the operator's Briefkopf.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,
    /// Dev-only escape hatch, named loudly on startup.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl mako_service::ServiceConfig for OutputdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9880))
    }
}
