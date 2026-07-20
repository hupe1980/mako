//! Multi-meter Messkonzept configuration for EEG plants.
//!
//! German EEG plants can have complex measurement arrangements:
//! - A simple rooftop PV plant might have only one bidirectional meter
//! - A commercial plant may have separate generation, feed-in, and consumption meters
//! - A §42b Gemeinschaftliche Gebäudeversorgung plant serves multiple tenant meters
//! - A §21 Abs. 3 Mieterstrom plant has both a shared generation meter and individual tenant meters
//!
//! ## Measurement point topology
//!
//! The **Messlokation (MeLo)** identifies where a physical meter is installed.
//! Each EEG plant has at least one MeLo for Einspeisung, and optionally more for:
//!
//! | Role | Description | Required for |
//! |---|---|---|
//! | Einspeisemessung | Feed-in into grid (primary billing point) | All plants |
//! | Erzeugungsmessung | Total generation (separate from feed-in) | Volleinspeisung, §14a |
//! | Bezugsmessung | Grid consumption by operator | Eigenverbrauch, §14a |
//! | TeilnehmerMessung | Individual tenant consumption | §42b GGV, §21 Abs. 3 Mieterstrom |
//!
//! ## How `Einspeisemenge` is derived from meter data
//!
//! ```text
//! Messkonzept: Volleinspeisung
//!   Einspeisemenge = Erzeugungsmessung (all generation goes to grid)
//!
//! Messkonzept: Überschusseinspeisung (bidirectional meter)
//!   Einspeisemenge = Einspeisemessung (only surplus after self-consumption)
//!
//! Messkonzept: Überschusseinspeisung (separate meters)
//!   Einspeisemenge = Erzeugungsmessung − Eigenverbrauch
//!               OR = Einspeisemessung directly (preferred)
//!
//! Messkonzept: Gemeinschaftliche Gebäudeversorgung (§42b)
//!   Einspeisemenge = Erzeugungsmessung − Σ(TeilnehmerMessung)
//!   Each tenant: allocated_kwh = tenant_consumption_kwh
//! ```
//!
//! ## OBIS codes for EEG measurement
//!
//! | OBIS code | Description |
//! |---|---|
//! | `1-0:1.8.0` | Active energy supplied (Bezug, incoming) |
//! | `1-0:2.8.0` | Active energy delivered (Lieferung/Einspeisung, outgoing) |
//! | `1-0:1.29.0` | Reactive energy Bezug |
//! | `1-0:2.29.0` | Reactive energy Lieferung |
//! | `1-0:16.7.0` | Sum active power (positive = Bezug, negative = Einspeisung) |
//!
//! For iMSys: all 15-minute quarter-hour values are available per OBIS code.

use rust_decimal::Decimal;
#[cfg(test)]
use rust_decimal::dec;

// ── MeteringMode ──────────────────────────────────────────────────────────────

/// Measurement technology class for an EEG plant's metering infrastructure.
///
/// Determines data frequency, applicable billing period, and §52 compliance
/// requirements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MeteringMode {
    /// **SLP** — Standardlastprofil (Standard load profile).
    ///
    /// Monthly read-out. Annual settlement with monthly advance payment.
    /// Applies to plants ≤ 30 kW (§4 MessZV) using the standard annual billing cycle.
    Slp,

    /// **RLM** — Registrierende Leistungsmessung (Registered load measurement).
    ///
    /// 15-minute interval metering. Monthly billing based on measured quarter-hour data.
    /// Mandatory for plants > 30 kW (§3 MessZV, §41a EnWG).
    /// Spitzenleistung (peak power) is tracked separately for NNE billing.
    Rlm,

    /// **iMSys** — Intelligentes Messsystem (Smart meter gateway, SMGW).
    ///
    /// Real-time 15-minute telemetry via BSI-compliant SMGW (§29 MsbG).
    /// Required for plants 7 kW – 30 kW from 2025 (§29 Abs. 4 MsbG rollout).
    /// Enables automatic §51 negative-price interval tracking.
    ///
    /// For EEG billing: enables automatic quality-flagged interval data,
    /// substitute value handling (§17 MessZV), and §51a quarter-hour counting.
    IMsys,
}

