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
    /// Netzbetreiber MP-ID that owns the imported MaLo grid records.
    /// All synced records will be associated with this NB.
    pub nb_mp_id: String,
    /// Optional webhook URL to receive `de.markt.grid.drift.detected` CloudEvents.
    ///
    /// When set and `drift_detected == true`, `nis-syncd` posts a CloudEvent
    /// to this URL after each sync pass so downstream consumers (e.g. `obsd`
    /// alerting, ERP systems) can react to topology changes.
    pub drift_webhook_url: Option<String>,
}
