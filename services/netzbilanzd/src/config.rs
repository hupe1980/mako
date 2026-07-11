//! Configuration for `netzbilanzd`.

use serde::Deserialize;

/// Top-level configuration loaded from `netzbilanzd.toml` / env vars.
#[derive(Debug, Deserialize)]
pub struct NetzbilanzConfig {
    /// PostgreSQL connection URL.
    pub database_url: String,
    /// HTTP port (default 8680).
    pub port: Option<u16>,
    /// `marktd` base URL for tariff lookups.
    pub marktd_url: String,
    /// `marktd` API key.
    pub marktd_api_key: String,
    /// `makod` base URL for command dispatch.
    pub makod_url: String,
    /// `makod` API key.
    pub makod_api_key: String,
}
