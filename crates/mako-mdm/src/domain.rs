#![allow(clippy::doc_markdown)]
//! Domain identifier types and shared enums.
//!
//! Identifier types (`MaloId`, `MeloId`, `MarktpartnerId` / `Gln`) are
//! re-exported from [`rubo4e::identifiers`] — validated at construction time
//! via `TryFrom` / `FromStr`.  No hand-rolled validation lives here.

use serde::{Deserialize, Serialize};

// ── Identifier re-exports ─────────────────────────────────────────────────────

pub use rubo4e::identifiers::{MaloId, MarktpartnerId, MeloId};

/// Type alias: `Gln` is a 13-digit BDEW/DVGW/GS1 Marktpartner-ID.
///
/// Same type as `MarktpartnerId`; the `Gln` alias is kept for code that uses
/// the EDIFACT GLN terminology.  Validates 13 ASCII digits at construction
/// (via `TryFrom<&str>` / `FromStr`).
///
/// # Construction
///
/// ```rust
/// use mako_mdm::domain::Gln;
///
/// let g: Gln = "9900357000004".parse().expect("valid 13-digit GLN");
/// assert_eq!(mako_mdm::domain::nad_agency_code(&g), "293");
/// ```
pub type Gln = MarktpartnerId;

/// Derive the NAD DE3055 agency code from the GLN prefix.
///
/// | Prefix | Agency | Source |
/// |--------|--------|--------|
/// | `99`   | `"293"` | BDEW Strom (NAD DE3055) |
/// | `98`   | `"332"` | DVGW Gas (NAD DE3055) |
/// | other 13-digit | `"9"` | GS1 (NAD DE3055) |
#[must_use]
pub fn nad_agency_code(id: &MarktpartnerId) -> &'static str {
    let s: &str = id.as_ref();
    if s.starts_with("99") {
        "293"
    } else if s.starts_with("98") {
        "332"
    } else {
        "9"
    }
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
    fn gln_valid_gs1() {
        // 13-digit GLN not starting with 98/99 → GS1 agency code "9"
        let g: Gln = "1234567890128".parse().unwrap();
        assert_eq!(nad_agency_code(&g), "9");
    }

    #[test]
    fn gln_valid_bdew() {
        let g: Gln = "9900357000004".parse().unwrap();
        assert_eq!(nad_agency_code(&g), "293");
    }

    #[test]
    fn gln_valid_dvgw() {
        let g: Gln = "9800001000003".parse().unwrap();
        assert_eq!(nad_agency_code(&g), "332");
    }

    #[test]
    fn gln_invalid_length() {
        assert!("123".parse::<Gln>().is_err());
    }
}
