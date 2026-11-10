//! Typed product definitions for the German retail energy market.
//!
//! The [`Product`] enum is the primary entry point. It deserializes directly from
//! `productd` product JSONB using the `"category"` discriminator field and provides
//! a [`Product::build_engine`] method to construct the matching `BillingEngine`.
//!
//! ## Architecture
//!
//! One typed struct per product category, each carrying only the fields its
//! category has. A flat input with every category's optional fields on it
//! cannot distinguish "not applicable" from "not supplied".
//!
//! ```text
//! Product::Strom(ElectricityProduct)           → ElectricityProvider / DynamicElectricityProvider
//! Product::Waermepumpe(ControllableLoadProduct) → ControllableLoadProvider (§14a)
//! Product::Wallbox(ControllableLoadProduct)     → ControllableLoadProvider (§14a)
//! Product::Gas(GasProduct)                      → GasProvider
//! Product::Waerme(HeatProduct)                  → HeatProvider
//! Product::Wasser(WaterProduct)                 → WaterProvider (Trinkwasser + Abwasser)
//! Product::Solar(SolarProduct)                  → SolarProvider
//! Product::Eeg(EegProduct)                      → EegProvider
//! Product::Einspeisung(EinspeisungProduct)       → EinspeisungProvider
//! Product::Hems(HemsProduct)                    → HemsProvider
//! Product::Emobility(EmobilityProduct)           → EmobilityProvider
//! Product::Energiedienstleistung(ServiceProduct) → ServiceProvider
//! Product::Sharing(SharingProduct)               → ElectricityProvider + EnergyShareProvider
//! ```
//!
//! ## Deserializing from `productd` JSONB
//!
//! ```rust
//! use energy_billing::Product;
//!
//! let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":"28.5"}"##;
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

// ── EnergieQuellen ────────────────────────────────────────────────────────────

/// §42 EnWG Abs. 2 Nr. 2 — typed energy source mix for invoice disclosure.
///
/// Replaces the legacy free-text energiemix string with structured data for
/// CO₂ label generation, HKN traceability, and BO4E Energiemix generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EnergieQuellen {
    /// Renewable share of the supply mix, in percent (`100` = 100 %).
    #[serde(default)]
    pub erneuerbar_pct: Decimal,
    /// Nuclear share of the supply mix, in percent; `None` when not stated.
    #[serde(default)]
    pub nuclear_pct: Option<Decimal>,
    /// Fossil share of the supply mix, in percent; `None` when not stated.
    #[serde(default)]
    pub fossil_pct: Option<Decimal>,
    /// Specific CO₂ emissions g/kWh — mandatory on electricity invoices (§42 Abs. 2 Nr. 2 EnWG).
    pub co2_g_per_kwh: Decimal,
    /// `true` when the renewable share is backed by Herkunftsnachweise.
    #[serde(default)]
    pub hkn_certified: bool,
    /// Country the Herkunftsnachweise were issued in, as free text.
    #[serde(default)]
    pub hkn_country: Option<String>,
    /// Free-text disclosure prose, carried inside the structured record.
    #[serde(default)]
    pub beschreibung: Option<String>,
}

// ── SeasonalPriceOverride ─────────────────────────────────────────────────────

/// One seasonal price band (Saisontarif). Months wrap: from_month=10, to_month=3 → Oct–Mar.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SeasonalPriceOverride {
    /// First month of the band, 1–12, inclusive.
    pub from_month: u8,
    /// Last month of the band, 1–12, inclusive; below `from_month` the band wraps over the year end.
    pub to_month: u8,
    /// Electricity Arbeitspreis for the band (ct/kWh); replaces `arbeitspreis_ct_per_kwh`.
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Gas Arbeitspreis for the band (ct/kWh_Hs); replaces `gas_arbeitspreis_ct_per_kwh_hs`.
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,
    /// Band name in the Arbeitspreis line, `Arbeitspreis Strom (<label>)` / `Arbeitspreis Gas (<label>)`;
    /// `Saisontarif` when `None`.
    #[serde(default)]
    pub label: Option<String>,
}

impl SeasonalPriceOverride {
    /// Whether `month` (1–12) falls inside the band, both ends inclusive.
    ///
    /// The providers look the band up by the month of the billing period's first day.
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
#[serde(deny_unknown_fields)]
pub struct BlockTierInput {
    /// Upper bound (kWh). None = open-ended last tier.
    pub bis_kwh: Option<Decimal>,
    /// Price for the kWh inside this tier (ct/kWh).
    pub preis_ct_per_kwh: Decimal,
}

// ── IndexedPriceConfig ────────────────────────────────────────────────────────

/// Indexed price for B2B contracts (§41 EnWG Sonderkundenvertrag).
///
/// Effective = base_ct + spread_ct + (index_value × factor_ct_per_unit).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IndexedPriceConfig {
    /// Fixed base component of the effective price (ct/kWh).
    pub base_ct_per_kwh: Decimal,
    /// Supplier spread added to the base (ct/kWh).
    pub spread_ct_per_kwh: Decimal,
    /// Index name printed in the Arbeitspreis line (e.g. a Phelix Base or TTF series).
    pub index_name: String,
    /// Index value for the billed period, in the index's own unit; `None` until it has arrived.
    pub index_value: Option<Decimal>,
    /// ct/kWh added per unit of `index_value`.
    pub factor_ct_per_unit: Decimal,
}

