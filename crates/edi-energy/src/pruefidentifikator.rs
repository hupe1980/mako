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
    /// entry points produce consistent error variants (F-028):
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
