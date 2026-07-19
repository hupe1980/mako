//! German Standard Load Profiles (Standardlastprofile, SLP).
//!
//! ## Legal basis
//!
//! - **BDEW Repräsentative VDEW-Lastprofile** (1999, updated 2015): defines
//!   the standard profiles used for German SLP metering.
//! - **§12 StromNZV / GaBi Gas 2.1 (BK7-24-01-008)**: the duty to apply standardised load
//!   profiles below 100 000 kWh/a (Strom) and 1.5 million kWh/a (Gas).
//!   ⚠️ Both ordinances were **repealed with effect from the end of
//!   31.12.2025** (Art. 15 Abs. 4 des Gesetzes vom 22.12.2023, BGBl. 2023 I
//!   Nr. 405); the substance now lives in BNetzA Festlegungen.
//!   (Neither StromGVV nor GasGVV ever governed load profiles — §18 of each is
//!   "Berechnungsfehler".)
//! - **BK6-22-024 (GPKE)**: SLP MaLos use profiles for advance billing and MaBiS.
//!
//! ## Profile families
//!
//! | Family | Usage | Commodity |
//! |---|---|---|
//! | H0 | Residential households | Electricity |
//! | G0–G6 | Commercial, various sub-types | Electricity |
//! | L0–L2 | Agricultural (Landwirtschaft) | Electricity |
//! | P0 | Pumping stations | Electricity |
//! | SLP-G | Gas standard profiles | Gas |
//!
//! ## Usage
//!
//! The `LoadProfile` type classifies MaLos
//! and drives the billing-period aggregation method in SLP billing runs.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// German Standard Load Profile identifier.
///
/// These are the official BDEW Standardlastprofile for electricity and gas.
/// Carried on the MaLo master record and on each billing period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum LoadProfile {
    // ── Electricity profiles ──────────────────────────────────────────────────
    /// H0 — Haushalt (residential household).
    ///
    /// Most common profile. Used for residential customers without RLM.
    /// Demand pattern peaks in morning and evening.
    H0,

    /// G0 — Gewerbe allgemein (general commercial).
    ///
    /// Catch-all for commercial customers not covered by G1–G6.
    G0,

    /// G1 — Gewerbe wochentags (weekday-heavy commercial, 08:00–18:00 CET).
    ///
    /// Offices, small service businesses. High load Mon–Fri only.
    G1,

    /// G2 — Gewerbe mit starkem Verbrauch abends (evening-heavy commercial).
    ///
    /// Restaurants, entertainment. Load peak in evenings.
    G2,

    /// G3 — Gewerbe durchlaufend (continuous round-the-clock commercial).
    ///
    /// Bakeries, 24/7 operations. Nearly flat profile.
    G3,

    /// G4 — Laden/Friseur (retail shop with strong Saturday peak).
    G4,

    /// G5 — Bäckerei mit Backstube (bakery with overnight baking).
    ///
    /// Highest load in early morning hours.
    G5,

    /// G6 — Wochenendbetrieb (weekend-only operation, e.g. campsite).
    G6,

    /// L0 — Landwirtschaft allgemein (general agricultural).
    ///
    /// Mixed agricultural use. High load during harvest season.
    L0,

    /// L1 — Landwirtschaft mit Milchwirtschaft (dairy farming).
    ///
    /// Regular milking schedule. Load peaks around 05:00 and 17:00.
    L1,

    /// L2 — Landwirtschaft Sonstige (other agricultural without milking).
    L2,

    /// P0 — Pumpen (pumping stations).
    ///
    /// Relatively flat profile. Used for pumping/water supply.
    P0,

    // ── Gas profiles ─────────────────────────────────────────────────────────
    /// EF — Einfamilienhaus Gas (single-family residential gas).
    ///
    /// Standard BDEW gas profile for residential heating customers.
    GasEF,

    /// MF — Mehrfamilienhaus Gas (multi-family residential gas).
    GasMF,

    /// GHD — Gewerbe, Handel, Dienstleistungen Gas (commercial gas).
    GasGHD,

    // ── Legacy / other ────────────────────────────────────────────────────────
    /// Custom profile — not a standard BDEW profile.
    /// The profile name is stored separately in the MaLo record.
    Custom,
}

