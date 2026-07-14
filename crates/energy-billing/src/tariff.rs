//! `TariffInput` — product pricing data from `tarifbd`.
//!
//! This is the JSON schema definition for the `tarifbd` product JSONB.
//! All fields are `Option` with `#[serde(default)]` — the product defines only
//! what is relevant; unrecognised fields are silently ignored.
//!
//! `TariffInput` is used to build concrete `BillingProvider` instances:
//!
//! ```rust,ignore
//! let tariff: TariffInput = tarifbd_client.get_tariff(malo_id).await?;
//! let provider = ElectricityProvider::from_tariff(&tariff, &grid);
//! ```

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

fn default_category() -> String {
    "STROM".to_owned()
}

/// Product pricing data from `tarifbd`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct TariffInput {
    /// Product category — drives which `BillingProvider` to instantiate.
    ///
    /// | Category | Provider |
    /// |---|---|
    /// | `STROM` | `ElectricityProvider` |
    /// | `WAERMEPUMPE` | `ElectricityProvider` + §14a |
    /// | `WALLBOX` | `ElectricityProvider` + §14a |
    /// | `GAS` | `GasProvider` |
    /// | `WAERME` | `HeatProvider` |
    /// | `SOLAR` | `SolarProvider` |
    /// | `EEG` | `EegProvider` |
    /// | `EINSPEISUNG` | `EinspeisungProvider` |
    /// | `HEMS` | `HemsProvider` |
    /// | `EMOBILITY` | `EmobilityProvider` |
    /// | `ENERGIEDIENSTLEISTUNG` | `ServiceProvider` |
    /// | `STROM` + `dynamic_epex: true` | `DynamicElectricityProvider` |
    #[serde(default = "default_category")]
    pub category: String,

    /// Optional product code (from `tarifbd`).
    #[serde(default)]
    pub product_code: Option<String>,

    // ── STROM / WAERMEPUMPE / WALLBOX ─────────────────────────────────────────
    #[serde(default)]
    pub grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub sect14a_modul1_nne_reduktion_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub sect14a_modul3_entschaedigung_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 1: annual capacity-based NNE reduction in EUR/kW/year.
    #[serde(default)]
    pub steuerungsrabatt_modul1_eur_per_kw_year: Option<Decimal>,
    /// §14a Modul 3: annual steuerung compensation in EUR/kW/year.
    #[serde(default)]
    pub steuerungsrabatt_modul3_eur_per_kw_year: Option<Decimal>,
    /// Accepts any JSON value (string like "Eintarif" or integer).
    #[serde(default)]
    pub register_count: Option<serde_json::Value>,

    // ── GAS ───────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub gas_grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,

    // ── WAERME ────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub waerme_grundpreis_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_year: Option<Decimal>,
    /// Monthly Leistungspreis alternative (EUR/kW/month, maps to /year×12).
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_month: Option<Decimal>,
    #[serde(default)]
    pub waerme_arbeitspreis_ct_per_kwh: Option<Decimal>,

    // ── SOLAR / GGV ───────────────────────────────────────────────────────────
    #[serde(default)]
    pub solar_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// §38a EEG Mieterstrom-Zuschlag.
    #[serde(default)]
    pub mieterstrom_aufschlag_ct_per_kwh: Option<Decimal>,
    /// §42a EEG GGV community discount.
    #[serde(default)]
    pub gemeinschaft_rabatt_ct_per_kwh: Option<Decimal>,
    /// `true` when Stromsteuer applies (typically `false` for §9a StromStG Eigenverbrauch).
    #[serde(default)]
    pub solar_include_stromsteuer: bool,

    // ── EEG (simplified LF credit note path) ─────────────────────────────────
    #[serde(default)]
    pub eeg_verguetungssatz_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_marktpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_managementpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub kwkg_zuschlag_ct_per_kwh: Option<Decimal>,

    // ── EINSPEISUNG (non-EEG Direktvermarktung) ───────────────────────────────
    #[serde(default)]
    pub marktwert_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub vermarktungsgebuehr_ct_per_kwh: Option<Decimal>,

    // ── HEMS ─────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub hems_subscription_eur_per_month: Option<Decimal>,
    /// Alias: hems_platform_fee_eur_per_month → hems_subscription_eur_per_month
    #[serde(default)]
    pub hems_platform_fee_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub hems_optimization_event_eur: Option<Decimal>,
    #[serde(default)]
    pub hems_readout_event_eur: Option<Decimal>,

    // ── EMOBILITY ─────────────────────────────────────────────────────────────
    #[serde(default)]
    pub emobility_service_fee_eur: Option<Decimal>,
    /// Alias: emobility_service_fee_eur_per_month → emobility_service_fee_eur
    #[serde(default)]
    pub emobility_service_fee_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub emobility_kwh_price_ct: Option<Decimal>,
    /// Alias: emobility_arbeitspreis_ct_per_kwh → emobility_kwh_price_ct
    #[serde(default)]
    pub emobility_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub emobility_session_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub emobility_roaming_fee_eur: Option<Decimal>,

    // ── ENERGIEDIENSTLEISTUNG ─────────────────────────────────────────────────
    #[serde(default)]
    pub service_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub service_event_price_eur: Option<Decimal>,

    // ── §41a dynamic EPEX tariff ──────────────────────────────────────────────
    /// `true` to enable §41a per-interval EPEX Spot billing.
    #[serde(default)]
    pub dynamic_epex: bool,
    /// Optional price floor for §41a (ct/kWh). `None` = full market exposure.
    #[serde(default)]
    pub dynamic_epex_floor_ct_kwh: Option<Decimal>,

    // ── Regulatory overrides ──────────────────────────────────────────────────
    #[serde(default)]
    pub stromsteuer_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub energiesteuer_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub behg_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

