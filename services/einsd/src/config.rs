//! Configuration for `einsd`.

use serde::Deserialize;

/// `einsd` runtime configuration — loaded via `mako_service::load_config`.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EinsdConfig {
    pub database_url: String,

    /// HTTP port.  Defaults to `9180` (billing extension range).
    pub port: Option<u16>,

    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,

    /// ERP webhook URL.  When set, `de.eeg.*` CloudEvents are POSTed here.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound CloudEvent POSTs.
    pub erp_hmac_secret: Option<String>,

    /// Optional `tarifbd` URL — used to fetch EPEX monthly prices for
    /// DIREKTVERMARKTUNG and POST_EEG_SPOT settlement models.
    /// When absent, operators must import prices via
    /// `PUT /api/v1/epex-monthly/{year}/{month}`.
    pub tarifbd_url: Option<String>,

    /// Optional `edmd` base URL — used to auto-fetch Einspeisemenge when
    /// `einspeisemenge_kwh` is not provided in a settlement request.
    ///
    /// When set, `POST /api/v1/anlagen/{tr_id}/settle/{year}/{month}` without
    /// `einspeisemenge_kwh` will call
    /// `GET {edmd_url}/api/v1/billing-period/{malo_id}?period_from=&period_to=`
    /// and use `arbeitsmenge_kwh` from the response.
    pub edmd_url: Option<String>,

    /// API key used for authenticated requests to `edmd`.
    pub edmd_api_key: Option<String>,

    /// How often (in seconds) the background alert worker checks for plants
    /// whose `foerderendedatum` is within 180 days.  Defaults to 21600 (6 h).
    pub alert_interval_secs: Option<u64>,

    /// URL template for auto-importing §20 Abs. 2 technology-specific Jahresmarktwert.
    ///
    /// When set, `einsd` auto-fetches technology-specific Marktwert values from the
    /// ÜNB publication (netztransparenz.de or a custom aggregator) on the 5th of each
    /// month. The URL must return a JSON array of `{ erzeugungsart, avg_ct_kwh }` objects
    /// for the given billing period.
    ///
    /// Example: `"https://api.netztransparenz.de/eeg/marktwert/{year}/{month}"`
    /// (The `{year}` and `{month}` placeholders are replaced with the billing period.)
    ///
    /// When absent, operators must import values manually via
    /// `PUT /api/v1/jahresmarktwert/{year}/{month}/{erzeugungsart}`.
    pub jahresmarktwert_url: Option<String>,

    /// Interval in seconds between Jahresmarktwert auto-import runs (default: 86400, once/day).
    /// On startup, the worker runs once after a 60-second delay.
    pub jahresmarktwert_import_interval_secs: Option<u64>,
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

    /// Start without token verification.
    ///
    /// Intended for local development and the demos. Every REST route is then
    /// reachable by any caller that can open a socket.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}
