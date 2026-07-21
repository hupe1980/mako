//! `Quantities` — all metered quantities for one billing period.
//!
//! The single container for all product meter data. Replaces positional
//! parameters passed to each `calculate_*` function.

use crate::rates::RoundMoney;
use rust_decimal::Decimal;
use std::collections::HashMap;
use time::OffsetDateTime;

// ── Meter input types ─────────────────────────────────────────────────────────

/// Metering mode of the delivery point (§3/§4 MessZV, §41a EnWG).
///
/// Determines billing granularity, permissible tariff types, and substitution
/// rules for missing interval data.
///
/// | Mode | Annual consumption | Billing basis | §41a dynamic tariff |
/// |---|---|---|---|
/// | `Slp` | < 100 MWh/year | Standard load profile (estimated) | ✗ |
/// | `Rlm` | ≥ 100 MWh/year | Registered 15-min values | ✗ |
/// | `Imsys` | ≥ 6 MWh/year (§31 MsbG) | Smart Meter Gateway | ✓ |
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MeteringMode {
    /// Standard load profile (SLP) — estimated annual consumption billing.
    /// Typical for residential and small commercial customers (< 100 MWh/year).
    #[default]
    Slp,
    /// Registrierende Leistungsmessung (RLM) — measured 15-minute interval billing.
    /// Required for customers ≥ 100 MWh/year (§4 MessZV, §14 NAV).
    Rlm,
    /// Intelligentes Messsystem (iMSys) — Smart Meter Gateway.
    /// Enables §41a EnWG dynamic tariffs. Required for > 6 MWh/year (§31 MsbG).
    Imsys,
}

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
    /// Zählernummer (§41 EnWG — mandatory on electricity invoices).
    ///
    /// When set, appears as an informational position on the invoice.
    /// Overrides `BillingContext::zaehler_id` for this specific meter.
    #[serde(default)]
    pub zaehlernummer: Option<String>,
    /// Zählerstand at the start of the billing period.
    ///
    /// §41 EnWG: billing invoices must show the meter reading at period start.
    #[serde(default)]
    pub zaehlerstand_von: Option<Decimal>,
    /// Zählerstand at the end of the billing period.
    ///
    /// §41 EnWG: billing invoices must show the meter reading at period end.
    #[serde(default)]
    pub zaehlerstand_bis: Option<Decimal>,

    /// Metering mode — SLP, RLM, or iMSys (Smart Meter).
    ///
    /// Used to validate tariff compatibility (§41a requires `Imsys`) and to
    /// label estimated readings correctly on the invoice.
    #[serde(default)]
    pub metering_mode: MeteringMode,

    /// `true` when the consumption figure is an estimate (§17 Abs. 1 MessZV Ersatzwert).
    ///
    /// Estimated readings must be labeled on the invoice. The meter operator must
    /// confirm or replace the estimate within 8 weeks (§17 Abs. 1 MessZV).
    #[serde(default)]
    pub is_estimated: bool,

    /// `true` when the meter was replaced during this billing period (Zählerwechsel).
    ///
    /// When set, `zaehlerstand_von` / `zaehlerstand_bis` may relate to different
    /// meter serial numbers. The invoice must note the meter exchange.
    #[serde(default)]
    pub zaehler_replaced: bool,
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
    /// Peak demand in kW (Spitzenleistung) for RLM gas billing.
    ///
    /// Required when `TariffInput::gas_leistungspreis_ct_per_kw_month` is set.
    /// Applicable to large gas customers with RLM metering (> 1.5 GWh/year).
    #[serde(default)]
    pub spitzenleistung_kw: Option<Decimal>,
    /// Zählernummer (§40 Abs. 2 Nr. 6 EnWG — meter identity on the bill).
    /// Overrides `BillingContext::zaehler_id` for this meter.
    #[serde(default)]
    pub zaehlernummer: Option<String>,
    /// Meter reading at period start, in m³ (§40 Abs. 2 Nr. 6 EnWG).
    #[serde(default)]
    pub zaehlerstand_von: Option<Decimal>,
    /// Meter reading at period end, in m³ (§40 Abs. 2 Nr. 6 EnWG).
    #[serde(default)]
    pub zaehlerstand_bis: Option<Decimal>,
    /// Reading is an estimate / Ersatzwert (§40a EnWG, §17 Abs. 1 MessZV).
    /// Must be prominently labeled on the bill; the customer may demand a
    /// correction once a real reading arrives.
    #[serde(default)]
    pub is_estimated: bool,
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
/// use rust_decimal::dec;
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
        use rust_decimal::dec;
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
    /// Returns `(malo_id, allocated_kwh)` pairs.
    ///
    /// Uses `billing::proportional_split` (Largest-Remainder / Hamilton method) —
    /// guarantees `Σ(allocated_kwh) == total_kwh` with each tenant within
    /// ±0.001 kWh of their exact share. No single entry absorbs all rounding error.
    pub fn allocate(&self, total_kwh: Decimal) -> Vec<(String, Decimal)> {
        if self.0.is_empty() || total_kwh <= Decimal::ZERO {
            return vec![];
        }
        let fractions: Vec<Decimal> = self.0.iter().map(|e| e.fraction).collect();
        // billing::proportional_split uses Largest-Remainder (Hamilton) method:
        // scale=3 → 0.001 kWh resolution; returns Err only for empty fractions.
        let parts = billing::proportional_split(total_kwh, &fractions, 3)
            .expect("fractions non-empty — checked above");
        self.0
            .iter()
            .zip(parts)
            .map(|(e, kwh)| (e.malo_id.clone(), kwh))
            .collect()
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

// ── §41a Abs. 6 — Annual savings comparison ───────────────────────────────────

/// §41a Abs. 6 EnWG — Annual savings comparison for dynamic tariff customers.
///
/// Lieferanten must provide dynamic tariff customers with an annual statement
/// of how much they saved (or paid more) compared to a reference fixed tariff.
///
/// ## Legal basis
///
/// §41a Abs. 6 EnWG: „Der Lieferant hat dem Letztverbraucher jährlich mitzuteilen,
/// wie viel er durch die dynamische Preiskomponente im Vergleich zu einem
/// Standardtarif eingespart oder mehr ausgegeben hat."
///
/// ## Usage
///
/// Compute via [`Sect41aAnnualComparison::compute`] and set in [`Quantities`].
/// `DynamicElectricityProvider` renders it as an informational position on the
/// annual invoice.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Sect41aAnnualComparison {
    /// kWh consumed under the dynamic tariff in the comparison period.
    pub actual_kwh: Decimal,
    /// Total amount paid under the dynamic tariff (EUR brutto, inclusive of MwSt).
    pub actual_eur_brutto: Decimal,
    /// Reference fixed-price (ct/kWh, brutto) for the annual comparison.
    ///
    /// Typically the customer's previous fixed tariff or the operator's standard
    /// product price at time of dynamic tariff contract start.
    pub reference_price_ct_per_kwh: Decimal,
    /// What the customer would have paid at the reference price (EUR brutto).
    pub reference_eur_brutto: Decimal,
    /// EUR difference: positive = saved money, negative = paid more.
    pub savings_eur: Decimal,
}

