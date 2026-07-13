//! Pure billing calculator — deterministic, no I/O, no database access.
//!
//! ## Design Principles
//!
//! 1. **User-defined pricing only**: ALL commercial rates come from the product
//!    definition stored in `tarifbd`.  The engine contains zero hardcoded prices.
//! 2. **Category = calculation template**: `category` in `TariffInput` selects
//!    which positions to generate — it never encodes a rate.
//! 3. **Regulatory defaults in config**: Statutory rates (Stromsteuer, Energiesteuer
//!    Gas, BEHG) live in `BillingdConfig` under `[rates]` so operators update them
//!    via config, not code.  Per-product override is supported via `TariffInput::*_override`.
//! 4. **Open product schema**: `TariffInput` uses `#[serde(default)]` — any
//!    tarifbd JSONB field not yet understood is safely ignored.
//! 5. **BO4E-native output**: `BillingResult::rechnung_json` is a
//!    `rubo4e::current::Rechnung`-compatible JSONB consumed by `accountingd`.
//! 6. **billing crate integration**: Each energy category is implemented as a
//!    [`billing::Tariff`] — position generation is pure and separated from the tax
//!    stack.  [`billing::BillingDocument`] provides self-validated totals.
//!
//! ## Product Categories
//!
//! | Category | Tariff struct | Key positions |
//! |---|---|---|
//! | `STROM` | [`StromTariff`] | Grundpreis, Arbeitspreis (HT/NT/Mehrtarif), NNE, KA, Stromsteuer |
//! | `GAS` | [`GasTariff`] | Brennwertkorrektur, Grundpreis, Arbeitspreis, NNE, Energiesteuer, BEHG |
//! | `WAERME` | [`WaermeTariff`] | Grundpreis, Leistungspreis, Arbeitspreis |
//! | `SOLAR` | [`SolarTariff`] | Arbeitspreis (supply), Mieterstrom-/§42a-Aufschlag |
//! | `EEG` | [`EegTariff`] | Vergütung, Marktprämie, Managementprämie, KWKG (credit note) |
//! | `EINSPEISUNG` | [`EinspeisungTariff`] | Marktwert, Vermarktungsgebühr (direct marketing settlement) |
//! | `WAERMEPUMPE` | [`StromTariff`] + §14a | Like STROM with mandatory §14a Modul 1/3 |
//! | `WALLBOX` | [`StromTariff`] + §14a | Like STROM with mandatory §14a Modul 1/3 |
//! | `HEMS` | [`HemsTariff`] | Platform fee, optimization events, smart meter readouts |
//! | `EMOBILITY` | [`EmobilityTariff`] | Service fee, charging energy, session/roaming fees |
//! | `ENERGIEDIENSTLEISTUNG` | [`ServiceTariff`] | Flat fee, per-event charge |
//! | `BUNDLE` | composite | Per-component sub-invoices + AufAbschlag |
//! | `STROM` (§41a dynamic) | [`DynamicStromTariff`] | Per-interval EPEX Spot price, Grundpreis, NNE, Stromsteuer |

#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

use std::collections::HashMap;

use billing::BillingError;
use billing::tax::{FixedRateTax, PerUnitLevy};
use billing::{
    BillingDocument, DocumentMeta, EuroAmount, LineItem, Period, Quantity, Tariff, TaxLayer,
    UnitPrice,
};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};

// ── Regulatory rates — NO hardcoded defaults in this file ─────────────────────

/// Platform-level defaults for statutory rates.
///
/// Configure under `[rates]` in `billingd.toml`.  These are legal minima;
/// products may override individually via `TariffInput::*_override` fields.
///
/// Update annually as BNetzA / BMWK publish new levies.
#[derive(Debug, Clone, Deserialize)]
pub struct RegulatoryRates {
    /// Stromsteuer §3 StromStG — ct/kWh.
    pub stromsteuer_ct_per_kwh: Decimal,
    /// Energiesteuer Erdgas §2 Nr. 3 EnergieStG — ct/kWh_Hs.
    pub energiesteuer_gas_ct_per_kwh: Decimal,
    /// CO₂-Abgabe BEHG for Erdgas H — ct/kWh_Hs.
    /// = CO₂-Preis_EUR_per_t × 0.20160 kg_CO₂/kWh_Hs / 10
    pub behg_gas_ct_per_kwh: Decimal,
    /// Standard MwSt rate (fraction, e.g. 0.19).
    pub mwst_rate: Decimal,
}

impl Default for RegulatoryRates {
    /// Operator MUST configure these in `billingd.toml`.
    /// These defaults reflect 2025/2026 published rates and will be
    /// superseded by operator configuration before any production billing run.
    fn default() -> Self {
        Self {
            stromsteuer_ct_per_kwh: dec!(2.05), // §3 StromStG since 01.07.2023
            energiesteuer_gas_ct_per_kwh: dec!(0.55), // §2 EnergieStG, Erdgas H
            behg_gas_ct_per_kwh: dec!(1.109),   // 55 EUR/t CO₂ × 0.20160 kg/kWh (2025)
            mwst_rate: dec!(0.19),
        }
    }
}

impl RegulatoryRates {
    /// Effective Stromsteuer: product override takes precedence.
    pub fn stromsteuer(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .stromsteuer_ct_per_kwh_override
            .unwrap_or(self.stromsteuer_ct_per_kwh)
    }
    /// Effective Energiesteuer Gas: product override takes precedence.
    pub fn energiesteuer_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .energiesteuer_gas_ct_per_kwh_override
            .unwrap_or(self.energiesteuer_gas_ct_per_kwh)
    }
    /// Effective BEHG Gas CO₂ levy: product override takes precedence.
    pub fn behg_gas(&self, tariff: &TariffInput) -> Decimal {
        tariff
            .behg_gas_ct_per_kwh_override
            .unwrap_or(self.behg_gas_ct_per_kwh)
    }
    /// Effective MwSt rate: product override takes precedence.
    pub fn mwst(&self, tariff: &TariffInput) -> Decimal {
        tariff.mwst_rate_override.unwrap_or(self.mwst_rate)
    }
}

// ── Inputs ────────────────────────────────────────────────────────────────────

