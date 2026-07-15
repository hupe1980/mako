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
    /// **Required** for pain.008 generation — the service will refuse to generate
    /// pain.008 XML when this is absent or contains an invalid IBAN.
    /// A missing creditor_iban is now a hard error (not a silent placeholder).
    pub creditor_iban: Option<String>,

    /// Enable automatic Mahnwesen escalation (P1-5).
    ///
    /// When `true`, the background dunning worker runs daily and automatically:
    /// - Creates Mahnstufe 1 for accounts overdue by > `dunning_grace_days`
    /// - Escalates Mahnstufe 1 → 2 → 3 when prior Mahnungen are unresolved
    ///
    /// Default: `false` (opt-in, safe for new deployments).
    /// Requires `dunning_grace_days` to be set for correct timing.
    #[serde(default)]
    pub dunning_auto_enabled: bool,

    /// Days between SEPA N-5 pre-notification and collection day (default: 5).
    ///
    /// SEPA CORE SDD Rulebook: the debtor must be notified at least 5 calendar
    /// days before the collection date (N-5). Set to 5 unless the mandate uses
    /// a shorter notice period (requires bilateral agreement with the bank).
    pub sepa_pre_notification_days: Option<i64>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:ACCOUNTINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,
}