impl Sect41aAnnualComparison {
    /// Compute the annual comparison from actual totals and a reference price.
    #[must_use]
    pub fn compute(
        actual_kwh: Decimal,
        actual_eur_brutto: Decimal,
        reference_price_ct_per_kwh: Decimal,
    ) -> Self {
        use rust_decimal::dec;
        let reference_eur_brutto =
            (actual_kwh * reference_price_ct_per_kwh / dec!(100)).round_kfm(2);
        let savings_eur = (reference_eur_brutto - actual_eur_brutto).round_kfm(2);
        Self {
            actual_kwh,
            actual_eur_brutto,
            reference_price_ct_per_kwh,
            reference_eur_brutto,
            savings_eur,
        }
    }
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

// ── EnergyShareMeterInput ─────────────────────────────────────────────────────

/// §42c EnWG Energy Sharing — metered allocation for one community participant.
///
/// Populated by `billingd` from the participant's virtual meter (Summenzeitreihe)
/// computed by `edmd` using the community's `AggregationRule::GgvConstantAllocation`
/// or `GgvProportionalAllocation` (same infrastructure as §42b GGV).
///
/// ## §42c EnWG vs §42b GGV
///
/// | | §42b GGV (Solarpaket I) | §42c Energiegemeinschaft |
/// |---|---|---|
/// | Scope | Building community | Grid area (0.4 kV) |
/// | Participants | Tenants in same building | Up to 100 members |
/// | Plant size | No limit | ≤ 500 kW total |
/// | Metering | Building meter | Smart meter (iMSys) mandatory |
/// | LF billing | via SolarProvider | via EnergyShareProvider |
///
/// ## Billing model
///
/// The LF bills the full grid consumption (via `ElectricityProvider`) and then
/// credits the sharing allocation (via `EnergyShareProvider`) at the contracted
/// rate — typically below the retail tariff and above the wholesale price.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct EnergyShareMeterInput {
    /// kWh allocated from the community energy pool to this participant.
    ///
    /// Computed from the community's total generation and this participant's
    /// allocation fraction (`GgvConstantAllocation.fraction` or proportional ratio).
    /// Limited to the participant's actual consumption (§42c cap clause).
    pub allocated_kwh: Decimal,

