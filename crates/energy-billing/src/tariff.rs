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

// ── ISO date serde helper for Option<time::Date> ──────────────────────────────

/// Serde adapter for `Option<time::Date>` using `YYYY-MM-DD` ISO 8601 strings.
///
/// Enables JSON fields like `"preisgarantie_bis": "2027-12-31"` to deserialize
/// correctly from tarifbd JSONB.
mod date_option_serde {
    use serde::{Deserialize, Deserializer, Serializer};
    use time::Date;
    use time::macros::format_description;

    const FORMAT: &[time::format_description::FormatItem<'static>] =
        format_description!("[year]-[month]-[day]");

    pub fn serialize<S: Serializer>(date: &Option<Date>, s: S) -> Result<S::Ok, S::Error> {
        match date {
            Some(d) => s.serialize_some(&d.format(FORMAT).map_err(serde::ser::Error::custom)?),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Date>, D::Error> {
        let opt: Option<String> = Option::deserialize(d)?;
        match opt {
            None => Ok(None),
            Some(s) => Date::parse(&s, FORMAT)
                .map(Some)
                .map_err(serde::de::Error::custom),
        }
    }
}

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

    /// RLM demand charge — Leistungspreis in ct/kW/month for large commercial customers.
    ///
    /// Applicable to RLM metering points (Registrierende Leistungsmessung ≥100 MWh/year).
    /// Billed on `MeterInput::spitzenleistung_kw` (Spitzenleistung, §2 Nr. 17 MessZV).
    ///
    /// ## Formula
    ///
    /// `Leistungspreis = spitzenleistung_kw × rate_ct_per_kw_month / 100`
    ///
    /// Applied once per billing period (the Spitzenleistung for the month, not pro-rated).
    ///
    /// ## Legal basis
    ///
    /// §41 EnWG, large commercial supply contracts. The demand charge captures the
    /// cost of maintaining peak network capacity for the customer.
    /// Net (Netto) position; MwSt is applied by `MwStProvider`.
    #[serde(default)]
    pub leistungspreis_strom_ct_per_kw_month: Option<Decimal>,

    // ── GAS ───────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub gas_grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,
    /// Gas Energiesteuer exemption for industrial/CHP customers.
    ///
    /// When `true`, the Energiesteuer position (§2 Nr. 3 EnergieStG, 0.55 ct/kWh_Hs)
    /// is replaced by an informational exemption note on the invoice.
    ///
    /// ## Legal basis
    ///
    /// - **§54 Abs. 1 EnergieStG**: full exemption for natural gas used in
    ///   **KWK plants** (Kraft-Wärme-Kopplung, combined heat and power).
    /// - **§54 Abs. 2 EnergieStG**: full exemption for gas used in
    ///   energy-intensive **industrial manufacturing** (produzierendes Gewerbe,
    ///   same classification as §9 Abs. 1 Nr. 2 StromStG).
    /// - **§56 EnergieStG**: reduced rate (½) for gas used in technical heating
    ///   processes in manufacturing. Set `gas_energiesteuer_befreiung = false`
    ///   and use `energiesteuer_gas_ct_per_kwh_override` for partial rates.
    ///
    /// **Operators must hold the customer’s formal exemption certificate
    /// (Steuerbescheid / Bestimmungserklärung) before enabling this flag.**
    #[serde(default)]
    pub gas_energiesteuer_befreiung: bool,
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
    /// §12 Abs. 2 Nr. 1 UStG — Fernwärme from renewable sources.
    ///
    /// When `true`, `HeatProvider` automatically applies `applicable_tax_rate = 0.07`
    /// to all heat positions. This overrides the engine-wide 19% default.
    ///
    /// Applies when heat is generated from: solar thermal, geothermal heat pumps,
    /// biomass, CHP from renewables, waste heat, or certified district heating
    /// networks with ≥ 50% renewable share (§12 Abs. 2 Nr. 1 UStG).
    ///
    /// Operators can still override with `mwst_rate_override` for edge cases.
    #[serde(default)]
    pub waerme_is_renewable: bool,

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
    #[serde(default)]
    pub hems_optimization_event_eur: Option<Decimal>,
    #[serde(default)]
    pub hems_readout_event_eur: Option<Decimal>,

    // ── EMOBILITY ─────────────────────────────────────────────────────────────
    #[serde(default)]
    pub emobility_service_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub emobility_kwh_price_ct: Option<Decimal>,
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

    // ── AufAbschlag / Rabatt ──────────────────────────────────────────────────
    /// Per-unit discount (negative) or surcharge (positive) in ct/kWh.
    ///
    /// Applied to the total Arbeitsmenge after all commodity positions.
    /// Use for campaign prices (Aktionspreis), loyalty bonuses, or indexed
    /// surcharges. Appears as a `Discount` position with `legal_basis = None`.
    ///
    /// ## Example
    /// - New customer bonus: `auf_abschlag_ct_per_kwh = Some(dec!(-2.00))` → -2 ct/kWh
    /// - Green energy surcharge: `auf_abschlag_ct_per_kwh = Some(dec!(0.50))` → +0.5 ct/kWh
    #[serde(default)]
    pub auf_abschlag_ct_per_kwh: Option<Decimal>,

    /// Monthly fixed discount (negative) or surcharge (positive) in EUR/month.
    ///
    /// Pro-rated to the billing period: `eur/month × billing_days / 30.4375`.
    /// Applied once per billing run regardless of consumption.
    /// Appears as a `Discount` position.
    #[serde(default)]
    pub auf_abschlag_eur_per_month: Option<Decimal>,

    // ── Messstellenbetreiber (MSB) fee ─────────────────────────────────────────
    /// MSB Grundgebühr passed through on the retail invoice (ct/day).
    ///
    /// Since MsbG 2016, the Messstellenbetreiber can be a third party.
    /// Lieferanten who bundle the MSB service into their retail offer
    /// must itemise the MSB fee separately on the invoice (§41 EnWG).
    ///
    /// Appears as a `Fee` position with `legal_basis = "MsbG"`.
    #[serde(default)]
    pub msb_gebuehr_ct_per_day: Option<Decimal>,

    // ── Block / Graduated tariff ──────────────────────────────────────────────

    // ── Minimum invoice ───────────────────────────────────────────────────────
    /// Minimum invoice amount (brutto, inclusive of MwSt) in EUR.
    ///
    /// When set and the computed `brutto_eur < minimum_invoice_eur_brutto`, a
    /// `Mindestbetrag` position is added to reach the minimum.
    ///
    /// Common in B2B contracts with a base consumption commitment
    /// (Mindestabnahmeverpflichtung) — the operator is billed at least this
    /// amount regardless of actual consumption.
    ///
    /// ## Implementation
    ///
    /// The top-up amount is added as a `Commodity` position tagged `"mindestbetrag"`.
    /// The engine applies this BEFORE MwSt (the position carries the appropriate
    /// `applicable_tax_rate` so MwSt is computed correctly).
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
    /// Block tariff tiers (Blocktarif / Staffelpreis) for electricity.
    ///
    /// When present, overrides `arbeitspreis_ct_per_kwh` / `arbeitspreis_ht/nt`.
    /// Tiers must be ordered ascending by `bis_kwh` (last tier has `bis_kwh = None`).
    ///
    /// ## Example (three-tier block tariff)
    /// ```json
    /// "block_tiers": [
    ///   { "bis_kwh": 1000.0, "preis_ct_per_kwh": 28.0 },
    ///   { "bis_kwh": 3000.0, "preis_ct_per_kwh": 24.0 },
    ///   { "preis_ct_per_kwh": 20.0 }
    /// ]
    /// ```
    ///
    /// ## Legal basis
    ///
    /// §41 EnWG allows Blocktarife; §40a requires the all-inclusive ct/kWh to
    /// be shown on the invoice regardless of tiering.
    #[serde(default)]
    pub block_tiers: Option<Vec<BlockTierInput>>,

    // ── Indexed / Variable prices (B2B) ───────────────────────────────────────
    /// Indexed price configuration for B2B contracts where the energy price
    /// tracks a commodity index (TTF, Phelix Base, LHKW, etc.).
    ///
    /// When set, the effective `arbeitspreis_ct_per_kwh` is computed as:
    /// `base_ct + spread_ct + (index_value × factor_ct_per_unit)`
    ///
    /// `arbeitspreis_ct_per_kwh` (if also set) acts as a fallback when
    /// `index_value` is `None` (index not yet available).
    ///
    /// ## Legal basis
    ///
    /// §41 Abs. 3 EnWG permits variable-price contracts with an index clause
    /// provided the index method is transparent and verifiable.
    #[serde(default)]
    pub indexed_price: Option<IndexedPriceConfig>,

    // ── Seasonal prices ───────────────────────────────────────────────────────
    /// Seasonal price overrides (Saisontarif / summer–winter pricing).
    ///
    /// When the billing period's middle month falls within a seasonal range,
    /// the matching override price is used instead of the base tariff price.
    /// Useful for gas contracts with higher winter rates and lower summer rates,
    /// or electricity contracts that differentiate by season.
    ///
    /// ## Example
    ///
    /// ```json
    /// "seasonal_prices": [
    ///   { "from_month": 10, "to_month": 3, "gas_arbeitspreis_ct_per_kwh_hs": 12.5, "label": "Winter" },
    ///   { "from_month": 4,  "to_month": 9, "gas_arbeitspreis_ct_per_kwh_hs": 8.0,  "label": "Sommer" }
    /// ]
    /// ```
    ///
    /// ## Legal basis
    ///
    /// §41 EnWG: variable prices are permitted provided the price change mechanism
    /// and effective dates are communicated transparently.
    #[serde(default)]
    pub seasonal_prices: Option<Vec<SeasonalPriceOverride>>,

    // ── Plant / installation metadata ─────────────────────────────────────────
    /// Rated power of the solar installation in kWp (kilowatt-peak).
    ///
    /// Enables automatic regulatory rate determinations:
    ///
    /// - **§12 Abs. 3 UStG** (Jahressteuergesetz 2022, since 01.01.2023): plants ≤ 30 kWp →
    ///   **0% MwSt** on electricity supply and self-consumption. `effective_mwst()` in
    ///   `RegulatoryRates` auto-applies this when `anlage_kwp` is set and ≤ 30.
    /// - **§9a Nr. 1 StromStG**: Stromsteuer-exempt self-consumption for ≤ 30 kWp plants
    ///   (already handled in `ElectricityProvider`; `anlage_kwp` makes it declarative).
    ///
    /// When `None`, no automatic kWp-based adjustments apply — use `mwst_rate_override`
    /// and `stromsteuer_ct_per_kwh_override` to set rates explicitly.
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,

    /// §9 Abs. 1 Nr. 4 StromStG — industrial customer Stromsteuer full exemption.
    ///
    /// Business customers consuming > 2 GWh/year as "Unternehmen des produzierenden
    /// Gewerbes" (§2 Nr. 4 StromStG) qualify for a full Stromsteuer exemption.
    ///
    /// When `true`, the Stromsteuer levy position is replaced by an informational
    /// exemption note on the invoice.
    ///
    /// **Operators must verify the customer's formal exemption certificate before enabling.**
    #[serde(default)]
    pub industrie_stromsteuer_befreiung: bool,

    /// Price guarantee expiry date (Preisgarantie bis).
    ///
    /// When set and still in the future relative to `ctx.period_to`, an informational
    /// position appears on the invoice noting the guarantee end date.
    ///
    /// §41 Abs. 1 Nr. 4 EnWG requires the invoice to state the applicable tariff
    /// and its conditions, including any price guarantee.
    ///
    /// This field is for **invoice display only** — price-freeze enforcement is
    /// `vertragd`'s responsibility.
    #[serde(default, with = "date_option_serde")]
    pub preisgarantie_bis: Option<time::Date>,
}

/// One seasonal price band (Saisontarif).
///
/// Months wrap around year boundaries: `from_month = 10, to_month = 3`
/// covers October through March (winter).
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct SeasonalPriceOverride {
    /// First month of this season (inclusive, 1–12).
    pub from_month: u8,
    /// Last month of this season (inclusive, 1–12, may be < from_month for wrap-around).
    pub to_month: u8,
    /// Override electricity arbeitspreis (ct/kWh). `None` = use base tariff.
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Override gas arbeitspreis (ct/kWh_Hs). `None` = use base tariff.
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<rust_decimal::Decimal>,
    /// Display label for the invoice position (e.g. `"Winter"`, `"Sommer"`).
    #[serde(default)]
    pub label: Option<String>,
}

impl SeasonalPriceOverride {
    /// Returns `true` when `month` (1–12) falls within this seasonal range.
    ///
    /// Handles wrap-around: `from_month=10, to_month=3` covers Oct–Mar.
    #[must_use]
    pub fn contains_month(&self, month: u8) -> bool {
        if self.from_month <= self.to_month {
            // Simple range: e.g. April (4) through September (9)
            month >= self.from_month && month <= self.to_month
        } else {
            // Wrap-around: e.g. October (10) through March (3)
            month >= self.from_month || month <= self.to_month
        }
    }
}

/// One consumption band in a block / graduated tariff (Blocktarif).
///
/// Common in B2B electricity and district heating contracts where higher
/// consumption earns a lower per-unit rate.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct BlockTierInput {
    /// Upper bound of this tier in kWh (exclusive). `None` = open-ended last tier.
    ///
    /// For the first tier: consumption from 0 to `bis_kwh`.
    /// For subsequent tiers: consumption from the previous tier's `bis_kwh`.
    pub bis_kwh: Option<rust_decimal::Decimal>,
    /// Rate for energy within this tier (ct/kWh).
    pub preis_ct_per_kwh: rust_decimal::Decimal,
}

/// Indexed price configuration for B2B contracts (§41 Abs. 3 EnWG).
///
/// The effective `arbeitspreis_ct_per_kwh` is computed as:
/// ```text
/// effective_ct = base_ct + spread_ct + (index_value × factor_ct_per_unit)
/// ```
///
/// ## Example — gas indexed to TTF
///
/// TTF at 35 EUR/MWh, conversion factor 0.1 (EUR/MWh → ct/kWh):
/// ```json
/// {
///   "base_ct_per_kwh": 0.5,
///   "spread_ct_per_kwh": 0.3,
///   "index_name": "TTF_Front_Month",
///   "index_value": 35.0,
///   "factor_ct_per_unit": 0.1
/// }
/// ```
/// → effective price = 0.5 + 0.3 + 35.0 × 0.1 = 4.3 ct/kWh
///
/// ## Example — electricity indexed to Phelix Base
///
/// Phelix Base at 80 EUR/MWh:
/// ```json
/// {
///   "base_ct_per_kwh": 1.0,
///   "spread_ct_per_kwh": 0.5,
///   "index_name": "Phelix_Base",
///   "index_value": 80.0,
///   "factor_ct_per_unit": 0.1
/// }
/// ```
/// → effective price = 1.0 + 0.5 + 80.0 × 0.1 = 9.5 ct/kWh
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct IndexedPriceConfig {
    /// Fixed base component (ct/kWh), independent of the index.
    pub base_ct_per_kwh: rust_decimal::Decimal,
    /// Supplier spread on top of the index value (ct/kWh).
    pub spread_ct_per_kwh: rust_decimal::Decimal,
    /// Name of the reference index for invoice display.
    ///
    /// Examples: `"TTF_Front_Month"`, `"Phelix_Base"`, `"EEX_Power_DE"`, `"LHKW"`.
    pub index_name: String,
    /// Current index value in the index's native unit.
    ///
    /// Must be set by `billingd` at bill-time by fetching from `marktd`
    /// or the operator's market data source.
    /// When `None`, the system falls back to `TariffInput::arbeitspreis_ct_per_kwh`.
    pub index_value: Option<rust_decimal::Decimal>,
    /// Conversion factor from index native unit to ct/kWh.
    ///
    /// For TTF / Phelix in EUR/MWh: use `0.1` (1 EUR/MWh = 0.1 ct/kWh).
    pub factor_ct_per_unit: rust_decimal::Decimal,
}

impl IndexedPriceConfig {
    /// Resolve the effective energy price in ct/kWh.
    ///
    /// Returns `None` when `index_value` is not available (caller should use
    /// `TariffInput::arbeitspreis_ct_per_kwh` as fallback).
    #[must_use]
    pub fn effective_ct_per_kwh(&self) -> Option<rust_decimal::Decimal> {
        let idx = self.index_value?;
        Some(self.base_ct_per_kwh + self.spread_ct_per_kwh + idx * self.factor_ct_per_unit)
    }

