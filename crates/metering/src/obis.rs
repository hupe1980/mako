//! Typed OBIS (Object Identification System) codes for German energy metering.
//!
//! ## Standard
//!
//! IEC 62056-21 / DLMS-COSEM, as adopted by BDEW and Messstellenbetreiber
//! in Germany. OBIS codes identify measurement channels on smart meters and
//! data communication systems.
//!
//! ## Format
//!
//! ```text
//! A-B:C.D.E*F
//! │ │ │ │ │ └ F  value group (storage number, 0 = current)
//! │ │ │ │ └── E  tariff (0 = total, 1 = HT, 2 = NT, …)
//! │ │ │ └──── D  measurement type (8 = forward, 9 = reverse, 0 = combined)
//! │ │ └────── C  quantity (1 = active energy, 3 = reactive energy, 7 = gas volume)
//! │ └──────── B  channel (0 = sum, otherwise sub-meter)
//! └────────── A  medium (0 = abstract, 1 = electricity, 7 = gas, 8 = heat/water)
//! ```
//!
//! ## Commonly used codes in German MaKo
//!
//! | Code | Description |
//! |---|---|
//! | `1-0:1.8.0*255` | Electricity forward active energy total (kWh) |
//! | `1-0:1.8.1*255` | Electricity forward active energy register 1 (HT) |
//! | `1-0:1.8.2*255` | Electricity forward active energy register 2 (NT) |
//! | `1-0:2.8.0*255` | Electricity reverse active energy (Einspeisung) |
//! | `1-0:1.29.0*255` | Electricity demand (kW, 15-min average) |
//! | `1-0:3.8.0*255` | Electricity reactive energy inductive (kvarh) |
//! | `1-0:4.8.0*255` | Electricity reactive energy capacitive (kvarh) |
//! | `7-0:3.0.0*255` | Gas volume (m³) |
//! | `8-0:1.0.0*255` | Heat energy (kWh_th) |

use std::fmt;
use std::str::FromStr;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A parsed OBIS code: `A-B:C.D.E*F`.
///
/// All six value groups are stored. The canonical serialised form is the
/// full `A-B:C.D.E*F` string; parsing is lenient about the storage number
/// suffix (`*F` — defaults to 255 when absent).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(try_from = "&str", into = "String"))]
pub struct ObisCode {
    /// Medium: 1 = electricity, 7 = gas, 8 = heat/cold.
    pub a: u8,
    /// Channel: 0 = sum channel, 1–n = sub-channel.
    pub b: u8,
    /// Quantity: 1 = active power/energy, 3/4 = reactive energy, 7 = gas volume.
    pub c: u8,
    /// Measurement: 8 = import/forward, 9 = export/reverse, 0 = combined.
    pub d: u8,
    /// Tariff: 0 = total, 1 = HT register, 2 = NT register.
    pub e: u8,
    /// Storage: 255 = current value, 0 = default.
    pub f: u8,
}

impl ObisCode {
    /// Electricity forward active energy — total (Bezug, kWh, gesamt).
    /// HT + NT combined.
    pub const STROM_BEZUG_TOTAL: Self = Self {
        a: 1,
        b: 0,
        c: 1,
        d: 8,
        e: 0,
        f: 255,
    };

    /// Electricity forward active energy — register 1 (HT, Hochtarif).
    pub const STROM_BEZUG_HT: Self = Self {
        a: 1,
        b: 0,
        c: 1,
        d: 8,
        e: 1,
        f: 255,
    };

    /// Electricity forward active energy — register 2 (NT, Niedertarif).
    pub const STROM_BEZUG_NT: Self = Self {
        a: 1,
        b: 0,
        c: 1,
        d: 8,
        e: 2,
        f: 255,
    };

    /// Electricity reverse active energy — total (Einspeisung, kWh, gesamt).
    pub const STROM_EINSPEISUNG_TOTAL: Self = Self {
        a: 1,
        b: 0,
        c: 2,
        d: 8,
        e: 0,
        f: 255,
    };