impl IndexedPriceConfig {
    /// The effective Arbeitspreis in ct/kWh: `base + spread + index_value × factor`.
    ///
    /// `None` while `index_value` is unset — no price can be formed then.
    #[must_use]
    pub fn effective_ct_per_kwh(&self) -> Option<Decimal> {
        let idx = self.index_value?;
        Some(self.base_ct_per_kwh + self.spread_ct_per_kwh + idx * self.factor_ct_per_unit)
    }

    /// Arbeitspreis line text, stating the formula with its figures, or that the index value is missing.
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
/// (§41a, when `dynamic_epex = true`). Also the `strom` part of
/// `ControllableLoadProduct` and `SharingProduct`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ElectricityProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Grundpreis in ct per day, billed for the contract's active days in the period.
    #[serde(default)]
    pub grundpreis_ct_per_day: Option<Decimal>,
    /// Eintarif Arbeitspreis (ct/kWh); used when no tier, HT/NT, indexed or seasonal price applies.
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Zweitarif Hochtarif Arbeitspreis (ct/kWh). Billed only together with
    /// `arbeitspreis_nt_ct_per_kwh` and both meter registers; one band priced alone is refused.
    #[serde(default)]
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    /// Zweitarif Niedertarif Arbeitspreis (ct/kWh). See `arbeitspreis_ht_ct_per_kwh`.
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
    /// B2B indexed price (Phelix Base, EEX etc.) — §41 EnWG Sonderkundenvertrag.
    #[serde(default)]
    pub indexed_price: Option<IndexedPriceConfig>,
    /// Saisontarif bands; the band containing the period's first month replaces the Eintarif Arbeitspreis.
    #[serde(default)]
    pub seasonal_prices: Option<Vec<SeasonalPriceOverride>>,
    /// true → §41a EPEX Spot per-interval billing. Requires iMSys (§41a Abs. 1 EnWG).
    #[serde(default)]
    pub dynamic_epex: bool,
    /// Price floor for §41a (ct/kWh). None = full market exposure.
    #[serde(default)]
    pub dynamic_epex_floor_ct_kwh: Option<Decimal>,
    /// Price ceiling for §41a (ct/kWh) — a contractual consumer-protection cap on
    /// the spot component. None = uncapped. Applied after the floor, so a plant
    /// with both bills the spot price clamped into `[floor, cap]`.
    #[serde(default)]
    pub dynamic_epex_cap_ct_kwh: Option<Decimal>,
    /// § 41a Arbeitspreis-Aufschlag (ct/kWh) — the supplier's margin on top of
    /// the clamped spot price. Distinct from `auf_abschlag_ct_per_kwh`, which
    /// is the static path's Rabatt/Aufschlag line.
    #[serde(default)]
    pub dynamic_aufschlag_ct_per_kwh: Option<Decimal>,
    /// Label of the price series the § 41a Arbeitspreis is formed from, as
    /// stated on the invoice — `productd`'s `dyn_source` column
    /// (e.g. `"EPEX Spot Day-Ahead"`). Defaults to EPEX day-ahead.
    #[serde(default)]
    pub dynamic_price_source: Option<String>,
    /// Per-kWh Rabatt (negative) or Aufschlag (positive) in ct/kWh, on the billed kWh.
    #[serde(default)]
    pub auf_abschlag_ct_per_kwh: Option<Decimal>,
    /// Monthly Rabatt (negative) or Aufschlag (positive) in EUR/month, scaled by the billed months.
    #[serde(default)]
    pub auf_abschlag_eur_per_month: Option<Decimal>,
    /// One-time Neukunden-/Sofortbonus (EUR, gross of the customer's credit).
    /// Credited on the invoice whose product carries it (usually the first or the
    /// Jahresabrechnung) as a Preisnachlass (§17 UStG Entgeltminderung).
    #[serde(default)]
    pub sofortbonus_eur: Option<Decimal>,
    /// Annual Treuebonus (EUR/year), pro-rated to the billed days and credited as
    /// a Preisnachlass (§17 UStG Entgeltminderung).
    #[serde(default)]
    pub treuebonus_eur_per_year: Option<Decimal>,
    /// MSB Grundgebühr bundled on retail invoice (ct/day). §41 EnWG.
    #[serde(default)]
    pub msb_gebuehr_ct_per_day: Option<Decimal>,
    /// How the Stromsteuer applies: Regelsatz, § 9 Abs. 1 Steuerbefreiung, or
    /// § 9 Abs. 2/3 ermäßigter Satz. See [`crate::steuer`] — a
    /// **Steuerentlastung** (§ 9a/§ 9b/§ 9c StromStG) is not one of these and
    /// belongs in `steuerentlastungen`.
    #[serde(default)]
    pub stromsteuer_tarif: crate::steuer::StromsteuerTarif,
    /// Reliefs the *customer* may claim afterwards at the Hauptzollamt. They do
    /// not change a single amount on this invoice; each renders one
    /// informational note so the customer knows what it carried.
    #[serde(default)]
    pub steuerentlastungen: Vec<crate::steuer::Steuerentlastung>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    /// Stromsteuer rate in ct/kWh replacing the period's statutory rate.
    #[serde(default)]
    pub stromsteuer_ct_per_kwh_override: Option<Decimal>,
    /// Price guarantee end date — invoice display only (§41 Abs. 1 Nr. 4 EnWG).
    #[serde(default, with = "date_option_serde")]
    pub preisgarantie_bis: Option<time::Date>,
    /// Mindestbetrag in EUR brutto per billing period. Copied onto
    /// `BillingContext::minimum_invoice_eur_brutto`, it makes the engine top a
    /// lower invoice up to it; read through [`Product::minimum_invoice_eur_brutto`].
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
    /// Plant capacity (kWp) — informational (prosumer plant size / MaStR). Does
    /// not affect the consumption VAT rate: a retail supply is standard-rated
    /// even where the customer runs their own PV (§12 Abs. 3 UStG covers the PV
    /// system supply, not the electricity).
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    /// §42 EnWG typed energy source mix (CO₂ label).
    #[serde(default)]
    pub energiequellen: Option<EnergieQuellen>,
    /// EEG Gutschrift EUR pass-through — set at bill-time from einsd, not stored in productd.
    #[serde(default)]
    pub eeg_gutschrift_eur: Option<Decimal>,
    /// The feed-in operator has elected the Kleinunternehmerregelung (§19 UStG)
    /// → the `eeg_gutschrift_eur` credit carries 0 % USt.
    ///
    /// The feed-in is a **separate supply** from the consumption billed here and
    /// is governed by the operator's own USt status (`einspeiser.ust_status`).
    /// Without this, the credit nets against the standard-rate base and
    /// understates the supplier's output VAT on the consumption supply by 19 %
    /// of the credit. Does not affect the consumption positions.
    #[serde(default)]
    pub eeg_gutschrift_kleinunternehmer_19_ustg: bool,
}

