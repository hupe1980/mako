//! Validation report types for DVGW EDIFACT messages.
//!
//! [`DvgwReport`] is returned by [`DvgwPlatform::validate`] and holds the
//! complete set of validation findings for one DVGW interchange.
//!
//! [`DvgwPlatform::validate`]: crate::DvgwPlatform::validate

use edifact_rs::{ValidationIssue, ValidationSeverity};

use crate::DvgwMessageType;

/// A single validation finding for a DVGW EDIFACT message.
///
/// Produced by [`DvgwPlatform::validate`] and stored in [`DvgwReport::issues`].
///
/// [`DvgwPlatform::validate`]: crate::DvgwPlatform::validate
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DvgwIssue {
    /// Severity level: `"error"`, `"warning"`, or `"info"`.
    ///
    /// Both `Critical` and `Error` severity levels in the underlying validator
    /// are mapped to `"error"` here so callers only need to check one string.
    pub severity: &'static str,
    /// Human-readable description of the finding.
    pub message: String,
    /// The stable rule identifier, e.g. `"SEM-NOMINT-NAD-MS-REQUIRED"`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rule_id: Option<String>,
    /// The EDIFACT segment tag where the issue was found, e.g. `"NAD"`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub segment_tag: Option<String>,
    /// Byte offset of the first byte of the affected region in the source input.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub offset: Option<usize>,
    /// Exclusive end byte offset — forms the half-open range `[offset, byte_end)`.
    ///
    /// `None` for semantic issues that have no source byte position
    /// (e.g. a missing-segment error: there is no segment to point at).
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub byte_end: Option<usize>,
    /// Human-readable remediation hint, if available.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub suggestion: Option<String>,
}

impl From<ValidationIssue> for DvgwIssue {
    fn from(issue: ValidationIssue) -> Self {
        let severity = match issue.severity {
            ValidationSeverity::Critical | ValidationSeverity::Error => "error",
            ValidationSeverity::Warning => "warning",
            _ => "info",
        };
        let rule_id = issue.rule_id().map(str::to_owned);
        let segment_tag = issue.segment_tag().map(str::to_owned);
        let offset = issue.offset();
        let byte_end = issue.span().map(|s| s.end);
        let suggestion = issue.suggestion().map(str::to_owned);
        Self {
            severity,
            message: issue.message,
            rule_id,
            segment_tag,
            offset,
            byte_end,
            suggestion,
        }
    }
}

/// Validation report returned by [`DvgwPlatform::validate`].
///
/// Contains all findings (envelope structural errors, semantic warnings, and
/// informational notices) produced during validation of one DVGW interchange.
///
/// ## Error checking
///
/// ```rust,no_run
/// use dvgw_edi::DvgwPlatform;
///
/// # let input: &[u8] = b"";
/// let report = DvgwPlatform::default().validate(input)?;
///
/// if !report.is_valid() {
///     for e in report.errors() {
///         eprintln!("[{}] {} (rule: {:?})", e.severity, e.message, e.rule_id);
///     }
/// }
///
/// // Or use the Result pattern for early exit:
/// let valid = report.result().expect("DVGW message failed validation");
/// # Ok::<(), dvgw_edi::Error>(())
/// ```
///
/// [`DvgwPlatform::validate`]: crate::DvgwPlatform::validate
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DvgwReport {
    /// The DVGW message type that was validated.
    pub message_type: DvgwMessageType,
    /// The UNH message reference (DE 0062).
    pub message_ref: String,
    /// All validation findings in discovery order.
    pub issues: Vec<DvgwIssue>,
}

impl DvgwReport {
    /// Construct from raw `ValidationIssue` list.
    pub(crate) fn new(
        message_type: DvgwMessageType,
        message_ref: String,
        issues: Vec<ValidationIssue>,
    ) -> Self {
        Self {
            message_type,
            message_ref,
            issues: issues.into_iter().map(DvgwIssue::from).collect(),
        }
    }

    /// Returns `true` when there are no error-severity issues.
    ///
    /// Warnings and info notices do not affect validity.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.issues.iter().all(|i| i.severity != "error")
    }

    /// Converts `self` into `Ok(self)` when valid, `Err(self)` when there are
    /// one or more error-severity issues.
    ///
    /// Mirrors the `EdiEnergyReport::result()` pattern for idiomatic `?` use.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when [`is_valid`](Self::is_valid) returns `false`.
    pub fn result(self) -> Result<Self, Self> {
        if self.is_valid() { Ok(self) } else { Err(self) }
    }

    /// Iterates over error-severity issues.
    pub fn errors(&self) -> impl Iterator<Item = &DvgwIssue> {
        self.issues.iter().filter(|i| i.severity == "error")
    }

    /// Iterates over warning-severity issues.
    pub fn warnings(&self) -> impl Iterator<Item = &DvgwIssue> {
        self.issues.iter().filter(|i| i.severity == "warning")
    }
}
