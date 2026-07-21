//! Measurement point model — MaLo + MeLo + OBIS + market role binding.
//!
//! A [`MeasurementPoint`] is the logical binding of:
//! - A physical location (MeLo)
//! - A market billing location (MaLo)
//! - A specific OBIS register
//! - The accountable market role
//!
//! This is the structural metadata layer that connects raw [`crate::MeterInterval`]
//! data to the regulatory and billing context required by German MaKo.
//!
//! ## Relationship to MSCONS
//!
//! In MSCONS, each time series is identified by:
//! - `NAD+MS/MR` — sender/receiver market participant
//! - `LOC+172` — Marktlokations-ID (MaLo)
//! - `LOC+237` — Messlokations-ID (MeLo, optional)
//! - `PIA` — OBIS code
//!
//! `MeasurementPoint` binds all four together for a specific validity period.
//!
//! ## Multiple registers per MeLo
//!
//! A MeLo can have multiple registers at the same timestamp — e.g. HT (register 1)
//! and NT (register 2). Each register is a distinct `MeasurementPoint` with a
//! different OBIS code but the same MaLo and MeLo.
//!
//! ## Regulatory basis
//!
//! - **§ 2 MsbG**: MeLo is the physical measurement reference.
//! - **BDEW MaKo**: MaLo is the billing reference.
//! - **BSI TR-03109**: Zählpunkt-ID ties MeLo to the SMGW.

use time::Date;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::obis::ObisCode;

// ── MarktRolle ────────────────────────────────────────────────────────────────

/// Market role responsible for this measurement point.
///
/// Governs which entity owns the metering obligation and which processes
/// are triggered by changes (Messstellen­betreiberwechsel, Lieferbeginn, etc.).
///
/// Source: BDEW Rollenmodell V2.2 §2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum MarktRolle {
    /// Netzbetreiber — owns grid metering for supply accounting.
    Nb,
    /// Lieferant — owns load forecasting and billing.
    Lf,
    /// Messstellenbetreiber — physically operates the meter.
    Msb,
    /// Bilanzkreisverantwortlicher — balance group management.
    Bkv,
    /// Übertragungsnetzbetreiber (Strom) / FNB (Gas).
    Uenb,
    /// Einspeiseverantwortlicher — responsible for feed-in control.
    Eiv,
    /// Direktvermarkter — direct marketing of renewable energy.
    Direktvermarkter,
    /// Marktgebietsverantwortlicher (Gas).
    Mgv,
    /// Energieserviceanbieter des Anschlussnutzers (iMSys context).
    Esa,
}

impl MarktRolle {
    /// BDEW abbreviation.
    #[must_use]
    pub fn abbreviation(self) -> &'static str {
        match self {
            Self::Nb => "NB",
            Self::Lf => "LF",
            Self::Msb => "MSB",
            Self::Bkv => "BKV",
            Self::Uenb => "ÜNB",
            Self::Eiv => "EIV",
            Self::Direktvermarkter => "DV",
            Self::Mgv => "MGV",
            Self::Esa => "ESA",
        }
    }

    /// `true` for roles that receive meter data via MSCONS from NB/MSB.
    #[must_use]
    pub fn is_mscons_receiver(self) -> bool {
        matches!(
            self,
            Self::Lf | Self::Bkv | Self::Mgv | Self::Direktvermarkter
        )
    }
}

// ── EnergyFlow ────────────────────────────────────────────────────────────────

/// Energy flow type at this measurement point.
///
/// Determines billing logic: generation gets feed-in compensation (EEG),
/// consumption triggers NNE (Netznutzungsentgelt) billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EnergyFlow {
    /// Consumption — energy drawn from the grid.
    Consumption,
    /// Generation — energy fed into the grid (Einspeisung).
    Generation,
    /// Storage charging — battery, heat pump storage.
    StorageCharge,
    /// Storage discharging.
    StorageDischarge,
    /// Bidirectional — prosumer net metering (Vierquadrantenmessung).
    Bidirectional,
}

// ── MeasurementPoint ─────────────────────────────────────────────────────────