/// §14a EnWG controllable load — WAERMEPUMPE / WALLBOX.
///
/// Extends an `ElectricityProduct`, under `strom`, with §14a Steuerungsrabatt
/// fields. `ControllableLoadProvider` delegates standard electricity billing to
/// `ElectricityProvider` then appends the §14a credit positions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ControllableLoadProduct {
    /// The electricity supply the § 14a positions are added to.
    #[serde(rename = "strom", default)]
    pub base: ElectricityProduct,
    /// §14a **Modul 2** — prozentuale Reduzierung des NNE-Arbeitspreises,
    /// carried here as the resulting per-kWh credit (ct/kWh).
    ///
    /// The reduction attaches to the controllable device's own metered energy,
    /// which is why Modul 2 requires separate metering. It is the **alternative
    /// form of the base module**, so it excludes Modul 1
    /// (`MODUL1_AND_MODUL2`), and it takes no Modul 3
    /// (`MODUL2_AND_MODUL3`) — each pairing would reduce the same network usage
    /// twice.
    #[serde(default)]
    pub sect14a_modul2_nne_reduktion_ct_per_kwh: Option<Decimal>,

    /// §14a **Modul 3** — zeitvariables Netzentgelt (BK8-22/010-A), from 01.04.2025.
    ///
    /// Three Tarifstufen, not two: Hochtarif, Standardtarif and Niedertarif,
    /// each a NNE rate in ct/kWh published by the Netzbetreiber. All three must
    /// be set together; the matching band energies arrive on
    /// [`crate::Sect14aModul3Verbrauch`]. When Modul 3 is billed, the flat NNE
    /// Arbeitspreis from `GridInput` must be left unset — the bands *replace*
    /// it, and setting both raises `MODUL3_AND_FLAT_NNE`.
    #[serde(default)]
    pub sect14a_modul3_nne_ht_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 3 Standardtarif rate. See `sect14a_modul3_nne_ht_ct_per_kwh`.
    #[serde(default)]
    pub sect14a_modul3_nne_st_ct_per_kwh: Option<Decimal>,
    /// §14a Modul 3 Niedertarif rate. See `sect14a_modul3_nne_ht_ct_per_kwh`.
    #[serde(default)]
    pub sect14a_modul3_nne_nt_ct_per_kwh: Option<Decimal>,
    /// §14a Steuerungsentschädigung per kWh of dimmed energy (ct/kWh).
    ///
    /// Compensation for an actual Steuerungseingriff — **not** one of the three
    /// BK8-22/010-A modules, which are all rate reductions rather than payments
    /// for a dispatch that happened.
    #[serde(default)]
    pub sect14a_steuerungsentschaedigung_ct_per_kwh: Option<Decimal>,
    /// §14a **Modul 1** — pauschale Reduzierung des Netzentgelts, published by
    /// the Netzbetreiber as a flat annual amount (EUR/year).
    ///
    /// BK8-22/010-A fixes it nationwide as `80 EUR + 3 750 kWh × Arbeitspreis im
    /// Standardtarif × 0,2`, which lands between 110 and 190 EUR/year depending
    /// on the Netzbetreiber. There is no per-kW component: the reduction is the
    /// same whatever the steuerbare Leistung, which is precisely why it needs
    /// no additional metering and is the default where the connection holder
    /// makes no choice. It **may be combined with Modul 3**.
    #[serde(default)]
    pub sect14a_modul1_pauschale_eur_per_year: Option<Decimal>,
    /// §14a Steuerungsentschädigung as an annual capacity rate (EUR/kW/year),
    /// pro-rated by the hours actually dimmed. Not a BK8-22/010-A module.
    #[serde(default)]
    pub sect14a_steuerungsentschaedigung_eur_per_kw_year: Option<Decimal>,
}

