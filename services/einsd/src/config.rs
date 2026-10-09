//! Configuration for `einsd`.

use serde::Deserialize;

/// `einsd` runtime configuration — loaded via `mako_service::load_config`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EinsdConfig {
    /// PostgreSQL connection + pool tuning (`application_name` = `einsd`).
    pub database: mako_service::config::DatabaseConfig,

    /// HTTP port.  Defaults to `9180` (billing extension range).
    pub port: Option<u16>,

    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,

    /// ERP webhook URL.  When set, `de.eeg.*` CloudEvents are POSTed here.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound CloudEvent POSTs.
    pub erp_hmac_secret: Option<String>,

    /// Optional `edmd` base URL — used to auto-fetch Einspeisemenge when
    /// `einspeisemenge_kwh` is not provided in a settlement request.
    ///
    /// When set, `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}` without
    /// `einspeisemenge_kwh` calls
    /// `GET {edmd_url}/api/v1/energy/{malo_id}?direction=EINSPEISUNG` over the
    /// billing month and sums the projected intervals.
    ///
    /// Deliberately **not** `/api/v1/billing-period/{malo_id}`: its
    /// `arbeitsmenge_kwh` is the Bezug, projected onto the *consumption*
    /// registers. An Erzeugungs-MaLo reports only `1-0:2.8.x`, so that
    /// projection is empty and the field reads `Some(0)` rather than absent —
    /// a settlement on it pays nothing while a dry-run still counts the plant
    /// as having data. Only `/energy` lets the direction be stated.
    /// See `handlers::fetch_einspeisemenge_from_edmd`.
    pub edmd_url: Option<String>,

    /// API key used for authenticated requests to `edmd`.
    pub edmd_api_key: Option<String>,

    /// How often (in seconds) the background alert worker checks for plants
    /// whose `foerderendedatum` is within 180 days.  Defaults to 21600 (6 h).
    pub alert_interval_secs: Option<u64>,

    /// URL template for auto-importing Anlage 1 technology-specific Jahresmarktwert.
    ///
    /// When set, `einsd` auto-fetches technology-specific Marktwert values from the
    /// ÜNB publication (netztransparenz.de or a custom aggregator) on the 5th of each
    /// month. The URL must return a JSON array of `{ erzeugungsart, avg_ct_kwh }` objects
    /// for the given billing period.
    ///
    /// Example: `"https://api.netztransparenz.de/eeg/marktwert/{year}/{month}"`
    /// (The `{year}` and `{month}` placeholders are replaced with the billing period.)
    ///
    /// The feed is the **monthly** series (Anlage 1 Nr. 3); the Jahresmarktwert
    /// of Nr. 4 lands once a year and is imported by hand.
    ///
    /// When absent, operators import every value manually via
    /// `PUT /api/v1/marktwert/{year}/{art}/{erzeugungsart}`.
    pub jahresmarktwert_url: Option<String>,

    /// Interval in seconds between Jahresmarktwert auto-import runs (default: 86400, once/day).
    /// On startup, the worker runs once after a 60-second delay.
    pub jahresmarktwert_import_interval_secs: Option<u64>,

    /// Earliest day of the month on which the auto-settle worker settles the
    /// previous month. Defaults to 7.
    ///
    /// The ÜNB publishes the Marktwert around the 5th and edmd's month is not
    /// complete before then. Running earlier wrote `price_missing` / `no_data`
    /// receipts for plants that were merely early, and those had to be settled
    /// again afterwards.
    pub auto_settle_from_day: Option<u8>,

    /// How many months back the auto-settle worker sweeps on each run.
    /// Defaults to 3; clamped to 1–24.
    ///
    /// Settling only the previous month meant a period the service was down for
    /// — or whose ÜNB Marktwert arrived late — was never revisited: the window
    /// moved on and the plant simply went unpaid. §23 EEG 2023 puts the monthly
    /// payment on the Netzbetreiber, so the sweep re-checks a short tail.
    pub auto_settle_catchup_months: Option<u8>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:EINSD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC token verification for the REST API.
    ///
    /// Required unless `allow_insecure_no_auth` is set: the settlement endpoints
    /// create a payment obligation to the Anlagenbetreiber, so serving them
    /// unauthenticated has to be a decision someone wrote down.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// The Beihilfevorbehalt on EEG Teil-3 payments for supply periods from
    /// 01.01.2027 — `[beihilfevorbehalt]`.
    #[serde(default)]
    pub beihilfevorbehalt: BeihilfevorbehaltConfig,

    /// Start without token verification.
    ///
    /// Intended for local development and the demos. Every REST route is then
    /// reachable by any caller that can open a socket.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

/// `[beihilfevorbehalt]` — what einsd does with a Teil-3 payment no state-aid
/// approval covers, and the approvals recorded so far.
///
/// The Commission's approval of the EEG 2023 „bis zum 31. Dezember 2026
/// befristet ist" (BT-Drs. 21/7867, Begründung zu § 104), so a payment for a
/// supply period from 01.01.2027 has none until one is recorded here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeihilfevorbehaltConfig {
    /// What happens to an uncovered payment. Default: `verweigern`.
    #[serde(default)]
    pub politik: BeihilfePolitik,
    /// Each recorded approval: `eingang`, `gilt_ab`, optional `gilt_bis`,
    /// `bundesanzeiger`, `umfang`.
    #[serde(default)]
    pub genehmigungen: Vec<eeg_billing::Beihilfegenehmigung>,
}

/// What einsd does with a Teil-3 payment no recorded approval covers.
///
/// The legal answer is the operator's to give; neither policy is a legal
/// outcome mako asserts. The default refuses, because paying is the step that
/// cannot be taken back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BeihilfePolitik {
    /// Refuse the settlement, naming the reservation; no receipt is written,
    /// and the sweep retries the period once an approval is recorded.
    #[default]
    Verweigern,
    /// Settle and pay, with the reservation stated as a zero-euro position on
    /// the receipt and the Gutschrift.
    UnterVorbehalt,
}

impl EinsdConfig {
    /// `edmd`, addressed and credentialed, or `None` when it is not configured.
    ///
    /// The single place `edmd_url` and `edmd_api_key` are read, so where the
    /// credential goes is decided once.
    #[must_use]
    pub fn edmd(&self, client: reqwest::Client) -> Option<mako_service::http::Upstream> {
        self.edmd_url.as_deref().map(|url| {
            mako_service::http::Upstream::new(
                "edmd",
                url,
                self.edmd_api_key.clone().map(secrecy::SecretString::from),
                client,
            )
        })
    }
}

impl mako_service::ServiceConfig for EinsdConfig {
    fn database(&self) -> Option<&mako_service::config::DatabaseConfig> {
        Some(&self.database)
    }
    fn bind_addr(&self) -> String {
        format!("0.0.0.0:{}", self.port.unwrap_or(9180))
    }
}