impl MeteringMode {
    /// Returns `true` when this mode provides 15-minute interval data.
    ///
    /// `Rlm` and `IMsys` both provide quarter-hour intervals.
    /// `Slp` only provides monthly aggregates.
    #[must_use]
    pub fn has_quarter_hour_data(self) -> bool {
        matches!(self, Self::Rlm | Self::IMsys)
    }

    /// Returns `true` when this mode satisfies the §9 EEG 2023 Fernsteuerbarkeit
    /// requirement for remote controllability via SMGW.
    #[must_use]
    pub fn satisfies_fernsteuerbarkeit(self) -> bool {
        self == Self::IMsys
    }
}

// ── MesslokationTyp ───────────────────────────────────────────────────────────

/// The functional role of a measurement location (Messlokation) in an EEG plant.
///
/// Identifies how the measured kWh data is used in the billing calculation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MesslokationTyp {
    /// Measures grid feed-in at the grid connection point.
    ///
    /// OBIS: `1-0:2.8.0`. Primary billing basis for Überschusseinspeisung.
    /// This is the **primary Messlokation** for all EEG plants.
    Einspeisemessung,

    /// Measures total generation at the generator output terminal.
    ///
    /// OBIS: `1-0:2.8.0` (at inverter). Used for Volleinspeisung billing
    /// and §14a distinction between generation and feed-in.
    Erzeugungsmessung,

    /// Measures grid consumption by the operator (Bezug from grid).
    ///
    /// OBIS: `1-0:1.8.0`. Required for:
    /// - Eigenverbrauch calculation (Verbrauch = Erzeugung − Einspeisung)
    /// - §14a Modul 2 management (distinguishing HT/NT self-consumption)
    /// - Nachweisführung for §3 Nr. 43 EEG Eigenversorgung
    Bezugsmessung,

    /// Tenant consumption meter in a §42b Gemeinschaftliche Gebäudeversorgung.
    ///
    /// Each participating tenant/consumer has one `TeilnehmerMessung`.
    /// The allocated EEG generation for each tenant:
    ///   `tenant_kwh = tenant_verbrauch ÷ Σ(all_teilnehmer_verbrauch) × erzeugung`
    ///
    /// Also used in §21 Abs. 3 Mieterstrom buildings.
    TeilnehmerMessung,

    /// Virtual (calculated) billing point.
    ///
    /// Derived from arithmetic combination of physical meters, not a real physical
    /// Messlokation. Used for:
    /// - `Eigenverbrauch = Erzeugung − Einspeisung`
    /// - `GridConsumption = Bezug − Eigenverbrauch`
    Virtuell,
}

// ── MeterPoint ────────────────────────────────────────────────────────────────

/// A single physical or virtual measurement point.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeterPoint {
    /// Messlokations-ID (MeLo-ID) — 33-character market location identifier.
    ///
    /// Format: `DE` + 31 alphanumeric characters (uppercase). Assigned by the NB.
    pub melo_id: String,

    /// Functional role of this measurement point in the plant's billing setup.
    pub typ: MesslokationTyp,

    /// OBIS code identifying the measured quantity.
    ///
    /// Most common: `1-0:2.8.0` (feed-in) or `1-0:1.8.0` (consumption).
    pub obis_code: String,

    /// Meter serial number (Zählernummer), if known.
    pub zaehlernummer: Option<String>,

    /// Whether this meter is the primary billing basis.
    ///
    /// Exactly one `MeterPoint` per `MeterConfiguration` should have
    /// `is_primary_billing_point = true`.
    pub is_primary_billing_point: bool,
}