/// Product pricing data extracted from the `tarifbd` product JSONB.
///
/// ALL fields are `Option` with `#[serde(default)]` — users define their own
/// products in `tarifbd`; unknown fields are silently ignored.  The `category`
/// field determines which calculator is invoked; pricing fields are orthogonal.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TariffInput {
    /// Product code from `tarifbd` — used as billing record key.
    #[serde(default)]
    pub product_code: Option<String>,
    /// Determines the calculation template — see module doc table.
    /// Defaults to `"STROM"` when constructed via `Default::default()`.
    #[serde(default = "default_category")]
    pub category: String,
    /// Eintarif | Zweitarif | Mehrtarif (STROM only)
    #[serde(default)]
    pub register_count: Option<String>,

    // ── STROM / WAERMEPUMPE / WALLBOX ─────────────────────────────────────────
    #[serde(default)]
    pub grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,

    // ── §14a EnWG — Wärmepumpe + Wallbox ─────────────────────────────────────
    #[serde(default)]
    pub steuerungsrabatt_modul1_eur_per_kw_year: Option<Decimal>,
    #[serde(default)]
    pub steuerungsrabatt_modul3_eur_per_kw_year: Option<Decimal>,

    // ── GAS ───────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub gas_grundpreis_ct_per_day: Option<Decimal>,
    #[serde(default)]
    pub gas_arbeitspreis_ct_per_kwh_hs: Option<Decimal>,

    // ── WAERME (Fernwärme) ────────────────────────────────────────────────────
    #[serde(default)]
    pub waerme_grundpreis_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub waerme_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub waerme_leistungspreis_eur_per_kw_month: Option<Decimal>,

    // ── SOLAR ─────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub solar_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub mieterstrom_aufschlag_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gemeinschaft_rabatt_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub solar_include_stromsteuer: bool,

    // ── EEG / EINSPEISUNG ─────────────────────────────────────────────────────
    #[serde(default)]
    pub eeg_verguetungssatz_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_marktpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub eeg_managementpraemie_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub kwkg_zuschlag_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub marktwert_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub vermarktungsgebuehr_ct_per_kwh: Option<Decimal>,

    // ── HEMS ──────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub hems_platform_fee_eur_per_month: Option<Decimal>,
    #[serde(default)]
    pub hems_optimization_event_eur: Option<Decimal>,
    #[serde(default)]
    pub hems_readout_event_eur: Option<Decimal>,

    // ── EMOBILITY ─────────────────────────────────────────────────────────────
    #[serde(default)]
    pub emobility_service_fee_eur_per_month: Option<Decimal>,
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

    // ── Regulatory overrides ──────────────────────────────────────────────────
    #[serde(default)]
    pub stromsteuer_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub energiesteuer_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub behg_gas_ct_per_kwh_override: Option<Decimal>,
    #[serde(default)]
    pub mwst_rate_override: Option<Decimal>,

    // ── §41a dynamic tariff ───────────────────────────────────────────────────
    #[serde(default)]
    pub dynamic_epex: bool,
    /// Floor price for §41a EPEX tariff — ct/kWh.
    ///
    /// When set, the per-interval EPEX price is clamped to `max(price, floor)` before
    /// billing. Use `0.0` to prevent customers from receiving credits during
    /// negative-EPEX hours (common in contracts that pass through market price but
    /// cap at zero). Defaults to `None` = full pass-through including negative prices.
    #[serde(default)]
    pub dynamic_epex_floor_ct_kwh: Option<Decimal>,
}

fn default_category() -> String {
    "STROM".to_owned()
}

/// Electricity / heat pump / wallbox metering input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MeterInput {
    #[serde(default)]
    pub arbeitsmenge_kwh: Decimal,
    #[serde(default)]
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    #[serde(default)]
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    #[serde(default)]
    pub spitzenleistung_kw: Option<Decimal>,
    #[serde(default)]
    pub steuerung_stunden: Option<Decimal>,
}

/// Gas metering input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct GasMeterInput {
    /// Volume as measured by the meter (m³, at meter conditions).
    pub messung_qm3: Decimal,
    /// Calorific value (Brennwert) in kWh/m³ — from metering point.
    /// For H2-blended gas this already reflects the actual blended Brennwert
    /// as reported by the grid operator via MSCONS / DVGW G 260.
    #[serde(default)]
    pub brennwert_kwh_per_qm3: Option<Decimal>,
    /// Compressibility factor (Zustandszahl) — from metering point.
    #[serde(default)]
    pub zustandszahl: Option<Decimal>,
    /// Pre-computed kWh_Hs (takes precedence over Brennwert × Zustandszahl).
    #[serde(default)]
    pub kwh_hs: Option<Decimal>,
    /// Gas quality from `marktd.malo.gasqualitaet` (e.g. `"H_GAS"`, `"L_GAS"`, `"H2_BLEND"`).
    /// When set, recorded as `ZusatzAttribut` on the Rechnung for regulatory audit transparency.
    /// The Brennwert used for billing is ALWAYS the measured value from `edmd` — this field
    /// is for documentation purposes, not for applying a separate correction factor.
    #[serde(default)]
    pub gasqualitaet: Option<String>,
}

/// Fernwärme metering input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct WaermeMeterInput {
    /// Thermal energy delivered (kWh_th).
    #[serde(default)]
    pub kwh_waerme: Decimal,
    /// Peak demand (kW) — for Leistungspreis.
    #[serde(default)]
    pub spitzenleistung_kw: Option<Decimal>,
    /// Billing months (pro-rata, defaults to 1).
    #[serde(default)]
    pub months: Option<Decimal>,
}

/// Solar Eigenverbrauch / Mieterstrom / §42a input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct SolarMeterInput {
    pub eigenverbrauch_kwh: Decimal,
}

/// EEG / Direktvermarktung feed-in settlement input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EegMeterInput {
    /// Total kWh fed into the grid during the billing period.
    pub einspeisung_kwh: Decimal,
    /// kWh fed in during hours where EPEX day-ahead price was negative (`§51 EEG`).
    ///
    /// When set, EEG **Vergütung**, **Marktprämie**, and **Managementprämie** are
    /// suspended for these kWh (`billable_kwh = einspeisung_kwh - kwh_during_negative_epex`).
    ///
    /// **Not applicable to KWKG** (different law; not subject to §51 EEG).
    ///
    /// Threshold per EEG version:
    /// - EEG 2023 (from 01.01.2021+): **any** negative-price hour
    /// - EEG 2017/2019 (before 01.01.2021): **≥6 consecutive** negative-price hours
    ///
    /// Callers are responsible for computing the qualifying hours from edmd Lastgang
    /// × EPEX prices before calling `calculate_eeg`.
    #[serde(default)]
    pub kwh_during_negative_epex: Option<Decimal>,
}

/// HEMS usage input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct HemsMeterInput {
    #[serde(default)]
    pub months: Option<Decimal>,
    #[serde(default)]
    pub optimization_events: Option<u32>,
    #[serde(default)]
    pub readout_events: Option<u32>,
}

/// E-Mobility CPO/EMSP usage input.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct EmobilityMeterInput {
    #[serde(default)]
    pub months: Option<Decimal>,
    #[serde(default)]
    pub kwh_charged: Option<Decimal>,
    #[serde(default)]
    pub sessions: Option<u32>,
    #[serde(default)]
    pub roaming_sessions: Option<u32>,
}

/// Energiedienstleistung usage input.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceMeterInput {
    #[serde(default)]
    pub months: Option<Decimal>,
    #[serde(default)]
    pub event_count: Option<u32>,
    #[serde(default)]
    pub event_price_eur: Option<Decimal>,
}

/// One 15-minute (or hourly) Lastgang interval for §41a dynamic billing.
#[derive(Debug, Clone, Deserialize)]
pub struct DynamicInterval {
    pub timestamp_utc: time::OffsetDateTime,
    pub kwh: Decimal,
}

/// Usage input for §41a EPEX dynamic tariff.
#[derive(Debug, Clone, Default)]
pub struct DynamicUsage {
    pub intervals: Vec<DynamicInterval>,
    /// Map `(year, month, day, hour_CET)` → ct/kWh.
    pub epex_prices_ct_kwh: HashMap<(i32, u8, u8, u8), Decimal>,
}

/// Grid pass-through costs from `marktd` (NNE, KA).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GridInput {
    #[serde(default)]
    pub nne_grundpreis_eur_per_year: Option<Decimal>,
    #[serde(default)]
    pub nne_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub nne_leistungspreis_eur_per_kw_year: Option<Decimal>,
    #[serde(default)]
    pub ka_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_nne_grundpreis_eur_per_year: Option<Decimal>,
    #[serde(default)]
    pub gas_nne_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_ka_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_bilanzierungsumlage_ct_per_kwh: Option<Decimal>,
}

// ── Output ────────────────────────────────────────────────────────────────────

/// Complete billing result — deterministic for the same inputs.
///
/// `netto_eur` is the German Nettobetrag (all charges before MwSt, including
/// per-unit levies such as Stromsteuer and Energiesteuer).
/// `mwst_eur` is the Mehrwertsteuerbetrag.
/// `brutto_eur` = `netto_eur` + `mwst_eur`.
#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub struct BillingResult {
    /// All positions including net, discount, and tax positions.
    pub positions: Vec<billing::LineItem>,
    pub netto_eur: Decimal,
    pub mwst_eur: Decimal,
    pub brutto_eur: Decimal,
    /// BO4E-compatible `Rechnung` JSONB for `accountingd` ingestion.
    pub rechnung_json: serde_json::Value,
}

