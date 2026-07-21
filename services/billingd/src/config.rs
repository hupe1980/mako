//! Configuration for `billingd`.

use rust_decimal::Decimal;
use serde::Deserialize;

/// Platform-level statutory rate configuration.
/// Configure under `[rates]` in `billingd.toml`.
/// Update annually as BNetzA / BMWK publish new levies.
#[derive(Debug, Deserialize)]
pub struct RatesConfig {
    /// Stromsteuer §3 StromStG — ct/kWh (default 2.05, valid since 01.07.2023).
    pub stromsteuer_ct_per_kwh: Option<Decimal>,
    /// Energiesteuer Erdgas §2 Nr. 3 EnergieStG — ct/kWh_Hs (default 0.55).
    pub energiesteuer_gas_ct_per_kwh: Option<Decimal>,
    /// CO₂-Abgabe BEHG Erdgas — ct/kWh_Hs (default 1.310 = 65 EUR/t × 0.20160 kg/kWh, 2026).
    /// From 2026 the nEHS price is set by auction inside the §10 BEHG corridor;
    /// configure the operator's actual procurement cost here.
    pub behg_gas_ct_per_kwh: Option<Decimal>,
    /// MwSt rate as decimal fraction (default 0.19).
    pub mwst_rate: Option<Decimal>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct BillingdConfig {
    pub database_url: String,

    /// HTTP listen port.  Defaults to `9280`.
    pub port: Option<u16>,

    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable unique string is valid.
    pub tenant: String,

    /// `tarifbd` base URL — product catalog and EPEX prices.
    pub tarifbd_url: String,

    /// `edmd` base URL — `MeterBillingPeriod` for consumption data.
    pub edmd_url: String,

    /// `edmd` bearer token.
    pub edmd_api_key: Option<String>,

    /// `marktd` base URL — `PreisblattNetznutzung` + `PreisblattKonzessionsabgabe`.
    pub marktd_url: String,

    /// `marktd` bearer token.
    pub marktd_api_key: Option<String>,

    /// `vertragd` base URL — Rahmenvertrag + MaLo enumeration for Sammelrechnung (L2).
    pub vertragd_url: Option<String>,

    /// ERP webhook URL — receives `de.billing.rechnung.erstellt` CloudEvents.
    pub erp_webhook_url: Option<String>,

    /// HMAC-SHA256 secret for signing outbound webhooks.
    pub erp_hmac_secret: Option<String>,

    /// Seller name for XRechnung generation (BG-4, BT-27). Defaults to tenant ID.
    pub seller_name: Option<String>,

    /// Seller VAT registration number (Umsatzsteuer-ID) for XRechnung output.
    pub seller_vat_id: Option<String>,

    /// §40 Abs. 2 Nr. 1 EnWG — supplier postal address as shown on invoices.
    pub seller_address: Option<String>,

    /// §40 Abs. 2 Nr. 8 EnWG — annual consumption of the comparable customer
    /// group in kWh/a (e.g. Stromspiegel reference value for the operator's
    /// dominant customer segment). Pro-rated to each billing period. When
    /// unset, the comparison-group line is omitted from invoices.
    pub vergleichsgruppe_kwh_pro_jahr: Option<Decimal>,

    /// Label for the comparable customer group, e.g. `"2-Personen-Haushalt"`.
    pub vergleichsgruppe_label: Option<String>,

    /// §40 Abs. 2 Nr. 1 EnWG — customer-service contact (hotline / e-mail)
    /// as shown on invoices.
    pub seller_contact: Option<String>,

    /// Statutory rate defaults.  Override here instead of per-product.
    pub rates: Option<RatesConfig>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:BILLINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

    /// OIDC token verification for the HTTP API.  When omitted, every request
    /// is accepted with synthetic dev-admin claims — `main` refuses to start
    /// in that state unless [`Self::allow_insecure_no_auth`] is set.
    #[serde(default)]
    pub oidc: Option<mako_service::oidc::OidcConfig>,

    /// Start without HTTP token verification (dev/test only).
    ///
    /// Without `[oidc]` every billing endpoint — calculate, correction,
    /// VPP contract mutation — is open to anyone who can reach the port.
    /// That posture must be asked for by name.
    #[serde(default)]
    pub allow_insecure_no_auth: bool,

    /// §40b EnWG scheduled billing runs. When omitted or `enabled = false`,
    /// billing stays on-demand via `POST …/calculate`.
    #[serde(default)]
    pub billing_runs: BillingRunsConfig,

    /// Deterministic invoice risk scoring and the HELD dispatch gate.
    /// See `[risk]` — `crate::risk::RiskConfig` for bands and thresholds.
    #[serde(default)]
    pub risk: crate::risk::RiskConfig,

    /// Shared secret for verifying inbound webhook HMAC-SHA256 signatures.
    ///
    /// When set, `POST /api/v1/webhooks/vpp-dispatch` (and future inbound webhook
    /// endpoints) validate the `X-Mako-Signature: sha256=<hex>` header.
    /// When absent, signature verification is disabled (dev mode).
    pub inbound_webhook_secret: Option<String>,

    /// Enable automatic VPP settlement billing triggered by
    /// `de.vpp.dispatch.confirmed` CloudEvents on `POST /api/v1/webhooks/vpp-dispatch`.
    ///
    /// When `false` (default), the webhook endpoint still accepts events but
    /// returns `202 Accepted` without triggering billing.  The `POST
    /// /api/v1/billing/vpp/{vpp_id}` endpoint remains available for manual
    /// settlement in all configurations.
    #[serde(default)]
    pub vpp_auto_billing: bool,
}
/// §40b EnWG billing-run worker configuration (`[billing_runs]`).
///
/// The worker sweeps once per day after `run_hour_utc`: it pulls the active
/// contracts from vertragd, computes each contract's most recently completed
/// billing period from its `abrechnungszyklus`, bills every period that has
/// no invoice yet, and accumulates the month's `billing_run_log` row. For
/// iMSys MaLos it additionally delivers the free monthly
/// Abrechnungsinformation (§40b Abs. 2 EnWG) as a CloudEvent.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BillingRunsConfig {
    /// Whether the scheduled billing worker is active. Default: false.
    #[serde(default)]
    pub enabled: bool,
    /// UTC hour (0–23) after which the daily sweep runs. Default: 4.
    #[serde(default = "default_billing_run_hour")]
    pub run_hour_utc: u8,
    /// Emit the §40b Abs. 2 monthly Abrechnungsinformation for iMSys MaLos.
    /// Default: true (only effective while `enabled`).
    #[serde(default = "default_true")]
    pub abrechnungsinformation: bool,
}

