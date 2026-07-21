//! Configuration for `vertragd`.

use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct VertragdConfig {
    pub database_url: String,
    pub port: Option<u16>,
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator’s BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,
    pub lf_mp_id: String,
    /// `processd` — triggers Lieferbeginn/Lieferende per Vertragskomponente.
    pub processd_url: String,
    pub processd_api_key: Option<String>,
    /// `tarifbd` — product assignment after MaKo confirmation.
    pub tarifbd_url: String,
    pub tarifbd_api_key: Option<String>,
    /// `accountingd` — provision billing account on Vertrag AKTIV.
    pub accountingd_url: String,
    pub accountingd_api_key: Option<String>,
    /// `edmd` — trigger Ablesesteuerung reading orders.
    pub edmd_url: String,
    pub edmd_api_key: Option<String>,
    /// ERP webhook — receives `de.vertrag.*` CloudEvents.
    pub erp_webhook_url: Option<String>,
    pub erp_hmac_secret: Option<String>,
    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:VERTRAGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
    /// OIDC/JWT authentication for all operator-facing REST endpoints.
    ///
    /// When absent the service starts in dev mode (OidcVerifier::disabled).
    /// Must be configured in production; all write endpoints require a valid
    /// Bearer token from the configured issuer.
    pub oidc: Option<mako_service::oidc::OidcConfig>,
    /// Operator escalation after N Werktage without MaKo response.
    pub mako_timeout_werktage: Option<u32>,
    /// Maximum active portal identities per Kunde (prevents resource exhaustion).
    /// Default: 50.
    #[serde(default = "VertragdConfig::default_max_identitaeten")]
    pub max_identitaeten_per_kunde: u32,
    /// Start without token verification.
    ///
    /// With `[oidc]` absent the verifier admits every request with dev claims,
    /// which satisfies every handler — GDPR export, IBAN write and customer
    /// mutation included. That posture must be asked for by name rather than
    /// reached by leaving the section out; `main` refuses to start otherwise.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,
}

impl VertragdConfig {
    fn default_max_identitaeten() -> u32 {
        50
    }
}
