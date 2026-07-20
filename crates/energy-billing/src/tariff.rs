//! Typed product definitions for the German retail energy market.
//!
//! The [`Product`] enum is the primary entry point. It deserializes directly from
//! `tarifbd` product JSONB using the `"category"` discriminator field and provides
//! a [`Product::build_engine`] method to construct the matching `BillingEngine`.
//!
//! ## Architecture
//!
//! The old flat `TariffInput` god-struct (50+ optional fields for all categories)
//! has been replaced by per-category typed structs. Each struct carries only the
//! fields relevant to its product category, preventing silent field confusion.
//!
//! ```text
//! Product::Strom(ElectricityProduct)           → ElectricityProvider / DynamicElectricityProvider
//! Product::Waermepumpe(ControllableLoadProduct) → ControllableLoadProvider (§14a)
//! Product::Wallbox(ControllableLoadProduct)     → ControllableLoadProvider (§14a)
//! Product::Gas(GasProduct)                      → GasProvider
//! Product::Waerme(HeatProduct)                  → HeatProvider
//! Product::Solar(SolarProduct)                  → SolarProvider
//! Product::Eeg(EegProduct)                      → EegProvider
//! Product::Einspeisung(EinspeisungProduct)       → EinspeisungProvider
//! Product::Hems(HemsProduct)                    → HemsProvider
//! Product::Emobility(EmobilityProduct)           → EmobilityProvider
//! Product::Energiedienstleistung(ServiceProduct) → ServiceProvider
//! Product::Sharing(SharingProduct)               → ElectricityProvider + EnergyShareProvider
//! ```
//!
//! ## Deserializing from `tarifbd` JSONB
//!
//! ```rust
//! use energy_billing::Product;
//!
//! let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":28.5}"##;
//! let product: Product = serde_json::from_str(json).unwrap();
//! assert!(matches!(product, Product::Strom(_)));
//! ```

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

// ── ISO date serde helper ─────────────────────────────────────────────────────

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

// ── StromsteuerBefreiung ──────────────────────────────────────────────────────

/// §9 StromStG — typed Stromsteuer exemption grounds.
///
/// When set to any variant except `Keine`, the `ElectricityProvider` replaces
/// the Stromsteuer levy with an informational exemption notice on the invoice.
/// **Operators must hold the formal certificate before enabling.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StromsteuerBefreiung {
    #[default]
    Keine,
    Bahnstrom,
    NachweisErneuerbarer,
    KwkSelbstverbrauch,
    IndustrieProduktionesGewerbe,
    LandForstwirtschaft,
    SolarEigenverbrauch,
}

impl StromsteuerBefreiung {
    #[must_use]
    pub fn citation(self) -> &'static str {
        match self {
            Self::Keine => "",
            Self::Bahnstrom => "§9 Nr. 1 StromStG",
            Self::NachweisErneuerbarer => "§9 Nr. 2 StromStG",
            Self::KwkSelbstverbrauch => "§9 Nr. 3 StromStG",
            Self::IndustrieProduktionesGewerbe => "§9 Nr. 4 StromStG",
            Self::LandForstwirtschaft => "§9 Nr. 5 StromStG",
            Self::SolarEigenverbrauch => "§9a Nr. 1 StromStG",
        }
    }

    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Keine => "",
            Self::Bahnstrom => "Stromsteuer: befreit gemäß §9 Nr. 1 StromStG (Bahnstrom)",
            Self::NachweisErneuerbarer => {
                "Stromsteuer: befreit gemäß §9 Nr. 2 StromStG (nachweisbar erneuerbarer Strom)"
            }
            Self::KwkSelbstverbrauch => {
                "Stromsteuer: befreit gemäß §9 Nr. 3 StromStG (KWK-Selbstverbrauch <2 MW)"
            }
            Self::IndustrieProduktionesGewerbe => {
                "Stromsteuer: befreit gemäß §9 Nr. 4 StromStG (produzierendes Gewerbe >2 GWh/a)"
            }
            Self::LandForstwirtschaft => {
                "Stromsteuer: befreit gemäß §9 Nr. 5 StromStG (Land-/Forstwirtschaft)"
            }
            Self::SolarEigenverbrauch => {
                "Stromsteuer: befreit gemäß §9a Nr. 1 StromStG (Eigenverbrauch PV ≤30 kWp)"
            }
        }
    }

    #[must_use]
    pub fn is_exempt(self) -> bool {
        !matches!(self, Self::Keine)
    }
}