    /// Total generation from the community's shared plant (kWh).
    ///
    /// Optional — for invoice display and audit trail only.
    #[serde(default)]
    pub total_plant_generation_kwh: Option<Decimal>,

    /// Participant's allocation fraction (0.0–1.0).
    ///
    /// Informational — used for invoice transparency only.
    #[serde(default)]
    pub allocation_fraction: Option<Decimal>,

    /// Community registration ID (from BNetzA Marktstammdatenregister).
    ///
    /// §42c Abs. 3 EnWG: communities must register with the BNetzA.
    /// The registration ID appears on invoices as a ZusatzAttribut.
    #[serde(default)]
    pub gemeinschaft_id: Option<String>,
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

    /// §14a Modul 2 — the controllable device's energy per Tarifstufe.
    ///
    /// The Netzbetreiber's time windows, not the supplier's HT/NT: the two
    /// gratings are set by different parties and rarely coincide, which is why
    /// this is not derived from `MeterInput`'s Zweitarif split.
    pub sect14a_modul2: Option<Sect14aModul2Verbrauch>,
    /// Natural gas consumption (GAS).
    pub gas: Option<GasMeterInput>,
    /// District heat / Fernwärme (WAERME).
    pub heat: Option<WaermeMeterInput>,
    /// Solar self-consumption / Mieterstrom / GGV (SOLAR) — simple single-rate path.
    pub solar: Option<SolarMeterInput>,
    /// §42b EEG 2023 (Solarpaket I) — GGV community solar hybrid billing.
    ///
    /// Use instead of (or in addition to) `solar` when the plant’s generation must be
    /// proportionally allocated among tenants. The `SolarProvider` will then generate
    /// **two** positions per tenant:
    /// - **PV portion**: `min(consumption, allocated_pv)` at the community solar rate
    /// - **Grid portion**: `max(0, consumption − allocated_pv)` at the regular electricity rate
    ///
    /// Computed via `GgvNutzungsplan::allocate(plant_generation_kwh)` in `billingd`.
    pub ggv_solar: Option<GgvSolarInput>,
    /// EEG feed-in meter data (simplified path — rates from TariffInput).
    pub eeg: Option<EegMeterInput>,
    /// Full EEG settlement via `eeg-billing` — set this for NB-side precision.
    ///
    /// When set, `EegProvider` calls `eeg_billing::calculate_settlement(eeg_full)`
    /// for version-aware §51/§52 rules. Supersedes `eeg` when both are present.
    ///
    /// Requires the `eeg` feature of this crate.
    #[cfg(feature = "eeg")]
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
    /// Prosumer meter data (PV self-consumption + grid draw).
    ///
    /// When set, `ElectricityProvider` uses the prosumer billing path:
    /// - Grid consumption is billed at full tariff (commodity + NNE + Stromsteuer)
    /// - Self-consumption is Stromsteuer-exempt (§9a Nr. 1 StromStG for ≤30 kWp)
    /// - NNE does NOT apply to self-consumed energy
    pub prosumer: Option<ProsumerMeterInput>,

