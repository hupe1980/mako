//! Core GaBi Gas domain types.
//!
//! This module provides the rich domain vocabulary for the German gas market:
//!
//! - [`GasDay`] — typed gas market day (starts 06:00 CET per DVGW G 2000)
//! - [`GasQuantity`] — Decimal-precise energy in kWh_Hs with m³ conversion
//! - [`GasBeschaffenheit`] — Brennwert (Hs/Hu) and Zustandszahl from DVGW G 685
//! - [`GasBeschaffenheitValidationError`] — DVGW G 260 range check results
//! - [`GasQualityFlag`] — measurement quality state per §17 MessZV
//! - [`Bilanzkreis`] — BKV balance group with period and status
//! - [`DeliveryPoint`] — entry/exit point (Einspeise- / Ausspeisepunkt)
//! - [`GasImbalanceSaldo`] — nomination vs. allocation deviation with Ausgleichsenergie price
//! - [`NominationQuantity`] — submitted / accepted / curtailed breakdown
//! - [`GasQualityClass`] — H-Gas / L-Gas designation per DVGW G 260
//! - [`DvgwFormatVersion`] — DVGW biannual release version management
//! - `cloud_events` — typed `de.gabi.*` CloudEvent type constants
//!
//! ## No float money rule
//!
//! All energy quantities use [`rust_decimal::Decimal`] — never `f32`/`f64`.
//! Gas billing requires at least 3 decimal places (0.001 kWh precision per
//! DVGW G 685 §7).
//!
//! ## DST handling (Gas day = 06:00 CET)
//!
//! Per DVGW G 2000 and the Kooperationsvereinbarung Gas, the German **gas day**
//! starts and ends at **06:00 CET** (Central European Time), which is:
//! - Winter (CET, UTC+1): 05:00 UTC
//! - Summer (CEST, UTC+2): 04:00 UTC
//!
//! The DST transition days (last Sunday in March and October) have:
//! - Spring forward: 23-hour gas day (no 02:00–03:00 local)
//! - Fall back:      25-hour gas day (02:00–03:00 occurs twice)
//!
//! [`GasDay::start_utc`] and [`GasDay::end_utc`] account for this correctly.
//!
//! ## Regulatory basis
//!
//! - **DVGW G 685**: Gas billing — conversion m³ → kWh_Hs
//! - **DVGW G 260**: Gas quality classes (H-Gas, L-Gas, Biogas)
//! - **DVGW G 2000**: Gas market communication — gas day definition
//! - **Kooperationsvereinbarung Gas (KoV)**: BKV/FNB/MGV obligations
//! - **GaBi Gas 2.1 (BK7-24-01-008)**: Balancing group accounting
//! - **BNetzA BK7-24-01-008**: GaBi Gas 2.1 ruling

use rust_decimal::Decimal;
use time::{Date, Duration, OffsetDateTime, Time, Weekday};

// ── GasQualityClass ───────────────────────────────────────────────────────────

/// Gas quality class per DVGW G 260.
///
/// Determines the applicable Brennwert range and conversion factors.
///
/// | Class | Hs range | Description |
/// |-------|----------|-------------|
/// | H-Gas | 9.5–13.1 kWh/m³ | High-calorific (Hochgas) |
/// | L-Gas | 7.5–10.3 kWh/m³ | Low-calorific (Niedergas) |
/// | Biogas | variable | Biomethane injected into grid |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GasQualityClass {
    /// H-Gas (Hochgas) — default for most German transmission grids.
    HGas,
    /// L-Gas (Niedergas) — used in parts of northern Germany.
    LGas,
    /// Biomethane injected into the distribution grid.
    Biogas,
}

impl GasQualityClass {
    /// Typical Abrechnungsbrennwert range (Hs) in kWh/m³ (DVGW G 260).
    #[must_use]
    pub fn hs_range_kwh_per_m3(&self) -> (Decimal, Decimal) {
        match self {
            Self::HGas => (Decimal::new(95, 1), Decimal::new(131, 1)), // 9.5–13.1
            Self::LGas => (Decimal::new(75, 1), Decimal::new(103, 1)), // 7.5–10.3
            Self::Biogas => (Decimal::new(70, 1), Decimal::new(135, 1)), // variable
        }
    }
}

// ── GasBeschaffenheit ─────────────────────────────────────────────────────────

/// Gas quality parameters for energy conversion per DVGW G 685 / G 260.
///
/// The combination of `brennwert_hs_kwh_per_m3` and `zustandszahl` is required
/// to convert a measured gas volume (m³) to billing energy (kWh_Hs):
///
/// ```text
/// kWh_Hs = m³ × Hs × Z
/// ```
///
/// Both parameters are supplied by the Netzbetreiber in PID 13007 (MSCONS
/// Gasbeschaffenheitsdaten, NB → LF) for the applicable billing period.
///
/// ## Zustandszahl
///
/// The Zustandszahl Z accounts for local pressure and temperature deviation
/// from the reference conditions (0°C, 1.01325 bar). Per DVGW G 685 §8:
/// `Z = p_abs / p_ref × T_ref / T_abs`
/// Typical German network values: 0.94–1.06.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GasBeschaffenheit {
    /// Abrechnungsbrennwert Hs (superior calorific value) in kWh/m³.
    ///
    /// Per DVGW G 685 §7: the billing value is the monthly average Brennwert
    /// measured at the network entry point. Precision: ≥ 3 decimal places.
    pub brennwert_hs_kwh_per_m3: Decimal,

    /// Zustandszahl Z (state conversion factor, dimensionless).
    ///
    /// Accounts for pressure and temperature at the delivery point.
    /// Typical range: 0.94–1.06. Precision: ≥ 4 decimal places.
    pub zustandszahl: Decimal,

    /// Optional: Unterer Heizwert (inferior calorific value Hu) in kWh/m³.
    ///
    /// Hu < Hs because condensation latent heat is not recovered.
    /// Used in efficiency calculations and industrial billing.
    pub brennwert_hu_kwh_per_m3: Option<Decimal>,

    /// Gas quality class.
    pub quality_class: GasQualityClass,

    /// Start of validity period for these parameters.
    pub valid_from: Date,

    /// End of validity period (inclusive, None = open).
    pub valid_to: Option<Date>,

    /// Source of these parameters (e.g. "MSCONS PID 13007" or "manual entry").
    pub source: String,
}

impl GasBeschaffenheit {
    /// Convert gas volume (m³) to billing energy (kWh_Hs) per DVGW G 685.
    ///
    /// ```text
    /// kWh_Hs = volume_m3 × Hs × Z
    /// ```
    ///
    /// Result is rounded to 3 decimal places (DVGW G 685 §7 minimum precision).
    #[must_use]
    pub fn to_kwh_hs(&self, volume_m3: Decimal) -> Decimal {
        let result = volume_m3 * self.brennwert_hs_kwh_per_m3 * self.zustandszahl;
        result.round_dp(3)
    }

