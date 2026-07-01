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
        Document::Stammdaten(d) => {
            // A Stammdaten document must describe at least one SR_Objekt
            // (controllable resource) unless it is a deactivation/withdrawal.
            use crate::documents::stammdaten::Meldungsstatus;
            if d.meldungsstatus != Meldungsstatus::Deactivation && d.sr_objekte.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "Stammdaten (creation/update) must contain at least one SR_Objekt".to_string(),
                ));
            }
        }
        Document::NetworkConstraint(d) => {
            // A NetworkConstraintDocument without a withdrawal status must carry
            // at least one time series.
            if d.doc_status.is_none() && d.time_series.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "NetworkConstraintDocument must contain at least one NetworkConstraintTimeSeries \
                     (or carry a DocStatus withdrawal)"
                        .to_string(),
                ));
            }
        }
        Document::Unavailability(d) => {
            // An unavailability document without a docStatus must carry at least
            // one TimeSeries.
            if d.doc_status.is_none() && d.time_series.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "Unavailability_MarketDocument must contain at least one TimeSeries \
                     (or carry a docStatus withdrawal)"
                        .to_string(),
                ));
            }
        }
        // Acknowledgement, StatusRequest, Kaskade: no additional semantic rules.
        _ => {}
    }
}