    /// §41a Abs. 6 EnWG — annual savings comparison for dynamic tariff customers.
    ///
    /// When set, `DynamicElectricityProvider` renders a mandatory informational
    /// position on the annual invoice comparing actual dynamic costs against a
    /// reference fixed tariff (§41a Abs. 6 EnWG).
    pub sect41a_annual_comparison: Option<Sect41aAnnualComparison>,

    /// §42c EnWG Energy Sharing — allocated community energy for this customer.
    ///
    /// When set, `EnergyShareProvider` generates a credit position for the
    /// customer's share of locally produced community electricity.
    ///
    /// ## Data source
    ///
    /// Populated by `billingd` after querying the sharing community's allocation
    /// data from `edmd` (virtual meter with `GgvConstantAllocation` or
    /// `GgvProportionalAllocation` rule — same infrastructure as §42b GGV).
    pub energy_share: Option<EnergyShareMeterInput>,
}

// ── ProsumerMeterInput ────────────────────────────────────────────────────────

/// Prosumer meter data — combines grid consumption with PV self-consumption.
///
/// A prosumer simultaneously consumes electricity (partly from the grid,
/// partly from their own PV plant) and may export surplus generation to the grid.
///
/// ## LF billing scope (energy-billing)
///
/// The Lieferant bills **grid consumption** only. Self-consumption billing and
/// EEG feed-in remuneration (Einspeisevergütung) are handled by `eeg-billing`.
///
/// ## Stromsteuer exemption
///
/// §9a Nr. 1 StromStG exempts self-consumed electricity from plants ≤30 kWp
/// from Stromsteuer. This is applied automatically when `self_consumption_kwh > 0`.
///
/// ## Network charge exemption
///
/// Self-consumed electricity does not transit the public grid; therefore no
/// NNE (§14a StromNEV) applies to `self_consumption_kwh`.
///
/// ## Example
///
/// ```rust
/// use energy_billing::ProsumerMeterInput;
/// use rust_decimal::dec;
///
/// let m = ProsumerMeterInput {
///     grid_consumption_kwh: dec!(250),   // drawn from grid → full tariff
///     self_consumption_kwh: dec!(150),   // from own PV → Stromsteuer-exempt, no NNE
///     export_kwh: Some(dec!(100)),       // fed back to grid (via eeg-billing)
/// };
/// assert_eq!(m.total_consumption_kwh(), dec!(400));
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProsumerMeterInput {
    /// Electricity drawn from the public grid (kWh).
    ///
    /// Full tariff applies: Arbeitspreis + NNE + Stromsteuer.
    pub grid_consumption_kwh: Decimal,

    /// Electricity generated by the customer's PV plant and consumed on-site (kWh).
    ///
    /// - No NNE (does not transit the grid)
    /// - No Stromsteuer for ≤30 kWp plants (§9a Nr. 1 StromStG)
    /// - Appears as an informational invoice line showing the self-supply ratio
    pub self_consumption_kwh: Decimal,

    /// Electricity exported to the grid (kWh). Informational only.
    ///
    /// This quantity is handled by `eeg-billing` (EEG Einspeisevergütung),
    /// not by `energy-billing`. Included here so the retail invoice can show
    /// the complete energy balance to the customer (§41 EnWG transparency).
    #[serde(default)]
    pub export_kwh: Option<Decimal>,
}

