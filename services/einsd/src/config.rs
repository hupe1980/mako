//! Configuration for `einsd`.

use serde::Deserialize;

/// `einsd` runtime configuration — loaded via `mako_service::load_config`.
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct EinsdConfig {
    pub database_url: String,

    /// HTTP port.  Defaults to `9180` (billing extension range).
    pub port: Option<u16>,

    /// Tenant identifier (the operator's primary MP-ID).
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
    /// Bearer token required on `Authorization: Bearer <key>` for `/mcp` requests.
    /// When absent, the MCP endpoint is unauthenticated (development only).
    pub mcp_api_key: Option<String>,
}
