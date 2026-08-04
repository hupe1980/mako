use std::{fmt, str::FromStr};

use crate::Error;

/// A validated EDI@Energy Pruefidentifikator (document-identifier code).
///
/// Pruefidentifikatoren are 5-digit decimal codes that identify the business
/// process variant of an EDI@Energy message (e.g. `11001` for a UTILMD
/// grid-connection registration, `21001` for an MSCONS day-ahead report).
///
/// The valid range is `10000–99999` (all 5-digit decimal numbers).
/// The value is extracted from element 1 of the BGM segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Pruefidentifikator(u32);

impl Pruefidentifikator {
    /// The inclusive lower bound of the valid range.
    pub const MIN: u32 = 10_000;
    /// The inclusive upper bound of the valid range.
    pub const MAX: u32 = 99_999;

    /// Construct a `Pruefidentifikator`, validating that `code` is in range.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPruefidentifikatorRange`] if `code` is outside `10000..=99999`.
    pub fn new(code: u32) -> Result<Self, Error> {
        if (Self::MIN..=Self::MAX).contains(&code) {
            Ok(Self(code))
        } else {
            Err(Error::InvalidPruefidentifikatorRange(code))
        }
    }

    /// Returns the numeric code.
    #[must_use]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    /// Parse from a string slice.
    ///
    /// `source_segment` is the EDIFACT segment tag where the value was read
    /// (e.g. `"BGM"` or `"RFF"`) and is included in the error message when
    /// the string is not a decimal integer.
    ///
    /// This method delegates to [`FromStr`] for the numeric parse so that both
    /// entry points produce consistent error variants:
    /// - Non-numeric input → [`Error::InvalidPruefidentifikatorFormat`] (carries the raw value).
    /// - Out-of-range integer → [`Error::InvalidPruefidentifikatorRange`].
    ///
    /// The `source_segment` parameter is retained for API compatibility; it is
    /// no longer used to select the error variant but may appear in future
    /// diagnostics context.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPruefidentifikatorRange`] if the value is out of range,
    /// or [`Error::InvalidPruefidentifikatorFormat`] if the string is not a decimal integer.
    pub fn parse(s: &str, _source_segment: &'static str) -> Result<Self, Error> {
        s.parse()
    }
}

impl fmt::Display for Pruefidentifikator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:05}", self.0)
    }
}

impl FromStr for Pruefidentifikator {
    type Err = Error;

    /// Parse a `Pruefidentifikator` from a decimal string.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPruefidentifikatorRange`] when the value is out of
    /// range, or [`Error::InvalidPruefidentifikatorFormat`] when the string is not a
    /// decimal integer.
    ///
    /// # Note
    ///
    /// For segment-context error messages (e.g. when the source segment is
    /// `"RFF"` for COMDIS/PRICAT), use [`Pruefidentifikator::parse`] directly.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u32>()
            .map_err(|_| Error::InvalidPruefidentifikatorFormat {
                raw_value: s.to_owned(),
            })
            .and_then(Self::new)
    }
}

// ── AHB answer triples ────────────────────────────────────────────────────────