impl ProsumerMeterInput {
    /// Total electricity consumption (grid + self, kWh).
    #[must_use]
    pub fn total_consumption_kwh(&self) -> Decimal {
        self.grid_consumption_kwh + self.self_consumption_kwh
    }

    /// Self-supply ratio (0.0–1.0): share of total consumption from own PV.
    #[must_use]
    pub fn self_supply_ratio(&self) -> Decimal {
        let total = self.total_consumption_kwh();
        if total.is_zero() {
            Decimal::ZERO
        } else {
            (self.self_consumption_kwh / total).min(Decimal::ONE)
        }
    }
}

/// §42b EEG 2023 (Solarpaket I, BGBl I 2024 Nr. 107) — GGV allocation for one tenant.
///
/// Use this for **Gemeinschaftliche Gebäudeversorgung** billing where the plant’s
/// generation is proportionally distributed among building participants.
/// The `SolarProvider` splits the tenant’s invoice into:
///
/// - **PV portion** (community solar at discounted GGV rate)
/// - **Grid portion** (residual demand from the public grid at standard electricity rate)
///
/// ## Computing allocations
///
/// ```rust
/// use energy_billing::{GgvNutzungsplan, GgvNutzungsplanEntry, GgvSolarInput};
/// use rust_decimal::dec;
///
/// let plan = GgvNutzungsplan(vec![
///     GgvNutzungsplanEntry { malo_id: "A".into(), fraction: dec!(0.60) },
///     GgvNutzungsplanEntry { malo_id: "B".into(), fraction: dec!(0.40) },
/// ]);
/// let plant_kwh = dec!(100);
/// let allocs = plan.allocate(plant_kwh); // [("A", 60), ("B", 40)]
///
/// let tenant_a = GgvSolarInput {
///     pv_allocated_kwh: dec!(60),
///     actual_consumption_kwh: dec!(80),   // needs 80, gets 60 PV + 20 grid
/// };
/// assert_eq!(tenant_a.pv_delivered_kwh(), dec!(60));
/// assert_eq!(tenant_a.grid_kwh(), dec!(20));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GgvSolarInput {
    /// PV energy allocated to this tenant via the GGV Nutzungsplan.
    ///
    /// = `plant_generation_kwh × tenant_fraction` (from `GgvNutzungsplan::allocate`).
    pub pv_allocated_kwh: Decimal,
    /// Actual metered energy consumption at this tenant’s delivery point.
    ///
    /// Sourced from `edmd` for the billing period.
    pub actual_consumption_kwh: Decimal,
}

impl GgvSolarInput {
    /// PV energy actually delivered to this tenant.
    ///
    /// Capped at the tenant’s consumption: a tenant cannot receive more PV than they use.
    #[must_use]
    pub fn pv_delivered_kwh(&self) -> Decimal {
        self.actual_consumption_kwh.min(self.pv_allocated_kwh)
    }

    /// Residual grid electricity needed beyond the PV allocation.
    ///
    /// This quantity is billed at the standard electricity (STROM) rate.
    #[must_use]
    pub fn grid_kwh(&self) -> Decimal {
        (self.actual_consumption_kwh - self.pv_allocated_kwh).max(Decimal::ZERO)
    }

