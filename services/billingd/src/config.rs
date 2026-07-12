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

    /// Operator BDEW-Codenummer.
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

    /// Seller VAT registration number (Umsatzsteuer-ID) for XRechnung output.
    pub seller_vat_id: Option<String>,

    /// Statutory rate defaults.  Override here instead of per-product.
    pub rates: Option<RatesConfig>,
}

impl BillingdConfig {
    /// Build `RegulatoryRates` from config, falling back to statutory defaults.
    pub fn regulatory_rates(&self) -> crate::calculator::RegulatoryRates {
        use rust_decimal_macros::dec;
        let r = self.rates.as_ref();
        crate::calculator::RegulatoryRates {
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
