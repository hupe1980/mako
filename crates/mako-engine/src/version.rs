//! Workflow versioning types.
//!
//! BDEW publishes format updates approximately twice per year (1 April and
//! 1 October). Each format version has an effective date that is used as the
//! versioning key — **not** semver — because business semantics change on BDEW
//! release boundaries, not on library release boundaries.
//!
//! A [`WorkflowId`] permanently identifies the combination of workflow name and
//! format version under which a process was started. Events carry this ID so
//! replay and migration tooling can route to the correct logic.

use std::fmt;

// ── FormatVersion ─────────────────────────────────────────────────────────────

/// A BDEW EDI@Energy format version effective-date identifier.
///
/// BDEW uses the convention `FV<YYYY>-<MM>-<DD>`, e.g. `FV2024-10-01` for the
/// format version that became effective on 1 October 2024.
///
/// Use [`FormatVersion::parse`] to construct from user-supplied strings with
/// pattern validation. Use [`FormatVersion::new`] only for compile-time
/// constants where the value is already known-valid.
///
/// The inner string is stored opaquely so future BDEW versioning conventions can
/// be accommodated without breaking the engine API.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FormatVersion(Box<str>);

/// Error returned when a string does not match the `FV<YYYY>-<MM>-<DD>` pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatVersionError {
    /// The string that failed validation.
    pub input: String,
    /// Human-readable explanation.
    pub reason: &'static str,
}

impl fmt::Display for FormatVersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "invalid FormatVersion {:?}: {}; expected pattern FV<YYYY>-<MM>-<DD>",
            self.input, self.reason
        )
    }
}

impl std::error::Error for FormatVersionError {}

impl FormatVersion {
    /// **Unchecked constructor for known-valid compile-time literals only.**
    ///
    /// Constructs a `FormatVersion` without validating the input against the
    /// BDEW `FV<YYYY>-<MM>-<DD>` pattern. Passing an invalid string will not
    /// panic, but it will produce a value that fails later assertions, fails
    /// round-trip equality with `FormatVersion::parse`, and may cause
    /// confusing errors when stored or transmitted.
    ///
    /// **Use [`parse`] for all runtime and user-supplied strings.** This
    /// includes strings read from config files, environment variables,
    /// EDIFACT messages, API request bodies, or any other external source.
    ///
    /// Correct usage — compile-time literal only:
    ///
    /// ```
    /// use mako_engine::version::FormatVersion;
    ///
    /// // ✓ Known-valid compile-time literal
    /// let fv = FormatVersion::new("FV2024-10-01");
    /// assert_eq!(fv.as_str(), "FV2024-10-01");
    /// ```
    ///
    /// Incorrect usage — use `parse` instead:
    ///
    /// ```
    /// use mako_engine::version::FormatVersion;
    ///
    /// // ✗ Do NOT pass user-supplied or deserialized strings to `new`
    /// // let fv = FormatVersion::new(some_config_value);
    ///
    /// // ✓ Use parse for anything that is not a compile-time literal
    /// let fv = FormatVersion::parse("FV2024-10-01").unwrap();
    /// ```
    ///
    /// [`parse`]: FormatVersion::parse
    #[must_use]
    pub fn new(v: impl Into<Box<str>>) -> Self {
        Self(v.into())
    }