    /// Electricity reverse active energy — register 1 (HT Einspeisung).
    pub const STROM_EINSPEISUNG_HT: Self = Self {
        a: 1,
        b: 0,
        c: 2,
        d: 8,
        e: 1,
        f: 255,
    };

    /// Electricity reverse active energy — register 2 (NT Einspeisung).
    pub const STROM_EINSPEISUNG_NT: Self = Self {
        a: 1,
        b: 0,
        c: 2,
        d: 8,
        e: 2,
        f: 255,
    };

    /// Active power demand, average over interval — 15-min Leistungsmittelwert (kW).
    pub const STROM_DEMAND_INTERVAL: Self = Self {
        a: 1,
        b: 0,
        c: 1,
        d: 29,
        e: 0,
        f: 255,
    };

    /// Reactive energy inductive (kvarh) — import direction.
    ///
    /// Used for RLM power quality monitoring and reactive energy billing.
    /// Source: IEC 62056-21, BDEW Lastenheft Smart Meter Gateway.
    pub const STROM_REACTIVE_INDUCTIVE: Self = Self {
        a: 1,
        b: 0,
        c: 3,
        d: 8,
        e: 0,
        f: 255,
    };

    /// Reactive energy capacitive (kvarh) — import direction.
    pub const STROM_REACTIVE_CAPACITIVE: Self = Self {
        a: 1,
        b: 0,
        c: 4,
        d: 8,
        e: 0,
        f: 255,
    };

    /// Reactive energy inductive (kvarh) — export direction.
    pub const STROM_REACTIVE_INDUCTIVE_EXPORT: Self = Self {
        a: 1,
        b: 0,
        c: 3,
        d: 9,
        e: 0,
        f: 255,
    };

    /// Reactive energy capacitive (kvarh) — export direction.
    pub const STROM_REACTIVE_CAPACITIVE_EXPORT: Self = Self {
        a: 1,
        b: 0,
        c: 4,
        d: 9,
        e: 0,
        f: 255,
    };

    /// Gas volume (m³, not yet converted to kWh_Hs).
    pub const GAS_VOLUME_M3: Self = Self {
        a: 7,
        b: 0,
        c: 3,
        d: 0,
        e: 0,
        f: 255,
    };

    /// Heat / cold energy (kWh_th).
    pub const WAERME_ENERGY: Self = Self {
        a: 8,
        b: 0,
        c: 1,
        d: 0,
        e: 0,
        f: 255,
    };

    /// `true` when this code refers to electricity (medium A = 1).
    #[must_use]
    pub fn is_electricity(&self) -> bool {
        self.a == 1
    }

    /// `true` when this code refers to gas (medium A = 7).
    #[must_use]
    pub fn is_gas(&self) -> bool {
        self.a == 7
    }

    /// `true` when this code refers to heat/cold (medium A = 8).
    #[must_use]
    pub fn is_heat(&self) -> bool {
        self.a == 8
    }

    /// `true` when this code measures reactive energy (C = 3 or C = 4 in IEC 62056-21).
    ///
    /// Reactive energy (kvarh) is required for:
    /// - Power factor billing (Blindstromberechnung) in industrial tariffs
    /// - Smart Meter Gateway power quality monitoring (BSI TR-03109)
    /// - RLM power quality records
    #[must_use]
    pub fn is_reactive(&self) -> bool {
        matches!(self.c, 3 | 4)
    }

    /// `true` when this code measures power demand (C = 1, D = 29 = max demand).
    ///
    /// Demand intervals (15-min Leistungsmittelwert) are used for:
    /// - Spitzenleistung billing (§18 Abs. 1 StromNEV)
    /// - RLM peak demand tracking
    #[must_use]
    pub fn is_demand(&self) -> bool {
        self.c == 1 && self.d == 29
    }