/// Natural gas supply product — GAS (SLP / RLM / B2B indexed).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GasProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Grundpreis in ct per day, billed for the contract's active days in the period.
    #[serde(default)]
    pub gas_grundpreis_ct_per_day: Option<Decimal>,
    /// Arbeitspreis in ct per kWh_Hs (Brennwert basis); used when no indexed or seasonal price applies.
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,
    /// RLM demand charge (ct/kW/month) for large gas customers. §41 EnWG.
    #[serde(default)]
    pub gas_leistungspreis_ct_per_kw_month: Option<Decimal>,
    /// B2B indexed gas price (TTF, NCG, GASPOOL) — §41 EnWG Sonderkundenvertrag.
    #[serde(default)]
    pub gas_indexed_price: Option<IndexedPriceConfig>,
    /// Saisontarif bands; the band containing the period's first month replaces `gas_arbeitspreis_ct_per_kwh_hs`.
    #[serde(default)]
    pub seasonal_prices: Option<Vec<SeasonalPriceOverride>>,
    /// How the Energiesteuer applies: Regelsatz, or steuerfreie Verwendung
    /// against an Erlaubnis nach § 24 Abs. 2 EnergieStG. See [`crate::steuer`].
    ///
    /// §§ 53a and 54 EnergieStG are **Steuerentlastungen** the customer claims
    /// afterwards, not exemptions at supply — they belong in
    /// `steuerentlastungen`.
    #[serde(default)]
    pub energiesteuer_tarif: crate::steuer::EnergiesteuerTarif,
    /// Reliefs the *customer* may claim afterwards at the Hauptzollamt. Render
    /// as notes; they change no amount on this invoice.
    #[serde(default)]
    pub steuerentlastungen: Vec<crate::steuer::Steuerentlastung>,
    /// Per-kWh Rabatt (negative) or Aufschlag (positive) in ct/kWh_Hs, on the billed kWh_Hs.
    #[serde(default)]
    pub auf_abschlag_ct_per_kwh: Option<Decimal>,
    /// Monthly Rabatt (negative) or Aufschlag (positive) in EUR/month, scaled by the billed months.
    #[serde(default)]
    pub auf_abschlag_eur_per_month: Option<Decimal>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    /// Energiesteuer rate in ct/kWh_Hs replacing the period's statutory rate.
    #[serde(default)]
    pub energiesteuer_gas_ct_per_kwh_override: Option<Decimal>,
    /// BEHG CO₂ rate in ct/kWh_Hs; replaces the statutory rate only where the period has one.
    #[serde(default)]
    pub behg_gas_ct_per_kwh_override: Option<Decimal>,
    /// Price-guarantee end date (`YYYY-MM-DD`). `GasProvider` does not read it: unlike
    /// `ElectricityProduct::preisgarantie_bis`, it renders no notice and raises no warning.
    #[serde(default, with = "date_option_serde")]
    pub preisgarantie_bis: Option<time::Date>,
    /// Mindestbetrag in EUR brutto per billing period. Copied onto
    /// `BillingContext::minimum_invoice_eur_brutto`, it makes the engine top a
    /// lower invoice up to it; read through [`Product::minimum_invoice_eur_brutto`].
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// District heating / Fernwärme product — WAERME.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HeatProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Grundpreis in EUR/month, times the meter input's months or else the billed months.
    #[serde(default)]
    pub waerme_grundpreis_eur_per_month: Option<Decimal>,
    /// Annual capacity charge (EUR/kW/year). Pro-rated per billing period.
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_year: Option<Decimal>,
    /// Monthly capacity charge alternative (EUR/kW/month).
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_month: Option<Decimal>,
    /// Arbeitspreis in ct/kWh; `waerme_indexed_price`, once its index value is known, takes precedence.
    #[serde(default)]
    pub waerme_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// AVBFernwärmeV §24 Abs. 4 Preisänderungsklausel — the Arbeitspreis is
    /// index-linked (fuel/wage/market indices). When set it resolves the effective
    /// Arbeitspreis and takes precedence over the static `waerme_arbeitspreis_ct_per_kwh`.
    #[serde(default)]
    pub waerme_indexed_price: Option<IndexedPriceConfig>,
    /// Wärmeplanungsgesetz §14 renewable share (0.0–1.0). Mandatory disclosure.
    ///
    /// Disclosure only — it does **not** change the VAT rate. District heating is
    /// standard-rated (19 %); the 7 % on gas/Fernwärme was the temporary §28 Abs. 5/6
    /// UStG crisis measure (01.10.2022–31.03.2024), resolved period-aware by
    /// [`crate::rates::mwst_rate_for_gas_waerme_period`], not by renewable share.
    #[serde(default)]
    pub waerme_erneuerbar_anteil_pct: Option<Decimal>,
    /// CO₂ cost passed through on the heat supply, in ct/kWh_th.
    ///
    /// **CO2KostAufG § 3** obliges the supplier to state the CO₂ cost it
    /// actually bore on a Wärmelieferung, and § 5 makes that statement part of
    /// the Abrechnung. The rate is *not* the gas BEHG rate: the heat generator's
    /// fuel mix and conversion losses sit between the two, so it is a property
    /// of the heat product and the operator states it.
    ///
    /// `None` leaves the position off. `Some(0)` is how a fully renewable
    /// network says it bears none, and still prints the § 5 statement.
    #[serde(default)]
    pub waerme_co2_kosten_ct_per_kwh: Option<Decimal>,
    /// Specific CO₂ emissions of the delivered heat in g/kWh — the
    /// CO2KostAufG § 3 Abs. 2 figure the cost statement is derived from.
    /// Disclosure only.
    #[serde(default)]
    pub waerme_co2_emission_g_per_kwh: Option<Decimal>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    /// Mindestbetrag in EUR brutto per billing period. Copied onto
    /// `BillingContext::minimum_invoice_eur_brutto`, it makes the engine top a
    /// lower invoice up to it; read through [`Product::minimum_invoice_eur_brutto`].
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// Legal regime of the Abwasser charge — decides the USt treatment.
///
/// Over 90 % of German municipalities levy wastewater as a public-law fee
/// (Gebühr per KAG-Satzung), which is a hoheitliche Leistung outside the
/// scope of USt. Privatised operators charge a privatrechtliches Entgelt at
/// the 19 % Regelsteuersatz instead.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AbwasserRegime {
    /// Öffentlich-rechtliche Gebühr (KAG-Satzung) — no USt.
    #[default]
    PublicLawFee,
    /// Privatrechtliches Entgelt — 19 % USt.
    PrivateLawCharge,
}

