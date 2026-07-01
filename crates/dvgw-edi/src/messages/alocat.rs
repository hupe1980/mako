//! ALOCAT — Allokationsnachricht (gas quantity allocation message).
//!
//! Communicates allocated gas quantities per exit zone, entry point, or
//! measurement point. Exchanged between FNB, VNB, MGV, and BKV.
//!
//! **Format version:** ALOCAT 5.11a (valid from 2024-10-01)
//! **UN/EDIFACT base:** D03A

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// An allocation quantity line item (SG per LIN/LOC/QTY).
///
/// Each instance represents one allocated quantity for a specific location
/// in a specific time window.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AlocatQuantity {
    /// Location code from the LOC segment (entry/exit zone or measurement point).
    pub location_code: String,
    /// Location qualifier from LOC element 0 (e.g. "Z15" for exit zone).
    pub location_qualifier: String,
    /// Quantity value as a string (from QTY element 0, component 1).
    pub quantity: String,
    /// Quantity qualifier from QTY element 0, component 0 (e.g. "136" for nominated).
    pub quantity_qualifier: String,
    /// Measurement unit from QTY element 0, component 2 (e.g. "KWH", "KW").
    pub unit: Option<String>,
    /// Status qualifier from STS element 0 (optional, e.g. "Z01").
    pub status: Option<String>,
    /// Period start timestamp from DTM (qualifier 163).
    pub period_start: Option<String>,
    /// Period end timestamp from DTM (qualifier 164).
    pub period_end: Option<String>,
}

impl AlocatQuantity {
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

/// A parsed ALOCAT message (Allokationsnachricht).
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AlocatMessage {
    pub(crate) core: MessageCore,
    /// Allocation gas day or reference period from BGM-adjacent DTM (qualifier 137).
    pub reference_date: Option<String>,
    /// Clearing number / message reference from the leading RFF segment.
    pub clearing_number: Option<String>,
    /// The extracted quantity line items (one per location/period).
    pub quantities: Vec<AlocatQuantity>,
}

impl_dvgw_message!(AlocatMessage);

impl AlocatMessage {
    /// Construct from raw segments.
    ///
    /// Extracts typed fields eagerly. Returns an error if the UNH segment is
    /// malformed or the message type is wrong.
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Alocat);
        let reference_date = extract_dtm(&core.segments, "137");
        let clearing_number = find_segment(&core.segments, "RFF")
            .and_then(|s| s.component_str(0, 1))
            .map(str::to_owned);
        let quantities = extract_quantities(&core.segments);
        Self {
            core,
            reference_date,
            clearing_number,
            quantities,
        }
    }
}

// ── Extraction helpers ────────────────────────────────────────────────────────

/// Extract the value of the first DTM segment with the given qualifier.
///
/// DTM element 0 (C507) component 0 is the qualifier; component 1 is the value.
fn extract_dtm(segs: &[OwnedSegment], qualifier: &str) -> Option<String> {
    find_all_segments(segs, "DTM")
        .find(|s| s.component_str(0, 0) == Some(qualifier))
        .and_then(|s| s.component_str(0, 1))
        .map(str::to_owned)
}

/// Extract all quantity line items from the flat segment list.
///
/// The ALOCAT structure uses repeated LOC/QTY pairs. Each LOC starts a new
/// allocation item; the following QTY and optional STS/DTM segments belong to it.
fn extract_quantities(segs: &[OwnedSegment]) -> Vec<AlocatQuantity> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let seg = &segs[i];
        if seg.tag == "LOC" {
            let location_qualifier = seg.element_str(0).unwrap_or("").to_owned();
            let location_code = seg.component_str(1, 0).unwrap_or("").to_owned();
            // Gather following QTY / DTM / STS until next LOC or end
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
                        status = segs[j].component_str(0, 0).map(str::to_owned);
                    }
                    "DTM" => {
                        let q = segs[j].component_str(0, 0).unwrap_or("");
                        let v = segs[j].component_str(0, 1).map(str::to_owned);
                        match q {
                            "163" => period_start = v,
                            "164" => period_end = v,
                            _ => {}
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if !location_code.is_empty() && !quantity.is_empty() {
                result.push(AlocatQuantity {
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
