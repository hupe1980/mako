#![allow(clippy::doc_markdown)]
//! Domain identifier types and shared enums.
//!
//! Identifier types (`MaloId`, `MeloId`, `MarktpartnerId`) are
//! re-exported from [`rubo4e::identifiers`] — validated at construction time
//! via `TryFrom` / `FromStr`.  No hand-rolled validation lives here.
//!
//! # Marktpartner-ID vs. GLN
//!
//! In BO4E and the BDEW *Allgemeine Festlegungen*, the correct term for a
//! 13-digit market participant identifier is **`MarktpartnerId`**
//! (German: *Rollencodenummer*).  Three coding authorities issue these IDs:
//!
//! | Prefix | Authority | NAD DE3055 | UNB DE0007 |
//! |--------|-----------|------------|------------|
//! | `99…`  | BDEW-Codenummer (Strom) | `293` | `500` |
//! | `98…`  | DVGW-Codenummer (Gas)  | `332` | `502` |
//! | other  | GS1 **GLN**            | `9`   | `14`  |
//!
//! **Only** the GS1-issued identifiers are true GLNs.  BDEW and DVGW codes are
//! *not* GLNs.  Using "GLN" as a generic alias is therefore misleading;
//! use `MarktpartnerId` for the type and [`nad_agency_code()`] to resolve the
//! correct coding authority at EDIFACT encoding time.

use serde::{Deserialize, Serialize};

// ── Identifier re-exports ─────────────────────────────────────────────────────

pub use rubo4e::identifiers::{MaloId, MarktpartnerId, MeloId};

/// Derive the NAD DE3055 agency code from a `MarktpartnerId`.
///
/// | Prefix | Agency code | Standard |
/// |--------|-------------|----------|
/// | `99`   | `"293"` | BDEW-Codenummer Strom (NAD DE3055) |
/// | `98`   | `"332"` | DVGW-Codenummer Gas (NAD DE3055) |
/// | other 13-digit | `"9"` | GS1 GLN (NAD DE3055) |
///
/// **Note:** NAD DE3055 and UNB DE0007 use different values for the same
/// authority (`9` vs. `14` for GS1; `293` vs. `500` for BDEW).
/// This function returns NAD DE3055 values only.
/// Use [`MarktpartnerId::unb_agency_code`] for UNB DE0007.
///
/// This is a thin wrapper around [`MarktpartnerId::nad_agency_code`],
/// kept for call-site compatibility.  Prefer calling the method directly.
#[must_use]
#[inline]
pub fn nad_agency_code(id: &MarktpartnerId) -> &'static str {
    id.nad_agency_code()
}

// ── Sparte ───────────────────────────────────────────────────────────────────

/// Energy commodity type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Sparte {
    Strom,
    Gas,
}

impl std::fmt::Display for Sparte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Strom => write!(f, "STROM"),
            Self::Gas => write!(f, "GAS"),
        }
    }
}

impl std::str::FromStr for Sparte {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "STROM" => Ok(Self::Strom),
            "GAS" => Ok(Self::Gas),
            other => Err(format!("unknown Sparte '{other}'; expected STROM or GAS")),
        }
    }
}

// ── ProcessStatus ─────────────────────────────────────────────────────────────

/// Status of a correlated MaKo process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProcessStatus {
    Running,
    Completed,
    Failed,
}

impl std::fmt::Display for ProcessStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "RUNNING"),
            Self::Completed => write!(f, "COMPLETED"),
            Self::Failed => write!(f, "FAILED"),
        }
    }
}

impl std::str::FromStr for ProcessStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "RUNNING" => Ok(Self::Running),
            "COMPLETED" => Ok(Self::Completed),
            "FAILED" => Ok(Self::Failed),
            other => Err(format!("unknown process status: {other}")),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malo_id_valid() {
        // Known-good MaLo-ID (checksum 0)
        assert!("51238696780".parse::<MaloId>().is_ok());
    }

    #[test]
    fn malo_id_wrong_checksum() {
        assert!("51238696781".parse::<MaloId>().is_err());
    }

    #[test]
    fn malo_id_too_short() {
        assert!("1234567890".parse::<MaloId>().is_err());
    }

    #[test]
    fn melo_id_valid() {
        let id = "DE0001234567890123456789012345678";
        assert!(id.parse::<MeloId>().is_ok());
    }

    #[test]
    fn melo_id_invalid_prefix() {
        // rubo4e::MeloId accepts any ISO 3166-1 alpha-2 prefix (BO4E is international).
        // Truly invalid: starts with a digit (not uppercase ASCII letter).
        assert!(
            "1X0001234567890123456789012345678"
                .parse::<MeloId>()
                .is_err()
        );
    }

    #[test]
    fn melo_id_invalid_prefix_lowercase() {
        // Lowercase country codes are also invalid — must be uppercase ASCII letters.
        assert!(
            "de0001234567890123456789012345678"
                .parse::<MeloId>()
                .is_err()
        );
    }

    #[test]
    fn melo_id_invalid_length() {
        assert!("DE00012345678".parse::<MeloId>().is_err());
    }

    #[test]
    fn melo_id_non_de_prefix_valid() {
        // rubo4e follows the BO4E spec — any ISO 3166-1 alpha-2 country code is valid,
        // not just DE.  This documents the intentional break from the previous
        // hand-rolled parser that was restricted to "DE" only.
        assert!(
            "AT0001234567890123456789012345678"
                .parse::<MeloId>()
                .is_ok()
        );
    }

    #[test]
    fn marktpartner_id_gs1_gln() {
        // GS1 GLN: 13-digit not starting with 98/99 → NAD DE3055 agency code "9"
        let id: MarktpartnerId = "1234567890128".parse().unwrap();
        assert_eq!(nad_agency_code(&id), "9");
    }

    #[test]
    fn marktpartner_id_bdew() {
        // BDEW-Codenummer Strom (prefix 99) → NAD DE3055 agency code "293"
        let id: MarktpartnerId = "9900357000004".parse().unwrap();
        assert_eq!(nad_agency_code(&id), "293");
    }

    #[test]
    fn marktpartner_id_dvgw() {
        // DVGW-Codenummer Gas (prefix 98) → NAD DE3055 agency code "332"
        let id: MarktpartnerId = "9800001000003".parse().unwrap();
        assert_eq!(nad_agency_code(&id), "332");
    }

    #[test]
    fn marktpartner_id_invalid_length() {
        assert!("123".parse::<MarktpartnerId>().is_err());
    }
}
