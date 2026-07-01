//! NOMRES — Nominierungsantwort (nomination response message).
//!
//! FNB or MGV response confirming or rejecting a NOMINT nomination. Correlates
//! back to the originating NOMINT via a `RFF+Z13` reference segment.
//!
//! **Format version:** NOMRES 4.7 FK (valid from 2026-02-01)
//! **UN/EDIFACT base:** D01B

use std::fmt;

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// Acceptance status of the nomination response.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum NomresStatus {
    /// The nomination was accepted as submitted.
    Accepted,
    /// The nomination was partially accepted (quantities were curtailed).
    PartiallyAccepted,
    /// The nomination was rejected.
    Rejected,
    /// Status code not mapped to a known variant.
    Other(String),
}

impl NomresStatus {
    fn from_sts(qualifier: &str) -> Self {
        match qualifier {
            "Z01" | "AC" | "1" => Self::Accepted,
            "Z02" | "PA" => Self::PartiallyAccepted,
            "Z03" | "RE" | "7" => Self::Rejected,
            other => Self::Other(other.to_owned()),
        }
    }
}

impl fmt::Display for NomresStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Accepted => f.write_str("Accepted"),
            Self::PartiallyAccepted => f.write_str("PartiallyAccepted"),
            Self::Rejected => f.write_str("Rejected"),
            Self::Other(code) => write!(f, "Other({code})"),
        }
    }
}

/// A confirmed/curtailed quantity for one location in the NOMRES.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct NomresQuantity {
    /// Location code from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0.
    pub location_qualifier: String,
    /// Confirmed quantity value (from QTY C186 component 1).
    pub quantity: String,
    /// Quantity qualifier (from QTY C186 component 0).
    pub quantity_qualifier: String,
    /// Measurement unit (from QTY C186 component 2).
    pub unit: Option<String>,
    /// Acceptance status for this quantity (from STS segment).
    pub status: Option<NomresStatus>,
    /// Period start timestamp (DTM qualifier 318 or 163).
    pub period_start: Option<String>,
    /// Period end timestamp (DTM qualifier 164).
    pub period_end: Option<String>,
}

impl NomresQuantity {
    /// Parses the `quantity` string as a 64-bit float.
    ///
    /// Returns `None` when the string is empty or not a valid decimal number.
    /// EDIFACT decimal notation uses `.` as the decimal mark.
    #[must_use]
    pub fn quantity_f64(&self) -> Option<f64> {
        if self.quantity.is_empty() {
            None
        } else {
            self.quantity.parse().ok()
        }
    }
}

// ── Main message struct ───────────────────────────────────────────────────────
///
/// The key routing field is `nomination_ref` — it carries the `RFF+Z13` value
/// from the originating NOMINT and is used to correlate this response back to
/// the outbound nomination workflow.
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NomresMessage {
    pub(crate) core: MessageCore,
    /// Reference to the originating NOMINT (from `RFF+Z13:<value>`).
    ///
    /// Use this for `ProcessRegistry::lookup_by_correlation` to route the
    /// response to the correct outbound nomination workflow.
    pub nomination_ref: Option<String>,
    /// Overall acceptance status from the leading STS segment.
    pub overall_status: Option<NomresStatus>,
    /// Confirmed/curtailed quantity line items.
    pub quantities: Vec<NomresQuantity>,
    /// Response date/time from BGM-adjacent DTM (qualifier 137).
    pub reference_date: Option<String>,
}

impl_dvgw_message!(NomresMessage);

impl NomresMessage {
    /// Construct from raw segments.
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Nomres);
        // Correlation reference: first RFF with qualifier Z13
        let nomination_ref = find_all_segments(&core.segments, "RFF")
            .find(|s| s.component_str(0, 0) == Some("Z13"))
            .and_then(|s| s.component_str(0, 1))
            .map(str::to_owned);
        let overall_status = find_segment(&core.segments, "STS")
            .and_then(|s| s.component_str(0, 0))
            .map(NomresStatus::from_sts);
        let reference_date = extract_dtm(&core.segments, "137");
        let quantities = extract_nomres_quantities(&core.segments);
        Self {
            core,
            nomination_ref,
            overall_status,
            quantities,
            reference_date,
        }
    }
}

// ── Extraction helpers ────────────────────────────────────────────────────────

fn extract_dtm(segs: &[OwnedSegment], qualifier: &str) -> Option<String> {
    find_all_segments(segs, "DTM")
        .find(|s| s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .map(str::to_owned)
}

fn extract_nomres_quantities(segs: &[OwnedSegment]) -> Vec<NomresQuantity> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let seg = &segs[i];
        if seg.tag == "LOC" {
            let location_qualifier = seg.element_str(0).unwrap_or("").to_owned();
            let location_code = seg.component_str(1, 0).unwrap_or("").to_owned();
            let mut quantity = String::new();
            let mut quantity_qualifier = String::new();
            let mut unit = None;
            let mut status = None;
            let mut period_start = None;
            let mut period_end = None;
            let mut j = i + 1;
            while j < segs.len() && segs[j].tag != "LOC" {
                match segs[j].tag.as_str() {
                    "QTY" => {
                        segs[j]
                            .component_str(0, 0)
                            .unwrap_or("")
                            .clone_into(&mut quantity_qualifier);
                        segs[j]
                            .component_str(0, 1)
                            .unwrap_or("")
                            .clone_into(&mut quantity);
                        unit = segs[j].component_str(0, 2).map(str::to_owned);
                    }
                    "STS" => {
                        let q = segs[j].component_str(0, 0).unwrap_or("");
                        status = Some(NomresStatus::from_sts(q));
                    }
                    "DTM" => {
                        let q = segs[j].component_str(0, 0).unwrap_or("");
                        let v = segs[j].component_str(0, 1).map(str::to_owned);
                        match q {
                            "318" | "163" => period_start = v,
                            "164" => period_end = v,
                            _ => {}
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if !location_code.is_empty() {
                result.push(NomresQuantity {
                    location_code,
                    location_qualifier,
                    quantity,
                    quantity_qualifier,
                    unit,
                    status,
                    period_start,
                    period_end,
                });
            }
            i = j;
        } else {
            i += 1;
        }
    }
    result
}