/// The `(Bestätigung, Ablehnung)` Prüfidentifikatoren the AHB assigns to an
/// Anfrage, or `None` when `anfrage` is not a request PID.
///
/// BDEW numbers most request/response families as a triple — Anfrage, then `+1`
/// for the Bestätigung and `+2` for the Ablehnung — but the pattern does **not**
/// hold everywhere, which is why this is a table:
///
/// - GPKE 55077 (Anmeldung erz. `MaLo`) rejects with **55080**, because 55079 is
///   unassigned.
/// - `GeLi` Gas 44020 has a Bestätigung (44021) but no Ablehnung; 44019 has
///   neither. [`answer_pids`] returns `None` for both, since neither yields a
///   complete pair — use [`bestaetigung_pid`] when only the positive answer
///   matters.
///
/// This table is the single source of truth: the GPKE and `GeLi` Gas workflows
/// derive their outbound response PID from it, and `makotest` binds it so a
/// simulated counterparty answers with the same code the platform expects.
/// A second copy would disagree at the first Formatumstellung.
#[must_use]
pub fn answer_pids(anfrage: u32) -> Option<(u32, u32)> {
    Some(match anfrage {
        // ── GPKE Strom — UTILMD ──────────────────────────────────────────────
        55001 => (55002, 55003), // Anmeldung verbrauchende MaLo
        55004 => (55005, 55006), // Abmeldung
        55016 => (55017, 55018), // Kündigung Lieferbeginn (LFN → LFA)
        55077 => (55078, 55080), // Anmeldung erz. MaLo — 55079 unassigned
        // ── GeLi Gas — UTILMD G ──────────────────────────────────────────────
        44001 => (44002, 44003), // Anmeldung NN
        44004 => (44005, 44006), // Abmeldung NN
        44007 => (44008, 44009), // Abmeldung NN vom NB
        44010 => (44011, 44012), // Abmeldungsanfrage des NB
        44013 => (44014, 44015), // Anmeldung / Zuordnung EoG
        44016 => (44017, 44018), // Kündigung Lieferbeginn
        _ => return None,
    })
}

/// The Bestätigung Prüfidentifikator for an Anfrage, if the AHB defines one.
///
/// Covers the asymmetric families [`answer_pids`] cannot express: `GeLi` Gas
/// 44020 is confirmed with 44021 but has no Ablehnung.
#[must_use]
pub fn bestaetigung_pid(anfrage: u32) -> Option<u32> {
    match anfrage {
        44020 => Some(44021),
        other => answer_pids(other).map(|(ok, _)| ok),
    }
}

/// The Ablehnung Prüfidentifikator for an Anfrage, if the AHB defines one.
#[must_use]
pub fn ablehnung_pid(anfrage: u32) -> Option<u32> {
    answer_pids(anfrage).map(|(_, nok)| nok)
}

#[cfg(test)]
mod answer_pid_tests {
    use super::*;

    /// The `+1 / +2` shorthand is wrong for two families; both must stay pinned.
    #[test]
    fn documented_deviations_from_the_plus_one_pattern_hold() {
        assert_eq!(
            answer_pids(55077),
            Some((55078, 55080)),
            "55079 is unassigned, so the Ablehnung is 55080 — not Anfrage+2"
        );
        assert_eq!(
            bestaetigung_pid(44020),
            Some(44021),
            "44020 is confirmable even though it has no Ablehnung"
        );
        assert_eq!(ablehnung_pid(44020), None);
        assert_eq!(answer_pids(44020), None, "no complete pair for 44020");
        assert_eq!(answer_pids(44019), None, "44019 has neither answer");
    }

    /// Every answer PID must be a constructible Prüfidentifikator, and no code
    /// may serve as both a request and an answer.
    #[test]
    fn the_table_is_internally_consistent() {
        let requests: Vec<u32> = (44000..=44999).chain(55000..=55999).collect();
        let mut answers = Vec::new();
        for anfrage in requests.iter().copied() {
            let Some((ok, nok)) = answer_pids(anfrage) else {
                continue;
            };
            assert!(Pruefidentifikator::new(ok).is_ok(), "{ok} constructible");
            assert!(Pruefidentifikator::new(nok).is_ok(), "{nok} constructible");
            assert_ne!(ok, nok, "{anfrage}: Bestätigung and Ablehnung must differ");
            assert!(
                ok > anfrage && nok > anfrage,
                "{anfrage}: answers follow it"
            );
            answers.push(ok);
            answers.push(nok);
        }
        assert!(!answers.is_empty(), "the table must not be empty");

        for a in &answers {
            assert!(
                answer_pids(*a).is_none(),
                "PID {a} is an answer — it must not also be a request"
            );
        }

        let mut sorted = answers.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            answers.len(),
            "no PID may answer two different Anfragen"
        );
    }
}