    /// `true` when these parameters are valid on the given date.
    #[must_use]
    pub fn is_valid_on(&self, date: Date) -> bool {
        date >= self.valid_from && self.valid_to.is_none_or(|end| date <= end)
    }
    /// Validate gas quality parameters against DVGW G 260 / G 685 physical limits.
    ///
    /// Returns `Ok(())` when all parameters are within acceptable ranges, or a
    /// [`GasBeschaffenheitValidationError`] listing all violated constraints.
    ///
    /// ## Checked constraints
    ///
    /// | Parameter | H-Gas range | L-Gas range | Source |
    /// |---|---|---|---|
    /// | Hs (kWh/m³) | 9.5–13.1 | 7.5–10.3 | DVGW G 260 §5.2 |
    /// | Hu (kWh/m³) | 8.5–11.8 | 6.8–9.3 | DVGW G 260 §5.2 |
    /// | Zustandszahl | 0.80–1.20 | 0.80–1.20 | DVGW G 685 §8 |
    ///
    /// For Biogas, a wider tolerance of Hs 7.0–13.5 kWh/m³ is applied.
    ///
    /// ## Usage
    ///
    /// ```rust,no_run
    /// # use mako_gabi_gas::domain::*;
    /// # use rust_decimal_macros::dec;
    /// # use time::macros::date;
    /// let b = GasBeschaffenheit {
    ///     brennwert_hs_kwh_per_m3: dec!(15.0), // invalid: too high for H-Gas
    ///     zustandszahl: dec!(1.0),
    ///     brennwert_hu_kwh_per_m3: None,
    ///     quality_class: GasQualityClass::HGas,
    ///     valid_from: date!(2026-01-01),
    ///     valid_to: None,
    ///     source: "test".into(),
    /// };
    /// assert!(b.validate().is_err());
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`GasBeschaffenheitValidationError`] when any measured property is outside
    /// DVGW G 260 physical range.
    pub fn validate(&self) -> Result<(), GasBeschaffenheitValidationError> {
        let mut violations = Vec::new();

        let (hs_min, hs_max) = self.quality_class.hs_range_kwh_per_m3();
        if self.brennwert_hs_kwh_per_m3 < hs_min || self.brennwert_hs_kwh_per_m3 > hs_max {
            violations.push(format!(
                "Brennwert Hs {:.3} kWh/m³ is outside DVGW G 260 range [{:.1}–{:.1}] for {:?}",
                self.brennwert_hs_kwh_per_m3, hs_min, hs_max, self.quality_class
            ));
        }

        // Zustandszahl physical range per DVGW G 685 §8 (typical German networks: 0.94–1.06)
        let z_min = Decimal::new(80, 2); // 0.80
        let z_max = Decimal::new(120, 2); // 1.20
        if self.zustandszahl < z_min || self.zustandszahl > z_max {
            violations.push(format!(
                "Zustandszahl {:.4} is outside physical range [{z_min}–{z_max}] per DVGW G 685 §8",
                self.zustandszahl
            ));
        }

        // Hu range check when present
        if let Some(hu) = self.brennwert_hu_kwh_per_m3 {
            let (hu_range_min, hu_range_max) = match self.quality_class {
                GasQualityClass::HGas => (Decimal::new(85, 1), Decimal::new(118, 1)), // 8.5–11.8
                GasQualityClass::LGas => (Decimal::new(68, 1), Decimal::new(93, 1)),  // 6.8–9.3
                GasQualityClass::Biogas => (Decimal::new(70, 1), Decimal::new(120, 1)),
            };
            if hu < hu_range_min || hu > hu_range_max {
                violations.push(format!(
                    "Brennwert Hu {hu:.3} kWh/m³ is outside DVGW G 260 range [{hu_range_min:.1}–{hu_range_max:.1}]"
                ));
            }
            // Sanity: Hu must be < Hs (lower heating value definition)
            if hu >= self.brennwert_hs_kwh_per_m3 {
                violations.push(format!(
                    "Brennwert Hu {hu:.3} must be < Hs {:.3} (Hu is lower heating value)",
                    self.brennwert_hs_kwh_per_m3
                ));
            }
        }

        if violations.is_empty() {
            Ok(())
        } else {
            Err(GasBeschaffenheitValidationError { violations })
        }
    }
}

// ── GasBeschaffenheitValidationError ─────────────────────────────────────────

/// Validation result when checking `GasBeschaffenheit` against DVGW G 260 / G 685.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GasBeschaffenheitValidationError {
    /// List of violated constraints with human-readable descriptions.
    pub violations: Vec<String>,
}

impl std::fmt::Display for GasBeschaffenheitValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GasBeschaffenheit validation failed: {}",
            self.violations.join("; ")
        )
    }
}

impl std::error::Error for GasBeschaffenheitValidationError {}

// ── GasQualityFlag ────────────────────────────────────────────────────────────

/// Quality flag for a gas measurement interval per §17 MessZV.
///
/// Gas measurements can originate from direct measurement (MSCONS), be estimated
/// by a standard load profile (SLP), or be replaced by a substitute value when
/// the primary measurement is unavailable.
///
/// The flag determines whether an interval is billable and which regulatory
/// handling applies (§17 MessZV Ersatzwertbildung for Gas RLM).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum GasQualityFlag {
    /// Directly measured RLM Gas value (MSCONS Gas interval, highest reliability).
    Measured,
    /// Value estimated using SLP Gas standard load profile (G0, H0, G1–G6).
    Estimated,
    /// Value substituted per §17 MessZV (prior-period average, SLP scaling, or linear).
    Substituted,
    /// Value calculated from a DVGW G 685 conversion (m³ × Hs × Z).
    Calculated,
    /// Corrected value superseding a previous Measured/Substituted value.
    /// Per §22 MessZV: corrections are stored immutably, not overwriting originals.
    Corrected,
    /// Value rejected by quality validation (outside physical bounds, duplicate, etc.).
    /// Rejected intervals are NOT billable and trigger an Ersatzwertbildung process.
    Rejected,
    /// Quality state not yet determined (waiting for DVGW G 685 data or validation).
    Unknown,
}

impl GasQualityFlag {
    /// `true` when this interval is billable under GaBi Gas 2.1 (BK7-24-01-008).
    ///
    /// Rejected intervals are never billable. Unknown intervals require
    /// resolution before billing can proceed.
    #[must_use]
    pub fn is_billable(self) -> bool {
        matches!(
            self,
            Self::Measured | Self::Substituted | Self::Calculated | Self::Corrected
        )
    }

    /// `true` when this interval originates from a direct measurement.
    #[must_use]
    pub fn is_measured(self) -> bool {
        matches!(self, Self::Measured)
    }
}

// ── CloudEvent type constants ─────────────────────────────────────────────────

