//! Configuration for `portald`.

use serde::Deserialize;

/// Runtime configuration loaded from `portald.toml` or environment variables.
#[derive(Debug, Clone, Deserialize)]
pub struct PortaldConfig {
    /// HTTP listen port (default: 9480).
    #[serde(default = "default_port")]
    pub port: u16,

    /// Operator tenant identifier.
    pub tenant: String,

    /// `edmd` base URL — `GET /api/v1/lastgang/{malo_id}`, `/billing-period/{malo_id}`
    pub edmd_url: Option<String>,
    /// Bearer token for `edmd` API.
    pub edmd_api_key: Option<String>,

    /// `billingd` base URL — `GET /api/v1/billing?malo_id=…`
    pub billingd_url: Option<String>,
    /// Bearer token for `billingd` API.
    pub billingd_api_key: Option<String>,

    /// `accountingd` base URL — `GET /api/v1/accounts/{malo_id}/…`
    pub accountingd_url: Option<String>,
    /// Bearer token for `accountingd` API.
    pub accountingd_api_key: Option<String>,

    /// `einsd` base URL — `GET /api/v1/anlagen/{tr_id}/settlements`
    pub einsd_url: Option<String>,
    /// Bearer token for `einsd` API.
    pub einsd_api_key: Option<String>,

    /// `marktd` base URL — `GET /api/v1/versorgung/{malo_id}`
    pub marktd_url: Option<String>,
    /// Bearer token for `marktd` API.
    pub marktd_api_key: Option<String>,

    /// `vertragd` base URL — customer authorization (OIDC sub → MaLo IDs).
    /// **Required for production**: without this, any JWT bearer can access any MaLo.
    pub vertragd_url: Option<String>,
    /// Bearer token for `vertragd` API.
    pub vertragd_api_key: Option<String>,

    /// OIDC issuer URL for customer JWT validation.
    /// When absent, authentication is skipped (dev mode only).
    #[allow(dead_code)] // read via PortaldConfig at runtime; cfg derives Deserialize
    pub oidc_issuer: Option<String>,
    /// Expected JWT `aud` claim.
    #[allow(dead_code)]
    pub oidc_audience: Option<String>,
}

fn default_port() -> u16 {
    9480
}
