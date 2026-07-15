//! `mabis-syncd` configuration.

use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub edmd: EdmdConfig,
    pub makod: MakodConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpConfig {
    #[serde(default = "default_addr")]
    pub addr: String,
}

fn default_addr() -> String {
    "0.0.0.0:8880".to_owned()
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            addr: default_addr(),
        }
    }
}

pub use mako_service::config::DatabaseConfig;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    /// Tenant identifier (BDEW Codenummer of ÜNB / NB).
    pub tenant: String,
    /// BDEW Codenummer of the sender (ÜNB / NB). Used in UTILTS `NAD+MS`.
    pub sender_mp_id: String,
    /// BDEW Codenummer of the BIKO receiver. Used in UTILTS `NAD+MR`.
    pub receiver_mp_id: String,
    /// Bilanzierungsgebiet identifier (BNetzA zone code).
    pub bilanzierungsgebiet_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdmdConfig {
    /// `edmd` base URL (e.g. `http://edmd:8380`).
    pub url: String,
    /// Bearer token for `edmd` MCP/API authentication.
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL (e.g. `http://makod:8080`).
    pub url: String,
    /// Bearer token for `makod` command API.
    pub api_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduleConfig {
    /// Day of month (1–28) to trigger vorlaeufig submission. Default: 3.
    #[serde(default = "default_prelim_day")]
    pub preliminary_day: u8,
    /// Day of month (1–28) to trigger endgueltig submission. Default: 8.
    #[serde(default = "default_final_day")]
    pub final_day: u8,
    /// UTC hour (0–23) to run submissions. Default: 5 (= 06:00 CET).
    #[serde(default = "default_run_hour")]
    pub run_hour_utc: u8,
}

fn default_prelim_day() -> u8 {
    3
}
fn default_final_day() -> u8 {
    8
}
fn default_run_hour() -> u8 {
    5
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            preliminary_day: default_prelim_day(),
            final_day: default_final_day(),
            run_hour_utc: default_run_hour(),
        }
    }
}

pub use mako_service::telemetry::OtelConfig;

pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| anyhow::anyhow!("config parse error: {e}"))
}
