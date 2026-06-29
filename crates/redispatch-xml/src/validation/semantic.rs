//! Semantic validation — cross-field rules from the BDEW AWT.
//!
//! These rules require context from more than one field and cannot be derived
//! from the XSD alone.

use super::{ValidationError, ValidationResult};
use crate::documents::activation::ActivationDocType;
use crate::parse::Document;

/// Run semantic checks on any [`Document`] variant.
pub fn validate(doc: &Document, result: &mut ValidationResult) {
    match doc {
        Document::Activation(d) => {
            // ACO (A96) and ACR (A41) documents must carry at least one time series.
            match d.document_type.v {
                ActivationDocType::RedispatchActivation | ActivationDocType::ActivationResponse => {
                    if d.time_series.is_empty() {
                        result.errors.push(ValidationError::Semantic(
                            "ACO/ACR ActivationDocument must contain at least one ActivationTimeSeries"
                                .to_string(),
                        ));
                    }
                }
                // AAR (A42) may have zero time series (tender reduction).
                ActivationDocType::TenderReduction => {}
            }
        }
        Document::Kostenblatt(d) => {
            if d.time_series.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "Kostenblatt must contain at least one CostTimeSeries".to_string(),
                ));
            }
        }
        Document::PlannedResourceSchedule(d) => {
            if d.time_series.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "PlannedResourceScheduleDocument must contain at least one PlannedResourceTimeSeries"
                        .to_string(),
                ));
            }
        }
        // Other document types: no additional semantic rules at this time.
        _ => {}
    }
}
