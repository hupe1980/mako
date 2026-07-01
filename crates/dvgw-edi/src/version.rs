use std::fmt;

/// Version identifier for a DVGW message format.
///
/// DVGW uses a `<major>.<minor>[letter]` versioning scheme
/// (e.g. `5.11a`, `4.6`, `4.7`) with optional Fehlerkorrektur (`FK`) suffix
/// for editorial-only corrections.  The version string appears in the UNH
/// segment DE 0057 (association assigned code).
///
/// # Version vs. release
///
/// DVGW distinguishes:
/// - **Version** (major bump): structural change — codelist change, new segments, etc.
/// - **Fehlerkorrektur** (`FK`): editorial correction — no structural change.
///   The version string stays the same; only the publication date changes.
///
/// `DvgwVersion` stores the raw wire string so it round-trips faithfully.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct DvgwVersion(String);

impl DvgwVersion {
    /// Construct a version from a known-valid string without validation.
    ///
    /// Prefer `DvgwVersion::parse(s)` for user-supplied or deserialized input.
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Parse a version string, accepting any non-empty value.
    ///
    /// DVGW version strings are not formally specified beyond the conventions
    /// documented in the DVGW Versionsmanagement page.  This method accepts
    /// any non-empty ASCII string and returns `None` for empty input.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.is_empty() {
            None
        } else {
            Some(Self(s.to_owned()))
        }
    }

    /// Returns the version string as it appears on the wire (UNH DE 0057).
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
