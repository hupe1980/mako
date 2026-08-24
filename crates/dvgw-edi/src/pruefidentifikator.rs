//! The DVGW Prüfidentifikator and its published catalogue.
//!
//! DVGW messages **do** carry a Prüfidentifikator. It is not in `BGM` where BDEW
//! puts it — `SG1 RFF+Z13` DE 1153 is literally named „Prüfidentifikator" in
//! every Nachrichtenbeschreibung, and DE 1154 holds the code:
//!
//! ```text
//! RFF+Z13:70001'      ← ALOCAT: Allokation anhand von SLP (NB an MGV)
//! ```
//!
//! DVGW allocates from `70000–79999`, which does not overlap the BDEW ranges, so
//! a single PID router can carry both markets without a synthetic encoding.
//!
//! Source: DVGW-Nachrichtenbeschreibungen ALOCAT 5.11a §3.3/§4, NOMINT 4.6 §4,
//! NOMRES 4.7 §4.

use std::{fmt, str::FromStr};

use crate::document::DvgwMessageType;

/// A validated DVGW Prüfidentifikator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Pruefidentifikator(u32);

/// Lowest code DVGW allocates.
pub const PID_MIN: u32 = 70_000;
/// Highest code DVGW allocates.
pub const PID_MAX: u32 = 79_999;

impl Pruefidentifikator {
    /// Construct from a numeric code.
    ///
    /// Returns `None` outside the DVGW range `70000–79999`. Rejecting out-of-range
    /// values here is what keeps a Belegnummer that happens to be numeric from
    /// being mistaken for a process code.
    #[must_use]
    pub fn new(code: u32) -> Option<Self> {
        (PID_MIN..=PID_MAX).contains(&code).then_some(Self(code))
    }

    /// The numeric code.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// The catalogue entry for this code, when DVGW has published one.
    ///
    /// `None` means the code is inside the DVGW range but not in the packages
    /// this crate ships — a newly published Anwendungsfall, not an error.
    #[must_use]
    pub fn info(self) -> Option<&'static PidInfo> {
        CATALOGUE.iter().find(|e| e.pid == self.0)
    }

    /// The message family this code belongs to, when it is catalogued.
    #[must_use]
    pub fn message_type(self) -> Option<DvgwMessageType> {
        self.info().map(|e| e.message_type)
    }
}

impl fmt::Display for Pruefidentifikator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:05}", self.0)
    }
}

impl FromStr for Pruefidentifikator {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>().ok().and_then(Self::new).ok_or(())
    }
}

/// One published DVGW Anwendungsfall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PidInfo {
    /// The numeric Prüfidentifikator.
    pub pid: u32,
    /// The message family that carries it.
    pub message_type: DvgwMessageType,
    /// The Anwendungsfall description, verbatim from the Nachrichtenbeschreibung.
    pub description: &'static str,
    /// The communication direction, e.g. `"NB an MGV"`.
    pub direction: &'static str,
}

macro_rules! pid_catalogue {
    ($($pid:literal, $mt:ident, $desc:literal, $dir:literal;)*) => {
        &[$(PidInfo {
            pid: $pid,
            message_type: DvgwMessageType::$mt,
            description: $desc,
            direction: $dir,
        }),*]
    };
}

