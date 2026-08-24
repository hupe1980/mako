//! Validation findings.

use std::fmt;

use crate::document::{DvgwDocument, DvgwMessageType};

/// How badly a finding breaks the message.
///
/// A typed severity, not a string: `issue.severity == "error"` compiles happily
/// when it is misspelled and then silently matches nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
pub enum Severity {
    /// Advisory; the message is conformant.
    Info,
    /// The message is processable but deviates from the Nachrichtenbeschreibung.
    Warning,
    /// The message violates a `Muss` row and must be rejected.
    Error,
}

impl Severity {
    /// The lowercase name, for logs and JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single validation finding.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DvgwIssue {
    /// How badly this breaks the message.
    pub severity: Severity,
    /// What is wrong, in prose.
    pub message: String,
    /// The stable rule identifier, e.g. `"DVGW-RFF-Z13"`.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub rule_id: Option<&'static str>,
    /// The EDIFACT segment tag the finding is about.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub segment_tag: Option<&'static str>,
    /// How to fix it.
    #[cfg_attr(feature = "serde", serde(skip_serializing_if = "Option::is_none"))]
    pub suggestion: Option<String>,
}

impl DvgwIssue {
    pub(crate) fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self {
            severity,
            message: message.into(),
            rule_id: None,
            segment_tag: None,
            suggestion: None,
        }
    }

    pub(crate) fn with_rule(mut self, rule_id: &'static str) -> Self {
        self.rule_id = Some(rule_id);
        self
    }

    pub(crate) fn with_segment(mut self, tag: &'static str) -> Self {
        self.segment_tag = Some(tag);
        self
    }

    pub(crate) fn with_suggestion(mut self, text: impl Into<String>) -> Self {
        self.suggestion = Some(text.into());
        self
    }
}

impl fmt::Display for DvgwIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}]", self.severity)?;
        if let Some(rule) = self.rule_id {
            write!(f, " {rule}")?;
        }
        if let Some(tag) = self.segment_tag {
            write!(f, " ({tag})")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// The result of validating one DVGW message.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DvgwReport {
    /// The family that was validated.
    pub message_type: DvgwMessageType,
    /// The `BGM` document-name code that was validated.
    pub document: DvgwDocument,
    /// The `UNH` message reference.
    pub message_ref: String,
    /// All findings, in rule order.
    pub issues: Vec<DvgwIssue>,
}

impl DvgwReport {
    pub(crate) fn new(
        message_type: DvgwMessageType,
        document: DvgwDocument,
        message_ref: String,
        issues: Vec<DvgwIssue>,
    ) -> Self {
        Self {
            message_type,
            document,
            message_ref,
            issues,
        }
    }

    /// `true` when nothing at [`Severity::Error`] was found.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.issues.iter().any(|i| i.severity == Severity::Error)
    }

    /// The error-severity findings.
    pub fn errors(&self) -> impl Iterator<Item = &DvgwIssue> {
        self.issues.iter().filter(|i| i.severity == Severity::Error)
    }

    /// The warning-severity findings.
    pub fn warnings(&self) -> impl Iterator<Item = &DvgwIssue> {
        self.issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
    }

    /// `Ok(self)` when valid, `Err(self)` otherwise — for `?` at a call site
    /// that treats a non-conformant message as a failure.
    ///
    /// # Errors
    ///
    /// Returns `Err(self)` when [`is_valid`](Self::is_valid) is `false`.
    pub fn result(self) -> Result<Self, Self> {
        if self.is_valid() { Ok(self) } else { Err(self) }
    }
}

impl fmt::Display for DvgwReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} ({}) {}: {} error(s), {} warning(s)",
            self.message_type,
            self.document.code(),
            self.message_ref,
            self.errors().count(),
            self.warnings().count()
        )
    }
}