    /// Description for the invoice position label.
    #[must_use]
    pub fn position_description(&self) -> String {
        match self.index_value {
            Some(v) => format!(
                "Arbeitspreis {} ({:.4} + {:.4} + {:.4} \u{00d7} {:.4}\u{202f}ct/kWh)",
                self.index_name,
                self.base_ct_per_kwh,
                self.spread_ct_per_kwh,
                self.factor_ct_per_unit,
                v,
            ),
            None => format!(
                "Arbeitspreis {} (Indexwert nicht verf\u{00fc}gbar)",
                self.index_name
            ),
        }
    }
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

// ── PricingModel ──────────────────────────────────────────────────────────────

/// Typed product pricing model — the **preferred** dispatch type.
///
/// Converts a flat `TariffInput` into a typed enum for compile-time-checked
/// dispatch. Use `PricingModel::try_from(tariff)` to construct, then call
/// `.build_engine(grid, rates)`.
///
/// ## Why PricingModel over `TariffInput::build_engine()`?
///
/// | | `TariffInput::build_engine()` | `PricingModel::try_from()?.build_engine()` |
/// |---|---|---|
/// | Category dispatch | `&str` match, returns `Option` | enum variant, returns `Result` |
/// | Unknown categories | silently returns `None` | explicit `Err` with reason |
/// | Pattern matching | not supported | exhaustive match |
/// | Future typed variants | no | each variant can hold typed struct |
///
/// ## Migration path
///
/// New code in `billingd` handlers should use:
/// ```rust,ignore
/// let model = PricingModel::try_from(tariff)?;
/// let engine = model.build_engine(&grid, rates)?;
/// ```
///
/// The old `TariffInput::build_engine()` remains for backward compatibility
/// but delegates to `PricingModel` internally.
#[derive(Debug, Clone)]
pub enum PricingModel {
    /// Standard electricity supply contract (§41 EnWG). STROM.
    Electricity(TariffInput),
    /// §41a EnWG dynamic EPEX spot tariff. STROM with `dynamic_epex = true`.
    DynamicElectricity(TariffInput),
    /// §14a EnWG controllable heat pump tariff. WAERMEPUMPE.
    HeatPump(TariffInput),
    /// §14a EnWG controllable EV wallbox tariff. WALLBOX.
    Wallbox(TariffInput),
    /// Natural gas supply (SLP/RLM). GAS.
    Gas(TariffInput),
    /// District heating / Fernwärme supply. WAERME.
    Heat(TariffInput),
    /// Solar PV / Mieterstrom / §42b GGV community energy. SOLAR.
    Solar(TariffInput),
    /// EEG feed-in Gutschrift — LF pays generator (LF-side). EEG.
    Eeg(TariffInput),
    /// Direktvermarktung feed-in — LF as market premium payer. EINSPEISUNG.
    Einspeisung(TariffInput),
    /// Home Energy Management System subscription. HEMS.
    Hems(TariffInput),
    /// EV charging CPO/EMSP. EMOBILITY.
    Emobility(TariffInput),
    /// Energiedienstleistung / energy services subscription. ENERGIEDIENSTLEISTUNG.
    Service(TariffInput),
}

impl PricingModel {
    /// Get a reference to the inner `TariffInput`.
    #[must_use]
    pub fn tariff(&self) -> &TariffInput {
        match self {
            Self::Electricity(t)
            | Self::DynamicElectricity(t)
            | Self::HeatPump(t)
            | Self::Wallbox(t)
            | Self::Gas(t)
            | Self::Heat(t)
            | Self::Solar(t)
            | Self::Eeg(t)
            | Self::Einspeisung(t)
            | Self::Hems(t)
            | Self::Emobility(t)
            | Self::Service(t) => t,
        }
    }

