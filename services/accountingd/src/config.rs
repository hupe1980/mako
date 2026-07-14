//! Configuration for `accountingd`.

use serde::Deserialize;

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct AccountingdConfig {
    pub database_url: String,

    /// HTTP listen port.  Defaults to `9380`.
    pub port: Option<u16>,

    /// Operator tenant.
    pub tenant: String,

    /// ERP / portal webhook URL — receives outbound CloudEvents:
    /// `de.accounting.payment.due`, `de.accounting.mahnung.issued`,
    /// `de.accounting.sperrauftrag`, `de.accounting.bankruecklast`.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 signing secret for outbound webhooks.
    pub erp_hmac_secret: Option<String>,

    /// `sperrd` base URL — triggered when a Mahnstufe-3 dunning case is raised.
    /// When set, `accountingd` auto-creates a `sperr_orders` entry.
    pub sperrd_url: Option<String>,

    /// Dunning fee in ct (× 10⁻² EUR) per Mahnstufe level.
    /// Default: Stufe 1 = 0, Stufe 2 = 500 (= 5.00 EUR), Stufe 3 = 1000 (= 10.00 EUR)
    pub dunning_fee_stufe1_ct: Option<i64>,
    pub dunning_fee_stufe2_ct: Option<i64>,
    pub dunning_fee_stufe3_ct: Option<i64>,

    /// Days between issuing a Rechnung and sending Mahnstufe 1 (default: 30).
    pub dunning_grace_days: Option<i64>,

    /// IBAN of the LF's bank account used as SEPA creditor.
    /// Required for pain.008 generation; the N-5 scheduler logs a warning
    /// and uses a placeholder when not set.
    pub creditor_iban: Option<String>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:ACCOUNTINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}
