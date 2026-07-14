//! `Quantities` — all metered quantities for one billing period.
//!
//! The single container for all product meter data. Replaces positional
//! parameters passed to each `calculate_*` function.

use rust_decimal::Decimal;
use std::collections::HashMap;
use time::OffsetDateTime;

// ── Meter input types ─────────────────────────────────────────────────────────

/// Electricity meter data for one billing period.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MeterInput {
    /// Total energy in kWh (Arbeitsmenge).
    #[serde(default)]
    pub arbeitsmenge_kwh: Decimal,
    /// High-tariff energy in kWh (HT, for Zweitarif). `None` = single tariff.
    #[serde(default)]
    pub arbeitsmenge_ht_kwh: Option<Decimal>,
    /// Low-tariff energy in kWh (NT, for Zweitarif). `None` = single tariff.
    #[serde(default)]
    pub arbeitsmenge_nt_kwh: Option<Decimal>,
    /// Peak demand in kW (Spitzenleistung, §2 Nr. 17 MessZV).
    #[serde(default)]
    pub spitzenleistung_kw: Option<Decimal>,
    /// §14a EnWG: hours the controllable device was under NB management.
    #[serde(default)]
    pub steuerung_stunden: Option<Decimal>,
}

/// Gas meter data for one billing period.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GasMeterInput {
    /// Volume at meter conditions (m³).
    pub messung_qm3: Decimal,
    /// Calorific value (Brennwert Ho/Hs) in kWh/m³.
    #[serde(default)]
    pub brennwert_kwh_per_qm3: Option<Decimal>,
    /// Volume conversion factor (Zustandszahl, dimensionless).
    #[serde(default)]
    pub zustandszahl: Option<Decimal>,
    /// Pre-computed kWh_Hs (takes precedence over Brennwert × Zustandszahl).
    #[serde(default)]
    pub kwh_hs: Option<Decimal>,
    /// Gas quality annotation (e.g. `"H_GAS"`, `"L_GAS"`, `"H2_BLEND"`).
    /// Informational only — billing always uses the measured Brennwert.
    #[serde(default)]
    pub gasqualitaet: Option<String>,
}

/// District heat meter data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WaermeMeterInput {
    /// Thermal energy delivered (kWh_th).
    #[serde(default)]
    pub kwh_waerme: Decimal,
    /// Peak demand in kW (for Leistungspreis billing).
    #[serde(default)]
    pub spitzenleistung_kw: Option<Decimal>,
    /// Pro-rata months (defaults to 1 = one full billing month).
    #[serde(default)]
    pub months: Option<Decimal>,
}

/// Solar / Eigenverbrauch / Mieterstrom / GGV meter data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SolarMeterInput {
    /// Metered self-consumption or locally delivered kWh.
    pub eigenverbrauch_kwh: Decimal,
}

// ── GGV Nutzungsplan ──────────────────────────────────────────────────────────

/// §42b EEG 2023 — One entry in the GGV Nutzungsplan (tenant allocation table).
///
/// The Nutzungsplan distributes the plant's PV generation among participating
/// building occupants (Teilnehmer). Each entry maps one Marktlokation (tenant
/// delivery point) to its allocation fraction.
///
/// ## Legal basis
///
/// §42b Abs. 1 EEG 2023 (Solarpaket I): the Lieferant must maintain a Nutzungsplan
/// for the duration of the GGV contract. The sum of all fractions must equal 1.0.
///
/// ## Storage
///
/// Stored as `ggv_nutzungsplan JSONB` on `eeg_anlagen` (migration 0009).
/// Deserialize with `serde_json::from_value::<Vec<GgvNutzungsplanEntry>>(...)`.
///
/// ## Example
///
/// ```rust
/// use energy_billing::GgvNutzungsplanEntry;
/// use rust_decimal_macros::dec;
///
/// let plan = vec![
///     GgvNutzungsplanEntry { malo_id: "51238696780".into(), fraction: dec!(0.45) },
///     GgvNutzungsplanEntry { malo_id: "51238696781".into(), fraction: dec!(0.35) },
///     GgvNutzungsplanEntry { malo_id: "51238696782".into(), fraction: dec!(0.20) },
/// ];
/// let total: rust_decimal::Decimal = plan.iter().map(|e| e.fraction).sum();
/// assert_eq!(total, dec!(1.0));
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GgvNutzungsplanEntry {
    /// 11-digit Marktlokations-ID of the tenant delivery point.
    pub malo_id: String,

    /// Fraction of PV generation allocated to this tenant (0.0 < fraction ≤ 1.0).
    ///
    /// The sum of all fractions in the Nutzungsplan must equal exactly 1.0.
    /// Validate with `GgvNutzungsplan::validate()` before billing.
    pub fraction: Decimal,
}