    /// Fraction of this tenant’s consumption covered by community PV (0.0–1.0).
    ///
    /// Useful for §40a kilowattstundenpreis reporting and sustainability KPIs.
    #[must_use]
    pub fn pv_coverage_ratio(&self) -> Decimal {
        if self.actual_consumption_kwh <= Decimal::ZERO {
            return Decimal::ZERO;
        }
        (self.pv_delivered_kwh() / self.actual_consumption_kwh)
            .min(Decimal::ONE)
            .round_kfm(4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::dec;

    fn plan(fractions: &[(&str, &str)]) -> GgvNutzungsplan {
        GgvNutzungsplan(
            fractions
                .iter()
                .map(|(id, f)| GgvNutzungsplanEntry {
                    malo_id: (*id).to_owned(),
                    fraction: f.parse().unwrap(),
                })
                .collect(),
        )
    }

    /// Σ(allocated) must always equal total_kwh exactly.
    #[test]
    fn allocate_sum_equals_total() {
        let p = plan(&[("A", "0.333"), ("B", "0.333"), ("C", "0.334")]);
        let total = dec!(100.000);
        let allocs = p.allocate(total);
        let sum: Decimal = allocs.iter().map(|(_, k)| k).sum();
        assert_eq!(sum, total, "sum must equal total exactly");
    }

    /// With 3 equal tenants the old "dump remainder on last" method would give
    /// last tenant 0.001 kWh extra. LRM distributes evenly.
    #[test]
    fn allocate_lrm_distributes_evenly_not_just_last_entry() {
        // 3 equal tenants, 100.001 kWh → exact share = 33.333666…
        // floor 3dp = 33.333 each → 1 leftover unit (0.001 kWh)
        // LRM: give it to whichever has highest fractional part (they're equal, so first)
        // Old naive: last tenant gets all of it
        let p = plan(&[("A", "0.3333"), ("B", "0.3333"), ("C", "0.3334")]);
        let total = dec!(100.000);
        let allocs = p.allocate(total);

        // All within ±0.001 of their exact share
        for (id, kwh) in &allocs {
            let fraction: Decimal = p.0.iter().find(|e| &e.malo_id == id).unwrap().fraction;
            let exact = total * fraction;
            let diff = (kwh - exact).abs();
            assert!(
                diff <= dec!(0.001),
                "{id}: allocated {kwh}, exact {exact}, diff {diff} > 0.001"
            );
        }

        let sum: Decimal = allocs.iter().map(|(_, k)| k).sum();
        assert_eq!(sum, total);
    }

    /// Many tenants: no single tenant should absorb disproportionate error.
    #[test]
    fn allocate_lrm_no_disproportionate_last_entry() {
        // 10 equal tenants, 1000.001 kWh → each gets 100.0001 → floor = 100.000
        // 1 leftover 0.001 unit
        let tenants: Vec<(String, String)> = (0..10)
            .map(|i| (format!("T{i}"), "0.1".to_owned()))
            .collect();
        let p = GgvNutzungsplan(
            tenants
                .iter()
                .map(|(id, f)| GgvNutzungsplanEntry {
                    malo_id: id.clone(),
                    fraction: f.parse().unwrap(),
                })
                .collect(),
        );
        let total = dec!(1000.001);
        let allocs = p.allocate(total);

        // With old naive: T9 (last) gets 100.001, others get 100.000
        // With LRM: one tenant gets 100.001, the rest get 100.000 — but it's
        // the one with the highest fractional part, not necessarily the last.
        let over_base: Vec<_> = allocs.iter().filter(|(_, k)| *k > dec!(100.000)).collect();
        assert_eq!(
            over_base.len(),
            1,
            "exactly 1 tenant should get the extra 0.001"
        );

        let sum: Decimal = allocs.iter().map(|(_, k)| k).sum();
        assert_eq!(sum, total);
    }
}

// ── Sect14aModul2Verbrauch ────────────────────────────────────────────────────

/// Energy per §14a Modul 2 Tarifstufe (BK6-22-300 Anlage 2 §2).
///
/// All three bands are present by construction — a Modul 2 metering
/// configuration reports every window, and a zero band is a real zero, not an
/// absent one.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct Sect14aModul2Verbrauch {
    /// Hochtarif energy in kWh.
    pub ht_kwh: Decimal,
    /// Standardtarif energy in kWh.
    pub st_kwh: Decimal,
    /// Niedertarif energy in kWh.
    pub nt_kwh: Decimal,
}

// ── Abschlagsplan ─────────────────────────────────────────────────────────────

/// One scheduled advance payment entry (Abschlag) in an Abschlagsplan.
///
/// Advance payments must be based on the estimated annual consumption.
/// When the operator changes the Abschlag amount, customers must be notified
/// with adequate lead time per §41 Abs. 1 Nr. 6 EnWG.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AbschlagsplanEntry {
    /// Payment due date.
    pub faellig_am: time::Date,
    /// Amount to collect in EUR (brutto, i.e. inclusive of MwSt).
    pub betrag_eur: Decimal,
    /// Optional display label (e.g. `"Abschlag Januar 2026"`).
    #[serde(default)]
    pub beschreibung: Option<String>,
}