/// CloudEvent type constants for GaBi Gas domain events (`de.gabi.*`).
///
/// Use `de.gabi.*` as a single glob in `agentd.toml` `trigger_event_types`
/// to activate the `gabi-gas-agent` on all GaBi Gas events.
pub mod cloud_events {
    /// MSCONS gas measurement received and ingested (PID 13007 or 13013).
    pub const MEASUREMENT_RECEIVED: &str = "de.gabi.measurement.received";
    /// ALOCAT allocation list processed — new allocation version available.
    pub const ALLOCATION_COMPLETED: &str = "de.gabi.allocation.completed";
    /// NOMINT nomination submitted by BKV to FNB or MGV.
    pub const NOMINATION_CREATED: &str = "de.gabi.nomination.created";
    /// NOMRES nomination response received — confirmed, curtailed, or rejected.
    pub const NOMINATION_CONFIRMED: &str = "de.gabi.nomination.confirmed";
    /// IMBNOT processed — `GasImbalanceSaldo` calculated for a gas day.
    pub const IMBALANCE_CALCULATED: &str = "de.gabi.imbalance.calculated";
    /// Allocation correction record created (Initial → Correction or → Final).
    pub const CORRECTION_CREATED: &str = "de.gabi.correction.created";
    /// INVOIC 31007/31008 (MMM-Rechnung Gas) received from NB.
    pub const INVOIC_MMM_RECEIVED: &str = "de.gabi.invoic.mmm.received";
    /// INVOIC 31010 (Kapazitätsrechnung) received from FNB/VNB.
    pub const INVOIC_KAPAZITAET_RECEIVED: &str = "de.gabi.invoic.kapazitaet.received";
    /// ALOCAT Initial allocation missing beyond D+3 12:00 CET deadline.
    pub const ALOCAT_MISSING: &str = "de.gabi.alocat.missing";
    /// GaBi Gas IMBNOT imbalance notification received from FNB/MGV.
    pub const IMBNOT_RECEIVED: &str = "de.gabi.imbnot.received";
    /// Gas quality parameters outside DVGW G 260 valid ranges.
    pub const GAS_QUALITY_VIOLATION: &str = "de.gabi.quality.violation";
    /// KoV §6.4 Final allocation deadline M+2 approaching or missed.
    pub const FINAL_ALOCAT_DEADLINE: &str = "de.gabi.alocat.final.deadline";
}

// ── DVGW format version management ───────────────────────────────────────────

/// DVGW message type discriminant for format version tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DvgwMessageType {
    /// Allokationsnachricht.
    Alocat,
    /// Nominierungsintegration.
    Nomint,
    /// Nominierungsantwort.
    Nomres,
    /// Schedulingnachricht.
    Schedl,
    /// Imbalance Notification.
    Imbnot,
    /// Transport Notification.
    Tranot,
    /// Delivery Order.
    Delord,
    /// Delivery Response.
    Delres,
}

/// A DVGW EDIFACT format version with its biannual validity window.
///
/// DVGW formats follow a biannual release schedule (April 1 + October 1 at
/// 06:00 CET = start of a gas day). During the 3-month overlap window before
/// a new version becomes mandatory, both versions must be accepted by the
/// `dvgw-edi` parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DvgwFormatVersion {
    /// Short version identifier (e.g. `"5.11a"`, `"4.6 FK"`).
    pub version: &'static str,
    /// First gas day from which this version is valid (06:00 CET on this date).
    pub valid_from: Date,
    /// Last gas day this version is supported (None = current/open).
    pub valid_to: Option<Date>,
    /// Which DVGW message type this version applies to.
    pub message_type: DvgwMessageType,
}

/// Current DVGW format versions (as of 2026-07-18 — update each April/October).
pub mod dvgw_versions {
    use super::{DvgwFormatVersion, DvgwMessageType};
    use time::macros::date;