/// §42b EEG 2023 — GGV Nutzungsplan (complete tenant allocation table).
///
/// Wraps the list of entries and provides validation and allocation computation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GgvNutzungsplan(pub Vec<GgvNutzungsplanEntry>);

impl GgvNutzungsplan {
    /// Validate that all fractions are positive and sum to 1.0 (within 0.001 tolerance).
    ///
    /// Returns `Err` with a diagnostic message if validation fails.
    pub fn validate(&self) -> Result<(), String> {
        use rust_decimal_macros::dec;
        if self.0.is_empty() {
            return Err("GGV Nutzungsplan must have at least one entry".to_owned());
        }
        for e in &self.0 {
            if e.fraction <= Decimal::ZERO {
                return Err(format!(
                    "GGV Nutzungsplan: fraction for {} must be > 0, got {}",
                    e.malo_id, e.fraction
                ));
            }
        }
        let total: Decimal = self.0.iter().map(|e| e.fraction).sum();
        let diff = (total - Decimal::ONE).abs();
        if diff > dec!(0.001) {
            return Err(format!(
                "GGV Nutzungsplan: fractions sum to {total}, must be 1.0 (±0.001)"
            ));
        }
        Ok(())
    }

    /// Allocate a generation quantity proportionally among tenants.
    ///
    /// Returns `(malo_id, allocated_kwh)` pairs. Any remainder from rounding
    /// is added to the last entry to ensure the sum equals `total_kwh`.
    pub fn allocate(&self, total_kwh: Decimal) -> Vec<(String, Decimal)> {
        let n = self.0.len();
        if n == 0 || total_kwh <= Decimal::ZERO {
            return vec![];
        }
        let mut allocated: Vec<(String, Decimal)> = self
            .0
            .iter()
            .map(|e| {
                let kwh = (total_kwh * e.fraction).round_dp(3);
                (e.malo_id.clone(), kwh)
            })
            .collect();

        // Add rounding remainder to last entry
        let sum: Decimal = allocated.iter().map(|(_, k)| k).cloned().sum();
        let remainder = total_kwh - sum;
        if let Some(last) = allocated.last_mut() {
            last.1 += remainder;
        }
        allocated
    }
}

/// EEG feed-in settlement meter data (simplified LF view).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EegMeterInput {
    /// Total kWh fed into the grid during the billing period.
    pub einspeisung_kwh: Decimal,
    /// kWh during negative-EPEX hours (§51 EEG suspension).
    #[serde(default)]
    pub kwh_during_negative_epex: Option<Decimal>,
}

/// HEMS (Home Energy Management System) subscription usage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct HemsMeterInput {
    /// Billing months (for monthly subscription fee).
    #[serde(default)]
    pub months: Option<Decimal>,
    /// Number of optimisation events.
    #[serde(default)]
    pub optimization_events: Option<u32>,
    /// Number of smart-meter readout events.
    #[serde(default)]
    pub readout_events: Option<u32>,
}

/// E-Mobility CPO/EMSP usage data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
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

/// Energiedienstleistung service usage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ServiceMeterInput {
    #[serde(default)]
    pub months: Option<Decimal>,
    #[serde(default)]
    pub event_count: Option<u32>,
    #[serde(default)]
    pub event_price_eur: Option<Decimal>,
}