// ── EnergieQuellen ────────────────────────────────────────────────────────────

/// §42 EnWG Abs. 2 Nr. 2 — typed energy source mix for invoice disclosure.
///
/// Replaces the legacy free-text energiemix string with structured data for
/// CO₂ label generation, HKN traceability, and BO4E Energiemix generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EnergieQuellen {
    #[serde(default)]
    pub erneuerbar_pct: Decimal,
    #[serde(default)]
    pub nuclear_pct: Option<Decimal>,
    #[serde(default)]
    pub fossil_pct: Option<Decimal>,
    /// Specific CO₂ emissions g/kWh — mandatory on electricity invoices (§42 Abs. 2 Nr. 2 EnWG).
    pub co2_g_per_kwh: Decimal,
    #[serde(default)]
    pub hkn_certified: bool,
    #[serde(default)]
    pub hkn_country: Option<String>,
    #[serde(default)]
    pub beschreibung: Option<String>,
}

// ── SeasonalPriceOverride ─────────────────────────────────────────────────────

/// One seasonal price band (Saisontarif). Months wrap: from_month=10, to_month=3 → Oct–Mar.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SeasonalPriceOverride {
    pub from_month: u8,
    pub to_month: u8,
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,
    #[serde(default)]
    pub label: Option<String>,
}

impl SeasonalPriceOverride {
    #[must_use]
    pub fn contains_month(&self, month: u8) -> bool {
        if self.from_month <= self.to_month {
            month >= self.from_month && month <= self.to_month
        } else {
            month >= self.from_month || month <= self.to_month
        }
    }
}

// ── BlockTierInput ────────────────────────────────────────────────────────────

/// One tier in a block/graduated tariff (Blocktarif / Staffelpreis).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BlockTierInput {
    /// Upper bound (kWh). None = open-ended last tier.
    pub bis_kwh: Option<Decimal>,
    pub preis_ct_per_kwh: Decimal,
}

// ── IndexedPriceConfig ────────────────────────────────────────────────────────

/// Indexed price for B2B contracts — §41 Abs. 3 EnWG.
///
/// Effective = base_ct + spread_ct + (index_value × factor_ct_per_unit).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IndexedPriceConfig {
    pub base_ct_per_kwh: Decimal,
    pub spread_ct_per_kwh: Decimal,
    pub index_name: String,
    pub index_value: Option<Decimal>,
    pub factor_ct_per_unit: Decimal,
}

impl IndexedPriceConfig {
    #[must_use]
    pub fn effective_ct_per_kwh(&self) -> Option<Decimal> {
        let idx = self.index_value?;
        Some(self.base_ct_per_kwh + self.spread_ct_per_kwh + idx * self.factor_ct_per_unit)
    }

    #[must_use]
    pub fn position_description(&self) -> String {
        match self.index_value {
            Some(v) => format!(
                "Arbeitspreis {} ({:.4} + {:.4} + {:.4}\u{d7}{:.4}\u{202f}ct/kWh)",
                self.index_name,
                self.base_ct_per_kwh,
                self.spread_ct_per_kwh,
                self.factor_ct_per_unit,
                v,
            ),
            None => format!(
                "Arbeitspreis {} (Indexwert nicht verf\u{fc}gbar)",
                self.index_name
            ),
        }
    }
}

// ── Per-category product structs ──────────────────────────────────────────────