    /// Parse and validate a BDEW `FV<YYYY>-<MM>-<DD>` format version string.
    ///
    /// Accepts exactly the BDEW naming convention. Rejects:
    /// - Missing `FV` prefix (`"2024-10-01"`)
    /// - Malformed date components (`"FV2024-13-01"` — month 13)
    /// - Non-numeric year/month/day
    /// - Any other format
    ///
    /// The year must be ≥ 2000 (no BDEW EDI\@Energy format versions exist before
    /// then). There is no upper bound: the date is validated using the calendar,
    /// eliminating the former year-2100 ceiling.
    ///
    /// # Errors
    ///
    /// Returns [`FormatVersionError`] with the rejected input and a reason
    /// string when the input does not match the expected pattern.
    ///
    /// # Example
    ///
    /// ```
    /// use mako_engine::version::FormatVersion;
    ///
    /// assert!(FormatVersion::parse("FV2024-10-01").is_ok());
    /// assert!(FormatVersion::parse("FV2025-04-01").is_ok());
    /// // No upper year bound:
    /// assert!(FormatVersion::parse("FV2101-04-01").is_ok());
    /// assert!(FormatVersion::parse("FV9999-12-31").is_ok());
    ///
    /// assert!(FormatVersion::parse("2024-10-01").is_err());  // missing FV prefix
    /// assert!(FormatVersion::parse("FV2024-13-01").is_err()); // invalid month
    /// assert!(FormatVersion::parse("FV2024-00-01").is_err()); // zero month
    /// assert!(FormatVersion::parse("FV2024-10-00").is_err()); // zero day
    /// assert!(FormatVersion::parse("FV2024-10-32").is_err()); // day > 31
    /// assert!(FormatVersion::parse("v2024").is_err());       // wrong prefix
    /// ```
    pub fn parse(s: &str) -> Result<Self, FormatVersionError> {
        let err = |reason| FormatVersionError {
            input: s.to_owned(),
            reason,
        };

        // Reject oversized inputs before any allocation.
        // "FV" + "YYYY-MM-DD" = 12 characters exactly.
        if s.len() > 12 {
            return Err(err(
                "input too long; expected exactly 12 characters (FV<YYYY>-<MM>-<DD>)",
            ));
        }

        // Reject NUL bytes: they pass length checks but produce malformed
        // JSON when serialized.
        if s.contains('\0') {
            return Err(err("input contains NUL bytes"));
        }

        // Must start with "FV"
        let rest = s
            .strip_prefix("FV")
            .ok_or_else(|| err("must start with 'FV'"))?;

        // Must be exactly "YYYY-MM-DD" (10 chars)
        if rest.len() != 10 {
            return Err(err("date part must be exactly 10 characters (YYYY-MM-DD)"));
        }

        let parts: Vec<&str> = rest.splitn(3, '-').collect();
        if parts.len() != 3 {
            return Err(err("date part must contain exactly two '-' separators"));
        }

        if parts[0].len() != 4 {
            return Err(err("year must be exactly 4 digits"));
        }
        if parts[1].len() != 2 {
            return Err(err("month must be exactly 2 digits"));
        }
        if parts[2].len() != 2 {
            return Err(err("day must be exactly 2 digits"));
        }

        let year: i32 = parts[0]
            .parse()
            .map_err(|_| err("year must be a 4-digit number"))?;
        let month: u8 = parts[1]
            .parse()
            .map_err(|_| err("month must be a 2-digit number"))?;
        let day: u8 = parts[2]
            .parse()
            .map_err(|_| err("day must be a 2-digit number"))?;

        if year < 2000 {
            return Err(err(
                "year must be ≥ 2000 (no BDEW format versions exist before then)",
            ));
        }

        // Validate using the calendar — this checks month range, day-in-month,
        // leap-year validity, and future centuries without any year ceiling.
        let month_enum =
            time::Month::try_from(month).map_err(|_| err("month must be in range 01–12"))?;
        time::Date::from_calendar_date(year, month_enum, day)
            .map_err(|_| err("date components do not form a valid calendar date"))?;

        // BDEW releases on 01-04 or 01-10 in the normal cycle, but interim
        // corrections (e.g. APERAK MIG 2.1i effective 2025-06-06, REMADV MIG
        // 2.9e effective 2026-04-01) use non-01 days. We accept any valid date.

        Ok(Self(s.into()))
    }

    /// The raw format version string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FormatVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for FormatVersion {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for FormatVersion {
    fn from(s: String) -> Self {
        Self::new(s.into_boxed_str())
    }
}

// ── WorkflowId ────────────────────────────────────────────────────────────────

/// Uniquely identifies a versioned workflow definition.
///
/// A process started under `gpke-supplier-change / FV2024-10-01` continues to
/// execute under that version until it completes, even after `FV2025-10-01` is
/// deployed. Both versions coexist in the running engine.
///
/// # Example
///
/// ```
/// use mako_engine::version::{FormatVersion, WorkflowId};
///
/// let id = WorkflowId::new("gpke-supplier-change", "FV2024-10-01");
/// assert_eq!(id.name.as_ref(), "gpke-supplier-change");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WorkflowId {
    /// Workflow name, e.g. `"gpke-supplier-change"`.
    pub name: Box<str>,
    /// BDEW format version under which this workflow was initiated.
    pub format_version: FormatVersion,
}

