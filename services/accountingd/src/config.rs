//! Configuration for `accountingd`.

use serde::Deserialize;
use secrecy::SecretString;

/// EEG Einspeisevergütung payout configuration (`[eeg]` table in TOML).
///
/// Controls whether `accountingd` automatically generates SEPA Credit Transfer
/// pain.001 XML when `de.eeg.verguetung.berechnet` is received from `einsd`.
///
/// ## SCT Inst vs SCT CORE
///
/// | Mode | TOML | Payment type | Settlement | Regulatory basis |
/// |---|---|---|---|---|
/// | SCT Inst | `sepa_instant = true` | SCT_INST | <10 seconds | EU Reg 2024/886 |
/// | SCT CORE | `sepa_instant = false` | SCT_CORE | D+1 | SEPA Rulebook |
///
/// §25 Abs. 1 EEG 2023: Vergütung must be credited *"unverzüglich nach Ende des Monats"*.
/// SCT Inst satisfies this stronger than CORE (D+1 effectively means D+2 across weekends).
///
/// ## Example config
///
/// ```toml
/// [eeg]
/// sepa_instant   = true
/// auto_payout    = true
/// debtor_iban    = "env:LF_BANK_IBAN"   # LF's own bank account (debit side)
/// bank_submit_url = "https://banking-adapter.internal/api/v1/pain001"
/// bank_api_key   = "env:BANK_API_KEY"
/// ```
#[derive(Debug, Deserialize, Default)]
pub struct EegConfig {
    /// Use SEPA Instant Credit Transfer (pain.001.001.09 / SCT Inst) for EEG payouts.
    ///
    /// When `true`, the pain.001 XML carries `<LclInstrm><Cd>INST</Cd></LclInstrm>`
    /// and the bank adapter must support SCT Inst.  Default: `false` (SCT CORE).
    #[serde(default)]
    pub sepa_instant: bool,

    /// Automatically generate and schedule a pain.001 immediately when
    /// `de.eeg.verguetung.berechnet` is received.
    ///
    /// When `false` (default), operators manually call
    /// `POST /api/v1/eeg/payouts/run` to batch-generate pain.001 files.
    #[serde(default)]
    pub auto_payout: bool,

    /// IBAN of the LF's bank account (debtor / sending account for EEG payouts).
    ///
    /// **Required** when `auto_payout = true`.  Must be a valid SEPA IBAN.
    /// The service validates this at startup and refuses to auto-generate pain.001
    /// when this is absent or invalid.
    pub debtor_iban: Option<String>,

    /// Bank adapter base URL for automatic pain.001 submission.
    ///
    /// When configured, `accountingd` POSTs the pain.001 XML to this URL
    /// immediately after generation and sets `submitted_at` in `eeg_payout_orders`.
    ///
    /// Expected: `POST {bank_submit_url}` with `Content-Type: application/xml`
    /// and `Authorization: Bearer {bank_api_key}` returns 200–204 on acceptance.
    pub bank_submit_url: Option<String>,

    /// Bearer API key for the bank adapter.  Use `"env:VAR_NAME"` for secret injection.
    pub bank_api_key: Option<String>,
}

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

    /// HMAC-SHA256 signing secret for outbound webhooks (never logged — P2-1 fix).
    pub erp_hmac_secret: Option<SecretString>,

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

    /// Display name of the LF as it appears in pain.008 XML (`<Cdtr><Nm>`).
    /// If absent, defaults to `tenant` — set this to the company's legal name.
    pub creditor_name: Option<String>,

    /// SEPA Creditor Identifier (Gläubiger-ID, EPC AT-02 / ISO 20022 CdtrId).
    ///
    /// Mandatory for pain.008 SDD. Format: `DE98ZZZ09999999999`
    /// Issued by the Bundesbank: https://extranet.bundesbank.de/scp/
    ///
    /// When absent, pain.008 generation is **blocked** (hard error at startup
    /// and at generation time) to prevent bank batch rejections.
    pub creditor_id: Option<String>,

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

    /// EEG Einspeisevergütung payout configuration.
    ///
    /// Controls SCT Inst behaviour and bank adapter integration.
    /// See [`EegConfig`] for full documentation.
    #[serde(default)]
    pub eeg: EegConfig,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:ACCOUNTINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC configuration for authenticating financial write endpoints.
    /// When absent: dev mode (all requests accepted, WARN emitted at startup).
    pub oidc: Option<mako_service::oidc::OidcConfig>,
}