/// Complete advance payment schedule for a customer contract.
///
/// Provides the statutory context (estimated annual cost and consumption) to
/// satisfy §41 Abs. 1 Nr. 6 EnWG requirements.
///
/// ## Legal basis
///
/// §41 Abs. 1 Nr. 6 EnWG: the invoice must show the current and planned
/// advance payment amounts and collection dates.
///
/// ## Example — generate a 12-month uniform schedule
///
/// ```rust
/// use energy_billing::Abschlagsplan;
/// use rust_decimal::dec;
/// use time::macros::date;
///
/// let plan = Abschlagsplan::monthly_uniform(
///     "51238696781",
///     date!(2026-01-01),
///     12,
///     dec!(1440.00), // annual brutto estimate
///     dec!(3600),    // annual kWh estimate
/// );
/// assert_eq!(plan.entries.len(), 12);
/// assert_eq!(plan.entries[0].betrag_eur, dec!(120.00));
/// ```
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Abschlagsplan {
    /// Market location this plan belongs to.
    pub malo_id: String,
    /// Contract reference for ERP routing.
    #[serde(default)]
    pub contract_id: Option<String>,
    /// Scheduled advance payment entries in chronological order.
    pub entries: Vec<AbschlagsplanEntry>,
    /// Annual consumption estimate used to derive the plan (kWh).
    pub jahresverbrauch_schaetzung_kwh: Decimal,
    /// Annual cost estimate used to derive the plan (EUR brutto).
    pub jahreskosten_schaetzung_eur: Decimal,
}

impl Abschlagsplan {
    /// Build a uniform monthly advance payment plan for `months` months.
    ///
    /// The annual amount is **distributed exactly** over a 12-month cycle via
    /// [`billing::Amount::distribute`] (largest-remainder): any 12 consecutive
    /// instalments sum to precisely `annual_brutto_eur` at cent precision.
    /// Naïve `round(annual / 12)` per month drifts up to 6 ct per year — a
    /// reconciliation gap §13 Abs. 3 StromGVV's refund duty would surface on
    /// every Jahresrechnung.
    #[must_use]
    pub fn monthly_uniform(
        malo_id: impl Into<String>,
        start_date: time::Date,
        months: u32,
        annual_brutto_eur: Decimal,
        jahresverbrauch_kwh: Decimal,
    ) -> Self {
        use rust_decimal::dec;
        // Exact-sum 12-month cycle; conversion failure (absurd magnitude)
        // degrades to the plain division, never to a panic.
        let cycle: Vec<Decimal> = billing::Amount::<2>::checked_from_decimal(annual_brutto_eur)
            .and_then(|a| a.distribute(12))
            .map(|parts| {
                parts
                    .into_iter()
                    .map(billing::Amount::into_decimal)
                    .collect()
            })
            .unwrap_or_else(|_| vec![(annual_brutto_eur / dec!(12)).round_kfm(2); 12]);
        let entries = (0..months)
            .filter_map(|i| {
                let total_months = start_date.month() as u32 - 1 + i;
                let year = start_date.year() + (total_months / 12) as i32;
                let month_idx = (total_months % 12 + 1) as u8;
                let month = time::Month::try_from(month_idx).ok()?;
                let max_day = month.length(time::util::is_leap_year(year) as i32) as u8;
                let day = start_date.day().min(max_day);
                let date = time::Date::from_calendar_date(year, month, day).ok()?;
                Some(AbschlagsplanEntry {
                    faellig_am: date,
                    betrag_eur: cycle[(i % 12) as usize],
                    beschreibung: Some(format!("Abschlag {:02}/{}", month as u8, year)),
                })
            })
            .collect();
        Self {
            malo_id: malo_id.into(),
            contract_id: None,
            entries,
            jahresverbrauch_schaetzung_kwh: jahresverbrauch_kwh,
            jahreskosten_schaetzung_eur: annual_brutto_eur,
        }
    }