impl WorkflowId {
    /// Construct a workflow identity.
    #[must_use]
    pub fn new(name: impl Into<Box<str>>, format_version: impl Into<FormatVersion>) -> Self {
        Self {
            name: name.into(),
            format_version: format_version.into(),
        }
    }
}

impl fmt::Display for WorkflowId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.format_version)
    }
}

// ── WorkflowVersionPolicy ─────────────────────────────────────────────────────

/// Declares which BDEW format versions a [`Workflow`] implementation can
/// accept over the lifetime of in-flight processes.
///
/// BDEW releases two annual format updates. Processes that span a release
/// boundary (e.g. a MABIS billing process that starts in October and settles
/// in January) must accept inbound messages from both the old and the new
/// format version. `WorkflowVersionPolicy` makes this acceptance declaration
/// explicit and compiler-checked.
///
/// The engine can use this policy to validate that an incoming message's
/// format version is acceptable *before* constructing the command, surfacing
/// the gap at dispatch time rather than during a runtime deserialization error.
///
/// # Default
///
/// The default implementation of [`Workflow::version_policy()`] returns
/// [`WorkflowVersionPolicy::ForwardCompatible`], which accepts messages in any
/// format version ≥ the creation FV. This is the correct default for the
/// majority of BDEW MaKo processes, which routinely span annual release
/// boundaries. Override to [`Pinned`] only for strictly short-lived workflows
/// that are guaranteed to complete within a single BDEW release cycle.
///
/// [`Workflow::version_policy()`]: crate::workflow::Workflow::version_policy
/// [`Pinned`]: WorkflowVersionPolicy::Pinned
///
/// # Example
///
/// ```rust,ignore
/// use mako_engine::version::{FormatVersion, WorkflowVersionPolicy};
///
/// // A GPKE process that lives at most 24 hours — pinned to creation FV:
/// fn version_policy() -> WorkflowVersionPolicy {
///     WorkflowVersionPolicy::Pinned
/// }
///
/// // A MABIS billing process that spans the annual release boundary:
/// fn version_policy() -> WorkflowVersionPolicy {
///     WorkflowVersionPolicy::Explicit(vec![
///         FormatVersion::new("FV2025-10-01"),
///         FormatVersion::new("FV2026-10-01"),
///     ])
/// }
///
/// // An open-ended process that accepts all FVs >= creation:
/// fn version_policy() -> WorkflowVersionPolicy {
///     WorkflowVersionPolicy::ForwardCompatible
/// }
/// ```
///
/// [`Workflow`]: crate::workflow::Workflow
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum WorkflowVersionPolicy {
    /// Accept only the format version recorded at process creation.
    ///
    /// Use for strictly short-lived workflows that are guaranteed to complete
    /// within a single BDEW release cycle (< 6 months). All counterparty
    /// messages must arrive before the next October-1 or April-1 FV boundary.
    ///
    /// This is a **stricter** policy than the default
    /// [`ForwardCompatible`](WorkflowVersionPolicy::ForwardCompatible). Most
    /// BDEW market-communication processes span release boundaries; prefer
    /// `ForwardCompatible` unless you have an explicit reason to pin.
    Pinned,

    /// Accept any format version greater than or equal to the creation FV.
    ///
    /// **This is the default** (via `#[default]`) for all MaKo workflows.
    ///
    /// MaKo processes routinely span BDEW annual release boundaries: a
    /// Lieferbeginn process started on 2025-09-20 must still accept the
    /// counterparty's APERAK reply sent on 2025-11-10 under the new
    /// FV2025-10-01 rules. `ForwardCompatible` handles this transparently.
    ///
    /// [`Workflow::version_policy()`]: crate::workflow::Workflow::version_policy
    #[default]
    ForwardCompatible,

    /// Accept exactly the listed format versions.
    ///
    /// Use when the set of acceptable format versions is known at compile time
    /// (e.g. a billing process that must handle exactly FV2025-10-01 and
    /// FV2026-10-01).
    Explicit(Vec<FormatVersion>),
}

