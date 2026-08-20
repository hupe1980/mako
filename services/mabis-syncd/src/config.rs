//! `mabis-syncd` configuration.

use serde::Deserialize;

// NOTE: no `deny_unknown_fields` on the top-level struct — `mako_service::run`
// loads config via `load_config`, whose env layer (`MABIS_SYNCD_*`) also surfaces
// the `MABIS_SYNCD_CONFIG` path variable as a stray `config` key. Rejecting
// unknown fields here would make every deployment that points at its config via
// that variable fail to start. Nested blocks keep `deny_unknown_fields`.
#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub http: HttpConfig,
    pub database: DatabaseConfig,
    pub identity: IdentityConfig,
    pub edmd: EdmdConfig,
    pub marktd: MarktdConfig,
    pub makod: MakodConfig,
    /// Where Summenzeitreihen are filed — bilateral BIKO today, MaBiS-Hub after
    /// BK6-24-210. See [`crate::submission::SubmissionTarget`]; an unimplemented
    /// target is rejected at startup rather than mid-run.
    #[serde(default)]
    pub submission_target: crate::submission::SubmissionTarget,
    #[serde(default)]
    pub schedule: ScheduleConfig,
    /// Webhook that receives this service's `de.mabis.*` CloudEvents, drained
    /// from the transactional outbox (persist-before-dispatch). Unset means the
    /// events are still enqueued but nothing delivers them — set it wherever a
    /// consumer (marktd fan-out, ERP, agentd) should hear about a failed
    /// submission or an opened Korrekturbedarf.
    #[serde(default)]
    pub erp_webhook_url: Option<String>,
    /// HMAC-SHA256 secret for signing outbound webhook deliveries.
    #[serde(default)]
    pub erp_hmac_secret: Option<String>,
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

impl mako_service::ServiceConfig for Config {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        self.http.addr.clone()
    }
}

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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

#[derive(Debug, Clone, Deserialize)]
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
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarktdConfig {
    /// `marktd` base URL (e.g. `http://marktd:8180`).
    pub url: String,
    /// Bearer token for `marktd` API authentication.
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakodConfig {
    /// `makod` base URL (e.g. `http://makod:8080`).
    pub url: String,
    /// Bearer token for `makod` command API.
    pub api_key: String,
}

#[derive(Debug, Clone, Deserialize)]
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

impl Config {
    /// Refuse a configuration that cannot produce a valid submission.
    ///
    /// Called from the daemon's `build`, which is the only start-up path there
    /// is. Both checks exist to fail here rather than at 05:00 on the
    /// Erstaufschlag-Werktag, by which point a run has aggregated a month of
    /// metering data and consumed its version number.
    ///
    /// # Errors
    ///
    /// - The configured [`crate::submission::SubmissionTarget`] has no
    ///   implementation.
    /// - `identity.bilanzierungsgebiet_id` is not a Y-type (Area) EIC. A
    ///   Bilanzkreis is type `X` (Party) and the same length, so only the
    ///   object type separates them — and `LOC+107` carries the value as free
    ///   text, which means the BIKO would accept either.
    pub fn validate(&self) -> anyhow::Result<()> {
        self.submission_target.ensure_supported()?;
        rubo4e::identifiers::BilanzierungsgebietId::new(&self.identity.bilanzierungsgebiet_id)
            .map_err(|e| {
                anyhow::anyhow!(
                    "identity.bilanzierungsgebiet_id `{}` is not a Bilanzierungsgebiet-EIC: {e}. \
                     A Bilanzierungsgebiet is a 16-character EIC of ENTSO-E object type `Y` \
                     (Area); a Bilanzkreis is type `X` (Party) and belongs in a different field.",
                    self.identity.bilanzierungsgebiet_id
                )
            })?;
        Ok(())
    }

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
        if let Some(url) = &self.erp_webhook_url {
            self.erp_webhook_url = Some(resolve_env(url)?);
        }
        if let Some(secret) = &self.erp_hmac_secret {
            self.erp_hmac_secret = Some(resolve_env(secret)?);
        }
        Ok(())
    }
}