/// One interval for §41a dynamic tariff billing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DynamicInterval {
    /// Interval start (UTC).
    pub timestamp_utc: OffsetDateTime,
    /// Energy in kWh for this interval.
    pub kwh: Decimal,
}

// ── Grid pass-through costs ───────────────────────────────────────────────────

/// Grid infrastructure charges sourced from `marktd` or supplied directly.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GridInput {
    // ── Strom ─────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub nne_grundpreis_eur_per_year: Option<Decimal>,
    #[serde(default)]
    pub nne_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub nne_leistungspreis_eur_per_kw_year: Option<Decimal>,
    #[serde(default)]
    pub ka_ct_per_kwh: Option<Decimal>,
    // ── Gas ───────────────────────────────────────────────────────────────────
    #[serde(default)]
    pub gas_nne_grundpreis_eur_per_year: Option<Decimal>,
    #[serde(default)]
    pub gas_nne_arbeitspreis_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_ka_ct_per_kwh: Option<Decimal>,
    #[serde(default)]
    pub gas_bilanzierungsumlage_ct_per_kwh: Option<Decimal>,
}

// ── Quantities ────────────────────────────────────────────────────────────────

/// All metered quantities for one billing period.
///
/// Replaces the scattered positional parameters of the old `calculate_*` functions.
/// Set only the fields relevant for the current billing run — defaults are `None`/
/// empty for unused products.
///
/// ## Multi-product billing
///
/// To bill a customer with electricity + solar + HEMS on one invoice:
///
/// ```rust,ignore
/// let quantities = Quantities {
///     electricity: Some(MeterInput { arbeitsmenge_kwh: dec!(500), ..Default::default() }),
///     solar: Some(SolarMeterInput { eigenverbrauch_kwh: dec!(120) }),
///     hems: Some(HemsMeterInput { months: Some(dec!(1)), ..Default::default() }),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Default)]
pub struct Quantities {
    /// Electricity consumption (STROM, WAERMEPUMPE, WALLBOX).
    pub electricity: Option<MeterInput>,
    /// Natural gas consumption (GAS).
    pub gas: Option<GasMeterInput>,
    /// District heat / Fernwärme (WAERME).
    pub heat: Option<WaermeMeterInput>,
    /// Solar self-consumption / Mieterstrom / GGV (SOLAR).
    pub solar: Option<SolarMeterInput>,
    /// EEG feed-in meter data (simplified path — rates from TariffInput).
    pub eeg: Option<EegMeterInput>,
    /// Full EEG settlement via `eeg-billing` — set this for NB-side precision.
    ///
    /// When set, `EegProvider` calls `eeg_billing::calculate_settlement(eeg_full)`
    /// for version-aware §51/§52 rules. Supersedes `eeg` when both are present.
    pub eeg_full: Option<eeg_billing::SettleInput>,
    /// Non-EEG Direktvermarktung feed-in (EINSPEISUNG).
    pub einspeisung: Option<EegMeterInput>,
    /// HEMS subscription and event data.
    pub hems: Option<HemsMeterInput>,
    /// E-mobility CPO/EMSP data.
    pub emobility: Option<EmobilityMeterInput>,
    /// Energiedienstleistung service data.
    pub service: Option<ServiceMeterInput>,
    /// §41a dynamic tariff intervals (15-min Lastgang from edmd).
    pub dynamic_intervals: Vec<DynamicInterval>,
    /// EPEX Spot price map for §41a billing: `(year, month, day, hour_CET)` → ct/kWh.
    ///
    /// Set by the service layer (billingd) after fetching from `marktd GET /api/v1/epex-preise`.
    /// `DynamicElectricityProvider` reads this map as a fallback when its internal
    /// `SpotPriceSource` has no data for an interval. This is the standard production path:
    /// `build_engine()` creates the provider with an empty source, and prices flow in here
    /// at `bill()` time.
    pub dynamic_epex_prices: HashMap<(i32, u8, u8, u8), Decimal>,
    /// EEG Gutschrift credit passed through to electricity billing (e.g. from einsd).
    pub eeg_gutschrift_eur: Option<Decimal>,
}
