//! SCHEDL — Schedulingnachricht (transport schedule message).
//!
//! The SCHEDL message communicates day-ahead or intraday transport schedules
//! between market participants (BKV, FNB, MGV). It is informational — no
//! formal response is required.
//!
//! **Format versions:** DVGW G685 / G2000
//! **UN/EDIFACT base:** D03A

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// A scheduled quantity for one location in one time window.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct SchedlQuantity {
    /// Location code (entry/exit zone or VHP code) from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0 (e.g. "Z01" for Einspeisepunkt).
    pub location_qualifier: String,
    /// Scheduled quantity value from QTY C186 component 1.
    pub quantity: String,
    /// Quantity qualifier from QTY C186 component 0.
    pub quantity_qualifier: String,
    /// Measurement unit from QTY C186 component 2 (e.g. "KWH", "KW").
    pub unit: Option<String>,
    /// Period start timestamp (DTM qualifier 163).
    pub period_start: Option<String>,
    /// Period end timestamp (DTM qualifier 164).
    pub period_end: Option<String>,
}

impl SchedlQuantity {
    /// Parse `quantity` as f64. Returns `None` for empty or non-numeric values.
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

/// A parsed SCHEDL message (Schedulingnachricht).
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SchedlMessage {
    pub(crate) core: MessageCore,
    /// Schedule document reference from BGM element 1, component 0.
    pub document_ref: Option<String>,
    /// Schedule period from DTM qualifier 137.
    pub schedule_period: Option<String>,
    /// Schedule version / revision number from BGM element 2 (`C002` component 1).
    pub schedule_version: Option<String>,
    /// Scheduled quantity line items (one per location/period).
    pub quantities: Vec<SchedlQuantity>,
}

impl_dvgw_message!(SchedlMessage);

impl SchedlMessage {
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Schedl);
        let document_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let schedule_version = find_segment(&core.segments, "BGM")
            .and_then(|s| s.element_str(2))
            .map(str::to_owned);
        let schedule_period = extract_dtm(&core.segments, "137");
        let quantities = extract_quantities(&core.segments);
        Self {
            core,
            document_ref,
            schedule_period,
            schedule_version,
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

fn extract_quantities(segs: &[OwnedSegment]) -> Vec<SchedlQuantity> {
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
            if !location_code.is_empty() {
                result.push(SchedlQuantity {
                    location_code,
                    location_qualifier,
                    quantity,
                    quantity_qualifier,
                    unit,
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::{DvgwMessageType, message::DvgwMessage};

    use super::*;

    fn minimal_segments() -> Vec<OwnedSegment> {
        // Minimal set: UNH + BGM + DTM + UNT
        vec![
            seg(
                "UNH",
                vec![vec!["00001"], vec!["SCHEDL", "", "", "", "4.0"]],
            ),
            seg("BGM", vec![vec![""], vec!["SCHED-001"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001368S"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001369Q"]]),
            seg("DTM", vec![vec!["137", "20260115", "102"]]),
            seg("UNT", vec![vec!["5"], vec!["00001"]]),
        ]
    }

    fn seg(tag: &str, elements: Vec<Vec<&str>>) -> OwnedSegment {
        use edifact_rs::{OwnedElement, Span};
        OwnedSegment {
            tag: tag.to_owned(),
            span: Span::new(0, 0),
            tag_span: Span::new(0, 0),
            elements: elements
                .into_iter()
                .map(|comps| OwnedElement {
                    span: Span::new(0, 0),
                    components: comps
                        .into_iter()
                        .map(|c| (c.to_owned(), Span::new(0, 0)))
                        .collect(),
                })
                .collect(),
        }
    }

    #[test]
    fn from_segments_extracts_core_fields() {
        let msg = SchedlMessage::from_segments(minimal_segments());
        assert_eq!(msg.message_type(), DvgwMessageType::Schedl);
        assert_eq!(msg.sender_eic(), Some("21X000000001368S"));
        assert_eq!(msg.receiver_eic(), Some("21X000000001369Q"));
        assert_eq!(msg.document_ref.as_deref(), Some("SCHED-001"));
        assert_eq!(msg.schedule_period.as_deref(), Some("20260115"));
    }

    #[test]
    fn empty_quantities_for_minimal_message() {
        let msg = SchedlMessage::from_segments(minimal_segments());
        assert!(msg.quantities.is_empty());
    }
}