/// Municipal drinking-water / wastewater product — WASSER.
///
/// Water is not a MaKo commodity — there is no market-communication regime,
/// no EDIFACT, no BNetzA. The category exists for multi-utility (Stadtwerke)
/// deployments that bill water alongside energy on the same platform:
///
/// - **Trinkwasser** (AVBWasserV / municipal statute): Grundpreis +
///   Mengenpreis, always at the ermäßigter USt-Satz of 7 %
///   (§12 Abs. 2 Nr. 1 UStG, Anlage 2 Nr. 34 — water is a foodstuff).
/// - **Abwasser** (gesplittete Abwassergebühr, mandated by case law —
///   VGH Baden-Württemberg, 11.03.2010): Schmutzwasser per m³ on the
///   Frischwassermaßstab minus metered Absetzungen, plus
///   Niederschlagswasser per m² of sealed surface per year.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WaterProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Trinkwasser Grundpreis (Systempreis, typically by meter size) in EUR/month.
    #[serde(default)]
    pub wasser_grundpreis_eur_per_month: Option<Decimal>,
    /// Trinkwasser Mengenpreis in EUR/m³ (7 % USt).
    #[serde(default)]
    pub wasser_mengenpreis_eur_per_m3: Option<Decimal>,
    /// Schmutzwassergebühr in EUR/m³ (Frischwassermaßstab minus Absetzungen).
    #[serde(default)]
    pub schmutzwasser_eur_per_m3: Option<Decimal>,
    /// Niederschlagswassergebühr in EUR per m² sealed surface per **year**.
    #[serde(default)]
    pub niederschlagswasser_eur_per_m2_year: Option<Decimal>,
    /// USt treatment of the Abwasser positions. Trinkwasser is always 7 %.
    #[serde(default)]
    pub abwasser_regime: AbwasserRegime,
    /// Override for the Trinkwasser USt rate (edge cases only).
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Solar PV / Eigenverbrauch / §42b EnWG GGV / §21 Abs. 3 EEG Mieterstrom product — SOLAR.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SolarProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Arbeitspreis for the PV supply (ct/kWh) — the Eigenverbrauch kWh, or the PV portion under GGV.
    #[serde(default)]
    pub solar_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Grid remainder rate for §42b EnWG GGV hybrid billing (ct/kWh).
    ///
    /// In GGV (Gemeinschaftliche Gebäudeversorgung) billing, some consumption
    /// comes from the building's PV plant (charged at `solar_arbeitspreis_ct_per_kwh`)
    /// and the remainder comes from the grid (charged at this rate).
    ///
    /// When not set, `solar_arbeitspreis_ct_per_kwh` is used as fallback.
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// § 42a Abs. 4 EnWG — the Grundversorgungs-Arbeitspreis (ct/kWh) the
    /// Mieterstrompreis is capped against.
    ///
    /// A Mieterstromvertrag may charge at most **90 %** of the ortsüblichen
    /// Grundversorgungstarifs. Set this and the provider refuses a
    /// `solar_arbeitspreis_ct_per_kwh` above the cap.
    ///
    /// The **Mieterstromzuschlag (§ 21 Abs. 3 EEG 2023)** deliberately has no
    /// field here: it is the *Anlagenbetreiber's* claim against the
    /// *Netzbetreiber*, settled by `eeg-billing`'s `TenantElectricity` scheme,
    /// not a surcharge on the tenant.
    #[serde(default)]
    pub grundversorgung_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// §42b EnWG GGV community energy discount (ct/kWh).
    #[serde(default)]
    pub gemeinschaft_rabatt_ct_per_kwh: Option<Decimal>,
    /// How the Stromsteuer applies to the solar supply — required, because it
    /// turns on facts only the operator holds.
    ///
    /// `{"art": "KLEINANLAGE", …}` claims § 9 Abs. 1 Nr. 3 StromStG with the
    /// plant's Energieträger, Nennleistung, Entnahme, distance and Erlaubnis,
    /// and the run refuses a claim they do not carry. A Lieferant that neither
    /// operates the plant nor has it operated is outside Buchst. b (§ 12b
    /// Abs. 2 StromStV) and states `{"art": "REGEL"}`; so does an operator
    /// renouncing the exemption (§ 9 Abs. 1b StromStG).
    pub stromsteuer_tarif: crate::steuer::StromsteuerTarif,
    /// Plant capacity (kWp) — informational (MaStR / §48 EEG size class).
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    /// Operator has elected the Kleinunternehmerregelung (§19 UStG) → the feed-in
    /// Gutschrift carries 0 % USt. An election, not a function of plant size.
    #[serde(default)]
    pub kleinunternehmer_19_ustg: bool,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
    /// Mindestbetrag in EUR brutto per billing period. Copied onto
    /// `BillingContext::minimum_invoice_eur_brutto`, it makes the engine top a
    /// lower invoice up to it; read through [`Product::minimum_invoice_eur_brutto`].
    #[serde(default)]
    pub minimum_invoice_eur_brutto: Option<Decimal>,
}