impl LoadProfile {
    /// The canonical BDEW profile identifier string.
    ///
    /// Used in UTILMD `MR+Z07`/`MR+Z08` segments and the MaLo lastprofil field.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H0 => "H0",
            Self::G0 => "G0",
            Self::G1 => "G1",
            Self::G2 => "G2",
            Self::G3 => "G3",
            Self::G4 => "G4",
            Self::G5 => "G5",
            Self::G6 => "G6",
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::P0 => "P0",
            Self::GasEF => "EF",
            Self::GasMF => "MF",
            Self::GasGHD => "GHD",
            Self::Custom => "CUSTOM",
        }
    }

    /// Parse from the BDEW profile identifier string.
    ///
    /// Returns `None` for unknown profile codes.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_uppercase().as_str() {
            "H0" => Some(Self::H0),
            "G0" => Some(Self::G0),
            "G1" => Some(Self::G1),
            "G2" => Some(Self::G2),
            "G3" => Some(Self::G3),
            "G4" => Some(Self::G4),
            "G5" => Some(Self::G5),
            "G6" => Some(Self::G6),
            "L0" => Some(Self::L0),
            "L1" => Some(Self::L1),
            "L2" => Some(Self::L2),
            "P0" => Some(Self::P0),
            "EF" => Some(Self::GasEF),
            "MF" => Some(Self::GasMF),
            "GHD" => Some(Self::GasGHD),
            _ => None,
        }
    }

    /// `true` when this is a residential profile (H0 or Gas EF/MF).
    #[must_use]
    pub fn is_residential(self) -> bool {
        matches!(self, Self::H0 | Self::GasEF | Self::GasMF)
    }

    /// `true` when this is a commercial profile (G0–G6 or GHD).
    #[must_use]
    pub fn is_commercial(self) -> bool {
        matches!(
            self,
            Self::G0
                | Self::G1
                | Self::G2
                | Self::G3
                | Self::G4
                | Self::G5
                | Self::G6
                | Self::GasGHD
        )
    }

    /// `true` when this is an agricultural profile (L0–L2).
    #[must_use]
    pub fn is_agricultural(self) -> bool {
        matches!(self, Self::L0 | Self::L1 | Self::L2)
    }

    /// `true` when this is a gas SLP profile.
    #[must_use]
    pub fn is_gas(self) -> bool {
        matches!(self, Self::GasEF | Self::GasMF | Self::GasGHD)
    }
}

impl std::fmt::Display for LoadProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl std::str::FromStr for LoadProfile {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        LoadProfile::parse(s).ok_or_else(|| format!("unknown load profile: {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_all_profiles() {
        let profiles = [
            LoadProfile::H0,
            LoadProfile::G0,
            LoadProfile::G1,
            LoadProfile::G2,
            LoadProfile::G3,
            LoadProfile::G4,
            LoadProfile::G5,
            LoadProfile::G6,
            LoadProfile::L0,
            LoadProfile::L1,
            LoadProfile::L2,
            LoadProfile::P0,
            LoadProfile::GasEF,
            LoadProfile::GasMF,
            LoadProfile::GasGHD,
        ];
        for p in &profiles {
            let s = p.as_str();
            let parsed = LoadProfile::parse(s).expect("should round-trip");
            assert_eq!(*p, parsed, "round-trip failed for {s}");
        }
    }

    #[test]
    fn residential_classification() {
        assert!(LoadProfile::H0.is_residential());
        assert!(LoadProfile::GasEF.is_residential());
        assert!(!LoadProfile::G0.is_residential());
    }

    #[test]
    fn commercial_classification() {
        for p in [
            LoadProfile::G0,
            LoadProfile::G1,
            LoadProfile::G2,
            LoadProfile::G3,
            LoadProfile::G4,
            LoadProfile::G5,
            LoadProfile::G6,
        ] {
            assert!(p.is_commercial(), "{p} should be commercial");
        }
        assert!(!LoadProfile::H0.is_commercial());
    }

    #[test]
    fn gas_classification() {
        assert!(LoadProfile::GasEF.is_gas());
        assert!(LoadProfile::GasMF.is_gas());
        assert!(LoadProfile::GasGHD.is_gas());
        assert!(!LoadProfile::H0.is_gas());
    }

    #[test]
    fn case_insensitive_parse() {
        assert_eq!(LoadProfile::parse("h0"), Some(LoadProfile::H0));
        assert_eq!(LoadProfile::parse("H0"), Some(LoadProfile::H0));
        assert_eq!(LoadProfile::parse("ef"), Some(LoadProfile::GasEF));
    }

    #[test]
    fn unknown_profile_returns_none() {
        assert_eq!(LoadProfile::parse("X9"), None);
        assert_eq!(LoadProfile::parse(""), None);
    }
}