    /// `true` when this code represents import (forward) energy.
    ///
    /// In IEC 62056-21 / German OBIS practice:
    /// - `C=1, D=8` = forward active energy (Bezug, kWh import)
    /// - `D=8` always means "forward / import direction" in the measurement type field
    #[must_use]
    pub fn is_import(&self) -> bool {
        self.d == 8 && self.c == 1
    }

    /// `true` when this code represents reverse / export energy (Einspeisung).
    ///
    /// In IEC 62056-21 / German OBIS practice:
    /// - `C=2, D=8` = reverse active energy (Einspeisung, kWh export)
    /// - Note: `D=9` (reverse direction flag) is rarely used in German practice;
    ///   use `is_einspeisung()` for the commonly-used C=2 check.
    #[must_use]
    pub fn is_export(&self) -> bool {
        self.c == 2 && self.d == 8
    }

    /// `true` when this code represents Einspeisung (feed-in to the grid).
    ///
    /// Alias for `is_export()` using the German market terminology.
    /// Identifies `1-0:2.8.x*255` codes (reverse active energy).
    #[must_use]
    pub fn is_einspeisung(&self) -> bool {
        self.is_export()
    }

    /// Tariff register: `None` = total/combined, `Some(1)` = HT, `Some(2)` = NT.
    #[must_use]
    pub fn tariff_register(&self) -> Option<u8> {
        match self.e {
            0 => None,
            n => Some(n),
        }
    }

    /// `true` when this is the total / combined register (E = 0).
    #[must_use]
    pub fn is_total_register(&self) -> bool {
        self.e == 0
    }

    /// `true` when this is the HT (Hochtarif) register (E = 1).
    #[must_use]
    pub fn is_ht(&self) -> bool {
        self.e == 1
    }

    /// `true` when this is the NT (Niedertarif) register (E = 2).
    #[must_use]
    pub fn is_nt(&self) -> bool {
        self.e == 2
    }

    /// Default expected interval resolution for this OBIS code.
    ///
    /// RLM and iMSys electricity meters use 15-minute intervals.
    /// Gas meters and SLP typically use hourly or daily totals.
    /// Demand registers (D = 29) are always 15-minute.
    ///
    /// Returns `None` for codes where no standard resolution applies
    /// (e.g. cumulative registers, status codes).
    #[must_use]
    pub fn default_resolution(&self) -> Option<crate::resolution::IntervalResolution> {
        use crate::resolution::IntervalResolution;
        // Active energy electricity — RLM / iMSys: 15 min
        if self.a == 1 && (self.c == 1 || self.c == 2) && self.d == 8 {
            return Some(IntervalResolution::QuarterHour);
        }
        // Demand register (15-min average power)
        if self.a == 1 && self.d == 29 {
            return Some(IntervalResolution::QuarterHour);
        }
        // Reactive energy — usually 15-min alongside active
        if self.a == 1 && (self.c == 3 || self.c == 4) {
            return Some(IntervalResolution::QuarterHour);
        }
        // Gas volume — typically hourly or daily (SLP: daily, RLM Gas: hourly)
        if self.a == 7 {
            return Some(IntervalResolution::Hour);
        }
        // Heat / cold energy — usually hourly
        if self.a == 8 {
            return Some(IntervalResolution::Hour);
        }
        None
    }

    /// Parse from a string slice (lenient: storage number `*F` is optional).
    ///
    /// # Errors
    ///
    /// Returns an `ObisParseError` when the string does not conform to OBIS format.
    pub fn parse(s: &str) -> Result<Self, ObisParseError> {
        s.parse()
    }
}

impl fmt::Display for ObisCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}:{}.{}.{}*{}",
            self.a, self.b, self.c, self.d, self.e, self.f
        )
    }
}

impl From<ObisCode> for String {
    fn from(o: ObisCode) -> String {
        o.to_string()
    }
}

/// Error returned when an OBIS code string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObisParseError(pub String);

impl fmt::Display for ObisParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid OBIS code {:?}: expected A-B:C.D.E*F", self.0)
    }
}

impl std::error::Error for ObisParseError {}