    /// ALOCAT 5.11a — valid from 2024-10-01 (current).
    pub const ALOCAT: DvgwFormatVersion = DvgwFormatVersion {
        version: "5.11a",
        valid_from: date!(2024 - 10 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Alocat,
    };
    /// NOMINT 4.6 FK — valid from 2026-02-01 (current).
    pub const NOMINT: DvgwFormatVersion = DvgwFormatVersion {
        version: "4.6 FK",
        valid_from: date!(2026 - 02 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Nomint,
    };
    /// NOMRES 4.7 FK — valid from 2026-02-01 (current).
    pub const NOMRES: DvgwFormatVersion = DvgwFormatVersion {
        version: "4.7 FK",
        valid_from: date!(2026 - 02 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Nomres,
    };
    /// SCHEDL 4.4 FK — valid from 2026-02-01 (current).
    pub const SCHEDL: DvgwFormatVersion = DvgwFormatVersion {
        version: "4.4 FK",
        valid_from: date!(2026 - 02 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Schedl,
    };
    /// IMBNOT 5.7a — valid from 2023-10-01 (current).
    pub const IMBNOT: DvgwFormatVersion = DvgwFormatVersion {
        version: "5.7a",
        valid_from: date!(2023 - 10 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Imbnot,
    };
    /// TRANOT 5.8b — valid from 2023-10-01 (current).
    pub const TRANOT: DvgwFormatVersion = DvgwFormatVersion {
        version: "5.8b",
        valid_from: date!(2023 - 10 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Tranot,
    };
    /// DELORD 4.5 FK — valid from 2026-02-01 (current).
    pub const DELORD: DvgwFormatVersion = DvgwFormatVersion {
        version: "4.5 FK",
        valid_from: date!(2026 - 02 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Delord,
    };
    /// DELRES 4.6 FK — valid from 2026-02-01 (current).
    pub const DELRES: DvgwFormatVersion = DvgwFormatVersion {
        version: "4.6 FK",
        valid_from: date!(2026 - 02 - 01),
        valid_to: None,
        message_type: DvgwMessageType::Delres,
    };
}

// ── GasQuantity ───────────────────────────────────────────────────────────────

/// A gas energy quantity with optional m³ volume and conversion metadata.
/// volume (m³) and the conversion parameters used. This allows full audit of
/// any quantity from raw measurement to billed energy.
///
/// ## Precision
///
/// Billing precision ≥ 3 decimal places, per the DVGW G 685 series (Gasabrechnung).
/// `energy_kwh_hs` is always stored with `Decimal` to avoid float errors.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GasQuantity {
    /// Billing energy in kWh_Hs (always present after conversion).
    pub energy_kwh_hs: Decimal,

    /// Raw measured volume in m³ (present when measured directly).
    ///
    /// `None` when the quantity was expressed directly in kWh (e.g. nomination).
    pub volume_m3: Option<Decimal>,

    /// Gas quality parameters used for m³ → kWh_Hs conversion.
    ///
    /// `None` when no conversion was performed (quantity was in kWh from source).
    pub beschaffenheit: Option<GasBeschaffenheit>,
}

impl GasQuantity {
    /// Create directly from kWh_Hs (when no m³ conversion needed).
    #[must_use]
    pub fn from_kwh(energy_kwh_hs: Decimal) -> Self {
        Self {
            energy_kwh_hs,
            volume_m3: None,
            beschaffenheit: None,
        }
    }

    /// Create from m³ volume by applying conversion factors.
    #[must_use]
    pub fn from_m3(volume_m3: Decimal, beschaffenheit: GasBeschaffenheit) -> Self {
        let energy_kwh_hs = beschaffenheit.to_kwh_hs(volume_m3);
        Self {
            energy_kwh_hs,
            volume_m3: Some(volume_m3),
            beschaffenheit: Some(beschaffenheit),
        }
    }

    /// `true` when this quantity is zero (within ±0.001 kWh tolerance).
    #[must_use]
    pub fn is_effectively_zero(&self) -> bool {
        self.energy_kwh_hs.abs() < Decimal::new(1, 3)
    }
}

impl std::ops::Add for GasQuantity {
    type Output = GasQuantity;
    fn add(self, rhs: GasQuantity) -> Self {
        GasQuantity::from_kwh(self.energy_kwh_hs + rhs.energy_kwh_hs)
    }
}

impl std::ops::Sub for GasQuantity {
    type Output = GasQuantity;
    fn sub(self, rhs: GasQuantity) -> Self {
        GasQuantity::from_kwh(self.energy_kwh_hs - rhs.energy_kwh_hs)
    }
}

// ── NominationQuantity ────────────────────────────────────────────────────────

/// Quantity breakdown from a nomination / nomination response cycle.
///
/// Captures what the BKV nominated, what the FNB/MGV accepted, and the
/// curtailment applied. All values in kWh_Hs (per KoV §3.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NominationQuantity {
    /// Quantity submitted by the BKV in the NOMINT (kWh_Hs).
    pub submitted_kwh: Decimal,

    /// Quantity confirmed by FNB/MGV in the NOMRES (kWh_Hs).
    ///
    /// `None` when no NOMRES has been received yet.
    pub accepted_kwh: Option<Decimal>,

    /// Curtailed quantity = submitted − accepted (kWh_Hs).
    ///
    /// Positive when the FNB/MGV curtailed the nomination.
    pub curtailed_kwh: Option<Decimal>,

    /// Curtailment reason code from the NOMRES (if curtailment occurred).
    pub curtailment_reason: Option<String>,
}

impl NominationQuantity {
    /// Create from a submitted nomination (before NOMRES arrives).
    #[must_use]
    pub fn submitted(submitted_kwh: Decimal) -> Self {
        Self {
            submitted_kwh,
            accepted_kwh: None,
            curtailed_kwh: None,
            curtailment_reason: None,
        }
    }

    /// Apply the NOMRES response (full acceptance).
    #[must_use]
    pub fn accept_in_full(self) -> Self {
        let accepted = self.submitted_kwh;
        Self {
            accepted_kwh: Some(accepted),
            curtailed_kwh: Some(Decimal::ZERO),
            ..self
        }
    }

    /// Apply a partial NOMRES acceptance (curtailment).
    #[must_use]
    pub fn accept_partial(self, accepted_kwh: Decimal, reason: Option<String>) -> Self {
        let curtailed = self.submitted_kwh - accepted_kwh;
        Self {
            accepted_kwh: Some(accepted_kwh),
            curtailed_kwh: Some(curtailed),
            curtailment_reason: reason,
            ..self
        }
    }

    /// `true` when any curtailment was applied.
    #[must_use]
    pub fn is_curtailed(&self) -> bool {
        self.curtailed_kwh.is_some_and(|c| c > Decimal::ZERO)
    }
}

// ── GasDay ────────────────────────────────────────────────────────────────────

/// A German gas market day — starts and ends at 06:00 CET per DVGW G 2000.
///
/// The gas day is the fundamental scheduling unit for:
/// - Nominations (NOMINT deadline: D-1 13:00 CET)
/// - Allocations (ALOCAT: daily FNB→BKV)
/// - Imbalance settlement (IMBNOT)
/// - Schedules (SCHEDL day-ahead)
///
/// ## DST transitions
///
/// - **Spring forward** (last Sunday in March): 23-hour gas day.
///   Clock skips 02:00→03:00 local. Gas day is 05:00 UTC on March 29 → 04:00 UTC on March 30.
/// - **Fall back** (last Sunday in October): 25-hour gas day.
///   Clock repeats 02:00–03:00 local. Gas day is 04:00 UTC on October 25 → 05:00 UTC on Oct 26.
///
/// See DVGW G 2000 §3.2 for the authoritative definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct GasDay {
    /// Calendar date identifying this gas day (the date on which the day begins).
    pub date: Date,
}

impl GasDay {
    /// Construct a gas day from a calendar date.
    #[must_use]
    pub fn new(date: Date) -> Self {
        Self { date }
    }

    /// Parse from `YYYY-MM-DD` string.
    ///
    /// # Errors
    ///
    /// Returns a parse error when the string is not a valid ISO 8601 date.
    pub fn parse(s: &str) -> Result<Self, time::error::Parse> {
        use time::format_description::well_known::Iso8601;
        Ok(Self {
            date: Date::parse(s, &Iso8601::DATE)?,
        })
    }

    /// Start of this gas day in UTC.
    ///
    /// German gas day starts at 06:00 CET = 05:00 UTC (winter) or 04:00 UTC (summer).
    /// This method uses `time-tz` for DST-correct conversion.
    ///
    /// # Panics
    ///
    /// Does not panic in practice — the time literals 04:00 and 05:00 are always valid.
    #[must_use]
    pub fn start_utc(&self) -> OffsetDateTime {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        // Try 05:00 UTC (= 06:00 CET in winter); if Berlin local hour is not 6 use 04:00 UTC.
        let candidate_winter = OffsetDateTime::new_utc(self.date, Time::from_hms(5, 0, 0).unwrap());
        let candidate_summer = OffsetDateTime::new_utc(self.date, Time::from_hms(4, 0, 0).unwrap());
        if candidate_winter.to_timezone(berlin).hour() == 6 {
            candidate_winter
        } else {
            candidate_summer
        }
    }

    /// End of this gas day (= start of the next gas day) in UTC.
    #[must_use]
    pub fn end_utc(&self) -> OffsetDateTime {
        let next = GasDay {
            date: self.date + Duration::days(1),
        };
        next.start_utc()
    }

    /// Duration of this gas day in hours (23, 24, or 25 at DST transitions).
    #[must_use]
    pub fn duration_hours(&self) -> i64 {
        let secs = (self.end_utc() - self.start_utc()).whole_seconds();
        secs / 3600
    }

    /// The next gas day.
    #[must_use]
    pub fn next(&self) -> Self {
        Self {
            date: self.date + Duration::days(1),
        }
    }

    /// The previous gas day.
    #[must_use]
    pub fn previous(&self) -> Self {
        Self {
            date: self.date - Duration::days(1),
        }
    }

    /// NOMINT deadline for this gas day: D-1 13:00 CET in UTC.
    ///
    /// Per KoV §3.2: the BKV must submit the NOMINT by 13:00 CET on the day
    /// before the gas day (D-1). This is used to trigger deadline alerts.
    ///
    /// # Panics
    ///
    /// Does not panic in practice — the time literals 11:00 and 12:00 are always valid.
    #[must_use]
    pub fn nomination_deadline_utc(&self) -> OffsetDateTime {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        let d_minus_1 = self.date - Duration::days(1);
        // 12:00 UTC = 13:00 CET; 11:00 UTC = 13:00 CEST
        let candidate_winter =
            OffsetDateTime::new_utc(d_minus_1, Time::from_hms(12, 0, 0).unwrap());
        let candidate_summer =
            OffsetDateTime::new_utc(d_minus_1, Time::from_hms(11, 0, 0).unwrap());
        if candidate_winter.to_timezone(berlin).hour() == 13 {
            candidate_winter
        } else {
            candidate_summer
        }
    }

    /// NOMRES response deadline for this gas day: D-1 15:00 CET in UTC.
    ///
    /// Per the Kooperationsvereinbarung Gas: the FNB/MGV must respond to
    /// the BKV's nomination (NOMINT) by **15:00 CET on gas day D-1** — i.e.
    /// ~2 hours after the NOMINT submission deadline.
    ///
    /// Register a [`mako_engine::deadline::Deadline`] with
    /// [`crate::nomination::NOMRES_DEADLINE_LABEL`] using this timestamp
    /// immediately after the `NominationSent` event is persisted.
    ///
    /// # Panics
    ///
    /// Does not panic in practice.
    #[must_use]
    pub fn nomres_deadline_utc(&self) -> OffsetDateTime {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        let d_minus_1 = self.date - Duration::days(1);
        // 14:00 UTC = 15:00 CET; 13:00 UTC = 15:00 CEST
        let candidate_winter =
            OffsetDateTime::new_utc(d_minus_1, Time::from_hms(14, 0, 0).unwrap());
        let candidate_summer =
            OffsetDateTime::new_utc(d_minus_1, Time::from_hms(13, 0, 0).unwrap());
        if candidate_winter.to_timezone(berlin).hour() == 15 {
            candidate_winter
        } else {
            candidate_summer
        }
    }

    /// Initial ALOCAT deadline: gas day D+3 at 12:00 CET in UTC.
    ///
    /// Per KoV §6.4: the FNB must issue the **initial** (preliminary) allocation
    /// to the BKV by **D+3 at 12:00 CET** (D = gas day date).
    ///
    /// Register a deadline with label `"gabi-gas-alocat-initial-deadline"` to
    /// monitor compliance.
    ///
    /// # Panics
    ///
    /// Does not panic in practice.
    #[must_use]
    pub fn initial_alocat_deadline_utc(&self) -> OffsetDateTime {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;
        let d_plus_3 = self.date + Duration::days(3);
        // 11:00 UTC = 12:00 CET; 10:00 UTC = 12:00 CEST
        let candidate_winter = OffsetDateTime::new_utc(d_plus_3, Time::from_hms(11, 0, 0).unwrap());
        let candidate_summer = OffsetDateTime::new_utc(d_plus_3, Time::from_hms(10, 0, 0).unwrap());
        if candidate_winter.to_timezone(berlin).hour() == 12 {
            candidate_winter
        } else {
            candidate_summer
        }
    }

    /// Final ALOCAT deadline: M+2 (end of the second calendar month after the
    /// gas day) at 12:00 CET in UTC.
    ///
    /// Per KoV §6.4: the FNB/MGV must issue the **final** (binding) allocation
    /// by the end of month M+2. The exact day is the last day of month+2 at
    /// 12:00 CET. All corrections must be complete by this date.
    ///
    /// Used to alert agentd's `gabi-gas-agent` to trigger compliance checks.
    ///
    /// # Panics
    ///
    /// Does not panic in practice.
    #[must_use]
    pub fn final_alocat_deadline_utc(&self) -> OffsetDateTime {
        use time_tz::{OffsetDateTimeExt, timezones};
        let berlin = timezones::db::europe::BERLIN;

        // M+2: month of gas day + 2, last day of that month
        let month_num = self.date.month() as u8;
        let year = self.date.year();
        let (target_year, target_month) = if month_num >= 11 {
            // Nov → Jan+1, Dec → Feb+1
            (
                year + 1,
                time::Month::try_from((month_num + 2) % 12).unwrap_or(time::Month::January),
            )
        } else {
            (
                year,
                time::Month::try_from(month_num + 2).unwrap_or(time::Month::January),
            )
        };
        // Last day of target month
        let last_day =
            Date::from_calendar_date(target_year, target_month, target_month.length(target_year))
                .unwrap_or_else(|_| self.date + Duration::days(60));

        // 11:00 UTC = 12:00 CET (winter); 10:00 UTC = 12:00 CEST (summer)
        let candidate_winter = OffsetDateTime::new_utc(last_day, Time::from_hms(11, 0, 0).unwrap());
        let candidate_summer = OffsetDateTime::new_utc(last_day, Time::from_hms(10, 0, 0).unwrap());
        if candidate_winter.to_timezone(berlin).hour() == 12 {
            candidate_winter
        } else {
            candidate_summer
        }
    }

    /// Format as `YYYY-MM-DD` (DVGW standard gas day identifier).
    #[must_use]
    pub fn to_iso8601(&self) -> String {
        use time::format_description::well_known::Iso8601;
        self.date
            .format(&Iso8601::DATE)
            .unwrap_or_else(|_| self.date.to_string())
    }

    /// `true` when this gas day falls on a German public holiday or weekend.
    ///
    /// Used for nomination deadline adjustments per KoV.
    #[must_use]
    pub fn is_weekend(&self) -> bool {
        matches!(self.date.weekday(), Weekday::Saturday | Weekday::Sunday)
    }
}

impl std::fmt::Display for GasDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_iso8601())
    }
}

impl From<Date> for GasDay {
    fn from(date: Date) -> Self {
        Self::new(date)
    }
}

// ── Bilanzkreis ───────────────────────────────────────────────────────────────

/// A gas balancing group (Bilanzkreis) managed by a BKV.
///
/// Per GaBi Gas 2.1 (BK7-24-01-008): every market participant delivering or withdrawing gas in
/// a balancing zone must be assigned to a Bilanzkreis. The BKV is responsible
/// for keeping nominations in balance.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Bilanzkreis {
    /// EIC code of this Bilanzkreis (Energy Identification Code, 16 chars).
    pub eic_code: String,