/// STROM electricity product.
///
/// Used by `ElectricityProvider` (static) and `DynamicElectricityProvider`
/// (§41a, when `dynamic_epex = true`). Also the base for `ControllableLoadProduct`
/// and `SharingProduct` via `#[serde(flatten)]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ElectricityProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,
    /// Tariff register annotation (e.g. "Eintarif", "Zweitarif").
    #[serde(default)]
    pub register_count: Option<serde_json::Value>,
    /// RLM demand charge (ct/kW/month). §41 EnWG large commercial.
    #[serde(default)]
    pub leistungspreis_strom_ct_per_kw_month: Option<Decimal>,
    /// Block/graduated tariff tiers. Overrides arbeitspreis_ct_per_kwh when set.
    #[serde(default)]
    pub block_tiers: Option<Vec<BlockTierInput>>,
    /// B2B indexed price (Phelix Base, EEX etc.). §41 Abs. 3 EnWG.
    #[serde(default)]
    pub indexed_price: Option<IndexedPriceConfig>,
    #[serde(default)]
    pub seasonal_prices: Option<Vec<SeasonalPriceOverride>>,
    /// true → §41a EPEX Spot per-interval billing. Requires iMSys (§41b EnWG).
    #[serde(default)]
    pub dynamic_epex: bool,
    /// Price floor for §41a (ct/kWh). None = full market exposure.
    #[serde(default)]
    pub dynamic_epex_floor_ct_kwh: Option<Decimal>,
    #[serde(default)]
    pub auf_abschlag_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub auf_abschlag_eur_per_month: Option<Decimal>,
    /// MSB Grundgebühr bundled on retail invoice (ct/day). §41 EnWG.
    #[serde(default)]
    pub msb_gebuehr_ct_per_day: Option<Decimal>,
    /// §9 StromStG typed exemption.
    #[serde(default)]
    pub stromsteuer_befreiung: StromsteuerBefreiung,
    /// Legacy §9 Nr. 4 bool. New products: use `stromsteuer_befreiung`.
    #[serde(default)]
    pub industrie_stromsteuer_befreiung: bool,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    #[serde(default)]
    pub stromsteuer_ct_per_kwh_override: Option<Decimal>,
    /// Price guarantee end date — invoice display only (§41 Abs. 1 Nr. 4 EnWG).
    #[serde(default, with = "date_option_serde")]
    pub preisgarantie_bis: Option<time::Date>,
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
    /// Plant capacity (kWp) for §12 Abs. 3 UStG 0% MwSt auto-application.
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    /// §42 EnWG typed energy source mix (CO₂ label).
    #[serde(default)]
    pub energiequellen: Option<EnergieQuellen>,
    /// true when Stromsteuer applies to solar self-consumption (normally §9a exempt).
    #[serde(default)]
    pub solar_include_stromsteuer: bool,
    /// EEG Gutschrift EUR pass-through — set at bill-time from einsd, not stored in tarifbd.
    #[serde(default)]
    pub eeg_gutschrift_eur: Option<Decimal>,
}