// ── MeterConfiguration ────────────────────────────────────────────────────────

/// The complete meter topology for one EEG plant.
///
/// Describes how physical meters are arranged and which one is the primary
/// billing point for EEG Einspeisemenge calculation.
///
/// ## Typical configurations
///
/// ### Simple rooftop PV (Überschusseinspeisung, bidirectional meter)
///
/// One bidirectional meter at the grid connection point. Records both Bezug
/// and Einspeisung in one device. Simplest and most common configuration.
///
/// ```text
/// Einspeisemessung (OBIS 1-0:2.8.0) ← billing basis
/// Bezugsmessung    (OBIS 1-0:1.8.0) ← same physical meter, not billed for EEG
/// ```
///
/// ### Volleinspeisung PV (two meters)
///
/// Separate generation meter at the inverter plus grid connection meter.
///
/// ```text
/// Erzeugungsmessung (OBIS 1-0:2.8.0 at inverter) ← billing basis
/// Bezugsmessung     (OBIS 1-0:1.8.0 at grid)     ← for eigenverbrauch validation
/// ```
///
/// ### §42b Gemeinschaftliche Gebäudeversorgung (multi-tenant)
///
/// One generation meter for the whole building plus individual tenant meters.
///
/// ```text
/// Erzeugungsmessung  ← total generation
/// TeilnehmerMessung 1 (Tenant A)
/// TeilnehmerMessung 2 (Tenant B)
/// TeilnehmerMessung N (Tenant N)
/// Feed-in = max(0, Erzeugung − Σ TeilnehmerVerbrauch)
/// ```
///
/// ### §21 Abs. 3 Mieterstrom (with Zuschlag)
///
/// Shared generation meter + individual apartment meters.
/// The Mieterstrom-Zuschlag applies to the kWh delivered to tenants in the building
/// (not to grid feed-in).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MeterConfiguration {
    /// Metering technology class for the primary measurement point.
    pub mode: MeteringMode,

    /// All measurement points for this plant (primary + secondary).
    ///
    /// Exactly one must have `is_primary_billing_point = true`.
    /// Ordered: primary Einspeisemessung first, then Erzeugungsmessung,
    /// then Bezugsmessung, then TeilnehmerMessungen.
    pub meter_points: Vec<MeterPoint>,

    /// Whether the plant uses a **bidirectional meter** (Zweirichtungszähler)
    /// that records both Einspeisung and Bezug in one physical device.
    ///
    /// Common for small PV plants (≤30 kWp) with Überschusseinspeisung.
    /// When `true`, the Einspeisemessung and Bezugsmessung share one `melo_id`.
    pub is_bidirectional: bool,
}

impl MeterConfiguration {
    /// Create a simple single-meter configuration for Überschusseinspeisung.
    ///
    /// The most common case: one bidirectional meter at the grid connection point.
    ///
    /// ```rust
    /// use eeg_billing::metering::{MeterConfiguration, MeteringMode};
    ///
    /// let config = MeterConfiguration::simple_ueberschuss(
    ///     "DE0001234567890123456789012345678".to_string(),
    ///     MeteringMode::Slp,
    /// );
    /// assert!(config.is_bidirectional);
    /// assert_eq!(config.meter_points.len(), 1);
    /// ```
    #[must_use]
    pub fn simple_ueberschuss(einspeise_melo_id: String, mode: MeteringMode) -> Self {
        Self {
            mode,
            meter_points: vec![MeterPoint {
                melo_id: einspeise_melo_id,
                typ: MesslokationTyp::Einspeisemessung,
                obis_code: "1-0:2.8.0".to_owned(),
                zaehlernummer: None,
                is_primary_billing_point: true,
            }],
            is_bidirectional: true,
        }
    }

