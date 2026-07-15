//! Meter register model — physical meter ↔ OBIS binding.
//!
//! A [`MeterRegister`] ties a physical meter (by serial number) to a single
//! measurement channel (register) identified by its OBIS code. It is the
//! structural metadata layer above [`crate::MeterInterval`].
//!
//! ## Relationship to MSCONS and OBIS
//!
//! In German MSCONS messages, each time series is identified by:
//! - A `PIA` segment containing the OBIS code
//! - A `NAD` segment with the Messtechnische Einrichtungs-ID (MeLo-ID or meter serial)
//!
//! `MeterRegister` binds these two identifiers together for a specific MaLo.
//!
//! ## Example: dual-tariff meter (HT/NT)
//!
//! A typical HT/NT electricity meter has three registers:
//! - Register 0 (`1-0:1.8.0*255`) — total energy (sum)
//! - Register 1 (`1-0:1.8.1*255`) — HT energy
//! - Register 2 (`1-0:1.8.2*255`) — NT energy
//!
//! Each register maps to a separate `MeterInterval` time series in MSCONS.

use rust_decimal::Decimal;
use time::Date;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::obis::ObisCode;

/// Energy flow direction of a meter register.
///
/// Maps to the OBIS `D` field: `8` = import/forward, `9` = export/reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum EnergyDirection {
    /// Forward energy (Import / Bezug) — electricity consumed from grid.
    Import,
    /// Reverse energy (Export / Einspeisung) — electricity fed into grid.
    Export,
    /// Combined / bidirectional — net metering at a prosumer connection point.
    Combined,
}

impl EnergyDirection {
    /// Human-readable German label.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Import => "Bezug",
            Self::Export => "Einspeisung",
            Self::Combined => "Kombiniert",
        }
    }
}

/// Physical unit of measurement for a meter register.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "SCREAMING_SNAKE_CASE"))]
pub enum RegisterUnit {
    /// Active energy (kWh).
    KWh,
    /// Reactive energy (kvarh).
    KVarh,
    /// Active power demand (kW) — for 15-min demand registers.
    KW,
    /// Reactive power demand (kvar).
    KVar,
    /// Gas volume (m³) — before conversion to kWh_Hs.
    M3,
    /// Thermal energy (kWh_th).
    KWhTh,
}

impl RegisterUnit {
    /// SI unit symbol.
    #[must_use]
    pub fn symbol(self) -> &'static str {
        match self {
            Self::KWh => "kWh",
            Self::KVarh => "kvarh",
            Self::KW => "kW",
            Self::KVar => "kvar",
            Self::M3 => "m³",
            Self::KWhTh => "kWh_th",
        }
    }
}

/// A single measurement register on a physical meter, bound to a MaLo.
///
/// ## Wandlerfaktor (current transformer multiplier)
///
/// For Wandlermessungen (metering via current transformers), the raw meter
/// display must be multiplied by the `wandler_factor` (e.g. 100, 600) to
/// obtain actual kWh values. All `MeterInterval` values stored in `edmd`
/// **must already have the wandler factor applied** — this field is metadata
/// for traceability and audit only.
///
/// ## Dual-tariff configuration
///
/// A dual-tariff meter with HT/NT registers exposes three `MeterRegister` rows
/// per MaLo (registers 0, 1, 2). The MSCONS message sends separate time series
/// for each register, identified by OBIS `E` field.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct MeterRegister {
    /// 11-digit Marktlokations-ID this register serves.
    pub malo_id: String,

    /// Physical meter serial number (Zähler-Seriennummer / Geräteidentifikation).
    ///
    /// Corresponds to the meter device ID in MSCONS `NAD` segments and in
    /// the Geräte-ID field of `marktd` device registry.
    pub meter_serial: String,

    /// Register number (0–9). Matches OBIS `E` (tariff) field.
    ///
    /// - 0 = total (HT + NT combined, or single-tariff)
    /// - 1 = register 1 (Hochtarif, HT)
    /// - 2 = register 2 (Niedertarif, NT)
    pub register_number: u8,

    /// OBIS code identifying this measurement channel (e.g. `1-0:1.8.1*255` for HT).
    pub obis_code: ObisCode,

    /// Energy flow direction for this register.
    pub direction: EnergyDirection,

    /// Physical unit of measurement.
    pub unit: RegisterUnit,

    /// Multiplier applied to raw meter counter readings to obtain kWh.
    ///
    /// Direct metering: `1.0`. Wandlermessung: typically 100–1000.
    /// All `MeterInterval` values in `edmd` already have this applied.
    pub wandler_factor: Decimal,

    /// Date from which this register configuration is valid (German local date).
    pub valid_from: Date,

    /// Date until which this register configuration is valid (inclusive).
    ///
    /// `None` = still active.
    pub valid_to: Option<Date>,
}