/// §14a EnWG controllable load — WAERMEPUMPE / WALLBOX.
///
/// Extends `ElectricityProduct` (via `#[serde(flatten)]`) with §14a Steuerungsrabatt
/// fields. `ControllableLoadProvider` delegates standard electricity billing to
/// `ElectricityProvider` then appends the §14a credit positions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControllableLoadProduct {
    #[serde(flatten)]
    pub base: ElectricityProduct,
    /// §14a Modul 1: per-kWh NNE reduction credit (ct/kWh).
    #[serde(default)]
    pub sect14a_modul1_nne_reduktion_ct_per_kwh: Option<Decimal>,

    /// §14a Modul 2 — zeitvariables Netzentgelt (BK6-22-300 Anlage 2 §2).
    ///
    /// Three Tarifstufen, not two: Hochtarif, Standardtarif and Niedertarif,
    /// each a NNE rate in ct/kWh published by the Netzbetreiber. All three must
    /// be set together; the matching band energies arrive on
    /// [`crate::Sect14aModul2Verbrauch`]. When Modul 2 is billed, the flat NNE
    /// Arbeitspreis from `GridInput` must be left unset — the bands *replace*
    /// it, and setting both raises `MODUL2_AND_FLAT_NNE`.
    #[serde(default)]
    pub sect14a_modul2_nne_ht_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 2 Standardtarif rate. See `sect14a_modul2_nne_ht_ct_per_kwh`.
    #[serde(default)]
    pub sect14a_modul2_nne_st_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 2 Niedertarif rate. See `sect14a_modul2_nne_ht_ct_per_kwh`.
    #[serde(default)]
    pub sect14a_modul2_nne_nt_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 3: per-kWh Steuerungsentschädigung (ct/kWh).
    #[serde(default)]
    pub sect14a_modul3_entschaedigung_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 1: annual capacity-based NNE reduction (EUR/kW/year).
    #[serde(default)]
    pub steuerungsrabatt_modul1_eur_per_kw_year: Option<Decimal>,
    /// §14a Modul 3: annual capacity-based Entschädigung (EUR/kW/year).
    #[serde(default)]
    pub steuerungsrabatt_modul3_eur_per_kw_year: Option<Decimal>,
}

/// Natural gas supply product — GAS (SLP / RLM / B2B indexed).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GasProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub gas_grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,
    /// RLM demand charge (ct/kW/month) for large gas customers. §41 EnWG.
    #[serde(default)]
    pub gas_leistungspreis_ct_per_kw_month: Option<Decimal>,
    /// B2B indexed gas price (TTF, NCG, GASPOOL). §41 Abs. 3 EnWG.
    /// Also accepted as `"indexed_price"` for backward compat.
    #[serde(default, alias = "indexed_price")]
    pub gas_indexed_price: Option<IndexedPriceConfig>,
    #[serde(default)]
    pub seasonal_prices: Option<Vec<SeasonalPriceOverride>>,
    /// §54 EnergieStG full exemption (KWK / industrial manufacturing).
    #[serde(default)]
    pub gas_energiesteuer_befreiung: bool,
    #[serde(default)]
    pub auf_abschlag_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub auf_abschlag_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    #[serde(default)]
    pub energiesteuer_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub behg_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default, with = "date_option_serde")]
    pub preisgarantie_bis: Option<time::Date>,
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// District heating / Fernwärme product — WAERME.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HeatProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub waerme_grundpreis_eur_per_month: Option<Decimal>,
    /// Annual capacity charge (EUR/kW/year). Pro-rated per billing period.
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_year: Option<Decimal>,
    /// Monthly capacity charge alternative (EUR/kW/month).
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_month: Option<Decimal>,
    #[serde(default)]
    pub waerme_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// true → §12 Abs. 2 Nr. 1 UStG 7% MwSt auto-applied (renewable Fernwärme).
    #[serde(default)]
    pub waerme_is_renewable: bool,
    /// Wärmeplanungsgesetz §14 renewable share (0.0–1.0). Mandatory disclosure.
    #[serde(default)]
    pub waerme_erneuerbar_anteil_pct: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// Solar PV / Eigenverbrauch / §42b GGV / §21 Abs. 3 Mieterstrom product — SOLAR.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SolarProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub solar_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Grid remainder rate for §42b GGV hybrid billing (ct/kWh).
    ///
    /// In GGV (Gemeinschaftliche Gebäudeversorgung) billing, some consumption
    /// comes from the building's PV plant (charged at `solar_arbeitspreis_ct_per_kwh`)
    /// and the remainder comes from the grid (charged at this rate).
    ///
    /// When not set, `solar_arbeitspreis_ct_per_kwh` is used as fallback.
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// §21 Abs. 3 EEG Mieterstrom-Zuschlag (ct/kWh).
    #[serde(default)]
    pub mieterstrom_aufschlag_ct_per_kwh: Option<Decimal>,
    /// §42a EEG GGV community energy discount (ct/kWh).
    #[serde(default)]
    pub gemeinschaft_rabatt_ct_per_kwh: Option<Decimal>,
    /// true when Stromsteuer applies (normally exempt §9a Nr. 1 StromStG).
    #[serde(default)]
    pub solar_include_stromsteuer: bool,
    /// Plant capacity (kWp) for §12 Abs. 3 UStG 0% MwSt.
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// EEG feed-in Gutschrift — LF pays generator. Category: EEG.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EegProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub eeg_verguetungssatz_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_marktpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_managementpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub kwkg_zuschlag_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Direktvermarktung feed-in — LF as market premium payer. Category: EINSPEISUNG.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EinspeisungProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub marktwert_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub vermarktungsgebuehr_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Home Energy Management System subscription. Category: HEMS.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HemsProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub hems_subscription_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub hems_optimization_event_eur: Option<Decimal>,
    #[serde(default)]
    pub hems_readout_event_eur: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// EV charging CPO/EMSP. Category: EMOBILITY.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EmobilityProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub emobility_service_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub emobility_kwh_price_ct: Option<Decimal>,
    #[serde(default)]
    pub emobility_session_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub emobility_roaming_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Energy services subscription. Category: ENERGIEDIENSTLEISTUNG.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServiceProduct {
    #[serde(default)]
    pub product_code: Option<String>,
    #[serde(default)]
    pub service_fee_eur: Option<Decimal>,
    #[serde(default)]
    pub service_event_price_eur: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// §42c EnWG community energy sharing. Category: SHARING.
///
/// Bills the full grid consumption via `ElectricityProvider` (base), then
/// credits the allocated community generation via `EnergyShareProvider`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SharingProduct {
    #[serde(flatten)]
    pub electricity: ElectricityProduct,
    /// Community sharing credit rate (ct/kWh) applied to allocated community kWh.
    #[serde(default)]
    pub sharing_credit_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub sharing_description: Option<String>,
}