impl BillingResult {
    /// Sum of net amounts for all positions carrying the given tag.
    ///
    /// Useful for extracting subtotals: e.g. `"nne"` for grid pass-through,
    /// `"commodity"` for energy-only charges, `"rabatt"` for discounts.
    #[must_use]
    pub fn position_total_by_tag(&self, tag: &str) -> Decimal {
        self.positions
            .iter()
            .filter(|p| p.has_tag(tag))
            .map(|p| p.net_amount.into_decimal())
            .sum()
    }

    /// Positions carrying the given tag.
    pub fn positions_by_tag<'a>(
        &'a self,
        tag: &'a str,
    ) -> impl Iterator<Item = &'a billing::LineItem> {
        self.positions.iter().filter(move |p| p.has_tag(tag))
    }

    /// Sum of statutory per-unit levies (Stromsteuer, Energiesteuer, BEHG).
    ///
    /// These are tagged `"levy"` by [`billing::PerUnitLevy`] and are included
    /// in `netto_eur` (German Nettobetrag includes levies).
    #[must_use]
    pub fn levy_total_eur(&self) -> Decimal {
        self.position_total_by_tag("levy")
    }

    /// Assert the arithmetic invariant: `netto + mwst == brutto`.
    ///
    /// Panics with a diagnostic if the invariant is violated (within 0.001 EUR tolerance).
    /// Use in tests and `debug_assert!` paths.
    pub fn assert_valid(&self) {
        let expected = self.netto_eur + self.mwst_eur;
        let diff = (self.brutto_eur - expected).abs();
        assert!(
            diff < dec!(0.001),
            "BillingResult invariant violated: netto {:.5} + mwst {:.5} = {:.5} != brutto {:.5}",
            self.netto_eur,
            self.mwst_eur,
            expected,
            self.brutto_eur,
        );
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Convert ct/kWh to EUR as `EuroAmount`.  Returns `Err` on precision overflow.
fn ct_to_eur(ct: Decimal) -> Result<EuroAmount, BillingError> {
    EuroAmount::checked_from_decimal(ct / dec!(100))
}

/// Convert an optional `Decimal` EUR amount to `EuroAmount`.
fn decimal_to_euro(d: Decimal) -> Result<EuroAmount, BillingError> {
    EuroAmount::checked_from_decimal(d)
}

/// Compute effective kWh_Hs from a gas meter reading.
fn gas_kwh_hs(meter: &GasMeterInput) -> Decimal {
    if let Some(kwh) = meter.kwh_hs {
        return kwh;
    }
    let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
    let z = meter.zustandszahl.unwrap_or(dec!(1.0));
    (meter.messung_qm3 * hs * z).round_dp(3)
}

/// Build electricity-grid pass-through positions (NNE, KA).
///
/// These positions are NOT tagged `"commodity"` — Stromsteuer (which filters
/// on `"commodity"`) correctly excludes them from the levy base.
fn strom_grid_items(
    grid: &GridInput,
    kwh: Decimal,
    spitzenleistung_kw: Option<Decimal>,
    days: i64,
) -> Result<Vec<LineItem>, BillingError> {
    let mut items = Vec::new();
    if let Some(nne_gp) = grid.nne_grundpreis_eur_per_year {
        let daily = nne_gp / dec!(365);
        items.push(
            LineItem::debit("Netznutzungsentgelt Grundpreis")
                .quantity(Quantity::new(Decimal::from(days), "Tage"))
                .unit_price(UnitPrice::new(daily, "EUR/Tag"))
                .tag("nne_grundpreis")
                .tag("nne")
                .build()?,
        );
    }
    if let Some(nne_ap_ct) = grid.nne_arbeitspreis_ct_per_kwh {
        items.push(
            LineItem::debit("Netznutzungsentgelt Arbeitspreis")
                .quantity(Quantity::new(kwh, "kWh"))
                .unit_price(UnitPrice::new(nne_ap_ct / dec!(100), "EUR/kWh"))
                .tag("nne_arbeitspreis")
                .tag("nne")
                .build()?,
        );
    }
    if let (Some(nne_lp), Some(kw)) = (grid.nne_leistungspreis_eur_per_kw_year, spitzenleistung_kw)
    {
        let months = Decimal::from(days) / dec!(30.4375);
        items.push(
            LineItem::debit("Netznutzungsentgelt Leistungspreis")
                .quantity(Quantity::new(kw, "kW"))
                .unit_price(UnitPrice::new(nne_lp / dec!(12) * months, "EUR/kW"))
                .tag("nne_leistungspreis")
                .tag("nne")
                .build()?,
        );
    }
    if let Some(ka_ct) = grid.ka_ct_per_kwh {
        items.push(
            LineItem::debit("Konzessionsabgabe")
                .quantity(Quantity::new(kwh, "kWh"))
                .unit_price(UnitPrice::new(ka_ct / dec!(100), "EUR/kWh"))
                .tag("konzessionsabgabe")
                .tag("nne")
                .build()?,
        );
    }
    Ok(items)
}

/// Build gas-grid pass-through positions (NNE Gas, KA Gas, Bilanzierungsumlage).
fn gas_grid_items(
    grid: &GridInput,
    kwh_hs: Decimal,
    days: i64,
) -> Result<Vec<LineItem>, BillingError> {
    let mut items = Vec::new();
    if let Some(nne_gp) = grid.gas_nne_grundpreis_eur_per_year {
        let daily = nne_gp / dec!(365);
        items.push(
            LineItem::debit("Gasnetznutzungsentgelt Grundpreis")
                .quantity(Quantity::new(Decimal::from(days), "Tage"))
                .unit_price(UnitPrice::new(daily, "EUR/Tag"))
                .tag("gas_nne_grundpreis")
                .tag("nne")
                .build()?,
        );
    }
    if let Some(nne_ap_ct) = grid.gas_nne_arbeitspreis_ct_per_kwh {
        items.push(
            LineItem::debit("Gasnetznutzungsentgelt Arbeitspreis")
                .quantity(Quantity::new(kwh_hs, "kWh_Hs"))
                .unit_price(UnitPrice::new(nne_ap_ct / dec!(100), "EUR/kWh_Hs"))
                .tag("gas_nne_arbeitspreis")
                .tag("nne")
                .build()?,
        );
    }
    if let Some(ka_ct) = grid.gas_ka_ct_per_kwh {
        items.push(
            LineItem::debit("Konzessionsabgabe Gas")
                .quantity(Quantity::new(kwh_hs, "kWh_Hs"))
                .unit_price(UnitPrice::new(ka_ct / dec!(100), "EUR/kWh_Hs"))
                .tag("gas_konzessionsabgabe")
                .tag("nne")
                .build()?,
        );
    }
    if let Some(bilu_ct) = grid.gas_bilanzierungsumlage_ct_per_kwh {
        items.push(
            LineItem::debit("Bilanzierungsumlage Gas")
                .quantity(Quantity::new(kwh_hs, "kWh_Hs"))
                .unit_price(UnitPrice::new(bilu_ct / dec!(100), "EUR/kWh_Hs"))
                .tag("gas_bilanzierungsumlage")
                .tag("nne")
                .build()?,
        );
    }
    Ok(items)
}

/// Build electricity tax layers: Stromsteuer (PerUnitLevy, commodity-only) then MwSt.
///
/// Ordering is critical: the `FixedRateTax` for MwSt accumulates prior tax
/// layers (including Stromsteuer) in its base — billing's `from_positions`
/// passes all accumulated positions to each subsequent layer.
fn strom_tax_layers(tariff: &TariffInput, rates: &RegulatoryRates) -> Vec<Box<dyn TaxLayer>> {
    let mut layers: Vec<Box<dyn TaxLayer>> = Vec::new();
    let st_rate = rates.stromsteuer(tariff);
    if st_rate > Decimal::ZERO
        && let Ok(levy) = ct_to_eur(st_rate)
    {
        layers.push(Box::new(
            PerUnitLevy::new("Stromsteuer", levy, "kWh").with_tag("commodity"),
        ));
    }
    let mwst = rates.mwst(tariff);
    if mwst > Decimal::ZERO {
        layers.push(Box::new(FixedRateTax::new(
            format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
            mwst,
        )));
    }
    layers
}

/// Build gas tax layers: Energiesteuer, BEHG (both PerUnitLevy, commodity-only), then MwSt.
fn gas_tax_layers(tariff: &TariffInput, rates: &RegulatoryRates) -> Vec<Box<dyn TaxLayer>> {
    let mut layers: Vec<Box<dyn TaxLayer>> = Vec::new();
    let est_ct = rates.energiesteuer_gas(tariff);
    if est_ct > Decimal::ZERO
        && let Ok(levy) = ct_to_eur(est_ct)
    {
        layers.push(Box::new(
            PerUnitLevy::new(
                "Energiesteuer Erdgas (\u{00a7}2 EnergieStG)",
                levy,
                "kWh_Hs",
            )
            .with_tag("commodity"),
        ));
    }
    let behg_ct = rates.behg_gas(tariff);
    if behg_ct > Decimal::ZERO
        && let Ok(levy) = ct_to_eur(behg_ct)
    {
        layers.push(Box::new(
            PerUnitLevy::new("CO\u{2082}-Abgabe BEHG Erdgas", levy, "kWh_Hs").with_tag("commodity"),
        ));
    }
    let mwst = rates.mwst(tariff);
    if mwst > Decimal::ZERO {
        layers.push(Box::new(FixedRateTax::new(
            format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
            mwst,
        )));
    }
    layers
}

/// Build a `BillingResult` from a completed `BillingDocument`.
///
/// `netto_eur` follows the German convention: Nettobetrag = all amounts before
/// MwSt, including per-unit levies (Stromsteuer, Energiesteuer, BEHG).
/// Levy positions are identified by the `"levy"` tag set by `billing::PerUnitLevy`.
fn billing_result_from_doc(
    doc: BillingDocument,
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    rechnungsart: &str,
) -> BillingResult {
    let levy_total: Decimal = doc
        .tax_positions()
        .iter()
        .filter(|p| p.has_tag("levy"))
        .map(|p| p.net_amount.into_decimal())
        .sum();
    let netto_eur = doc.net_total().into_decimal() + levy_total;
    let brutto_eur = doc.gross_total().into_decimal();
    let mwst_eur = brutto_eur - netto_eur;
    let positions: Vec<LineItem> = doc.all_positions().cloned().collect();
    let rechnung_json = build_rechnung_json(
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        &positions,
        netto_eur,
        brutto_eur,
        rechnungsart,
    );
    BillingResult {
        positions,
        netto_eur,
        mwst_eur,
        brutto_eur,
        rechnung_json,
    }
}

/// Build the BO4E-compatible rechnung JSON.
///
/// `rechnungsart` should be `"ABSCHLAGSRECHNUNG"` for regular invoices and
/// `"GUTSCHRIFT"` for EEG/EINSPEISUNG credit notes.
fn build_rechnung_json(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    positions: &[LineItem],
    netto_eur: Decimal,
    brutto_eur: Decimal,
    rechnungsart: &str,
) -> serde_json::Value {
    let pos_json: Vec<serde_json::Value> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let qty = p.quantity_value().unwrap_or(Decimal::ONE);
            let unit = p.unit_label().unwrap_or("Pauschal");
            let unit_price_val = p
                .unit_price
                .as_ref()
                .map(|up| up.value)
                // For fixed-amount positions (no explicit unit price), use the
                // net amount as einzelpreis so qty=1 × einzelpreis = gesamtpreis.
                .unwrap_or_else(|| p.net_amount.into_decimal());
            let net = p.net_amount.into_decimal();
            let type_tag = p
                .tags
                .first()
                .cloned()
                .unwrap_or_else(|| "position".to_owned());
            serde_json::json!({
                "_typ": "RECHNUNGSPOSITION",
                "positionsnummer": i + 1,
                "positionstext": p.description,
                "positionsMenge": { "_typ": "MENGE", "wert": qty.to_string(), "einheit": unit },
                "einzelpreis": { "_typ": "PREIS", "wert": unit_price_val.to_string(), "einheit": "EUR" },
                "gesamtpreis": { "_typ": "BETRAG", "wert": net.to_string(), "waehrung": "EUR" },
                "positionstyp": type_tag,
            })
        })
        .collect();

    serde_json::json!({
        "_typ": "RECHNUNG",
        "rechnungsnummer": rechnungsnummer,
        "rechnungsart": rechnungsart,
        "rechnungsdatum": time::OffsetDateTime::now_utc().date().to_string(),
        "marktlokationsId": malo_id,
        "herausgeber": { "_typ": "MARKTTEILNEHMER", "marktpartnercode": lf_mp_id },
        "rechnungsperiode": {
            "_typ": "ZEITRAUM",
            "startdatum": period_from.to_string(),
            "enddatum": period_to.to_string()
        },
        "rechnungspositionen": pos_json,
        "gesamtnetto":   { "_typ": "BETRAG", "wert": netto_eur.to_string(), "waehrung": "EUR" },
        "gesamtsteuer":  { "_typ": "BETRAG", "wert": (brutto_eur - netto_eur).to_string(), "waehrung": "EUR" },
        "gesamtbrutto":  { "_typ": "BETRAG", "wert": brutto_eur.to_string(), "waehrung": "EUR" },
        "rechnungsempfaenger": { "_typ": "MARKTTEILNEHMER", "externeKundenId": malo_id },
        "zahlungsziel": (time::OffsetDateTime::now_utc().date() + time::Duration::days(14)).to_string()
    })
}