/// EEG feed-in Gutschrift — LF pays generator. Category: EEG.
///
/// It carries no KWKG-Zuschlag: § 6 Abs. 1 KWKG gives the Betreiber of a
/// KWK-Anlage the claim „gegenüber dem Netzbetreiber, mit dessen Netz ihre
/// KWK-Anlage unmittelbar oder mittelbar verbunden ist", which `einsd` settles.
/// A product naming `kwkg_zuschlag_ct_per_kwh` is refused as an unknown field.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EegProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Einspeisevergütung (ct/kWh), credited on the fed-in kWh outside the negative-price hours.
    #[serde(default)]
    pub eeg_verguetungssatz_ct_per_kwh: Option<Decimal>,
    /// Marktprämie (ct/kWh), credited on the fed-in kWh outside the negative-price hours.
    #[serde(default)]
    pub eeg_marktpraemie_ct_per_kwh: Option<Decimal>,
    /// Contractual Direktvermarktung management fee (ct/kWh), credited on every fed-in kWh,
    /// negative-price hours included.
    #[serde(default)]
    pub eeg_managementpraemie_ct_per_kwh: Option<Decimal>,
    /// Plant capacity (kWp) — informational (MaStR / §48 EEG size class).
    #[serde(default)]
    pub anlage_kwp: Option<Decimal>,
    /// Operator has elected the Kleinunternehmerregelung (§19 UStG) → 0 % USt on
    /// the feed-in Gutschrift. An election, not a function of plant size.
    #[serde(default)]
    pub kleinunternehmer_19_ustg: bool,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Direktvermarktung feed-in — LF as market premium payer. Category: EINSPEISUNG.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EinspeisungProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Marktwert (ct/kWh) credited to the operator on the fed-in kWh.
    #[serde(default)]
    pub marktwert_ct_per_kwh: Option<Decimal>,
    /// Vermarktungsgebühr (ct/kWh, positive), deducted from the settlement on the fed-in kWh.
    #[serde(default)]
    pub vermarktungsgebuehr_ct_per_kwh: Option<Decimal>,
    /// Operator has elected the Kleinunternehmerregelung (§19 UStG) → 0 % USt on
    /// the feed-in Gutschrift. An election, not a function of plant size.
    #[serde(default)]
    pub kleinunternehmer_19_ustg: bool,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Home Energy Management System subscription. Category: HEMS.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct HemsProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Subscription fee in EUR/month, times the usage's months (one when unstated).
    #[serde(default)]
    pub hems_subscription_eur_per_month: Option<Decimal>,
    /// Price per optimisation event (EUR); `0.0` when events are included in the fee.
    #[serde(default)]
    pub hems_optimization_event_eur: Option<Decimal>,
    /// Price per smart-meter readout (EUR); `0.0` when readouts are included in the fee.
    #[serde(default)]
    pub hems_readout_event_eur: Option<Decimal>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// EV charging CPO/EMSP. Category: EMOBILITY.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EmobilityProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Servicegebühr in EUR/month, times the usage's months (one when unstated).
    #[serde(default)]
    pub emobility_service_fee_eur: Option<Decimal>,
    /// Charging-energy price (ct/kWh); `0.0` when charging is bundled into the fee.
    #[serde(default)]
    pub emobility_kwh_price_ct: Option<Decimal>,
    /// Fee per charging session (EUR).
    #[serde(default)]
    pub emobility_session_fee_eur: Option<Decimal>,
    /// Fee per roaming session (EUR).
    #[serde(default)]
    pub emobility_roaming_fee_eur: Option<Decimal>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// Energy services subscription. Category: ENERGIEDIENSTLEISTUNG.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ServiceProduct {
    /// `productd` product code, carried for ERP routing and the audit trail ([`Product::product_code`]).
    #[serde(default)]
    pub product_code: Option<String>,
    /// Service fee in EUR/month, times the usage's months (one when unstated).
    #[serde(default)]
    pub service_fee_eur: Option<Decimal>,
    /// Price per billable event (EUR); a price on the usage input takes precedence.
    #[serde(default)]
    pub service_event_price_eur: Option<Decimal>,
    /// MwSt rate as a fraction (e.g. `0.19`) replacing the default rate on this product's positions.
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,
}

