//! DELORD — Delivery Order message.
//!
//! The DELORD message allows a BKV or gas wholesaler (GH) to request a specific
//! gas delivery at a defined delivery point from an FNB or MGV.
//!
//! **Format versions:** DVGW G685 / G2000
//! **UN/EDIFACT base:** D03A

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// A quantity line in the delivery order.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct DeliveryOrderLine {
    /// Delivery point / location code from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0.
    pub location_qualifier: String,
    /// Ordered quantity from QTY C186 component 1.
    pub quantity: String,
    /// Quantity qualifier from QTY C186 component 0.
    pub quantity_qualifier: String,
    /// Measurement unit from QTY C186 component 2 (e.g. "KWH").
    pub unit: Option<String>,
    /// Delivery window start from DTM qualifier 2.
    pub delivery_start: Option<String>,
    /// Delivery window end from DTM qualifier 3.
    pub delivery_end: Option<String>,
}

impl DeliveryOrderLine {
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

/// A parsed DELORD message (Delivery Order).
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeliveryOrderMessage {
    pub(crate) core: MessageCore,
    /// Delivery order reference from BGM element 1, component 0.
    ///
    /// The corresponding DELRES will reference this in `RFF+Z13` for correlation.
    pub order_ref: Option<String>,
    /// Gas day / delivery period from DTM qualifier 137.
    pub gas_day: Option<String>,
    /// Requested delivery lines (one per delivery point/window).
    pub lines: Vec<DeliveryOrderLine>,
}

impl_dvgw_message!(DeliveryOrderMessage);

impl DeliveryOrderMessage {
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Delord);
        let order_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let gas_day = extract_dtm(&core.segments, "137");
        let lines = extract_lines(&core.segments);
        Self {
            core,
            order_ref,
            gas_day,
            lines,
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

fn extract_lines(segs: &[OwnedSegment]) -> Vec<DeliveryOrderLine> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].tag == "LOC" {
            let location_qualifier = segs[i].element_str(0).unwrap_or("").to_owned();
            let location_code = segs[i].component_str(1, 0).unwrap_or("").to_owned();
            let mut quantity = String::new();
            let mut quantity_qualifier = String::new();
            let mut unit = None;
            let mut delivery_start = None;
            let mut delivery_end = None;
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
                            "2" => delivery_start = v,
                            "3" => delivery_end = v,
                            _ => {}
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if !location_code.is_empty() {
                result.push(DeliveryOrderLine {
                    location_code,
                    location_qualifier,
                    quantity,
                    quantity_qualifier,
                    unit,
                    delivery_start,
                    delivery_end,
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

    fn minimal_segments() -> Vec<OwnedSegment> {
        vec![
            seg(
                "UNH",
                vec![vec!["00001"], vec!["DELORD", "", "", "", "3.0"]],
            ),
            seg("BGM", vec![vec![""], vec!["DORD-001"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001368S"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001370C"]]),
            seg("DTM", vec![vec!["137", "20260115", "102"]]),
            seg("UNT", vec![vec!["5"], vec!["00001"]]),
        ]
    }

    #[test]
    fn from_segments_extracts_core_fields() {
        let msg = DeliveryOrderMessage::from_segments(minimal_segments());
        assert_eq!(msg.message_type(), DvgwMessageType::Delord);
        assert_eq!(msg.sender_eic(), Some("21X000000001368S"));
        assert_eq!(msg.receiver_eic(), Some("21X000000001370C"));
        assert_eq!(msg.order_ref.as_deref(), Some("DORD-001"));
        assert_eq!(msg.gas_day.as_deref(), Some("20260115"));
    }

    #[test]
    fn with_quantity_line() {
        let mut segs = minimal_segments();
        segs.insert(5, seg("LOC", vec![vec!["Z02"], vec!["LOCATION-001"]]));
        segs.insert(6, seg("QTY", vec![vec!["136", "1000000", "KWH"]]));
        let msg = DeliveryOrderMessage::from_segments(segs);
        assert_eq!(msg.lines.len(), 1);
        assert_eq!(msg.lines[0].location_code, "LOCATION-001");
        assert_eq!(msg.lines[0].quantity_f64(), Some(1_000_000.0));
    }
}