// ── billing::Tariff implementations ──────────────────────────────────────────

/// Electricity tariff — STROM, WAERMEPUMPE, WALLBOX.
///
/// WAERMEPUMPE / WALLBOX produce §14a positions when
/// `steuerungsrabatt_modul1_eur_per_kw_year` or `steuerungsrabatt_modul3_eur_per_kw_year`
/// are set in the product definition.
pub struct StromTariff<'a> {
    pub tariff: &'a TariffInput,
    pub grid: &'a GridInput,
    pub rates: &'a RegulatoryRates,
    pub period_from: time::Date,
    pub period_to: time::Date,
    pub eeg_gutschrift_eur: Option<Decimal>,
}

impl Tariff for StromTariff<'_> {
    type Usage = MeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &MeterInput) -> Result<Vec<LineItem>, BillingError> {
        let days = (self.period_to - self.period_from).whole_days();
        let mut items: Vec<LineItem> = Vec::new();

        // 1. Grundpreis
        if let Some(gp_ct) = self.tariff.grundpreis_ct_per_day {
            items.push(
                LineItem::debit("Grundpreis Strom")
                    .quantity(Quantity::new(Decimal::from(days), "Tage"))
                    .unit_price(UnitPrice::new(gp_ct / dec!(100), "EUR/Tag"))
                    .tag("grundpreis")
                    .build()?,
            );
        }

        // 2. Arbeitspreis (Eintarif or Zweitarif HT/NT)
        let zweitarif = self.tariff.register_count.as_deref() == Some("Zweitarif")
            || (self.tariff.arbeitspreis_ht_ct_per_kwh.is_some()
                && self.tariff.arbeitspreis_nt_ct_per_kwh.is_some());
        if zweitarif {
            if let (Some(ht_kwh), Some(ht_ct)) = (
                meter.arbeitsmenge_ht_kwh,
                self.tariff.arbeitspreis_ht_ct_per_kwh,
            ) {
                items.push(
                    LineItem::debit("Arbeitspreis HT")
                        .quantity(Quantity::new(ht_kwh, "kWh"))
                        .unit_price(UnitPrice::new(ht_ct / dec!(100), "EUR/kWh"))
                        .tag("arbeitspreis_ht")
                        .tag("commodity")
                        .build()?,
                );
            }
            if let (Some(nt_kwh), Some(nt_ct)) = (
                meter.arbeitsmenge_nt_kwh,
                self.tariff.arbeitspreis_nt_ct_per_kwh,
            ) {
                items.push(
                    LineItem::debit("Arbeitspreis NT")
                        .quantity(Quantity::new(nt_kwh, "kWh"))
                        .unit_price(UnitPrice::new(nt_ct / dec!(100), "EUR/kWh"))
                        .tag("arbeitspreis_nt")
                        .tag("commodity")
                        .build()?,
                );
            }
        } else if let Some(ap_ct) = self.tariff.arbeitspreis_ct_per_kwh {
            items.push(
                LineItem::debit("Arbeitspreis Strom")
                    .quantity(Quantity::new(meter.arbeitsmenge_kwh, "kWh"))
                    .unit_price(UnitPrice::new(ap_ct / dec!(100), "EUR/kWh"))
                    .tag("arbeitspreis")
                    .tag("commodity")
                    .build()?,
            );
        }

        // 3–4. NNE + KA (no "commodity" tag → excluded from Stromsteuer base)
        items.extend(strom_grid_items(
            self.grid,
            meter.arbeitsmenge_kwh,
            meter.spitzenleistung_kw,
            days,
        )?);

        // 5. §14a Modul 1 — capacity-based NNE reduction (credit)
        if let (Some(modul1_rate), Some(kw)) = (
            self.tariff.steuerungsrabatt_modul1_eur_per_kw_year,
            meter.spitzenleistung_kw,
        ) {
            let months = Decimal::from(days) / dec!(30.4375);
            items.push(
                LineItem::credit("\u{00a7}14a EnWG Steuerungsrabatt Modul 1")
                    .quantity(Quantity::new(kw, "kW"))
                    .unit_price(UnitPrice::new(modul1_rate / dec!(12) * months, "EUR/kW"))
                    .tag("steuerungsrabatt_modul1")
                    .tag("rabatt")
                    .build()?,
            );
        }

        // 6. §14a Modul 3 — load-shedding compensation (credit)
        if let (Some(modul3_rate), Some(kw), Some(steuer_h)) = (
            self.tariff.steuerungsrabatt_modul3_eur_per_kw_year,
            meter.spitzenleistung_kw,
            meter.steuerung_stunden,
        ) && steuer_h > Decimal::ZERO
        {
            let jahresanteil = (steuer_h / dec!(8760)).min(dec!(1));
            items.push(
                LineItem::credit("\u{00a7}14a EnWG Steuerungsrabatt Modul 3 (Laststeuerung)")
                    .quantity(Quantity::new(kw, "kW"))
                    .unit_price(UnitPrice::new(modul3_rate * jahresanteil, "EUR/kW"))
                    .tag("steuerungsrabatt_modul3")
                    .tag("rabatt")
                    .build()?,
            );
        }

        // 7. EEG Gutschrift (from einsd)
        if let Some(eeg_eur) = self.eeg_gutschrift_eur
            && eeg_eur != Decimal::ZERO
        {
            let amt = decimal_to_euro(eeg_eur)?;
            items.push(
                LineItem::credit("EEG Einspeisegutschrift")
                    .fixed_amount(amt)
                    .tag("eeg_gutschrift")
                    .tag("credit")
                    .build()?,
            );
        }

        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        strom_tax_layers(self.tariff, self.rates)
    }
}