/// §42c EnWG community energy sharing. Category: SHARING.
///
/// Bills the full grid consumption via `ElectricityProvider` (`strom`), then
/// credits the allocated community generation via `EnergyShareProvider`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SharingProduct {
    /// The electricity supply the sharing credit is applied to.
    #[serde(rename = "strom", default)]
    pub electricity: ElectricityProduct,
    /// Community sharing credit rate (ct/kWh) applied to allocated community kWh.
    #[serde(default)]
    pub sharing_credit_ct_per_kwh: Option<Decimal>,
    /// Text of the sharing credit line; defaults to „Energiegemeinschaft Gutschrift (§42c EnWG)".
    #[serde(default)]
    pub sharing_description: Option<String>,
}

// ── Product enum ──────────────────────────────────────────────────────────────

/// Typed product dispatch enum — primary entry point for `energy-billing`.
///
/// Deserializes from `productd` JSONB via the `"category"` field.
///
/// ```rust
/// use energy_billing::Product;
/// let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":"28.5"}"##;
/// let p: Product = serde_json::from_str(json).unwrap();
/// assert!(matches!(p, Product::Strom(_)));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "category")]
pub enum Product {
    /// `STROM` — electricity supply.
    #[serde(rename = "STROM")]
    Strom(ElectricityProduct),
    /// `WAERMEPUMPE` — heat-pump supply with the § 14a positions.
    #[serde(rename = "WAERMEPUMPE")]
    Waermepumpe(ControllableLoadProduct),
    /// `WALLBOX` — wallbox supply with the § 14a positions.
    #[serde(rename = "WALLBOX")]
    Wallbox(ControllableLoadProduct),
    /// `GAS` — natural-gas supply.
    #[serde(rename = "GAS")]
    Gas(GasProduct),
    /// `WAERME` — Fernwärme supply.
    #[serde(rename = "WAERME")]
    Waerme(HeatProduct),
    /// `WASSER` — Trinkwasser and Abwasser.
    #[serde(rename = "WASSER")]
    Wasser(WaterProduct),
    /// `SOLAR` — PV Eigenverbrauch, Mieterstrom or GGV supply.
    #[serde(rename = "SOLAR")]
    Solar(SolarProduct),
    /// `EEG` — EEG feed-in Gutschrift.
    #[serde(rename = "EEG")]
    Eeg(EegProduct),
    /// `EINSPEISUNG` — Direktvermarktung feed-in settlement.
    #[serde(rename = "EINSPEISUNG")]
    Einspeisung(EinspeisungProduct),
    /// `HEMS` — Home Energy Management subscription.
    #[serde(rename = "HEMS")]
    Hems(HemsProduct),
    /// `EMOBILITY` — EV charging.
    #[serde(rename = "EMOBILITY")]
    Emobility(EmobilityProduct),
    /// `ENERGIEDIENSTLEISTUNG` — energy-service subscription.
    #[serde(rename = "ENERGIEDIENSTLEISTUNG")]
    Energiedienstleistung(ServiceProduct),
    /// `SHARING` — electricity supply with a community sharing credit.
    #[serde(rename = "SHARING")]
    Sharing(SharingProduct),
}

