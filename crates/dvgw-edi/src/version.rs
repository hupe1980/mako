//! The `UNH` S009 DE 0057 value.

use std::fmt;

/// The Anwendungscode from `UNH` S009 DE 0057.
///
/// DVGW puts two different things in this field and does not distinguish them
/// syntactically:
///
/// | Message | DE 0057 | Meaning |
/// |---|---|---|
/// | NOMINT 4.6, NOMRES 4.7 | `DVGW17` | Nachrichtentypen-Paket 17 |
/// | ALOCAT 5.11a | `5.11a` | the message version itself |
///
/// It is therefore **not** a uniform version key and nothing in this crate
/// selects behaviour from it. It is captured verbatim so it round-trips and so
/// operators can see which package a counterparty claims to be on.
///
/// A structural change (new segments, changed code lists) bumps the number; a
/// *Fehlerkorrektur* (`FK`) is editorial and leaves this field untouched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct DvgwVersion(Box<str>);

impl DvgwVersion {
    /// Capture a non-empty DE 0057 value.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        (!s.is_empty()).then(|| Self(s.into()))
    }

    /// The value as it appears on the wire.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DvgwVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for DvgwVersion {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::DvgwVersion;

    #[test]
    fn both_shapes_round_trip_verbatim() {
        assert_eq!(DvgwVersion::parse("DVGW17").unwrap().as_str(), "DVGW17");
        assert_eq!(DvgwVersion::parse("5.11a").unwrap().as_str(), "5.11a");
        assert_eq!(DvgwVersion::parse(""), None);
    }
}