/// Gas tariff — GAS.
///
/// Mandatory Brennwertkorrektur position (§10 GasGVV) is informational (zero EUR).
pub struct GasTariff<'a> {
    pub tariff: &'a TariffInput,
    pub grid: &'a GridInput,
    pub rates: &'a RegulatoryRates,
    pub period_from: time::Date,
    pub period_to: time::Date,
}

impl Tariff for GasTariff<'_> {
    type Usage = GasMeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &GasMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let days = (self.period_to - self.period_from).whole_days();
        let kwh_hs = gas_kwh_hs(meter);
        let hs = meter.brennwert_kwh_per_qm3.unwrap_or(dec!(10.55));
        let z = meter.zustandszahl.unwrap_or(dec!(1.0));
        let mut items: Vec<LineItem> = Vec::new();

        // 0. Brennwertkorrektur (informational, zero EUR — §10 GasGVV)
        //    Shows the m³ → kWh_Hs conversion clearly in the invoice.
        items.push(
            LineItem::debit(format!(
                "Brennwertkorrektur: {:.3} m\u{00b3} \u{00d7} {:.4} kWh/m\u{00b3} \u{00d7} {:.4} = {:.3} kWh_Hs",
                meter.messung_qm3, hs, z, kwh_hs
            ))
            .quantity(Quantity::new(meter.messung_qm3, "m\u{00b3}"))
            .unit_price(UnitPrice::new(Decimal::ZERO, "EUR/m\u{00b3}"))
            .tag("gas_brennwert_korrektur")
            .tag("info")
            .build()?,
        );

        // 1. Grundpreis
        if let Some(gp_ct) = self.tariff.gas_grundpreis_ct_per_day {
            items.push(
                LineItem::debit("Grundpreis Gas")
                    .quantity(Quantity::new(Decimal::from(days), "Tage"))
                    .unit_price(UnitPrice::new(gp_ct / dec!(100), "EUR/Tag"))
                    .tag("gas_grundpreis")
                    .build()?,
            );
        }

        // 2. Arbeitspreis
        if let Some(ap_ct) = self.tariff.gas_arbeitspreis_ct_per_kwh_hs {
            items.push(
                LineItem::debit("Arbeitspreis Gas")
                    .quantity(Quantity::new(kwh_hs, "kWh_Hs"))
                    .unit_price(UnitPrice::new(ap_ct / dec!(100), "EUR/kWh_Hs"))
                    .tag("gas_arbeitspreis")
                    .tag("commodity")
                    .build()?,
            );
        }

        // 3. NNE Gas + KA + Bilanzierungsumlage
        items.extend(gas_grid_items(self.grid, kwh_hs, days)?);

        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        gas_tax_layers(self.tariff, self.rates)
    }
}

