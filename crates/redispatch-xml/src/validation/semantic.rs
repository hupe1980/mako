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
            use crate::documents::stammdaten::{Bilanzierungsmodell, Meldungsstatus};
            if d.meldungsstatus != Meldungsstatus::Deactivation && d.sr_objekte.is_empty() {
                result.errors.push(ValidationError::Semantic(
                    "Stammdaten (creation/update) must contain at least one SR_Objekt".to_string(),
                ));
            }
            for (i, sr) in d.sr_objekte.iter().enumerate() {
                // BilAReM Kap. 6.1.5: „Eine SR setzt sich aus mindestens einer
                // TR zusammen." The XSD says minOccurs="1"; an SR with none is
                // a resource nothing can be dispatched against.
                if sr.enthaltene_tr.is_empty() {
                    result.errors.push(ValidationError::Semantic(format!(
                        "SR_Objekt[{i}] contains no Enthaltene_TR — BilAReM Kap. 6.1.5 \
                         requires at least one Technische Ressource per Steuerbare Ressource"
                    )));
                }

                // The Individuelle_Quote shares are percentages of one
                // bilanzieller Ausgleich, so they have to add up. A short set
                // books less than the Maßnahme caused and an over-long one
                // books more, and neither is visible downstream: each Fahrplan
                // on its own looks well-formed.
                if let Some(q) = &sr.individuelle_quote {
                    let summe: f64 = q.quoten.iter().map(|x| x.wert.value()).sum();
                    // Decimal3 is three fractional digits, so anything further
                    // from 100 than half a unit in the last place is a real
                    // discrepancy rather than binary rounding.
                    if (summe - 100.0).abs() > 0.000_5 {
                        result.errors.push(ValidationError::Semantic(format!(
                            "SR_Objekt[{i}] Individuelle_Quote sums to {summe} %, not 100 %"
                        )));
                    }
                }

                // BilAReM Kap. 2.3.2 lists the Redispatch-Bilanzkreis among the
                // three things a Planwertmodell Zuordnung must name. Without it
                // the LF and EIV learn that an SR moved into the Planwertmodell
                // but not where the Ausgleich will be booked.
                let nennt_bilanzkreis = d.bilanzkreis_ausgleichsfahrplan_anf_nb.is_some()
                    || sr
                        .individuelle_quote
                        .as_ref()
                        .is_some_and(|q| !q.quoten.is_empty());
                if sr.bilanzierungsmodell == Bilanzierungsmodell::Planwert
                    && d.meldungsstatus != Meldungsstatus::Deactivation
                    && !nennt_bilanzkreis
                {
                    result.errors.push(ValidationError::Semantic(format!(
                        "SR_Objekt[{i}] is in the Planwertmodell but the document names no \
                         Redispatch-Bilanzkreis (neither Individuelle_Quote nor \
                         Bilanzkreis_Ausgleichsfahrplan_anfNB) — BilAReM Kap. 2.3.2"
                    )));
                }
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