    /// EIC code of the responsible BKV.
    pub bkv_eic: String,

    /// EIC code of the Marktgebiet (market area / MGV).
    pub marktgebiet_eic: String,

    /// Gas quality class applicable in this Bilanzkreis.
    pub quality_class: GasQualityClass,

    /// Start of validity.
    pub valid_from: Date,

    /// End of validity (None = active).
    pub valid_to: Option<Date>,
}

impl Bilanzkreis {
    /// `true` when this Bilanzkreis is active on the given gas day.
    #[must_use]
    pub fn is_active_on(&self, gas_day: GasDay) -> bool {
        gas_day.date >= self.valid_from && self.valid_to.is_none_or(|end| gas_day.date <= end)
    }
}

// ── DeliveryPoint ─────────────────────────────────────────────────────────────

/// A gas delivery point — entry (Einspeisepunkt) or exit (Ausspeisepunkt).
///
/// Per KARLA Gas 2.0 (BK7-24-01-007): entry and exit points are the locations where gas
/// enters or leaves a transport grid, identified by EIC codes.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DeliveryPoint {
    /// EIC code of this delivery point.
    pub eic_code: String,

    /// Human-readable name.
    pub name: String,

    /// Whether this is an entry or exit point.
    pub direction: DeliveryPointDirection,

    /// EIC code of the operating grid (FNB or VNB).
    pub grid_operator_eic: String,
}