impl WorkflowVersionPolicy {
    /// Returns `true` if `fv` is acceptable under this policy given
    /// `creation_fv` (the format version recorded in the process's
    /// [`WorkflowId`]).
    ///
    /// # Behaviour
    ///
    /// | Policy | Acceptance |
    /// |--------|-----------|
    /// | `Pinned` | `fv == creation_fv` |
    /// | `ForwardCompatible` | always (caller treats any FV as acceptable) |
    /// | `Explicit(list)` | `fv` is in `list` |
    #[must_use]
    pub fn accepts(&self, fv: &FormatVersion, creation_fv: &FormatVersion) -> bool {
        match self {
            Self::Pinned => fv == creation_fv,
            Self::ForwardCompatible => true,
            Self::Explicit(list) => list.contains(fv),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_bdew_versions() {
        assert!(FormatVersion::parse("FV2024-10-01").is_ok());
        assert!(FormatVersion::parse("FV2025-04-01").is_ok());
        assert!(FormatVersion::parse("FV2026-10-01").is_ok());
        assert!(FormatVersion::parse("FV2000-01-01").is_ok());
    }

    ///  the former year-2100 ceiling must be gone.
    #[test]
    fn parse_accepts_years_beyond_2100() {
        assert!(
            FormatVersion::parse("FV2101-04-01").is_ok(),
            "2101 must now be valid"
        );
        assert!(
            FormatVersion::parse("FV2500-10-01").is_ok(),
            "far-future years must be valid"
        );
        assert!(
            FormatVersion::parse("FV9999-12-31").is_ok(),
            "max 4-digit year must be valid"
        );
    }

    /// Calendar validation catches impossible dates (e.g. Feb 30).
    #[test]
    fn parse_rejects_impossible_calendar_dates() {
        assert!(
            FormatVersion::parse("FV2024-02-30").is_err(),
            "Feb 30 is impossible"
        );
        assert!(
            FormatVersion::parse("FV2025-04-31").is_err(),
            "Apr 31 is impossible"
        );
        assert!(
            FormatVersion::parse("FV2100-02-29").is_err(),
            "2100 is not a leap year"
        );
        // 2104 IS a leap year, so Feb 29 is valid.
        assert!(
            FormatVersion::parse("FV2104-02-29").is_ok(),
            "2104-02-29 must be valid"
        );
    }

    #[test]
    fn parse_missing_fv_prefix() {
        let err = FormatVersion::parse("2024-10-01").unwrap_err();
        assert!(err.reason.contains("'FV'"), "reason: {}", err.reason);
    }

    #[test]
    fn parse_wrong_prefix_lowercase() {
        assert!(FormatVersion::parse("fv2024-10-01").is_err());
    }

    #[test]
    fn parse_invalid_month() {
        assert!(FormatVersion::parse("FV2024-13-01").is_err(), "month 13");
        assert!(FormatVersion::parse("FV2024-00-01").is_err(), "month 0");
    }

    #[test]
    fn parse_invalid_day() {
        assert!(FormatVersion::parse("FV2024-10-00").is_err(), "day 0");
        assert!(FormatVersion::parse("FV2024-10-32").is_err(), "day 32");
        //  non-01 days are now VALID (APERAK MIG 2.1i: FV2025-06-06)
        assert!(
            FormatVersion::parse("FV2025-06-06").is_ok(),
            "mid-cycle day must be accepted"
        );
        assert!(
            FormatVersion::parse("FV2026-04-01").is_ok(),
            "non-October date must be accepted"
        );
    }

    #[test]
    fn parse_roundtrip() {
        let s = "FV2025-10-01";
        let fv = FormatVersion::parse(s).unwrap();
        assert_eq!(fv.as_str(), s);
        assert_eq!(fv.to_string(), s);
    }

    #[test]
    fn parse_non_numeric_components() {
        assert!(FormatVersion::parse("FVaaaa-10-01").is_err());
        assert!(FormatVersion::parse("FV2024-bb-01").is_err());
        assert!(FormatVersion::parse("FV2024-10-cc").is_err());
    }
}
