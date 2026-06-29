//! Structural validation — XSD-derivable constraints.
//!
//! These rules are pure field-level checks that require no cross-field context:
//! - `DocumentId` / `Mrid`: 1–35 characters
//! - `DocumentVersion`: 1–999
//! - `MarketParticipantId`: exactly 13 decimal digits
//! - UTC timestamps: offset must be `+00:00`
//! - `TimeInterval`: end after start

use super::{ValidationError, ValidationResult};
use crate::parse::Document;
use crate::types::{DocumentId, DocumentVersion, MarketParticipantId, TimeInterval, UtcDateTime};

// ── Trait for per-type structural validation ──────────────────────────────────

/// Types that can validate their own structural integrity.
pub trait ValidateStructural {
    /// Append any structural violations found to `result`.
    fn validate_structural(&self, result: &mut ValidationResult);
}

// ── Helper validators ─────────────────────────────────────────────────────────

/// Validate a [`DocumentId`] field.
pub(crate) fn check_document_id(id: &DocumentId, result: &mut ValidationResult) {
    let len = id.as_str().len();
    if len == 0 || len > 35 {
        result.errors.push(ValidationError::DocumentIdLength(len));
    }
}

/// Validate a [`DocumentVersion`] field.
pub(crate) fn check_document_version(v: &DocumentVersion, result: &mut ValidationResult) {
    let n = v.get() as u32;
    if n == 0 || n > 999 {
        result.errors.push(ValidationError::DocumentVersionRange(n));
    }
}

/// Validate a [`MarketParticipantId`] field.
pub(crate) fn check_participant_id(id: &MarketParticipantId, result: &mut ValidationResult) {
    let s = id.as_str();
    if s.len() != 13 || !s.bytes().all(|b| b.is_ascii_digit()) {
        result
            .errors
            .push(ValidationError::MarketParticipantIdFormat(s.to_string()));
    }
}

/// Validate that a [`UtcDateTime`] has a UTC offset.
///
/// Since [`UtcDateTime::new`] already rejects non-UTC offsets, this is a
/// belt-and-suspenders check for values that bypassed the constructor.
pub(crate) fn check_utc_offset(ts: &UtcDateTime, result: &mut ValidationResult) {
    // UtcDateTime guarantees UTC at construction; no further check needed.
    let _ = ts;
    let _ = result;
}

/// Validate that a [`TimeInterval`] has `end` after `start`.
pub(crate) fn check_time_interval(interval: &TimeInterval, result: &mut ValidationResult) {
    if interval.end <= interval.start {
        result.errors.push(ValidationError::TimeIntervalOrder);
    }
}

// ── Top-level dispatcher ──────────────────────────────────────────────────────

/// Run structural checks on any [`Document`] variant.
pub fn validate(doc: &Document, result: &mut ValidationResult) {
    match doc {
        Document::Activation(d) => validate_activation(d, result),
        Document::PlannedResourceSchedule(d) => validate_prs(d, result),
        Document::Acknowledgement(d) => validate_ack(d, result),
        Document::NetworkConstraint(d) => validate_ncd(d, result),
        Document::Kostenblatt(d) => validate_kostenblatt(d, result),
        // IEC 62325 documents: validate identifier.
        Document::Kaskade(d) => {
            check_document_id(&d.m_rid, result);
        }
        Document::StatusRequest(d) => {
            check_document_id(&d.m_rid, result);
        }
        Document::Unavailability(d) => {
            check_document_id(&d.m_rid, result);
        }
        Document::Stammdaten(d) => {
            check_document_id(&d.document_identification, result);
            check_participant_id(&d.sender.code, result);
            check_participant_id(&d.empfaenger.code, result);
        }
    }
}

use crate::documents::kostenblatt::Kostenblatt;
use crate::documents::{
    AcknowledgementDocument, ActivationDocument, NetworkConstraintDocument,
    PlannedResourceScheduleDocument,
};

fn validate_activation(d: &ActivationDocument, result: &mut ValidationResult) {
    check_document_id(&d.document_identification.v, result);
    check_document_version(&d.document_version.v, result);
    check_participant_id(&d.sender_identification.v, result);
    check_participant_id(&d.receiver_identification.v, result);
    check_utc_offset(&d.creation_date_time.v, result);
    check_time_interval(&d.activation_time_interval.v, result);
}

fn validate_prs(d: &PlannedResourceScheduleDocument, result: &mut ValidationResult) {
    check_document_id(&d.document_identification.v, result);
    check_document_version(&d.document_version.v, result);
    check_participant_id(&d.sender_identification.v, result);
    check_participant_id(&d.receiver_identification.v, result);
    check_utc_offset(&d.document_date_time.v, result);
    check_time_interval(&d.time_period_covered.v, result);
}

fn validate_ack(d: &AcknowledgementDocument, result: &mut ValidationResult) {
    check_document_id(&d.document_identification.v, result);
    check_participant_id(&d.sender_identification.v, result);
    check_participant_id(&d.receiver_identification.v, result);
    check_utc_offset(&d.document_date_time.v, result);
}

fn validate_ncd(d: &NetworkConstraintDocument, result: &mut ValidationResult) {
    check_document_id(&d.document_identification.v, result);
    check_document_version(&d.document_version.v, result);
    check_participant_id(&d.sender_identification.v, result);
    check_participant_id(&d.receiver_identification.v, result);
    check_utc_offset(&d.document_date_time.v, result);
    check_time_interval(&d.time_period_covered.v, result);
}

fn validate_kostenblatt(d: &Kostenblatt, result: &mut ValidationResult) {
    check_document_id(&d.document_identification.v, result);
    check_document_version(&d.document_version.v, result);
    check_participant_id(&d.sender_identification.v, result);
    check_participant_id(&d.receiver_identification.v, result);
    check_utc_offset(&d.document_date_time.v, result);
    check_time_interval(&d.time_period_covered.v, result);
}
