//! NOMINT — Nominierungsintegration (nomination integration message).
//!
//! Aggregated nomination submitted by a BKV (balance responsible party) to an
//! FNB (transmission system operator) or MGV (market area manager).
//!
//! **Format version:** NOMINT 4.6 FK (valid from 2026-02-01)
//! **UN/EDIFACT base:** D01B

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// A nominated quantity for one location in one time window.
///
/// Corresponds to one LOC + following QTY + optional DTM instance in the
/// NOMINT segment hierarchy.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct NomintQuantity {
    /// Location code (entry/exit zone or VHP code) from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0 (e.g. "Z02" for Einspeisepunkt).
    pub location_qualifier: String,
    /// Nominated quantity value (from QTY C186 component 1).
    pub quantity: String,
    /// Quantity qualifier (from QTY C186 component 0, e.g. "136" for nominated).
    pub quantity_qualifier: String,
    /// Measurement unit (from QTY C186 component 2, e.g. "KWH", "KW").
    pub unit: Option<String>,
    /// Gas day start timestamp (DTM qualifier 318, format 203 = `CCYYMMDDHHmm`).
    pub gas_day_start: Option<String>,
    /// Period end timestamp (DTM qualifier 164).
    pub period_end: Option<String>,
}

impl NomintQuantity {
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
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NomintMessage {
    pub(crate) core: MessageCore,
    /// Nomination document number from BGM element 1, component 0.
    ///
    /// This is the NOMINT's own reference. The corresponding NOMRES will cite
    /// this value in `RFF+Z13:<nomination_ref>` for correlation.
    pub nomination_ref: Option<String>,
    /// Balance group code (Bilanzkreisverantwortlicher) from NAD+Z01 or NAD+Z08.
    pub balance_group_code: Option<String>,
    /// Gas day / nomination period from BGM-adjacent DTM (qualifier 137).
    pub reference_date: Option<String>,
    /// Nomination submission deadline from DTM qualifier 461 (D-1 13:00 CET).
    ///
    /// Per the Kooperationsvereinbarung Gas (`KoV`), nominations must be submitted
    /// by D-1 13:00 CET. This field captures the deadline as transmitted in the
    /// message (`DTM+461`).
    pub nomination_deadline: Option<String>,
    /// Nominated quantity line items (one per location/period).
    pub quantities: Vec<NomintQuantity>,
}

impl_dvgw_message!(NomintMessage);

impl NomintMessage {
    /// Construct from raw segments.
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Nomint);
        // NOMINT's own document number from BGM element 1 (C106), component 0.
        // The corresponding NOMRES will cite this in RFF+Z13 for correlation.
        let nomination_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let balance_group_code = extract_balance_group(&core.segments);
        let reference_date = extract_dtm(&core.segments, "137");
        let nomination_deadline = extract_dtm(&core.segments, "461");
        let quantities = extract_nomint_quantities(&core.segments);
        Self {
            core,
            nomination_ref,
            balance_group_code,
            reference_date,
            nomination_deadline,
            quantities,
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

/// Extract the balance group / BKV code from NAD segments with qualifiers
/// typical for NOMINT (Z01 = Bilanzkreisverantwortlicher, Z08 = BKV-Code).
fn extract_balance_group(segs: &[OwnedSegment]) -> Option<String> {
    for qualifier in &["Z01", "Z08", "ZSO"] {
        if let Some(val) = segs
            .iter()
            .find(|s| s.tag == "NAD" && s.element_str(0) == Some(qualifier))
            .and_then(|s| s.component_str(1, 0))
        {
            return Some(val.to_owned());
        }
    }
    None
}

/// Extract quantity line items from the flat NOMINT segment list.
fn extract_nomint_quantities(segs: &[OwnedSegment]) -> Vec<NomintQuantity> {
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
            let mut gas_day_start = None;
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
                    "DTM" => {
                        let q = segs[j].component_str(0, 0).unwrap_or("");
                        let v = segs[j].component_str(0, 1).map(str::to_owned);
                        match q {
                            "318" | "137" => gas_day_start = v,
                            "164" => period_end = v,
                            _ => {}
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if !location_code.is_empty() && !quantity.is_empty() {
                result.push(NomintQuantity {
                    location_code,
                    location_qualifier,
                    quantity,
                    quantity_qualifier,
                    unit,
                    gas_day_start,
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