// ── Product enum ──────────────────────────────────────────────────────────────

/// Typed product dispatch enum — primary entry point for `energy-billing`.
///
/// Deserializes from `tarifbd` JSONB via the `"category"` field.
///
/// ```rust
/// use energy_billing::Product;
/// let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":28.5}"##;
/// let p: Product = serde_json::from_str(json).unwrap();
/// assert!(matches!(p, Product::Strom(_)));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "category")]
pub enum Product {
    #[serde(rename = "STROM")]
    Strom(ElectricityProduct),
    #[serde(rename = "WAERMEPUMPE")]
    Waermepumpe(ControllableLoadProduct),
    #[serde(rename = "WALLBOX")]
    Wallbox(ControllableLoadProduct),
    #[serde(rename = "GAS")]
    Gas(GasProduct),
    #[serde(rename = "WAERME")]
    Waerme(HeatProduct),
    #[serde(rename = "SOLAR")]
    Solar(SolarProduct),
    #[serde(rename = "EEG")]
    Eeg(EegProduct),
    #[serde(rename = "EINSPEISUNG")]
    Einspeisung(EinspeisungProduct),
    #[serde(rename = "HEMS")]
    Hems(HemsProduct),
    #[serde(rename = "EMOBILITY")]
    Emobility(EmobilityProduct),
    #[serde(rename = "ENERGIEDIENSTLEISTUNG")]
    Energiedienstleistung(ServiceProduct),
    #[serde(rename = "SHARING")]
    Sharing(SharingProduct),
}