/// The complete regulatory and physical context for a meter register.
///
/// Binds together: Marktlokation (MaLo), Messlokation (MeLo), OBIS register,
/// accountable market role, and energy flow direction.
///
/// ## Bitemporal validity
///
/// `valid_from` / `valid_to` track when this configuration was active.
/// This is essential for:
/// - Meter exchange events (MeLo changes, MSB change per WiM process)
/// - Supplier switch (Lieferbeginn/-ende)
/// - Register reconfiguration (HT↔NT tariff changes per §14a)
///
/// ## Virtual meters
///
/// Virtual meters (GGV community solar, Residuallast) also have `MeasurementPoint`
/// entries with `is_virtual = true`. Their `obis_code` is a conventional code
/// like `1-0:1.8.0*255` (total import) since virtual meters are logical.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeasurementPoint {
    /// 11-digit Marktlokations-ID (billing reference).
    pub malo_id: String,

    /// 33-character Messlokations-ID (physical metering reference).
    ///
    /// `None` for SLP customers without an explicit MeLo.
    pub melo_id: Option<String>,

    /// Physical meter serial number.
    ///
    /// `None` for virtual meters or when meter identity is not yet known.
    pub meter_serial: Option<String>,

    /// OBIS code identifying this register on the meter.
    pub obis_code: ObisCode,

    /// Energy commodity.
    pub sparte: crate::interval::Sparte,

    /// Energy flow direction for this register.
    pub energy_flow: EnergyFlow,

    /// Market role accountable for this measurement point.
    pub accountable_role: MarktRolle,

    /// 13-digit BDEW or DVGW Codenummer of the accountable market participant.
    pub accountable_mp_id: String,

    /// `true` when this is a virtual/derived measurement point (GGV, Residuallast).
    pub is_virtual: bool,

    /// Validity start (German local date, inclusive).
    pub valid_from: Date,

    /// Validity end (German local date, inclusive).
    ///
    /// `None` = still active.
    pub valid_to: Option<Date>,
}

impl MeasurementPoint {
    /// `true` when this point is active on the given date.
    #[must_use]
    pub fn is_active(&self, on_date: Date) -> bool {
        on_date >= self.valid_from && self.valid_to.is_none_or(|end| on_date <= end)
    }

    /// `true` when this point represents electricity (Strom) import.
    #[must_use]
    pub fn is_bezug(&self) -> bool {
        matches!(self.energy_flow, EnergyFlow::Consumption) || self.obis_code.is_import()
    }

    /// `true` when this point represents electricity feed-in (Einspeisung).
    #[must_use]
    pub fn is_einspeisung(&self) -> bool {
        matches!(self.energy_flow, EnergyFlow::Generation) || self.obis_code.is_einspeisung()
    }

    /// `true` when this point measures reactive energy.
    #[must_use]
    pub fn is_reactive(&self) -> bool {
        self.obis_code.is_reactive()
    }

    /// `true` when this point measures Gas.
    #[must_use]
    pub fn is_gas(&self) -> bool {
        matches!(self.sparte, crate::interval::Sparte::Gas)
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{interval::Sparte, obis::ObisCode};
    use time::macros::date;

    fn bezug_point() -> MeasurementPoint {
        MeasurementPoint {
            malo_id: "51238696780".to_owned(),
            melo_id: Some("DE0012345678901234567890123456789".to_owned()),
            meter_serial: Some("MSN-001".to_owned()),
            obis_code: ObisCode::STROM_BEZUG_TOTAL,
            sparte: Sparte::Strom,
            energy_flow: EnergyFlow::Consumption,
            accountable_role: MarktRolle::Lf,
            accountable_mp_id: "9900987654321".to_owned(),
            is_virtual: false,
            valid_from: date!(2026 - 01 - 01),
            valid_to: None,
        }
    }

    #[test]
    fn active_within_validity() {
        let mp = bezug_point();
        assert!(mp.is_active(date!(2026 - 06 - 15)));
    }

    #[test]
    fn inactive_before_valid_from() {
        let mp = bezug_point();
        assert!(!mp.is_active(date!(2025 - 12 - 31)));
    }

    #[test]
    fn inactive_after_valid_to() {
        let mut mp = bezug_point();
        mp.valid_to = Some(date!(2026 - 06 - 30));
        assert!(!mp.is_active(date!(2026 - 07 - 01)));
        assert!(mp.is_active(date!(2026 - 06 - 30)));
    }

    #[test]
    fn bezug_detection() {
        let mp = bezug_point();
        assert!(mp.is_bezug());
        assert!(!mp.is_einspeisung());
    }

    #[test]
    fn einspeisung_detection() {
        let mut mp = bezug_point();
        mp.obis_code = ObisCode::STROM_EINSPEISUNG_TOTAL;
        mp.energy_flow = EnergyFlow::Generation;
        assert!(mp.is_einspeisung());
        assert!(!mp.is_bezug());
    }

    #[test]
    fn gas_detection() {
        let mut mp = bezug_point();
        mp.sparte = Sparte::Gas;
        mp.obis_code = ObisCode::GAS_VOLUME_M3;
        assert!(mp.is_gas());
    }

    #[test]
    fn virtual_meter_flag() {
        let mut mp = bezug_point();
        mp.is_virtual = true;
        assert!(mp.is_virtual);
    }

    #[test]
    fn markt_rolle_mscons_receiver() {
        assert!(MarktRolle::Lf.is_mscons_receiver());
        assert!(MarktRolle::Bkv.is_mscons_receiver());
        assert!(!MarktRolle::Nb.is_mscons_receiver());
        assert!(!MarktRolle::Msb.is_mscons_receiver());
    }
}