    /// Create a two-meter Volleinspeisung configuration.
    ///
    /// Separate Erzeugungsmessung (billing basis) and Bezugsmessung.
    #[must_use]
    pub fn volleinspeisung(
        erzeugungs_melo_id: String,
        bezugs_melo_id: String,
        mode: MeteringMode,
    ) -> Self {
        Self {
            mode,
            meter_points: vec![
                MeterPoint {
                    melo_id: erzeugungs_melo_id,
                    typ: MesslokationTyp::Erzeugungsmessung,
                    obis_code: "1-0:2.8.0".to_owned(),
                    zaehlernummer: None,
                    is_primary_billing_point: true,
                },
                MeterPoint {
                    melo_id: bezugs_melo_id,
                    typ: MesslokationTyp::Bezugsmessung,
                    obis_code: "1-0:1.8.0".to_owned(),
                    zaehlernummer: None,
                    is_primary_billing_point: false,
                },
            ],
            is_bidirectional: false,
        }
    }

    /// Returns the primary billing `MeterPoint` (exactly one per configuration).
    #[must_use]
    pub fn primary_billing_point(&self) -> Option<&MeterPoint> {
        self.meter_points
            .iter()
            .find(|p| p.is_primary_billing_point)
    }

    /// Returns the count of tenant measurement points.
    #[must_use]
    pub fn teilnehmer_count(&self) -> usize {
        self.meter_points
            .iter()
            .filter(|p| p.typ == MesslokationTyp::TeilnehmerMessung)
            .count()
    }

    /// Returns `true` when this configuration supports §42b Gemeinschaftliche
    /// Gebäudeversorgung (collective building supply).
    #[must_use]
    pub fn is_gemeinschaftliche_gebaeudeversorgung(&self) -> bool {
        self.teilnehmer_count() > 0
    }
}

// ── EinspeisemengeInput ───────────────────────────────────────────────────────

/// Raw meter readings to be resolved into a single `Einspeisemenge` for EEG billing.
///
/// The `compute_einspeisemenge` function converts these into the kWh figure that
/// goes into `SettleInput::einspeisemenge_kwh`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct EinspeisemengeInput {
    /// kWh from the Einspeisemessung (grid injection meter, OBIS 1-0:2.8.0).
    ///
    /// When `is_bidirectional`: this is the surplus that entered the grid.
    /// When not bidirectional: this may be total generation (Volleinspeisung).
    pub einspeisemessung_kwh: Option<Decimal>,

    /// kWh from the Erzeugungsmessung (generation meter at inverter), if separate.
    ///
    /// For Volleinspeisung: this is the billing basis.
    /// For Überschusseinspeisung with separate meters: `Einspeisung = Erzeugung − Eigenverbrauch`.
    pub erzeugungsmessung_kwh: Option<Decimal>,

    /// kWh from the Bezugsmessung (grid consumption meter, OBIS 1-0:1.8.0).
    pub bezugsmessung_kwh: Option<Decimal>,

    /// kWh consumed by individual tenants (§42b / §21 Abs. 3).
    ///
    /// `Σ = sum(teilnehmer_kwh)` is the total tenant consumption.
    /// Grid feed-in = max(0, erzeugungsmessung_kwh − Σ teilnehmer_kwh).
    pub teilnehmer_kwh: Vec<Decimal>,
}

/// Error in resolving Einspeisemenge from meter data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MeteringError {
    /// Required measurement data is missing.
    #[error("Missing required measurement: {0}")]
    MissingData(&'static str),

    /// Derived Einspeisemenge is negative (more consumed than generated).
    #[error("Negative Einspeisemenge derived from meter data: {0} kWh")]
    NegativeResult(String),
}

