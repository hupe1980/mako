//! `RegulatoryRates` — statutory levy rates, operator-configured.
//!
//! All rates come from `billingd.toml [rates]`. The library never hardcodes
//! statutory rates; they change with each legislative year.

use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

use crate::tariff::TariffInput;

/// Platform-level defaults for statutory rates.
///
/// Configure under `[rates]` in `billingd.toml`. These defaults reflect
/// 2025/2026 published rates and will be superseded by operator configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegulatoryRates {
    /// §3 StromStG — ct/kWh (current: 2.05 ct/kWh since 01.07.2023).
    pub stromsteuer_ct_per_kwh: Decimal,
    /// §2 Nr. 3 EnergieStG Erdgas H — ct/kWh_Hs (current: 0.55 ct/kWh).
    pub energiesteuer_gas_ct_per_kwh: Decimal,
    /// BEHG CO₂ levy for Erdgas H — ct/kWh_Hs.
    /// = CO₂-Preis EUR/t × 0.20160 kg_CO₂/kWh_Hs ÷ 10
    pub behg_gas_ct_per_kwh: Decimal,
    /// Standard MwSt rate (fraction, e.g. `0.19`).
    pub mwst_rate: Decimal,
}

impl Default for RegulatoryRates {
    fn default() -> Self {
        Self {
            stromsteuer_ct_per_kwh: dec!(2.05),
            energiesteuer_gas_ct_per_kwh: dec!(0.55),
            behg_gas_ct_per_kwh: dec!(1.310), // 65 EUR/t × 0.20160 kg_CO₂/kWh_Hs (2026, BEHG §10)
            mwst_rate: dec!(0.19),
        }
    }
}

impl RegulatoryRates {
    /// Effective Stromsteuer: product `stromsteuer_ct_per_kwh_override` wins.
    pub fn effective_stromsteuer(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .stromsteuer_ct_per_kwh_override
            .unwrap_or(self.stromsteuer_ct_per_kwh)
    }

    /// Effective Energiesteuer Gas: product override wins.
    pub fn effective_energiesteuer_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .energiesteuer_gas_ct_per_kwh_override
            .unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }

    /// Effective BEHG CO₂ levy: product override wins.
    pub fn effective_behg_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .behg_gas_ct_per_kwh_override
            .unwrap_or(self.behg_gas_ct_per_kwh)
    }

    /// Effective MwSt rate: product override wins.
    pub fn effective_mwst(&self, tariff: &TariffInput) -> Decimal {
        tariff.mwst_rate_override.unwrap_or(self.mwst_rate)
    }
}