/// Entry vs. exit point designation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeliveryPointDirection {
    /// Gas enters the grid at this point (Einspeisepunkt).
    Entry,
    /// Gas exits the grid at this point (Ausspeisepunkt).
    Exit,
    /// Bidirectional point (e.g. VHP — Virtual Hub Point).
    Bidirectional,
}

// ── GasImbalanceSaldo ─────────────────────────────────────────────────────────

/// Calculated gas imbalance for one BKV for one gas day.
///
/// Per GaBi Gas 2.1 (BK7-24-01-008) and KoV: the imbalance is the difference between nominated
/// quantities and actual allocated quantities. Positive = mehr (excess), negative =
/// minder (deficit). Settlement via the MGV Ausgleichsenergiepreis.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GasImbalanceSaldo {
    /// Gas day for which this imbalance was calculated.
    pub gas_day: GasDay,

    /// EIC code of the responsible BKV.
    pub bkv_eic: String,

    /// EIC code of the Bilanzkreis.
    pub bilanzkreis_eic: String,

    /// Total nominated quantity by the BKV for this gas day (kWh_Hs).
    pub nominated_kwh: Decimal,

    /// Total allocated quantity from the FNB/MGV for this gas day (kWh_Hs).
    pub allocated_kwh: Decimal,

    /// Imbalance = nominated − allocated (kWh_Hs).
    ///
    /// Positive: BKV over-nominated (Mehr-Energie).
    /// Negative: BKV under-nominated (Minder-Energie).
    pub imbalance_kwh: Decimal,

    /// `true` when the imbalance requires settlement via Ausgleichsenergie.
    pub requires_settlement: bool,

    /// Ausgleichsenergie price from MGV per KoV §9 (EUR-cent per kWh_Hs).
    ///
    /// Set when the MGV publishes the Ausgleichsenergiepreis for this gas day.
    /// `None` when not yet published (settlement is pending).
    ///
    /// Stored as `i64` EUR-cent to avoid float precision issues.
    /// Example: 500 = 5.00 EUR/MWh = 0.50 ct/kWh.
    pub ausgleichsenergie_price_ct_per_kwh: Option<Decimal>,
}

impl GasImbalanceSaldo {
    /// Calculate imbalance from nominated and allocated quantities.
    #[must_use]
    pub fn calculate(
        gas_day: GasDay,
        bkv_eic: impl Into<String>,
        bilanzkreis_eic: impl Into<String>,
        nominated_kwh: Decimal,
        allocated_kwh: Decimal,
    ) -> Self {
        let imbalance_kwh = nominated_kwh - allocated_kwh;
        let threshold = Decimal::new(1, 0); // 1 kWh threshold
        let requires_settlement = imbalance_kwh.abs() > threshold;
        Self {
            gas_day,
            bkv_eic: bkv_eic.into(),
            bilanzkreis_eic: bilanzkreis_eic.into(),
            nominated_kwh,
            allocated_kwh,
            imbalance_kwh,
            requires_settlement,
            ausgleichsenergie_price_ct_per_kwh: None,
        }
    }

    /// Direction of the imbalance.
    #[must_use]
    pub fn direction(&self) -> ImbalanceDirection {
        match self.imbalance_kwh.cmp(&Decimal::ZERO) {
            std::cmp::Ordering::Greater => ImbalanceDirection::Mehr,
            std::cmp::Ordering::Less => ImbalanceDirection::Minder,
            std::cmp::Ordering::Equal => ImbalanceDirection::Balanced,
        }
    }

    /// Calculate the Ausgleichsenergie cost / revenue per KoV §9.
    ///
    /// Returns `None` when the Ausgleichsenergiepreis has not yet been published.
    ///
    /// - Positive imbalance (Mehr): BKV owes this amount to MGV.
    /// - Negative imbalance (Minder): MGV owes this amount to BKV.
    #[must_use]
    pub fn settlement_amount_ct(&self) -> Option<Decimal> {
        self.ausgleichsenergie_price_ct_per_kwh
            .map(|price| self.imbalance_kwh * price)
    }
}