    /// Product category string (matches `tarifbd` JSONB `category` key).
    #[must_use]
    pub fn category_str(&self) -> &'static str {
        match self {
            Self::Electricity(_) | Self::DynamicElectricity(_) => "STROM",
            Self::HeatPump(_) => "WAERMEPUMPE",
            Self::Wallbox(_) => "WALLBOX",
            Self::Gas(_) => "GAS",
            Self::Heat(_) => "WAERME",
            Self::Solar(_) => "SOLAR",
            Self::Eeg(_) => "EEG",
            Self::Einspeisung(_) => "EINSPEISUNG",
            Self::Hems(_) => "HEMS",
            Self::Emobility(_) => "EMOBILITY",
            Self::Service(_) => "ENERGIEDIENSTLEISTUNG",
        }
    }

    /// Build a `BillingEngine` for this pricing model.
    ///
    /// Type-safe replacement for `TariffInput::build_engine()`. Returns `Err`
    /// instead of `None` for unsupported / unknown categories.
    ///
    /// # Errors
    ///
    /// Returns `Err(category_name)` for BUNDLE (requires component expansion)
    /// and any category not covered by the enum variants.
    pub fn build_engine(
        &self,
        grid: &crate::quantities::GridInput,
        rates: &crate::rates::RegulatoryRates,
    ) -> Result<crate::engine::BillingEngine, String> {
        self.tariff()
            .build_engine(grid, rates)
            .ok_or_else(|| format!("unsupported product category: {}", self.category_str()))
    }
}

impl TryFrom<TariffInput> for PricingModel {
    type Error = String;