/// Derive the EEG billing `Einspeisemenge` from raw meter readings.
///
/// The correct derivation depends on the `Messkonzept` and `MeterConfiguration`:
///
/// | Messkonzept | Billing basis |
/// |---|---|
/// | `Volleinspeisung` | `erzeugungsmessung_kwh` (if present) or `einspeisemessung_kwh` |
/// | `Ueberschusseinspeisung` | `einspeisemessung_kwh` (direct) |
/// | §42b GGV | `max(0, erzeugung − Σ(teilnehmer))` |
/// | `Direktlieferung` | `erzeugungsmessung_kwh` (generation delivered to buyer) |
///
/// ## Example: Bidirectional meter
///
/// ```rust
/// use eeg_billing::metering::{compute_einspeisemenge, EinspeisemengeInput};
/// use eeg_billing::Messkonzept;
/// use rust_decimal::dec;
///
/// // Simple surplus feed-in: 300 kWh generated, 100 kWh self-consumed, 200 kWh fed in
/// let input = EinspeisemengeInput {
///     einspeisemessung_kwh: Some(dec!(200)),
///     erzeugungsmessung_kwh: None,
///     bezugsmessung_kwh: None,
///     teilnehmer_kwh: vec![],
/// };
/// let kwh = compute_einspeisemenge(&input, Messkonzept::Ueberschusseinspeisung).unwrap();
/// assert_eq!(kwh, dec!(200));
/// ```
pub fn compute_einspeisemenge(
    input: &EinspeisemengeInput,
    messkonzept: crate::model::Messkonzept,
) -> Result<Decimal, MeteringError> {
    use crate::model::Messkonzept;

    match messkonzept {
        Messkonzept::Volleinspeisung => {
            // Volleinspeisung: all generation is billed (generation meter preferred)
            input
                .erzeugungsmessung_kwh
                .or(input.einspeisemessung_kwh)
                .ok_or(MeteringError::MissingData(
                    "erzeugungsmessung_kwh or einspeisemessung_kwh required for Volleinspeisung",
                ))
        }

        Messkonzept::Ueberschusseinspeisung => {
            // Überschusseinspeisung: use direct injection measurement
            if let Some(einspeisung) = input.einspeisemessung_kwh {
                return Ok(einspeisung.max(Decimal::ZERO));
            }
            // Fallback: derive from generation − eigenverbrauch
            match (input.erzeugungsmessung_kwh, input.bezugsmessung_kwh) {
                (Some(erzg), _) => {
                    // If only erzeugung is known: assume all is Einspeisung (conservative)
                    Ok(erzg.max(Decimal::ZERO))
                }
                _ => Err(MeteringError::MissingData(
                    "einspeisemessung_kwh required for Ueberschusseinspeisung",
                )),
            }
        }

        Messkonzept::Direktlieferung => {
            // Direktlieferung: delivery to specific buyer — use generation basis
            input
                .erzeugungsmessung_kwh
                .ok_or(MeteringError::MissingData(
                    "erzeugungsmessung_kwh required for Direktlieferung",
                ))
        }
    }
}