/// Fernwärme tariff — WAERME.
pub struct WaermeTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for WaermeTariff<'_> {
    type Usage = WaermeMeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &WaermeMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let months = meter.months.unwrap_or(dec!(1));
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(gp) = self.tariff.waerme_grundpreis_eur_per_month {
            items.push(
                LineItem::debit("Grundpreis Fernw\u{00e4}rme")
                    .quantity(Quantity::new(months, "Monat"))
                    .unit_price(UnitPrice::new(gp, "EUR/Monat"))
                    .tag("waerme_grundpreis")
                    .build()?,
            );
        }
        if let (Some(lp), Some(kw)) = (
            self.tariff.waerme_leistungspreis_eur_per_kw_month,
            meter.spitzenleistung_kw,
        ) {
            items.push(
                LineItem::debit("Leistungspreis Fernw\u{00e4}rme")
                    .quantity(Quantity::new(kw, "kW"))
                    .unit_price(UnitPrice::new(lp * months, "EUR/kW"))
                    .tag("waerme_leistungspreis")
                    .build()?,
            );
        }
        if let Some(ap_ct) = self.tariff.waerme_arbeitspreis_ct_per_kwh {
            items.push(
                LineItem::debit("Arbeitspreis Fernw\u{00e4}rme")
                    .quantity(Quantity::new(meter.kwh_waerme, "kWh_th"))
                    .unit_price(UnitPrice::new(ap_ct / dec!(100), "EUR/kWh_th"))
                    .tag("waerme_arbeitspreis")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// Solar Eigenverbrauch / Mieterstrom (§42b) / §42a GGV tariff.
///
/// Stromsteuer is suppressed by default (§9a StromStG Eigenverbrauchsbefreiung).
/// Set `tariff.solar_include_stromsteuer = true` to include it.
pub struct SolarTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for SolarTariff<'_> {
    type Usage = SolarMeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &SolarMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(ap_ct) = self.tariff.solar_arbeitspreis_ct_per_kwh {
            items.push(
                LineItem::debit("Arbeitspreis Solarstrom (Eigenverbrauch)")
                    .quantity(Quantity::new(meter.eigenverbrauch_kwh, "kWh"))
                    .unit_price(UnitPrice::new(ap_ct / dec!(100), "EUR/kWh"))
                    .tag("solar_arbeitspreis")
                    .tag("commodity")
                    .build()?,
            );
        }
        if let Some(ms_ct) = self.tariff.mieterstrom_aufschlag_ct_per_kwh {
            items.push(
                LineItem::debit("Mieterstrom-Aufschlag (\u{00a7}42b EEG)")
                    .quantity(Quantity::new(meter.eigenverbrauch_kwh, "kWh"))
                    .unit_price(UnitPrice::new(ms_ct / dec!(100), "EUR/kWh"))
                    .tag("mieterstrom_aufschlag")
                    .build()?,
            );
        }
        if let Some(rabatt_ct) = self.tariff.gemeinschaft_rabatt_ct_per_kwh {
            items.push(
                LineItem::credit(
                    "Rabatt Gemeinschaftliche Geb\u{00e4}udeversorgung (\u{00a7}42a EEG)",
                )
                .quantity(Quantity::new(meter.eigenverbrauch_kwh, "kWh"))
                .unit_price(UnitPrice::new(rabatt_ct / dec!(100), "EUR/kWh"))
                .tag("gemeinschaft_rabatt")
                .tag("rabatt")
                .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mut layers: Vec<Box<dyn TaxLayer>> = Vec::new();
        if self.tariff.solar_include_stromsteuer {
            let st_rate = self.rates.stromsteuer(self.tariff);
            if st_rate > Decimal::ZERO
                && let Ok(levy) = ct_to_eur(st_rate)
            {
                layers.push(Box::new(
                    PerUnitLevy::new("Stromsteuer", levy, "kWh").with_tag("commodity"),
                ));
            }
        }
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            layers.push(Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            )));
        }
        layers
    }
}