/// Every Prüfidentifikator published in the shipped Nachrichtentypen-Paket.
static CATALOGUE: &[PidInfo] = pid_catalogue![
    // ── ALOCAT 5.11a ─────────────────────────────────────────────────────────
    70001, Alocat, "Allokation anhand von Standardlastprofilen (SLP)", "NB an MGV";
    70002, Alocat, "Korrigierte Mengenmeldung NKP je Netzkonto", "NB an MGV";
    70003, Alocat, "Tägliche Mengenmeldung NKP je Netzkonto", "NB an MGV";
    70004, Alocat, "Vorläufige Allokation (Intraday)", "NB an MGV";
    70005, Alocat, "Endgültige Allokation (Bilanzierungsbrennwert)", "NB an MGV";
    70006, Alocat, "Korrigierte Allokation (Bilanzierungsbrennwert)", "NB an MGV";
    70007, Alocat, "Korrigierte Allokation (Abrechnungsbrennwert)", "NB an MGV";
    70008, Alocat, "SLP Clearing", "NB an MGV";
    70009, Alocat, "RLM Clearing (Bilanzierungsbrennwert)", "NB an MGV";
    70010, Alocat, "RLM Clearing (Abrechnungsbrennwert)", "NB an MGV";
    70011, Alocat, "Korrigierte Mengenmeldung NKP je Netzkonto", "ENB/ANB an NB";
    70012, Alocat, "Tägliche Mengenmeldung NKP je Netzkonto", "ENB/ANB an NB";
    70013, Alocat, "Allokation anhand von Standardlastprofilen (SLP)", "MGV an BKV";
    70014, Alocat, "Untertägige Allokation (Intraday)", "MGV an BKV";
    70015, Alocat, "Endgültige Allokation (Bilanzierungsbrennwert)", "MGV an BKV";
    70016, Alocat, "Korrigierte Allokation (Bilanzierungsbrennwert)", "MGV an BKV";
    70017, Alocat, "Korrigierte Allokation (Abrechnungsbrennwert)", "MGV an BKV";
    70018, Alocat, "SLP Clearing", "MGV an BKV";
    70019, Alocat, "RLM Clearing (Bilanzierungsbrennwert)", "MGV an BKV";
    70020, Alocat, "RLM Clearing (Abrechnungsbrennwert)", "MGV an BKV";
    70021, Alocat, "Ersatzwertversand an NB", "MGV an NB";
    70022, Alocat, "Optional auf Wunsch tägliche SLP Allokation", "NB an BKV";
    70023, Alocat, "Optional auf Wunsch monatlicher Datenrückversand je Netzkonto", "MGV an NB";
    // ── NOMINT 4.6 ───────────────────────────────────────────────────────────
    70030, Nomint, "Nominierung an einem physikalischen Punkt (ungebündelt)", "Transportkunde an NB";
    70031, Nomint, "Nominierung an einem virtuellen Handelspunkt", "Transportkunde an MGV";
    70032, Nomint, "Flexibilitätsübertragung", "Transportkunde an NB";
    70033, Nomint, "Gebündelte Nominierung", "Transportkunde an NB";
    70034, Nomint, "Nominierungsweitergabe zwischen Netzbetreibern", "NB an NB";
    // ── NOMRES 4.7 ───────────────────────────────────────────────────────────
    70035, Nomres, "Matching Benachrichtigung", "NB an Transportkunde";
    70036, Nomres, "Bestätigung", "NB an Transportkunde";
    70037, Nomres, "VHP Matching Benachrichtigung", "MGV an Transportkunde";
    70038, Nomres, "VHP Bestätigung", "MGV an Transportkunde";
    70039, Nomres, "Bestätigung Flexibilitätsübertragung", "NB an Transportkunde";
];

/// Every catalogued Anwendungsfall, ascending by Prüfidentifikator.
#[must_use]
pub fn catalogue() -> &'static [PidInfo] {
    CATALOGUE
}

/// The catalogued Anwendungsfälle for one message family.
pub fn catalogue_for(message_type: DvgwMessageType) -> impl Iterator<Item = &'static PidInfo> {
    CATALOGUE
        .iter()
        .filter(move |e| e.message_type == message_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_range_is_closed_and_excludes_bdew_codes() {
        assert_eq!(Pruefidentifikator::new(69_999), None);
        assert_eq!(Pruefidentifikator::new(80_000), None);
        // A BDEW UTILMD PID must never construct as a DVGW one.
        assert_eq!(Pruefidentifikator::new(55_001), None);
        assert!(Pruefidentifikator::new(70_001).is_some());
    }

    #[test]
    fn the_catalogue_is_sorted_unique_and_in_range() {
        let mut previous = 0;
        for entry in CATALOGUE {
            assert!(
                entry.pid > previous,
                "catalogue is not strictly ascending at {}",
                entry.pid
            );
            previous = entry.pid;
            assert!(
                Pruefidentifikator::new(entry.pid).is_some(),
                "{} is outside the DVGW range",
                entry.pid
            );
            assert!(!entry.description.is_empty());
            assert!(!entry.direction.is_empty());
        }
    }

    #[test]
    fn a_code_resolves_to_its_family() {
        let pid = Pruefidentifikator::new(70_031).unwrap();
        assert_eq!(pid.message_type(), Some(DvgwMessageType::Nomint));
        assert_eq!(pid.to_string(), "70031");
        // In range but not published — not an error, just uncatalogued.
        assert_eq!(Pruefidentifikator::new(70_500).unwrap().info(), None);
    }

    #[test]
    fn parsing_rejects_out_of_range_and_non_numeric() {
        assert!("70001".parse::<Pruefidentifikator>().is_ok());
        assert!("55001".parse::<Pruefidentifikator>().is_err());
        assert!("NOMINT007".parse::<Pruefidentifikator>().is_err());
    }
}