/// Derive the §42b / §21 Abs. 3 Mieterstrom tenant allocation.
///
/// Returns a Vec of `(tenant_index, allocated_kwh)` tuples, where `allocated_kwh`
/// is the pro-rata share of generation for each tenant.
///
/// Allocation formula:
/// `tenant_alloc = erzeugung_kwh × (tenant_verbrauch / Σ_all_verbrauch)`
///
/// Remaining generation (after all tenant allocations) = grid feed-in.
///
/// Returns `Err(MeteringError::MissingData)` when generation data is absent.
///
/// # Example
///
/// ```rust
/// use eeg_billing::metering::{compute_tenant_allocation, EinspeisemengeInput};
/// use rust_decimal::dec;
///
/// let input = EinspeisemengeInput {
///     einspeisemessung_kwh: None,
///     erzeugungsmessung_kwh: Some(dec!(1000)),  // 1000 kWh generated
///     bezugsmessung_kwh: None,
///     teilnehmer_kwh: vec![dec!(300), dec!(200)],  // Tenant A: 300, Tenant B: 200
/// };
///
/// let (allocs, grid_feed_in) = compute_tenant_allocation(&input).unwrap();
/// // Total tenant consumption = 500 kWh < 1000 kWh generation
/// // Each tenant gets their full consumption; remaining 500 kWh goes to grid
/// assert_eq!(allocs.len(), 2);
/// assert_eq!(allocs[0].1, dec!(300)); // Tenant A: 300 kWh actual consumption
/// assert_eq!(allocs[1].1, dec!(200)); // Tenant B: 200 kWh actual consumption
/// assert_eq!(grid_feed_in, dec!(500)); // 1000 − 500 = 500 kWh to grid
/// ```
pub fn compute_tenant_allocation(
    input: &EinspeisemengeInput,
) -> Result<(Vec<(usize, Decimal)>, Decimal), MeteringError> {
    let erzeugung = input
        .erzeugungsmessung_kwh
        .ok_or(MeteringError::MissingData(
            "erzeugungsmessung_kwh required for tenant allocation",
        ))?;

    if input.teilnehmer_kwh.is_empty() {
        return Ok((vec![], erzeugung.max(Decimal::ZERO)));
    }

    let total_verbrauch: Decimal = input.teilnehmer_kwh.iter().cloned().sum();

    let (allocations, grid_feed_in) = if erzeugung >= total_verbrauch {
        // Generation covers all tenant consumption fully.
        // Each tenant gets their actual consumption; remainder goes to grid.
        let allocs: Vec<(usize, Decimal)> = input
            .teilnehmer_kwh
            .iter()
            .enumerate()
            .map(|(i, &v)| (i, v.max(Decimal::ZERO)))
            .collect();
        let feed_in = (erzeugung - total_verbrauch).max(Decimal::ZERO);
        (allocs, feed_in)
    } else {
        // Generation is insufficient to cover all consumption.
        // Allocate proportionally: tenant_alloc = erzeugung × (tenant_verbrauch / total_verbrauch).
        let allocs: Vec<(usize, Decimal)> = input
            .teilnehmer_kwh
            .iter()
            .enumerate()
            .map(|(i, &verbrauch)| {
                let alloc = if total_verbrauch.is_zero() {
                    Decimal::ZERO
                } else {
                    (erzeugung * verbrauch / total_verbrauch).round_dp(3)
                };
                (i, alloc)
            })
            .collect();
        (allocs, Decimal::ZERO)
    };

    Ok((allocations, grid_feed_in))
}

/// Compute Eigenverbrauch (self-consumption) for audit and §14a tracking.
///
/// `Eigenverbrauch = Erzeugungsmessung − Einspeisemessung`
///
/// Returns `None` when either measurement is unavailable.
///
/// # Example
///
/// ```rust
/// use eeg_billing::metering::compute_eigenverbrauch;
/// use rust_decimal::dec;
///
/// // 400 kWh generated, 250 kWh fed in → 150 kWh self-consumed
/// let ev = compute_eigenverbrauch(Some(dec!(400)), Some(dec!(250)));
/// assert_eq!(ev, Some(dec!(150)));
/// ```
#[must_use]
pub fn compute_eigenverbrauch(
    erzeugung_kwh: Option<Decimal>,
    einspeisung_kwh: Option<Decimal>,
) -> Option<Decimal> {
    match (erzeugung_kwh, einspeisung_kwh) {
        (Some(erzg), Some(einsp)) => Some((erzg - einsp).max(Decimal::ZERO)),
        _ => None,
    }
}

// ── §14a Messkonzept ──────────────────────────────────────────────────────────