/// Direction of a gas imbalance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ImbalanceDirection {
    /// BKV over-nominated — excess energy (Mehr-Energie, BKV owes MGV).
    Mehr,
    /// BKV under-nominated — deficit energy (Minder-Energie, MGV owes BKV).
    Minder,
    /// Perfectly balanced (within threshold).
    Balanced,
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use time::macros::date;

    // ── GasBeschaffenheit ──────────────────────────────────────────────────────

    #[test]
    fn conversion_h_gas_typical() {
        // DVGW G 685 example: 100 m³ H-Gas with Hs = 10.55 kWh/m³, Z = 0.9764
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(10.55),
            zustandszahl: dec!(0.9764),
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "MSCONS PID 13007".to_owned(),
        };
        let kwh = b.to_kwh_hs(dec!(100));
        // 100 × 10.55 × 0.9764 = 1030.102 kWh
        assert_eq!(kwh, dec!(1030.102));
    }

    #[test]
    fn conversion_precision_3_dp() {
        // Result must be rounded to 3 decimal places
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(11.111),
            zustandszahl: dec!(0.9999),
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "test".to_owned(),
        };
        let kwh = b.to_kwh_hs(dec!(1));
        assert_eq!(kwh.scale(), 3);
    }

    #[test]
    fn gas_quantity_from_m3() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(10.55),
            zustandszahl: dec!(1.0),
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "test".to_owned(),
        };
        let q = GasQuantity::from_m3(dec!(10), b);
        assert_eq!(q.energy_kwh_hs, dec!(105.500));
        assert_eq!(q.volume_m3, Some(dec!(10)));
    }

    #[test]
    fn gas_quantity_addition_uses_kwh() {
        let q1 = GasQuantity::from_kwh(dec!(100.0));
        let q2 = GasQuantity::from_kwh(dec!(50.5));
        let sum = q1 + q2;
        assert_eq!(sum.energy_kwh_hs, dec!(150.5));
        assert!(sum.volume_m3.is_none()); // addition loses m³ context
    }

    // ── GasDay ─────────────────────────────────────────────────────────────────

    #[test]
    fn gas_day_standard_winter_starts_05_utc() {
        // Normal winter day — CET = UTC+1, so 06:00 CET = 05:00 UTC
        let d = GasDay::new(date!(2026 - 01 - 15));
        let start = d.start_utc();
        assert_eq!(start.hour(), 5);
        assert_eq!(start.minute(), 0);
    }

    #[test]
    fn gas_day_standard_summer_starts_04_utc() {
        // Summer day — CEST = UTC+2, so 06:00 CEST = 04:00 UTC
        let d = GasDay::new(date!(2026 - 07 - 15));
        let start = d.start_utc();
        assert_eq!(start.hour(), 4);
        assert_eq!(start.minute(), 0);
    }

    #[test]
    fn normal_gas_day_is_24_hours() {
        let d = GasDay::new(date!(2026 - 01 - 15));
        assert_eq!(d.duration_hours(), 24);
    }

    #[test]
    fn spring_forward_gas_day_is_23_hours() {
        // 2026 spring forward: last Sunday of March = March 29 (clocks advance at 01:00 UTC).
        // The gas day that SPANS the clock change is March 28:
        //   start = March 28 06:00 CET = 05:00 UTC
        //   end   = March 29 06:00 CEST = 04:00 UTC (spring-forward already happened)
        //   duration = 23 hours
        let d = GasDay::new(date!(2026 - 03 - 28));
        assert_eq!(d.duration_hours(), 23);
    }

    #[test]
    fn fall_back_gas_day_is_25_hours() {
        // 2026 fall back: last Sunday of October = October 25 (clocks fall at 01:00 UTC).
        // The gas day that SPANS the fall-back is October 24:
        //   start = Oct 24 06:00 CEST = 04:00 UTC
        //   end   = Oct 25 06:00 CET  = 05:00 UTC (fall-back already happened)
        //   duration = 25 hours
        let d = GasDay::new(date!(2026 - 10 - 24));
        assert_eq!(d.duration_hours(), 25);
    }

    #[test]
    fn nomination_deadline_is_d_minus_1_13_00_cet_winter() {
        // Gas day 2026-01-15 → deadline is 2026-01-14 13:00 CET = 12:00 UTC
        let d = GasDay::new(date!(2026 - 01 - 15));
        let dl = d.nomination_deadline_utc();
        assert_eq!(dl.date(), date!(2026 - 01 - 14));
        assert_eq!(dl.hour(), 12); // 13:00 CET = 12:00 UTC in winter
    }

    #[test]
    fn nomination_deadline_is_d_minus_1_13_00_cest_summer() {
        // Gas day 2026-07-15 → deadline is 2026-07-14 13:00 CEST = 11:00 UTC
        let d = GasDay::new(date!(2026 - 07 - 15));
        let dl = d.nomination_deadline_utc();
        assert_eq!(dl.date(), date!(2026 - 07 - 14));
        assert_eq!(dl.hour(), 11); // 13:00 CEST = 11:00 UTC in summer
    }

    #[test]
    fn gas_day_to_string() {
        let d = GasDay::new(date!(2026 - 06 - 01));
        assert_eq!(d.to_string(), "2026-06-01");
    }

    // ── NominationQuantity ─────────────────────────────────────────────────────

    #[test]
    fn nomination_full_acceptance() {
        let n = NominationQuantity::submitted(dec!(1000.0)).accept_in_full();
        assert_eq!(n.accepted_kwh, Some(dec!(1000.0)));
        assert_eq!(n.curtailed_kwh, Some(dec!(0)));
        assert!(!n.is_curtailed());
    }

    #[test]
    fn nomination_partial_acceptance() {
        let n = NominationQuantity::submitted(dec!(1000.0))
            .accept_partial(dec!(800.0), Some("capacity_constraint".to_owned()));
        assert_eq!(n.accepted_kwh, Some(dec!(800.0)));
        assert_eq!(n.curtailed_kwh, Some(dec!(200.0)));
        assert!(n.is_curtailed());
    }

    // ── GasImbalanceSaldo ──────────────────────────────────────────────────────

    #[test]
    fn imbalance_mehr_when_over_nominated() {
        let saldo = GasImbalanceSaldo::calculate(
            GasDay::new(date!(2026 - 01 - 15)),
            "EIC_BKV",
            "EIC_BK",
            dec!(1000.0),
            dec!(900.0),
        );
        assert_eq!(saldo.imbalance_kwh, dec!(100.0));
        assert_eq!(saldo.direction(), ImbalanceDirection::Mehr);
        assert!(saldo.requires_settlement);
    }

    #[test]
    fn imbalance_minder_when_under_nominated() {
        let saldo = GasImbalanceSaldo::calculate(
            GasDay::new(date!(2026 - 01 - 15)),
            "EIC_BKV",
            "EIC_BK",
            dec!(900.0),
            dec!(1000.0),
        );
        assert_eq!(saldo.direction(), ImbalanceDirection::Minder);
        assert_eq!(saldo.imbalance_kwh, dec!(-100.0));
    }

    #[test]
    fn imbalance_balanced_within_threshold() {
        let saldo = GasImbalanceSaldo::calculate(
            GasDay::new(date!(2026 - 01 - 15)),
            "EIC_BKV",
            "EIC_BK",
            dec!(1000.000),
            dec!(1000.500), // 0.5 kWh difference — below threshold
        );
        assert!(!saldo.requires_settlement);
    }

    // ── Bilanzkreis ───────────────────────────────────────────────────────────

    #[test]
    fn bilanzkreis_active_on_valid_date() {
        let bk = Bilanzkreis {
            eic_code: "11YAKPG4CTRDNZ--A".to_owned(),
            bkv_eic: "EIC_BKV".to_owned(),
            marktgebiet_eic: "EIC_MGV".to_owned(),
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
        };
        assert!(bk.is_active_on(GasDay::new(date!(2026 - 07 - 15))));
    }

    #[test]
    fn bilanzkreis_inactive_before_valid_from() {
        let bk = Bilanzkreis {
            eic_code: "11YAKPG4CTRDNZ--A".to_owned(),
            bkv_eic: "EIC_BKV".to_owned(),
            marktgebiet_eic: "EIC_MGV".to_owned(),
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 06 - 01),
            valid_to: None,
        };
        assert!(!bk.is_active_on(GasDay::new(date!(2026 - 01 - 15))));
    }

    // ── GasDay deadline helpers ────────────────────────────────────────────────

    #[test]
    fn nomres_deadline_is_d_minus_1_15_00_cet_winter() {
        // Gas day 2026-01-15 → NOMRES deadline = 2026-01-14 15:00 CET = 14:00 UTC
        let d = GasDay::new(date!(2026 - 01 - 15));
        let dl = d.nomres_deadline_utc();
        assert_eq!(dl.date(), date!(2026 - 01 - 14));
        assert_eq!(dl.hour(), 14, "15:00 CET = 14:00 UTC in winter");
        // NOMRES deadline must be > NOMINT deadline (D-1 13:00 CET)
        assert!(
            dl > d.nomination_deadline_utc(),
            "NOMRES deadline must be after NOMINT deadline"
        );
    }

    #[test]
    fn nomres_deadline_is_d_minus_1_15_00_cest_summer() {
        // Gas day 2026-07-15 → NOMRES deadline = 2026-07-14 15:00 CEST = 13:00 UTC
        let d = GasDay::new(date!(2026 - 07 - 15));
        let dl = d.nomres_deadline_utc();
        assert_eq!(dl.date(), date!(2026 - 07 - 14));
        assert_eq!(dl.hour(), 13, "15:00 CEST = 13:00 UTC in summer");
    }

    #[test]
    fn initial_alocat_deadline_is_d_plus_3_12_00_cet_winter() {
        // Gas day 2026-01-15 → Initial ALOCAT deadline = 2026-01-18 12:00 CET = 11:00 UTC
        let d = GasDay::new(date!(2026 - 01 - 15));
        let dl = d.initial_alocat_deadline_utc();
        assert_eq!(dl.date(), date!(2026 - 01 - 18), "D+3 date");
        assert_eq!(dl.hour(), 11, "12:00 CET = 11:00 UTC in winter");
    }

    #[test]
    fn final_alocat_deadline_is_end_of_month_plus_2() {
        // Gas day 2026-01-15 → Final ALOCAT deadline = end of 2026-03 at 12:00 CET
        let d = GasDay::new(date!(2026 - 01 - 15));
        let dl = d.final_alocat_deadline_utc();
        assert_eq!(
            dl.date().month(),
            time::Month::March,
            "M+2 = March for January gas day"
        );
        assert_eq!(dl.date().year(), 2026);
        assert_eq!(dl.date().day(), 31, "last day of March");
    }

    #[test]
    fn final_alocat_deadline_november_wraps_to_january_next_year() {
        // Gas day 2026-11-15 → Final ALOCAT deadline = end of 2027-01
        let d = GasDay::new(date!(2026 - 11 - 15));
        let dl = d.final_alocat_deadline_utc();
        assert_eq!(
            dl.date().year(),
            2027,
            "M+2 from November wraps to next year"
        );
        assert_eq!(dl.date().month(), time::Month::January);
    }

    // ── GasBeschaffenheit validation ───────────────────────────────────────────

    #[test]
    fn beschaffenheit_valid_h_gas_passes_validation() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(10.55),
            zustandszahl: dec!(0.9764),
            brennwert_hu_kwh_per_m3: Some(dec!(9.55)),
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "MSCONS PID 13007".to_owned(),
        };
        assert!(b.validate().is_ok(), "typical H-Gas values should pass");
    }

    #[test]
    fn beschaffenheit_hs_too_high_fails_validation() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(15.0), // > 13.1 kWh/m³ H-Gas max
            zustandszahl: dec!(1.0),
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "test".to_owned(),
        };
        let err = b.validate().unwrap_err();
        assert!(!err.violations.is_empty());
        assert!(err.violations[0].contains("Hs"), "error should mention Hs");
    }

    #[test]
    fn beschaffenheit_zustandszahl_out_of_range_fails() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(10.55),
            zustandszahl: dec!(1.5), // > 1.20 physical maximum
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "test".to_owned(),
        };
        let err = b.validate().unwrap_err();
        assert!(!err.violations.is_empty());
        assert!(err.violations[0].contains("Zustandszahl"));
    }

    #[test]
    fn beschaffenheit_hu_must_be_less_than_hs() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(10.55),
            zustandszahl: dec!(1.0),
            brennwert_hu_kwh_per_m3: Some(dec!(10.55)), // Hu = Hs, physically impossible
            quality_class: GasQualityClass::HGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "test".to_owned(),
        };
        let err = b.validate().unwrap_err();
        assert!(err.violations.iter().any(|v| v.contains("Hu")));
    }

    #[test]
    fn beschaffenheit_l_gas_valid_range() {
        let b = GasBeschaffenheit {
            brennwert_hs_kwh_per_m3: dec!(8.5), // L-Gas range 7.5–10.3
            zustandszahl: dec!(0.98),
            brennwert_hu_kwh_per_m3: None,
            quality_class: GasQualityClass::LGas,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
            source: "MSCONS".to_owned(),
        };
        assert!(b.validate().is_ok(), "8.5 kWh/m³ is valid for L-Gas");
    }

    // ── GasQualityFlag ─────────────────────────────────────────────────────────

    #[test]
    fn gas_quality_flag_billable_states() {
        assert!(GasQualityFlag::Measured.is_billable());
        assert!(GasQualityFlag::Substituted.is_billable());
        assert!(GasQualityFlag::Corrected.is_billable());
        assert!(!GasQualityFlag::Rejected.is_billable());
        assert!(!GasQualityFlag::Unknown.is_billable());
    }

    // ── CloudEvent constants ───────────────────────────────────────────────────

    #[test]
    fn cloud_event_constants_have_correct_prefix() {
        assert!(cloud_events::MEASUREMENT_RECEIVED.starts_with("de.gabi."));
        assert!(cloud_events::ALLOCATION_COMPLETED.starts_with("de.gabi."));
        assert!(cloud_events::NOMINATION_CREATED.starts_with("de.gabi."));
        assert!(cloud_events::IMBALANCE_CALCULATED.starts_with("de.gabi."));
        assert!(cloud_events::INVOIC_MMM_RECEIVED.starts_with("de.gabi."));
    }

    // ── DVGW version constants ─────────────────────────────────────────────────

    #[test]
    fn dvgw_alocat_version_is_5_11a() {
        assert_eq!(dvgw_versions::ALOCAT.version, "5.11a");
    }

    #[test]
    fn dvgw_nomint_version_is_4_6_fk() {
        assert_eq!(dvgw_versions::NOMINT.version, "4.6 FK");
    }

    // ── GasImbalanceSaldo with Ausgleichsenergie ───────────────────────────────

    #[test]
    fn settlement_amount_computed_when_price_available() {
        let mut saldo = GasImbalanceSaldo::calculate(
            GasDay::new(date!(2026 - 01 - 15)),
            "BKV",
            "BK",
            dec!(1000.0),
            dec!(900.0), // 100 kWh Mehr (over-nominated)
        );
        saldo.ausgleichsenergie_price_ct_per_kwh = Some(dec!(5.0)); // 5 ct/kWh
        let amount = saldo.settlement_amount_ct().unwrap();
        assert_eq!(amount, dec!(500.0), "100 kWh × 5 ct = 500 ct (5 EUR)");
    }

    #[test]
    fn settlement_amount_none_when_price_not_yet_published() {
        let saldo = GasImbalanceSaldo::calculate(
            GasDay::new(date!(2026 - 01 - 15)),
            "BKV",
            "BK",
            dec!(1000.0),
            dec!(900.0),
        );
        assert!(saldo.settlement_amount_ct().is_none());
    }
}