impl Product {
    /// Product category string matching the tarifbd JSONB `"category"` field.
    #[must_use]
    pub fn category_str(&self) -> &'static str {
        match self {
            Self::Strom(_) => "STROM",
            Self::Waermepumpe(_) => "WAERMEPUMPE",
            Self::Wallbox(_) => "WALLBOX",
            Self::Gas(_) => "GAS",
            Self::Waerme(_) => "WAERME",
            Self::Solar(_) => "SOLAR",
            Self::Eeg(_) => "EEG",
            Self::Einspeisung(_) => "EINSPEISUNG",
            Self::Hems(_) => "HEMS",
            Self::Emobility(_) => "EMOBILITY",
            Self::Energiedienstleistung(_) => "ENERGIEDIENSTLEISTUNG",
            Self::Sharing(_) => "SHARING",
        }
    }

    /// Product code from tarifbd (for ERP routing / audit trail).
    #[must_use]
    pub fn product_code(&self) -> Option<&str> {
        match self {
            Self::Strom(p) => p.product_code.as_deref(),
            Self::Waermepumpe(p) | Self::Wallbox(p) => p.base.product_code.as_deref(),
            Self::Gas(p) => p.product_code.as_deref(),
            Self::Waerme(p) => p.product_code.as_deref(),
            Self::Solar(p) => p.product_code.as_deref(),
            Self::Eeg(p) => p.product_code.as_deref(),
            Self::Einspeisung(p) => p.product_code.as_deref(),
            Self::Hems(p) => p.product_code.as_deref(),
            Self::Emobility(p) => p.product_code.as_deref(),
            Self::Energiedienstleistung(p) => p.product_code.as_deref(),
            Self::Sharing(p) => p.electricity.product_code.as_deref(),
        }
    }

    /// The §42 EnWG Stromkennzeichnung declared on this product, if any.
    ///
    /// Electricity variants only — §42 is an electricity-disclosure duty. The
    /// service copies this onto the `BillingContext` so it reaches the invoice.
    #[must_use]
    pub fn energiequellen(&self) -> Option<&EnergieQuellen> {
        match self {
            Self::Strom(p) => p.energiequellen.as_ref(),
            Self::Waermepumpe(p) | Self::Wallbox(p) => p.base.energiequellen.as_ref(),
            _ => None,
        }
    }

    /// Minimum invoice amount (brutto EUR) for B2B Mindestabnahme contracts.
    #[must_use]
    pub fn minimum_invoice_eur_brutto(&self) -> Option<Decimal> {
        match self {
            Self::Strom(p) => p.minimum_invoice_eur_brutto,
            Self::Waermepumpe(p) | Self::Wallbox(p) => p.base.minimum_invoice_eur_brutto,
            Self::Gas(p) => p.minimum_invoice_eur_brutto,
            Self::Waerme(p) => p.minimum_invoice_eur_brutto,
            Self::Solar(p) => p.minimum_invoice_eur_brutto,
            _ => None,
        }
    }

    /// Build a `BillingEngine` configured for this product.
    ///
    /// ```rust
    /// use energy_billing::{Product, GridInput, RegulatoryRates};
    /// let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":30.0}"##;
    /// let p: Product = serde_json::from_str(json).unwrap();
    /// let engine = p.build_engine(&GridInput::default(), &RegulatoryRates::default());
    /// ```
    #[must_use]
    pub fn build_engine(
        &self,
        grid: &crate::quantities::GridInput,
        rates: &crate::rates::RegulatoryRates,
    ) -> crate::engine::BillingEngine {
        use crate::engine::BillingEngine;
        use crate::providers::{
            ControllableLoadProvider, DynamicElectricityProvider, EegProvider, EinspeisungProvider,
            ElectricityProvider, EmobilityProvider, EnergyShareProvider, GasProvider, HeatProvider,
            HemsProvider, MwStProvider, ServiceProvider, SolarProvider,
        };
        use std::collections::HashMap;

        match self {
            Self::Strom(p) => {
                let mwst = rates.effective_mwst_electricity(p);
                if p.dynamic_epex {
                    BillingEngine::new()
                        .add(DynamicElectricityProvider::with_epex_map(
                            p.clone(),
                            grid.clone(),
                            HashMap::new(),
                        ))
                        .add(MwStProvider::new(mwst))
                } else {
                    BillingEngine::new()
                        .add(ElectricityProvider::new(p.clone(), grid.clone()))
                        .add(MwStProvider::new(mwst))
                }
            }
            Self::Waermepumpe(p) | Self::Wallbox(p) => {
                let mwst = rates.effective_mwst_electricity(&p.base);
                BillingEngine::new()
                    .add(ControllableLoadProvider::new(p.clone(), grid.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Gas(p) => {
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(GasProvider::new(p.clone(), grid.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Waerme(p) => {
                let mwst = if let Some(r) = p.mwst_rate_override {
                    r
                } else if p.waerme_is_renewable {
                    rust_decimal::dec!(0.07)
                } else {
                    rates.mwst_rate
                };
                BillingEngine::new()
                    .add(HeatProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Solar(p) => {
                let mwst = rates.effective_mwst_solar(p);
                BillingEngine::new()
                    .add(SolarProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Eeg(p) => {
                let mwst = rates.effective_mwst_eeg(p);
                BillingEngine::new()
                    .add(EegProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Einspeisung(p) => {
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(EinspeisungProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Hems(p) => {
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(HemsProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Emobility(p) => {
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(EmobilityProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Energiedienstleistung(p) => {
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(ServiceProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Sharing(p) => {
                let mwst = rates.effective_mwst_electricity(&p.electricity);
                BillingEngine::new()
                    .add(ElectricityProvider::new(
                        p.electricity.clone(),
                        grid.clone(),
                    ))
                    .add(EnergyShareProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_strom_roundtrip() {
        let json =
            r#"{"category":"STROM","arbeitspreis_ct_per_kwh":28.5,"grundpreis_ct_per_day":8.0}"#;
        let p: Product = serde_json::from_str(json).unwrap();
        match &p {
            Product::Strom(e) => {
                assert_eq!(e.arbeitspreis_ct_per_kwh, Some(rust_decimal::dec!(28.5)));
            }
            _ => panic!("expected Strom"),
        }
        let json2 = serde_json::to_string(&p).unwrap();
        let p2: Product = serde_json::from_str(&json2).unwrap();
        assert_eq!(p.category_str(), p2.category_str());
    }

    #[test]
    fn product_waermepumpe_flattens_electricity_base() {
        let json = r#"{"category":"WAERMEPUMPE","arbeitspreis_ct_per_kwh":20.0,"sect14a_modul1_nne_reduktion_ct_per_kwh":1.5}"#;
        let p: Product = serde_json::from_str(json).unwrap();
        match p {
            Product::Waermepumpe(c) => {
                assert_eq!(
                    c.base.arbeitspreis_ct_per_kwh,
                    Some(rust_decimal::dec!(20.0))
                );
                assert_eq!(
                    c.sect14a_modul1_nne_reduktion_ct_per_kwh,
                    Some(rust_decimal::dec!(1.5))
                );
            }
            _ => panic!("expected Waermepumpe"),
        }
    }

    #[test]
    fn product_gas_roundtrip() {
        let json = r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":7.5,"gas_grundpreis_ct_per_day":5.0}"#;
        let p: Product = serde_json::from_str(json).unwrap();
        match p {
            Product::Gas(g) => {
                assert_eq!(
                    g.gas_arbeitspreis_ct_per_kwh_hs,
                    Some(rust_decimal::dec!(7.5))
                );
            }
            _ => panic!("expected Gas"),
        }
    }

    #[test]
    fn product_unknown_category_errors() {
        let json = r#"{"category":"UNKNOWN_PRODUCT","foo":1}"#;
        assert!(serde_json::from_str::<Product>(json).is_err());
    }
}