impl TariffInput {
    /// Build a `BillingEngine` for this product category with the given rates.
    ///
    /// This is the **primary integration point** for `billingd`:
    /// load the product from tarifbd, call `build_engine`, then `engine.bill(ctx, quantities)`.
    ///
    /// Returns `None` for unsupported categories (e.g. legacy BUNDLE).
    pub fn build_engine(
        &self,
        grid: &crate::quantities::GridInput,
        rates: &crate::rates::RegulatoryRates,
    ) -> Option<crate::engine::BillingEngine> {
        use crate::engine::BillingEngine;

        use crate::providers::{
            DynamicElectricityProvider, EegProvider, EinspeisungProvider, ElectricityProvider,
            EmobilityProvider, GasProvider, HeatProvider, HemsProvider, MwStProvider,
            ServiceProvider, SolarProvider,
        };
        use std::collections::HashMap;

        let mwst = rates.effective_mwst(self);

        let engine = match self.category.as_str() {
            "STROM" | "WAERMEPUMPE" | "WALLBOX" => {
                if self.dynamic_epex {
                    BillingEngine::new()
                        .add(DynamicElectricityProvider::with_epex_map(
                            self.clone(),
                            grid.clone(),
                            HashMap::new(), // prices injected at bill() time via quantities.dynamic_epex_prices
                        ))
                        .add(MwStProvider::new(mwst))
                } else {
                    BillingEngine::new()
                        .add(ElectricityProvider::from_tariff(self, grid))
                        .add(MwStProvider::new(mwst))
                }
            }
            "GAS" => BillingEngine::new()
                .add(GasProvider::from_tariff(self, grid))
                .add(MwStProvider::new(mwst)),
            "WAERME" => BillingEngine::new()
                .add(HeatProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "SOLAR" => BillingEngine::new()
                .add(SolarProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "EEG" => BillingEngine::new()
                .add(EegProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "EINSPEISUNG" => BillingEngine::new()
                .add(EinspeisungProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "HEMS" => BillingEngine::new()
                .add(HemsProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "EMOBILITY" => BillingEngine::new()
                .add(EmobilityProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            "ENERGIEDIENSTLEISTUNG" => BillingEngine::new()
                .add(ServiceProvider::from_tariff(self))
                .add(MwStProvider::new(mwst)),
            _ => return None,
        };
        Some(engine)
    }
}