    /// Sum of all scheduled advance payment amounts.
    #[must_use]
    pub fn total_eur(&self) -> Decimal {
        self.entries.iter().map(|e| e.betrag_eur).sum()
    }
}

#[cfg(test)]
mod abschlagsplan_tests {
    use super::*;
    use rust_decimal::dec;
    use time::macros::date;

    #[test]
    fn monthly_uniform_12_months() {
        let plan = Abschlagsplan::monthly_uniform(
            "51238696781",
            date!(2026 - 01 - 01),
            12,
            dec!(1440.00),
            dec!(3600),
        );
        assert_eq!(plan.entries.len(), 12);
        assert_eq!(plan.entries[0].betrag_eur, dec!(120.00));
        assert_eq!(plan.entries[11].faellig_am.year(), 2026);
        assert_eq!(plan.total_eur(), dec!(1440.00));
    }

    #[test]
    fn monthly_uniform_distributes_indivisible_annual_exactly() {
        // 1000.00 / 12 = 83.333… — naïve per-month rounding gives
        // 12 × 83.33 = 999.96, a 4 ct gap the Jahresrechnung would have to
        // reconcile. Largest-remainder distribution closes it.
        let plan = Abschlagsplan::monthly_uniform(
            "51238696781",
            date!(2026 - 01 - 01),
            12,
            dec!(1000.00),
            dec!(2500),
        );
        assert_eq!(plan.total_eur(), dec!(1000.00), "instalments sum exactly");
        // Every instalment is within one cent of the uniform value.
        for e in &plan.entries {
            assert!(
                e.betrag_eur == dec!(83.33) || e.betrag_eur == dec!(83.34),
                "uniform ± 1 ct, got {}",
                e.betrag_eur
            );
        }
        // A 24-month plan sums to exactly two annual amounts.
        let two_years = Abschlagsplan::monthly_uniform(
            "51238696781",
            date!(2026 - 01 - 01),
            24,
            dec!(1000.00),
            dec!(2500),
        );
        assert_eq!(two_years.total_eur(), dec!(2000.00));
    }

    #[test]
    fn monthly_uniform_crosses_year_boundary() {
        let plan = Abschlagsplan::monthly_uniform(
            "51238696782",
            date!(2025 - 07 - 01),
            12,
            dec!(1200.00),
            dec!(3000),
        );
        assert_eq!(plan.entries.len(), 12);
        // July 2025 → June 2026
        assert_eq!(plan.entries[0].faellig_am.month(), time::Month::July);
        assert_eq!(plan.entries[0].faellig_am.year(), 2025);
        assert_eq!(plan.entries[5].faellig_am.month(), time::Month::December);
        assert_eq!(plan.entries[6].faellig_am.month(), time::Month::January);
        assert_eq!(plan.entries[6].faellig_am.year(), 2026);
    }
}