impl Product {
    /// Product category string matching the productd JSONB `"category"` field.
    #[must_use]
    pub fn category_str(&self) -> &'static str {
        match self {
            Self::Strom(_) => "STROM",
            Self::Waermepumpe(_) => "WAERMEPUMPE",
            Self::Wallbox(_) => "WALLBOX",
            Self::Gas(_) => "GAS",
            Self::Waerme(_) => "WAERME",
            Self::Wasser(_) => "WASSER",
            Self::Solar(_) => "SOLAR",
            Self::Eeg(_) => "EEG",
            Self::Einspeisung(_) => "EINSPEISUNG",
            Self::Hems(_) => "HEMS",
            Self::Emobility(_) => "EMOBILITY",
            Self::Energiedienstleistung(_) => "ENERGIEDIENSTLEISTUNG",
            Self::Sharing(_) => "SHARING",
        }
    }

    /// Product code from productd (for ERP routing / audit trail).
    #[must_use]
    pub fn product_code(&self) -> Option<&str> {
        match self {
            Self::Strom(p) => p.product_code.as_deref(),
            Self::Waermepumpe(p) | Self::Wallbox(p) => p.base.product_code.as_deref(),
            Self::Gas(p) => p.product_code.as_deref(),
            Self::Waerme(p) => p.product_code.as_deref(),
            Self::Wasser(p) => p.product_code.as_deref(),
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
    /// let json = r##"{"category":"STROM","arbeitspreis_ct_per_kwh":"30.0"}"##;
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
            HemsProvider, MwStProvider, ServiceProvider, SolarProvider, WaterProvider,
        };

        match self {
            Self::Strom(p) => {
                let mwst = rates.effective_mwst_electricity(p);
                if p.dynamic_epex {
                    BillingEngine::new()
                        .add(DynamicElectricityProvider::new(p.clone(), grid.clone()))
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
                // District heating is standard-rated. The 7 % on gas/Fernwärme was
                // the temporary §28 Abs. 5/6 UStG window (01.10.2022–31.03.2024) —
                // `rates.mwst_rate` is already period-aware (billingd sets it via
                // `mwst_rate_for_gas_waerme_period`); there is NO permanent reduced
                // rate for renewable heat (§12 Abs. 2 Nr. 1 UStG covers Anlage-2
                // goods, not district heating).
                let mwst = p.mwst_rate_override.unwrap_or(rates.mwst_rate);
                BillingEngine::new()
                    .add(HeatProvider::new(p.clone()))
                    .add(MwStProvider::new(mwst))
            }
            Self::Wasser(p) => {
                // Trinkwasser is 7 % (§12 Abs. 2 Nr. 1 UStG); the provider sets
                // explicit per-position rates, so the engine default only backs
                // up positions that somehow lack one.
                let mwst = p.mwst_rate_override.unwrap_or(rust_decimal::dec!(0.07));
                BillingEngine::new()
                    .add(WaterProvider::new(p.clone()))
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
                // §19 UStG Kleinunternehmer → 0 % on the feed-in Gutschrift.
                let mwst = if let Some(r) = p.mwst_rate_override {
                    r
                } else if p.kleinunternehmer_19_ustg {
                    rust_decimal::Decimal::ZERO
                } else {
                    rates.mwst_rate
                };
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
        let json = r#"{"category":"STROM","arbeitspreis_ct_per_kwh":"28.5","grundpreis_ct_per_day":"8.0"}"#;
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
    fn product_waermepumpe_nests_its_electricity_supply() {
        let json = r#"{"category":"WAERMEPUMPE","strom":{"arbeitspreis_ct_per_kwh":"20.0"},"sect14a_modul2_nne_reduktion_ct_per_kwh":"1.5"}"#;
        let p: Product = serde_json::from_str(json).unwrap();
        match p {
            Product::Waermepumpe(c) => {
                assert_eq!(
                    c.base.arbeitspreis_ct_per_kwh,
                    Some(rust_decimal::dec!(20.0))
                );
                assert_eq!(
                    c.sect14a_modul2_nne_reduktion_ct_per_kwh,
                    Some(rust_decimal::dec!(1.5))
                );
            }
            _ => panic!("expected Waermepumpe"),
        }
    }

    #[test]
    fn product_gas_roundtrip() {
        let json = r#"{"category":"GAS","gas_arbeitspreis_ct_per_kwh_hs":"7.5","gas_grundpreis_ct_per_day":"5.0"}"#;
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
    fn a_misspelt_key_is_refused_in_every_category() {
        for json in [
            r#"{"category":"STROM","arbeitspreis_ct_per_kwhh":"28.5"}"#,
            r#"{"category":"GAS","gas_arbeitspreis_ct_kwh_hs":"7.5"}"#,
            r#"{"category":"WAERMEPUMPE","strom":{"arbeitspreis":"20"}}"#,
            r#"{"category":"WALLBOX","arbeitspreis_ct_per_kwh":"20"}"#,
            r#"{"category":"SHARING","strom":{},"sharing_credit":"3"}"#,
            r#"{"category":"SOLAR","solar_arbeitspreis":"25"}"#,
            r#"{"category":"STROM","block_tiers":[{"bis_kwh":"1000","preis_ct_per_kwh":"30","rabatt":"1"}]}"#,
        ] {
            assert!(
                serde_json::from_str::<Product>(json).is_err(),
                "{json} must be refused, not billed without the price it misspells"
            );
        }
    }

    #[test]
    fn product_unknown_category_errors() {
        let json = r#"{"category":"UNKNOWN_PRODUCT","foo":1}"#;
        assert!(serde_json::from_str::<Product>(json).is_err());
    }
}
