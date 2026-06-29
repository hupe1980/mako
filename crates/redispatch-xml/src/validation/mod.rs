//! Structural and semantic validation for Redispatch 2.0 documents.
//!
//! ## Layers
//!
//! - **Structural** — verifies field constraints derivable from the XSD without
//!   cross-field context (identifier lengths, version range, timestamp offsets,
//!   participant ID format).
//! - **Semantic** — cross-field rules from the BDEW AWT (e.g. an `ACO`
//!   document must contain at least one `ActivationTimeSeries`).

use crate::error::RedispatchXmlError;
use crate::parse::Document;

pub mod semantic;
pub mod structural;

// ── Validation result types ───────────────────────────────────────────────────

/// A validation error (document is non-conformant and must not be processed).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ValidationError {
    #[error("document identifier must be 1–35 characters, got {0}")]
    DocumentIdLength(usize),
    #[error("document version must be 1–999, got {0}")]
    DocumentVersionRange(u32),
    #[error("market participant ID must be exactly 13 decimal digits, got {0:?}")]
    MarketParticipantIdFormat(String),
    #[error("timestamp must be UTC, got offset {0}")]
    TimestampNotUtc(String),
    #[error("time interval end must be after start")]
    TimeIntervalOrder,
    #[error("{0}")]
    Structural(String),
    #[error("{0}")]
    Semantic(String),
}

/// A validation warning (non-fatal; document should still be processed).
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationWarning(pub String);

/// The combined result of validating a document.
#[derive(Debug, Default, Clone)]
pub struct ValidationResult {
    /// Non-fatal warnings (processing may continue).
    pub warnings: Vec<ValidationWarning>,
    /// Validation errors (document is non-conformant).
    pub errors: Vec<ValidationError>,
}

impl ValidationResult {
    /// Return `true` if there are no validation errors.
    pub fn is_valid(&self) -> bool {
        self.errors.is_empty()
    }

    /// Convert to a [`Result`], returning the first error on failure.
    pub fn into_result(mut self) -> Result<Vec<ValidationWarning>, ValidationError> {
        if self.errors.is_empty() {
            Ok(self.warnings)
        } else {
            Err(self.errors.remove(0))
        }
    }
}

impl std::fmt::Display for ValidationResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.errors.is_empty() && self.warnings.is_empty() {
            return write!(f, "ok");
        }
        for e in &self.errors {
            writeln!(f, "error: {e}")?;
        }
        for w in &self.warnings {
            writeln!(f, "warning: {}", w.0)?;
        }
        Ok(())
    }
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Validate a parsed [`Document`], running both structural and semantic checks.
///
/// Returns a [`ValidationResult`] that collects all errors and warnings
/// (rather than stopping at the first problem). Check
/// [`ValidationResult::is_valid`] to determine whether the document is
/// conformant.
///
/// # Errors
///
/// This function does not return `Err`; all findings are collected in the
/// returned [`ValidationResult`]. Returns `Err` only when a validation
/// precondition check fails (not currently possible).
#[allow(unused_variables)]
pub fn validate(doc: &Document) -> ValidationResult {
    let mut result = ValidationResult::default();
    structural::validate(doc, &mut result);
    semantic::validate(doc, &mut result);
    result
}

/// Validate the structural integrity of a specific document without a
/// [`Document`] enum wrapper.
///
/// Convenience wrapper around [`structural::validate_raw`].
pub fn validate_structural<T>(doc: &T) -> Result<(), RedispatchXmlError>
where
    T: structural::ValidateStructural,
{
    let mut result = ValidationResult::default();
    doc.validate_structural(&mut result);
    result
        .into_result()
        .map(|_| ())
        .map_err(|e| RedispatchXmlError::StructuralError(e.to_string()))
}
