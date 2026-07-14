//! Configuration for `nis-syncd`.

use serde::Deserialize;

/// Top-level configuration loaded from `nis-syncd.toml`.
#[derive(Debug, Deserialize)]
pub struct NisSyncdConfig {
    /// HTTP port (default 9680).
    pub port: Option<u16>,
    /// `marktd` base URL.
    pub marktd_url: String,
    /// `marktd` API key (secret — use `env:VAR_NAME` syntax in config file).
    pub marktd_api_key: String,

    /// MCP server authentication. Supports API-key or dev mode.
    /// See `[mcp]` in TOML — e.g. `api_key = "env:NIS_SYNCD_MCP_API_KEY"`.
    /// Separate from `marktd_api_key` (outgoing calls to marktd).
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// Netzbetreiber MP-ID that owns the imported MaLo grid records.
    /// All synced records will be associated with this NB.
    pub nb_mp_id: String,
    /// Optional webhook URL to receive `de.markt.grid.drift.detected` CloudEvents.
    ///
    /// When set and `drift_detected == true`, `nis-syncd` posts a CloudEvent
    /// to this URL after each sync pass so downstream consumers (e.g. `obsd`
    /// alerting, ERP systems) can react to topology changes.
    pub drift_webhook_url: Option<String>,
    /// Maximum concurrent marktd PUT requests per sync pass.
    ///
    /// Defaults to 20. Increase for large NIS exports; reduce if marktd rate-limits.
    #[serde(default = "default_sync_concurrency")]
    pub sync_concurrency: usize,
    /// Maximum NIS entries accepted per single sync request body.
    ///
    /// Defaults to 50 000. Prevents accidental overload of marktd.
    #[serde(default = "default_max_batch_size")]
    pub max_batch_size: usize,
}

fn default_sync_concurrency() -> usize {
    20
}
fn default_max_batch_size() -> usize {
    50_000
}