    /// Convert a `TariffInput` to a typed `PricingModel`.
    ///
    /// # Errors
    ///
    /// - `BUNDLE` → `Err("BUNDLE requires component expansion before billing")`
    /// - Unknown category → `Err("unknown product category: <name>")`
    fn try_from(t: TariffInput) -> Result<Self, Self::Error> {
        match t.category.as_str() {
            "STROM" | "WAERMEPUMPE" | "WALLBOX" if t.dynamic_epex => {
                Ok(Self::DynamicElectricity(t))
            }
            "STROM" => Ok(Self::Electricity(t)),
            "WAERMEPUMPE" => Ok(Self::HeatPump(t)),
            "WALLBOX" => Ok(Self::Wallbox(t)),
            "GAS" => Ok(Self::Gas(t)),
            "WAERME" => Ok(Self::Heat(t)),
            "SOLAR" => Ok(Self::Solar(t)),
            "EEG" => Ok(Self::Eeg(t)),
            "EINSPEISUNG" => Ok(Self::Einspeisung(t)),
            "HEMS" => Ok(Self::Hems(t)),
            "EMOBILITY" => Ok(Self::Emobility(t)),
            "ENERGIEDIENSTLEISTUNG" => Ok(Self::Service(t)),
            "BUNDLE" => Err("BUNDLE requires component expansion before billing".to_owned()),
            other => Err(format!("unknown product category: {other}")),
        }
    }
}

impl From<PricingModel> for TariffInput {
    fn from(m: PricingModel) -> Self {
        match m {
            PricingModel::Electricity(t)
            | PricingModel::DynamicElectricity(t)
            | PricingModel::HeatPump(t)
            | PricingModel::Wallbox(t)
            | PricingModel::Gas(t)
            | PricingModel::Heat(t)
            | PricingModel::Solar(t)
            | PricingModel::Eeg(t)
            | PricingModel::Einspeisung(t)
            | PricingModel::Hems(t)
            | PricingModel::Emobility(t)
            | PricingModel::Service(t) => t,
        }
    }
}
