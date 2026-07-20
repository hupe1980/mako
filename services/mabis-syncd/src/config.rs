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
    pub marktd: MarktdConfig,
    pub makod: MakodConfig,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    #[serde(default)]
    pub otel: OtelConfig,
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// OIDC token verification. Required: a MaBiS submission is a binding
    /// filing to the BIKO, so the service refuses to start without it unless
    /// `allow_insecure_no_auth` is set explicitly.
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
    /// BDEW Codenummer of the sender (ÜNB / NB). Used in MSCONS `NAD+MS`.
    pub sender_mp_id: String,
    /// BDEW Codenummer of the BIKO receiver. Used in MSCONS `NAD+MR`.
    pub receiver_mp_id: String,
    /// Fallback Bilanzierungsgebiet for MaLos whose master data does not name
    /// one.
    ///
    /// The authoritative value is `marktd`'s per-MaLo `bilanzierungsgebiet`;
    /// this is only used when that lookup returns nothing, and such MaLos are
    /// logged rather than silently folded into the fallback zone.
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

/// `marktd` master-data service, queried for each MaLo's Bilanzierungsgebiet.
///
/// MaBiS aggregates **per Bilanzierungsgebiet**. Taking the territory from a
/// single config value put every MaLo of a tenant into one Summenzeitreihe
/// regardless of where it actually sits, which misfiles the whole submission for
/// any tenant spanning more than one zone.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL (e.g. `http://marktd:8180`).
    pub url: String,
    /// Bearer token for `marktd` API authentication.
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
    /// Werktag after the Bilanzierungsmonat on which to submit.
    ///
    /// BK6-24-174 Anlage 3 §3.10, Tabelle 2: the Erstaufschlag window for a
    /// BG-SZR is the 1.–10. Werktag. Submitting on the last of them maximises
    /// the input data while the BIKO still assigns `Abrechnungsdaten` directly;
    /// a version sent later starts as `Prüfdaten`.
    #[serde(default = "default_erstaufschlag_werktag")]
    pub erstaufschlag_werktag: u32,
    /// UTC hour (0–23) to run submissions. Default: 5 (= 06:00 CET).
    #[serde(default = "default_run_hour")]
    pub run_hour_utc: u8,
}

fn default_erstaufschlag_werktag() -> u32 {
    10
}
fn default_run_hour() -> u8 {
    5
}

impl Default for ScheduleConfig {
    fn default() -> Self {
        Self {
            erstaufschlag_werktag: default_erstaufschlag_werktag(),
            run_hour_utc: default_run_hour(),
        }
    }
}

pub use mako_service::telemetry::OtelConfig;

pub fn load_from_file(path: &Path) -> anyhow::Result<Config> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let mut cfg: Config =
        toml::from_str(&text).map_err(|e| anyhow::anyhow!("config parse error: {e}"))?;
    cfg.resolve_env_refs()?;
    Ok(cfg)
}

impl Config {
    /// Resolve every `env:VARNAME` indirection in the loaded config.
    ///
    /// Without this the placeholder is used verbatim: a documented
    /// `api_key = "env:MABIS_EDMD_API_KEY"` is sent as the literal bearer token
    /// `env:MABIS_EDMD_API_KEY`, so every upstream call 401s and the run reports
    /// a partial submission rather than a configuration failure.
    ///
    /// # Errors
    ///
    /// Fails at startup when a referenced variable is unset.
    pub fn resolve_env_refs(&mut self) -> anyhow::Result<()> {
        use mako_service::config::resolve_env;
        self.database.url = resolve_env(&self.database.url)?;
        self.edmd.url = resolve_env(&self.edmd.url)?;
        self.edmd.api_key = resolve_env(&self.edmd.api_key)?;
        self.marktd.url = resolve_env(&self.marktd.url)?;
        self.marktd.api_key = resolve_env(&self.marktd.api_key)?;
        self.makod.url = resolve_env(&self.makod.url)?;
        self.makod.api_key = resolve_env(&self.makod.api_key)?;
        self.identity.tenant = resolve_env(&self.identity.tenant)?;
        self.identity.sender_mp_id = resolve_env(&self.identity.sender_mp_id)?;
        self.identity.receiver_mp_id = resolve_env(&self.identity.receiver_mp_id)?;
        self.identity.bilanzierungsgebiet_id = resolve_env(&self.identity.bilanzierungsgebiet_id)?;
        Ok(())
    }
}