fn default_billing_run_hour() -> u8 {
    4
}
fn default_true() -> bool {
    true
}

impl Default for BillingRunsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            run_hour_utc: default_billing_run_hour(),
            abrechnungsinformation: true,
        }
    }
}

impl BillingdConfig {
    /// Build `RegulatoryRates` from config, falling back to statutory defaults.
    pub fn regulatory_rates(&self) -> energy_billing::RegulatoryRates {
        use rust_decimal::dec;
        let r = self.rates.as_ref();
        energy_billing::RegulatoryRates {
            stromsteuer_ct_per_kwh: r
                .and_then(|r| r.stromsteuer_ct_per_kwh)
                .unwrap_or(dec!(2.05)),
            energiesteuer_gas_ct_per_kwh: r
                .and_then(|r| r.energiesteuer_gas_ct_per_kwh)
                .unwrap_or(dec!(0.55)),
            behg_gas_ct_per_kwh: r.and_then(|r| r.behg_gas_ct_per_kwh).unwrap_or(dec!(1.310)),
            mwst_rate: r.and_then(|r| r.mwst_rate).unwrap_or(dec!(0.19)),
        }
    }
}

impl BillingdConfig {
    /// Regulatory rates for a billing period and commodity, not for today.
    ///
    /// A correction re-opens an old period, and that period is billed under its
    /// own rates: 2021 BEHG was 25 EUR/t, the second half of 2020 had 16 % VAT,
    /// and gas/Fernwärme carried **7 % USt from 01.10.2022 to 31.03.2024**
    /// (§28 Abs. 5/6 UStG) — which is why the product `category` is part of the
    /// lookup: the VAT history of gas differs from electricity.
    ///
    /// An explicitly configured rate still wins — configuration is the operator
    /// saying "I know better" — but the *defaults* come from the year tables.
    /// A period straddling a VAT boundary is refused upstream by the period
    /// helpers returning `None`; here it falls back to the configured default
    /// so preview paths keep working, and the engine's own per-position rates
    /// decide the rest.
    pub fn regulatory_rates_for_period(
        &self,
        category: &str,
        period_from: time::Date,
        period_to: time::Date,
    ) -> energy_billing::RegulatoryRates {
        let year = period_from.year();
        let configured = self.rates.as_ref();
        let defaults = self.regulatory_rates();
        energy_billing::RegulatoryRates {
            stromsteuer_ct_per_kwh: configured
                .and_then(|r| r.stromsteuer_ct_per_kwh)
                .or_else(|| energy_billing::stromsteuer_for_year(year))
                .unwrap_or(defaults.stromsteuer_ct_per_kwh),
            energiesteuer_gas_ct_per_kwh: configured
                .and_then(|r| r.energiesteuer_gas_ct_per_kwh)
                .or_else(|| energy_billing::energiesteuer_gas_for_year(year))
                .unwrap_or(defaults.energiesteuer_gas_ct_per_kwh),
            behg_gas_ct_per_kwh: configured
                .and_then(|r| r.behg_gas_ct_per_kwh)
                .or_else(|| energy_billing::behg_ct_per_kwh_for_year(year))
                .unwrap_or(defaults.behg_gas_ct_per_kwh),
            mwst_rate: configured
                .and_then(|r| r.mwst_rate)
                .or_else(|| {
                    if matches!(category, "GAS" | "WAERME") {
                        energy_billing::mwst_rate_for_gas_waerme_period(period_from, period_to)
                    } else {
                        energy_billing::mwst_rate_for_period(period_from, period_to)
                    }
                })
                .unwrap_or(defaults.mwst_rate),
        }
    }
}