impl FromStr for ObisCode {
    type Err = ObisParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_obis(s).ok_or_else(|| ObisParseError(s.to_owned()))
    }
}

impl TryFrom<&str> for ObisCode {
    type Error = ObisParseError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        s.parse()
    }
}

// ── Lenient parser ────────────────────────────────────────────────────────────

fn parse_obis(s: &str) -> Option<ObisCode> {
    // Split A-B:C.D.E*F
    let (a_b, rest) = s.split_once('-')?;
    let (b_str, cd_ef) = rest.split_once(':')?;
    let a: u8 = a_b.parse().ok()?;
    let b: u8 = b_str.parse().ok()?;

    // C.D.E*F or C.D.E (storage optional)
    let (cde_str, f_str) = match cd_ef.split_once('*') {
        Some((l, r)) => (l, r),
        None => (cd_ef, "255"),
    };
    let f: u8 = f_str.parse().ok()?;

    let parts: Vec<&str> = cde_str.splitn(3, '.').collect();
    if parts.len() != 3 {
        return None;
    }
    let c: u8 = parts[0].parse().ok()?;
    let d: u8 = parts[1].parse().ok()?;
    let e: u8 = parts[2].parse().ok()?;

    Some(ObisCode { a, b, c, d, e, f })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_standard_strom_bezug() {
        let code: ObisCode = "1-0:1.8.0*255".parse().unwrap();
        assert_eq!(code, ObisCode::STROM_BEZUG_TOTAL);
        assert!(code.is_electricity());
        assert!(code.is_import(), "1-0:1.8.0 C=1,D=8 should be import");
        assert!(code.is_total_register());
        assert!(!code.is_export());
    }

    #[test]
    fn parse_ht_register() {
        let code: ObisCode = "1-0:1.8.1*255".parse().unwrap();
        assert_eq!(code, ObisCode::STROM_BEZUG_HT);
        assert!(code.is_ht());
        assert!(!code.is_nt());
        assert_eq!(code.tariff_register(), Some(1));
    }

    #[test]
    fn parse_without_storage_number() {
        // *255 is optional — the parser defaults F to 255
        let code: ObisCode = "1-0:2.8.0".parse().unwrap();
        assert_eq!(code.f, 255);
        assert_eq!(code.c, 2);
        assert_eq!(code.d, 8);
        // C=2, D=8 = reverse active energy (Einspeisung)
        assert!(
            code.is_export(),
            "1-0:2.8.0 should be Einspeisung/export (C=2, D=8)"
        );
        assert!(code.is_einspeisung(), "should alias is_export()");
        assert_eq!(code, ObisCode::STROM_EINSPEISUNG_TOTAL);
    }

    #[test]
    fn parse_gas_volume() {
        let code: ObisCode = "7-0:3.0.0*255".parse().unwrap();
        assert_eq!(code, ObisCode::GAS_VOLUME_M3);
        assert!(code.is_gas());
        assert!(!code.is_electricity());
    }

    #[test]
    fn display_round_trip() {
        let s = "1-0:1.8.2*255";
        let code: ObisCode = s.parse().unwrap();
        assert_eq!(code.to_string(), s);
    }

    #[test]
    fn invalid_code_returns_error() {
        assert!("not-an-obis".parse::<ObisCode>().is_err());
        assert!("1-0:1.8".parse::<ObisCode>().is_err());
        assert!("".parse::<ObisCode>().is_err());
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(ObisCode::STROM_BEZUG_TOTAL.to_string(), "1-0:1.8.0*255");
        assert_eq!(ObisCode::STROM_BEZUG_HT.to_string(), "1-0:1.8.1*255");
        assert_eq!(ObisCode::STROM_BEZUG_NT.to_string(), "1-0:1.8.2*255");
        assert_eq!(
            ObisCode::STROM_EINSPEISUNG_TOTAL.to_string(),
            "1-0:2.8.0*255"
        );
        assert_eq!(ObisCode::GAS_VOLUME_M3.to_string(), "7-0:3.0.0*255");
    }
}
