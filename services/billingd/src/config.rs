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
    /// CO₂-Abgabe BEHG Erdgas — ct/kWh_Hs (default 1.109 = 55 EUR/t × 0.20160 kg/kWh, 2025).
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

    /// Statutory rate defaults.  Override here instead of per-product.
    pub rates: Option<RatesConfig>,

    /// MCP server authentication. Supports API-key, OIDC, or dev mode.
    /// See `[mcp]` section in TOML — e.g. `api_key = "env:BILLINGD_MCP_API_KEY"`.
    #[serde(default)]
    pub mcp: mako_service::mcp_auth::McpAuthConfig,

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
            behg_gas_ct_per_kwh: r.and_then(|r| r.behg_gas_ct_per_kwh).unwrap_or(dec!(1.109)),
            mwst_rate: r.and_then(|r| r.mwst_rate).unwrap_or(dec!(0.19)),
        }
    }
}

impl BillingdConfig {
    /// Regulatory rates for a billing period, not for today.
    ///
    /// A correction re-opens an old period, and that period is billed under its
    /// own rates: 2022 gas carried the emergency zero Energiesteuer, 2021 BEHG
    /// was 25 EUR/t, the second half of 2020 had 16 % VAT. The plain
    /// [`Self::regulatory_rates`] answered with today's constants for every
    /// period, so a historical correction was re-taxed at current rates.
    ///
    /// An explicitly configured rate still wins — configuration is the operator
    /// saying "I know better" — but the *defaults* come from the year tables.
    /// A period straddling the 2020 VAT change is refused upstream by
    /// [`energy_billing::mwst_rate_for_period`] returning `None`; here it falls
    /// back to the configured default so preview paths keep working, and the
    /// engine's own per-position rates decide the rest.
    pub fn regulatory_rates_for_period(
        &self,
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
                .or_else(|| energy_billing::mwst_rate_for_period(period_from, period_to))
                .unwrap_or(defaults.mwst_rate),
        }
    }
}