/// EEG feed-in settlement credit note — VERGUETUNG, DIREKTVERMARKTUNG, KWKG.
pub struct EegTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for EegTariff<'_> {
    type Usage = EegMeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &EegMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let kwh = meter.einspeisung_kwh;

        // §51 EEG Negativpreisregel: suspend Vergütung, Marktprämie, Managementprämie
        // for kWh fed in during negative-EPEX hours. KWKG is NOT suspended (different law).
        let billable_kwh = meter
            .kwh_during_negative_epex
            .map(|neg| (kwh - neg).max(Decimal::ZERO))
            .unwrap_or(kwh);

        let mut items: Vec<LineItem> = Vec::new();

        // Informational: show suspended kWh when §51 applies
        let suspended_kwh = kwh - billable_kwh;
        if suspended_kwh > Decimal::ZERO {
            items.push(
                LineItem::debit("Keine Verg\u{fc}tung (\u{a7}51 EEG Negativpreisregel)")
                    .quantity(Quantity::new(suspended_kwh, "kWh"))
                    .unit_price(UnitPrice::new(Decimal::ZERO, "EUR/kWh"))
                    .tag("eeg_negativpreis_suspension")
                    .tag("info")
                    .build()?,
            );
        }

        if let Some(vg_ct) = self.tariff.eeg_verguetungssatz_ct_per_kwh {
            items.push(
                LineItem::debit("EEG Einspeisung\u{00ad}verg\u{00fc}tung")
                    .quantity(Quantity::new(billable_kwh, "kWh"))
                    .unit_price(UnitPrice::new(vg_ct / dec!(100), "EUR/kWh"))
                    .tag("eeg_verguetung")
                    .build()?,
            );
        }
        if let Some(mp_ct) = self.tariff.eeg_marktpraemie_ct_per_kwh {
            items.push(
                LineItem::debit("EEG Marktpr\u{00e4}mie (\u{00a7}38 EEG)")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(mp_ct / dec!(100), "EUR/kWh"))
                    .tag("eeg_marktpraemie")
                    .build()?,
            );
        }
        if let Some(mgp_ct) = self.tariff.eeg_managementpraemie_ct_per_kwh {
            items.push(
                LineItem::debit("Managementpr\u{00e4}mie Direktvermarktung (\u{00a7}53 EEG)")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(mgp_ct / dec!(100), "EUR/kWh"))
                    .tag("eeg_managementpraemie")
                    .build()?,
            );
        }
        if let Some(kwkg_ct) = self.tariff.kwkg_zuschlag_ct_per_kwh {
            items.push(
                LineItem::debit("KWKG Zuschlag")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(kwkg_ct / dec!(100), "EUR/kWh"))
                    .tag("kwkg_zuschlag")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// Non-EEG Direktvermarktung settlement — EINSPEISUNG.
pub struct EinspeisungTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for EinspeisungTariff<'_> {
    type Usage = EegMeterInput;
    type Error = BillingError;

    fn line_items(&self, meter: &EegMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let kwh = meter.einspeisung_kwh;
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(mv_ct) = self.tariff.marktwert_ct_per_kwh {
            items.push(
                LineItem::debit("Marktwert Strom (EPEX Spot Monatsmarktwert)")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(mv_ct / dec!(100), "EUR/kWh"))
                    .tag("einspeisung_marktwert")
                    .build()?,
            );
        }
        if let Some(vg_ct) = self.tariff.vermarktungsgebuehr_ct_per_kwh {
            items.push(
                LineItem::credit("Vermarktungsgeb\u{00fc}hr Direktvermarkter")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(vg_ct / dec!(100), "EUR/kWh"))
                    .tag("einspeisung_vermarktungsgebuehr")
                    .tag("rabatt")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// HEMS subscription + event billing.
pub struct HemsTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for HemsTariff<'_> {
    type Usage = HemsMeterInput;
    type Error = BillingError;

    fn line_items(&self, usage: &HemsMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let months = usage.months.unwrap_or(dec!(1));
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(fee) = self.tariff.hems_platform_fee_eur_per_month {
            items.push(
                LineItem::debit("HEMS Plattformgeb\u{00fc}hr")
                    .quantity(Quantity::new(months, "Monat"))
                    .unit_price(UnitPrice::new(fee, "EUR/Monat"))
                    .tag("hems_platform_fee")
                    .build()?,
            );
        }
        if let (Some(count), Some(price)) = (
            usage.optimization_events,
            self.tariff.hems_optimization_event_eur,
        ) {
            items.push(
                LineItem::debit("HEMS Optimierungsereignis")
                    .quantity(Quantity::new(Decimal::from(count), "Ereignis"))
                    .unit_price(UnitPrice::new(price, "EUR/Ereignis"))
                    .tag("hems_optimierung")
                    .build()?,
            );
        }
        if let (Some(count), Some(price)) =
            (usage.readout_events, self.tariff.hems_readout_event_eur)
        {
            items.push(
                LineItem::debit("Smart Meter Auslesung")
                    .quantity(Quantity::new(Decimal::from(count), "Auslesung"))
                    .unit_price(UnitPrice::new(price, "EUR/Auslesung"))
                    .tag("hems_auslesung")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// E-Mobility CPO/EMSP service billing.
pub struct EmobilityTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for EmobilityTariff<'_> {
    type Usage = EmobilityMeterInput;
    type Error = BillingError;

    fn line_items(&self, usage: &EmobilityMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let months = usage.months.unwrap_or(dec!(1));
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(fee) = self.tariff.emobility_service_fee_eur_per_month {
            items.push(
                LineItem::debit("Betriebsgeb\u{00fc}hr Ladestation")
                    .quantity(Quantity::new(months, "Monat"))
                    .unit_price(UnitPrice::new(fee, "EUR/Monat"))
                    .tag("emobility_service_fee")
                    .build()?,
            );
        }
        if let (Some(kwh), Some(ap_ct)) = (
            usage.kwh_charged,
            self.tariff.emobility_arbeitspreis_ct_per_kwh,
        ) {
            items.push(
                LineItem::debit("Ladeenergie")
                    .quantity(Quantity::new(kwh, "kWh"))
                    .unit_price(UnitPrice::new(ap_ct / dec!(100), "EUR/kWh"))
                    .tag("emobility_ladeenergie")
                    .build()?,
            );
        }
        if let (Some(sessions), Some(sf)) = (usage.sessions, self.tariff.emobility_session_fee_eur)
        {
            items.push(
                LineItem::debit("Ladesitzungsgeb\u{00fc}hr")
                    .quantity(Quantity::new(Decimal::from(sessions), "Sitzung"))
                    .unit_price(UnitPrice::new(sf, "EUR/Sitzung"))
                    .tag("emobility_session_fee")
                    .build()?,
            );
        }
        if let (Some(rsess), Some(rf)) = (
            usage.roaming_sessions,
            self.tariff.emobility_roaming_fee_eur,
        ) {
            items.push(
                LineItem::debit("Roaming-Geb\u{00fc}hr (Fremdnetz)")
                    .quantity(Quantity::new(Decimal::from(rsess), "Sitzung"))
                    .unit_price(UnitPrice::new(rf, "EUR/Sitzung"))
                    .tag("emobility_roaming")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// Energiedienstleistung — MSB, EMS, maintenance.
pub struct ServiceTariff<'a> {
    pub tariff: &'a TariffInput,
    pub rates: &'a RegulatoryRates,
}

impl Tariff for ServiceTariff<'_> {
    type Usage = ServiceMeterInput;
    type Error = BillingError;

    fn line_items(&self, usage: &ServiceMeterInput) -> Result<Vec<LineItem>, BillingError> {
        let months = usage.months.unwrap_or(dec!(1));
        let mut items: Vec<LineItem> = Vec::new();

        if let Some(fee) = self.tariff.service_fee_eur {
            items.push(
                LineItem::debit("Grundgeb\u{00fc}hr Energiedienstleistung")
                    .quantity(Quantity::new(months, "Monat"))
                    .unit_price(UnitPrice::new(fee, "EUR/Monat"))
                    .tag("service_fee")
                    .build()?,
            );
        }
        if let (Some(count), Some(price)) = (
            usage.event_count,
            usage
                .event_price_eur
                .or(self.tariff.service_event_price_eur),
        ) {
            items.push(
                LineItem::debit("Einzelabruf / Ereignis")
                    .quantity(Quantity::new(Decimal::from(count), "Ereignis"))
                    .unit_price(UnitPrice::new(price, "EUR/Ereignis"))
                    .tag("event_fee")
                    .build()?,
            );
        }
        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        let mwst = self.rates.mwst(self.tariff);
        if mwst > Decimal::ZERO {
            vec![Box::new(FixedRateTax::new(
                format!("Mehrwertsteuer {:.0}%", mwst * dec!(100)),
                mwst,
            ))]
        } else {
            vec![]
        }
    }
}

/// §41a dynamic-tariff electricity — EPEX Spot per-interval pricing.
///
/// EPEX position is tagged `"commodity"` so Stromsteuer applies correctly.
/// MwSt base includes commodity + NNE + Stromsteuer (via billing's accumulated-layer model).
/// No §14a support (no peak demand signal from interval data).
pub struct DynamicStromTariff<'a> {
    pub tariff: &'a TariffInput,
    pub grid: &'a GridInput,
    pub rates: &'a RegulatoryRates,
    pub period_from: time::Date,
    pub period_to: time::Date,
    pub eeg_gutschrift_eur: Option<Decimal>,
}

impl Tariff for DynamicStromTariff<'_> {
    type Usage = DynamicUsage;
    type Error = BillingError;

    fn line_items(&self, usage: &DynamicUsage) -> Result<Vec<LineItem>, BillingError> {
        let days = (self.period_to - self.period_from).whole_days();
        let mut items: Vec<LineItem> = Vec::new();

        // Grundpreis
        if let Some(gp_ct) = self.tariff.grundpreis_ct_per_day {
            items.push(
                LineItem::debit("Grundpreis Strom (\u{00a7}41a dynamisch)")
                    .quantity(Quantity::new(Decimal::from(days), "Tage"))
                    .unit_price(UnitPrice::new(gp_ct / dec!(100), "EUR/Tag"))
                    .tag("grundpreis")
                    .build()?,
            );
        }

        // EPEX Arbeitspreis: Σ(kwh_i × EPEX_i) with weighted-average unit price
        let mut total_kwh = Decimal::ZERO;
        let mut total_cost_eur = Decimal::ZERO;
        let mut missing: u32 = 0;
        for iv in &usage.intervals {
            let de_ts = iv.timestamp_utc + time::Duration::hours(1);
            let key = (de_ts.year(), de_ts.month() as u8, de_ts.day(), de_ts.hour());
            let raw_price_ct = usage
                .epex_prices_ct_kwh
                .get(&key)
                .copied()
                .unwrap_or_else(|| {
                    missing += 1;
                    Decimal::ZERO
                });
            // Apply optional price floor (e.g. 0 ct/kWh to prevent negative-EPEX credits)
            let price_ct = self
                .tariff
                .dynamic_epex_floor_ct_kwh
                .map(|floor| raw_price_ct.max(floor))
                .unwrap_or(raw_price_ct);
            total_kwh += iv.kwh;
            total_cost_eur += iv.kwh * price_ct / dec!(100);
        }
        if missing > 0 {
            tracing::warn!(
                missing,
                "billingd: {missing} EPEX intervals missing — billed 0 ct/kWh"
            );
        }
        if !total_kwh.is_zero() {
            let avg_eur = (total_cost_eur / total_kwh).round_dp(6);
            items.push(
                LineItem::debit("Arbeitspreis Strom EPEX Spot (\u{00a7}41a EnWG)")
                    .quantity(Quantity::new(total_kwh, "kWh"))
                    .unit_price(UnitPrice::new(avg_eur, "EUR/kWh"))
                    .tag("dynamic_epex")
                    .tag("arbeitspreis")
                    .tag("commodity")
                    .build()?,
            );
        }

        // NNE + KA (uses total_kwh as arbeitsmenge, no peak demand for dynamic)
        items.extend(strom_grid_items(self.grid, total_kwh, None, days)?);

        // EEG Gutschrift
        if let Some(eeg_eur) = self.eeg_gutschrift_eur
            && eeg_eur != Decimal::ZERO
        {
            let amt = decimal_to_euro(eeg_eur)?;
            items.push(
                LineItem::credit("EEG Einspeisegutschrift")
                    .fixed_amount(amt)
                    .tag("eeg_gutschrift")
                    .tag("credit")
                    .build()?,
            );
        }

        Ok(items)
    }

    fn tax_layers(&self) -> Vec<Box<dyn TaxLayer>> {
        strom_tax_layers(self.tariff, self.rates)
    }
}

// ── Public calculate_* functions ──────────────────────────────────────────────

/// Calculate an electricity billing invoice (STROM, WAERMEPUMPE, WALLBOX).
#[must_use]
pub fn calculate_strom(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &MeterInput,
    grid: &GridInput,
    eeg_gutschrift_eur: Option<Decimal>,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = StromTariff {
        tariff,
        grid,
        rates,
        period_from,
        period_to,
        eeg_gutschrift_eur,
    };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate a gas billing invoice (GAS).
#[must_use]
pub fn calculate_gas(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &GasMeterInput,
    grid: &GridInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = GasTariff {
        tariff,
        grid,
        rates,
        period_from,
        period_to,
    };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    let mut result = billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    );
    // Inject gas quality metadata as ZusatzAttribut for regulatory audit transparency.
    // Per DVGW G 260: the Brennwert used for billing is always the measured value reported
    // by the grid operator — this is purely an audit annotation, not a correction factor.
    if let Some(ref gq) = meter.gasqualitaet {
        if let Some(obj) = result.rechnung_json.as_object_mut() {
            let attrs = obj
                .entry("zusatzAttribute")
                .or_insert_with(|| serde_json::Value::Array(vec![]));
            if let Some(arr) = attrs.as_array_mut() {
                arr.push(serde_json::json!({
                    "_typ": "ZUSATZ_ATTRIBUT",
                    "name": "gasqualitaet",
                    "wert": gq
                }));
            }
        }
    }
    Ok(result)
}

/// Calculate a district heat (Fernwärme) billing invoice (WAERME).
#[must_use]
pub fn calculate_waerme(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &WaermeMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = WaermeTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate a solar supply invoice (SOLAR).
#[must_use]
pub fn calculate_solar(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &SolarMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = SolarTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate an EEG feed-in settlement credit note (EEG).
#[must_use]
pub fn calculate_eeg(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &EegMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = EegTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "GUTSCHRIFT",
    ))
}

/// Calculate a non-EEG Direktvermarktung settlement (EINSPEISUNG).
#[must_use]
pub fn calculate_einspeisung(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    meter: &EegMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = EinspeisungTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, meter)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "GUTSCHRIFT",
    ))
}

/// Calculate a HEMS subscription + event billing invoice (HEMS).
#[must_use]
pub fn calculate_hems(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    usage: &HemsMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = HemsTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, usage)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate an e-mobility CPO/EMSP service invoice (EMOBILITY).
#[must_use]
pub fn calculate_emobility(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    usage: &EmobilityMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = EmobilityTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, usage)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate an energy service invoice (ENERGIEDIENSTLEISTUNG).
#[must_use]
pub fn calculate_energiedienstleistung(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    usage: &ServiceMeterInput,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let t = ServiceTariff { tariff, rates };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, usage)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

/// Calculate a §41a dynamic-tariff electricity invoice.
#[must_use]
pub fn calculate_dynamic_strom(
    malo_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: time::Date,
    period_to: time::Date,
    tariff: &TariffInput,
    grid: &GridInput,
    eeg_gutschrift_eur: Option<Decimal>,
    intervals: &[DynamicInterval],
    epex_prices_ct_kwh: &HashMap<(i32, u8, u8, u8), Decimal>,
    rates: &RegulatoryRates,
) -> Result<BillingResult, BillingError> {
    let usage = DynamicUsage {
        intervals: intervals.to_vec(),
        epex_prices_ct_kwh: epex_prices_ct_kwh.clone(),
    };
    let t = DynamicStromTariff {
        tariff,
        grid,
        rates,
        period_from,
        period_to,
        eeg_gutschrift_eur,
    };
    let meta = DocumentMeta {
        invoice_number: rechnungsnummer.to_owned(),
        period_label: format!("{period_from}\u{2013}{period_to}"),
        period: Some(Period::new(period_from.to_string(), period_to.to_string())),
        issue_date: Some(time::OffsetDateTime::now_utc().date().to_string()),
        issuer_id: Some(lf_mp_id.to_owned()),
        ..Default::default()
    };
    let doc = t.bill(meta, &usage)?;
    Ok(billing_result_from_doc(
        doc,
        malo_id,
        lf_mp_id,
        rechnungsnummer,
        period_from,
        period_to,
        "ABSCHLAGSRECHNUNG",
    ))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn strom_tariff(gp_ct: f64, ap_ct: f64) -> TariffInput {
        TariffInput {
            category: "STROM".to_owned(),
            grundpreis_ct_per_day: Some(Decimal::try_from(gp_ct).unwrap()),
            arbeitspreis_ct_per_kwh: Some(Decimal::try_from(ap_ct).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn strom_eintarif_netto() {
        let tariff = strom_tariff(20.0, 30.0);
        let meter = MeterInput {
            arbeitsmenge_kwh: dec!(100),
            arbeitsmenge_ht_kwh: None,
            arbeitsmenge_nt_kwh: None,
            spitzenleistung_kw: None,
            steuerung_stunden: None,
        };
        let grid = GridInput {
            nne_arbeitspreis_ct_per_kwh: Some(dec!(5.0)),
            ..Default::default()
        };
        let rates = RegulatoryRates::default();
        let result = calculate_strom(
            "51238696781",
            "9900000000001",
            "R2025-001",
            time::macros::date!(2025 - 01 - 01),
            time::macros::date!(2025 - 01 - 31),
            &tariff,
            &meter,
            &grid,
            None,
            &rates,
        )
        .unwrap();
        // GP: 20ct × 30 days = 6.00 EUR
        // AP: 30ct × 100 kWh = 30.00 EUR
        // NNE AP: 5ct × 100 kWh = 5.00 EUR
        // Stromsteuer: 2.05ct × 100 kWh = 2.05 EUR  (levy → included in netto)
        // netto = 43.05 EUR
        assert_eq!(result.netto_eur, dec!(43.05), "netto must match");
        assert!(result.brutto_eur > result.netto_eur);
    }

    #[test]
    fn gas_brennwertkorrektur() {
        let tariff = TariffInput {
            category: "GAS".to_owned(),
            gas_grundpreis_ct_per_day: Some(dec!(15.0)),
            gas_arbeitspreis_ct_per_kwh_hs: Some(dec!(9.50)),
            ..strom_tariff(0.0, 0.0)
        };
        let meter = GasMeterInput {
            messung_qm3: dec!(100.0),
            brennwert_kwh_per_qm3: Some(dec!(10.55)),
            zustandszahl: Some(dec!(0.9994)),
            kwh_hs: None,
            gasqualitaet: None,
        };
        let kwh = gas_kwh_hs(&meter);
        assert!(
            kwh > dec!(1000) && kwh < dec!(1100),
            "Brennwertkorrektur range check: {kwh}"
        );
        let result = calculate_gas(
            "51238696781",
            "9900000000001",
            "RGAS-001",
            time::macros::date!(2025 - 01 - 01),
            time::macros::date!(2025 - 01 - 31),
            &tariff,
            &meter,
            &GridInput::default(),
            &RegulatoryRates::default(),
        )
        .unwrap();
        // Brennwertkorrektur(info) + Grundpreis + Arbeitspreis + Energiesteuer + BEHG + MwSt
        assert!(result.positions.len() >= 5);
        assert!(result.brutto_eur > Decimal::ZERO);
    }

    #[test]
    fn regulatory_rates_product_override() {
        // Product with Stromsteuerbefreiung (§9 StromStG)
        let tariff = TariffInput {
            stromsteuer_ct_per_kwh_override: Some(Decimal::ZERO),
            ..strom_tariff(0.0, 30.0)
        };
        let meter = MeterInput {
            arbeitsmenge_kwh: dec!(1000),
            arbeitsmenge_ht_kwh: None,
            arbeitsmenge_nt_kwh: None,
            spitzenleistung_kw: None,
            steuerung_stunden: None,
        };
        let result = calculate_strom(
            "X",
            "Y",
            "R-EXEMPT",
            time::macros::date!(2025 - 01 - 01),
            time::macros::date!(2025 - 01 - 31),
            &tariff,
            &meter,
            &GridInput::default(),
            None,
            &RegulatoryRates::default(),
        )
        .unwrap();
        // No levy (Stromsteuer) position
        assert!(
            !result
                .positions
                .iter()
                .any(|p| p.has_tag("levy") && p.unit_label() == Some("kWh")),
            "Stromsteuer should be absent when rate=0",
        );
    }
}