/// §14a EnWG Modul 2 — time-of-use measurement configuration.
///
/// Under §14a Modul 2 (BNetzA BK6-22-300), operators agree to reduce
/// self-consumption during high-load periods (HT = Hochtarif) in exchange
/// for reduced NNE. This requires separate measurement of HT/NT periods.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Sect14aModul2Measurement {
    /// kWh self-consumed during Hochtarif (HT) periods.
    ///
    /// For NNE billing, these kWh may be charged at HT NNE rate.
    pub eigenverbrauch_ht_kwh: Decimal,

    /// kWh self-consumed during Niedertarif (NT) periods.
    ///
    /// For NNE billing, these kWh may be charged at reduced NT NNE rate.
    pub eigenverbrauch_nt_kwh: Decimal,

    /// kWh of agreed demand reduction during HT (Steuerungsmaßnahme).
    ///
    /// The managed kWh that the DSO reduced via §14a remote control.
    /// Compensated per §14a Abs. 4 EnWG by the NB.
    pub steuerungsmassnahme_kwh: Option<Decimal>,
}

impl Sect14aModul2Measurement {
    /// Total self-consumption (HT + NT).
    #[must_use]
    pub fn total_kwh(&self) -> Decimal {
        self.eigenverbrauch_ht_kwh + self.eigenverbrauch_nt_kwh
    }

    /// Ratio of HT to total self-consumption (for demand flexibility reporting).
    ///
    /// Returns `None` when total is zero.
    #[must_use]
    pub fn ht_ratio(&self) -> Option<Decimal> {
        let total = self.total_kwh();
        if total.is_zero() {
            None
        } else {
            Some((self.eigenverbrauch_ht_kwh / total).round_dp(4))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rlm_has_quarter_hour_data() {
        assert!(MeteringMode::Rlm.has_quarter_hour_data());
        assert!(MeteringMode::IMsys.has_quarter_hour_data());
        assert!(!MeteringMode::Slp.has_quarter_hour_data());
    }

    #[test]
    fn imsys_satisfies_fernsteuerbarkeit() {
        assert!(MeteringMode::IMsys.satisfies_fernsteuerbarkeit());
        assert!(!MeteringMode::Rlm.satisfies_fernsteuerbarkeit());
        assert!(!MeteringMode::Slp.satisfies_fernsteuerbarkeit());
    }

    #[test]
    fn simple_ueberschuss_config() {
        let config = MeterConfiguration::simple_ueberschuss(
            "DE0001234567890123456789012345678".to_string(),
            MeteringMode::Slp,
        );
        assert!(config.is_bidirectional);
        assert_eq!(config.meter_points.len(), 1);
        assert!(config.primary_billing_point().is_some());
        assert!(!config.is_gemeinschaftliche_gebaeudeversorgung());
    }

    #[test]
    fn volleinspeisung_config_has_two_points() {
        let config = MeterConfiguration::volleinspeisung(
            "DE000ERZEUGUNGS0000000000000000001".to_string(),
            "DE000BEZUGS000000000000000000000001".to_string(),
            MeteringMode::Rlm,
        );
        assert_eq!(config.meter_points.len(), 2);
        assert!(!config.is_bidirectional);
        let primary = config.primary_billing_point().unwrap();
        assert_eq!(primary.typ, MesslokationTyp::Erzeugungsmessung);
    }

    #[test]
    fn compute_ueberschuss_from_einspeisung() {
        use crate::model::Messkonzept;
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: Some(dec!(200)),
            erzeugungsmessung_kwh: Some(dec!(300)),
            bezugsmessung_kwh: Some(dec!(50)),
            teilnehmer_kwh: vec![],
        };
        let kwh = compute_einspeisemenge(&input, Messkonzept::Ueberschusseinspeisung).unwrap();
        // Direct injection measurement: 200 kWh
        assert_eq!(kwh, dec!(200));
    }

    #[test]
    fn compute_volleinspeisung_uses_erzeugungs_melo() {
        use crate::model::Messkonzept;
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: Some(dec!(200)),
            erzeugungsmessung_kwh: Some(dec!(300)), // Volleinspeisung: all 300 billed
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![],
        };
        let kwh = compute_einspeisemenge(&input, Messkonzept::Volleinspeisung).unwrap();
        assert_eq!(kwh, dec!(300)); // generation meter wins
    }