impl MeterRegister {
    /// `true` when this register is an HT (Hochtarif) register (OBIS E = 1).
    #[must_use]
    pub fn is_ht(&self) -> bool {
        self.obis_code.is_ht()
    }

    /// `true` when this register is an NT (Niedertarif) register (OBIS E = 2).
    #[must_use]
    pub fn is_nt(&self) -> bool {
        self.obis_code.is_nt()
    }

    /// `true` when this is a total register (OBIS E = 0, HT + NT sum).
    #[must_use]
    pub fn is_total(&self) -> bool {
        self.obis_code.is_total_register()
    }

    /// `true` when this register measures import energy (Bezug from grid).
    #[must_use]
    pub fn is_import(&self) -> bool {
        matches!(
            self.direction,
            EnergyDirection::Import | EnergyDirection::Combined
        ) || self.obis_code.is_import()
    }

    /// `true` when this register measures exported energy (Einspeisung into grid).
    #[must_use]
    pub fn is_einspeisung(&self) -> bool {
        matches!(self.direction, EnergyDirection::Export) || self.obis_code.is_einspeisung()
    }

    /// `true` when this register is currently active (valid_to is None or in the future).
    #[must_use]
    pub fn is_active(&self, today: Date) -> bool {
        self.valid_to.is_none_or(|end| end >= today)
    }

    /// Apply the wandler factor to a raw meter reading.
    ///
    /// Converts the counter display value to actual energy in the register's unit.
    #[must_use]
    pub fn apply_wandler(&self, raw_value: Decimal) -> Decimal {
        raw_value * self.wandler_factor
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::obis::ObisCode;
    use rust_decimal_macros::dec;
    use time::macros::date;

    fn make_register(obis: ObisCode, register_number: u8) -> MeterRegister {
        MeterRegister {
            malo_id: "11X0-0000-0000-1".to_owned(),
            meter_serial: "A1B2C3D4".to_owned(),
            register_number,
            obis_code: obis,
            direction: EnergyDirection::Import,
            unit: RegisterUnit::KWh,
            wandler_factor: dec!(1),
            valid_from: date!(2024 - 01 - 01),
            valid_to: None,
        }
    }

    #[test]
    fn total_register_classification() {
        let reg = make_register(ObisCode::STROM_BEZUG_TOTAL, 0);
        assert!(reg.is_total());
        assert!(!reg.is_ht());
        assert!(!reg.is_nt());
    }

    #[test]
    fn ht_register_classification() {
        let reg = make_register(ObisCode::STROM_BEZUG_HT, 1);
        assert!(reg.is_ht());
        assert!(!reg.is_total());
        assert!(!reg.is_nt());
    }

    #[test]
    fn nt_register_classification() {
        let reg = make_register(ObisCode::STROM_BEZUG_NT, 2);
        assert!(reg.is_nt());
        assert!(!reg.is_ht());
    }

    #[test]
    fn wandler_factor_applied() {
        let mut reg = make_register(ObisCode::STROM_BEZUG_TOTAL, 0);
        reg.wandler_factor = dec!(100);
        let raw = dec!(1234);
        assert_eq!(reg.apply_wandler(raw), dec!(123400));
    }

    #[test]
    fn active_status_check() {
        let today = date!(2026 - 07 - 15);
        let mut reg = make_register(ObisCode::STROM_BEZUG_TOTAL, 0);
        assert!(reg.is_active(today));
        reg.valid_to = Some(date!(2026 - 01 - 01));
        assert!(!reg.is_active(today));
    }

    #[test]
    fn einspeisung_direction() {
        let mut reg = make_register(ObisCode::STROM_EINSPEISUNG_TOTAL, 0);
        reg.direction = EnergyDirection::Export;
        assert!(reg.is_einspeisung());
        assert!(!reg.is_import());
    }

    #[test]
    fn unit_symbols() {
        assert_eq!(RegisterUnit::KWh.symbol(), "kWh");
        assert_eq!(RegisterUnit::M3.symbol(), "m³");
        assert_eq!(RegisterUnit::KVarh.symbol(), "kvarh");
    }
}