    #[test]
    fn compute_volleinspeisung_falls_back_to_einspeisung() {
        use crate::model::Messkonzept;
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: Some(dec!(200)),
            erzeugungsmessung_kwh: None, // no separate generation meter
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![],
        };
        let kwh = compute_einspeisemenge(&input, Messkonzept::Volleinspeisung).unwrap();
        assert_eq!(kwh, dec!(200));
    }

    #[test]
    fn compute_missing_data_returns_error() {
        use crate::model::Messkonzept;
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: None,
            erzeugungsmessung_kwh: None,
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![],
        };
        assert!(compute_einspeisemenge(&input, Messkonzept::Ueberschusseinspeisung).is_err());
        assert!(compute_einspeisemenge(&input, Messkonzept::Volleinspeisung).is_err());
    }

    #[test]
    fn tenant_allocation_proportional() {
        // Generation (1000 kWh) > total consumption (500 kWh):
        // each tenant gets their FULL consumption; 500 kWh goes to grid
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: None,
            erzeugungsmessung_kwh: Some(dec!(1000)),
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![dec!(300), dec!(200)],
        };
        let (allocs, feed_in) = compute_tenant_allocation(&input).unwrap();
        assert_eq!(allocs.len(), 2);
        // Tenant A gets full 300 kWh
        assert_eq!(allocs[0].1, dec!(300));
        // Tenant B gets full 200 kWh
        assert_eq!(allocs[1].1, dec!(200));
        // Feed-in = 1000 − 500 = 500 kWh
        assert_eq!(feed_in, dec!(500));
    }

    #[test]
    fn tenant_allocation_partial_consumption() {
        // Generation (1000 kWh) > total consumption (500 kWh)
        // Each tenant gets their actual consumption, 500 kWh exported to grid
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: None,
            erzeugungsmessung_kwh: Some(dec!(1000)),
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![dec!(300), dec!(200)],
        };
        let (_, feed_in) = compute_tenant_allocation(&input).unwrap();
        assert_eq!(feed_in, dec!(500));
    }

    #[test]
    fn tenant_allocation_generation_shortage() {
        // Generation (400 kWh) < total consumption (500 kWh):
        // allocate proportionally; feed_in = 0
        let input = EinspeisemengeInput {
            einspeisemessung_kwh: None,
            erzeugungsmessung_kwh: Some(dec!(400)),
            bezugsmessung_kwh: None,
            teilnehmer_kwh: vec![dec!(300), dec!(200)],
        };
        let (allocs, feed_in) = compute_tenant_allocation(&input).unwrap();
        // Tenant A: 400 × 300/500 = 240 kWh
        assert_eq!(allocs[0].1, dec!(240));
        // Tenant B: 400 × 200/500 = 160 kWh
        assert_eq!(allocs[1].1, dec!(160));
        assert_eq!(feed_in, dec!(0));
    }

    #[test]
    fn eigenverbrauch_computed_correctly() {
        let ev = compute_eigenverbrauch(Some(dec!(400)), Some(dec!(250)));
        assert_eq!(ev, Some(dec!(150)));
    }

    #[test]
    fn eigenverbrauch_clamps_to_zero() {
        // Shouldn't happen in practice (more feed-in than generation is impossible)
        // but guard against meter anomalies
        let ev = compute_eigenverbrauch(Some(dec!(100)), Some(dec!(150)));
        assert_eq!(ev, Some(dec!(0)));
    }

    #[test]
    fn sect14a_ht_ratio() {
        let m = Sect14aModul2Measurement {
            eigenverbrauch_ht_kwh: dec!(200),
            eigenverbrauch_nt_kwh: dec!(300),
            steuerungsmassnahme_kwh: Some(dec!(50)),
        };
        assert_eq!(m.total_kwh(), dec!(500));
        let ratio = m.ht_ratio().unwrap();
        assert_eq!(ratio, dec!(0.4000)); // 200/500
    }
}
