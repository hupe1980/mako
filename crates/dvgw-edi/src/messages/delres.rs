//! DELRES — Delivery Response message.
//!
//! FNB or MGV response confirming, modifying, or rejecting a DELORD delivery
//! order. Correlates back to the originating DELORD via an `RFF+Z13` reference.
//!
//! **Format versions:** DVGW G685 / G2000
//! **UN/EDIFACT base:** D03A

use std::fmt;

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// Response status from the FNB/MGV.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum DelresStatus {
    /// Delivery accepted as requested.
    Accepted,
    /// Delivery accepted with modifications (quantity adjusted).
    Modified,
    /// Delivery rejected.
    Rejected,
    /// Status code not mapped to a known variant.
    Other(String),
}

impl DelresStatus {
    fn from_sts(qualifier: &str) -> Self {
        match qualifier {
            "Z01" | "AC" | "1" => Self::Accepted,
            "Z02" | "PA" => Self::Modified,
            "Z03" | "RE" | "7" => Self::Rejected,
            other => Self::Other(other.to_owned()),
        }
    }

    /// Human-readable representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Accepted => "Accepted",
            Self::Modified => "Modified",
            Self::Rejected => "Rejected",
            Self::Other(code) => code.as_str(),
        }
    }
}

impl fmt::Display for DelresStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A confirmed/adjusted delivery quantity for one location.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct DeliveryResponseLine {
    /// Location code from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0.
    pub location_qualifier: String,
    /// Confirmed/adjusted quantity value from QTY C186 component 1.
    pub quantity: String,
    /// Quantity qualifier from QTY C186 component 0.
    pub quantity_qualifier: String,
    /// Measurement unit from QTY C186 component 2 (e.g. "KWH").
    pub unit: Option<String>,
    /// Per-line acceptance status from STS.
    pub status: Option<DelresStatus>,
    /// Delivery window start from DTM qualifier 2.
    pub delivery_start: Option<String>,
    /// Delivery window end from DTM qualifier 3.
    pub delivery_end: Option<String>,
}

impl DeliveryResponseLine {
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

/// A parsed DELRES message (Delivery Response).
///
/// The key routing field is `order_ref` — it carries the `RFF+Z13` value from
/// the originating DELORD and is used to correlate this response back to the
/// outbound delivery order workflow.
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct DeliveryResponseMessage {
    pub(crate) core: MessageCore,
    /// DELRES document reference from BGM element 1, component 0.
    pub response_ref: Option<String>,
    /// Reference to the originating DELORD document (`RFF+Z13`).
    ///
    /// Match `delres.order_ref == delord.order_ref` to correlate the response
    /// back to the outbound delivery order workflow.
    pub order_ref: Option<String>,
    /// Overall status from the leading STS segment.
    pub status: Option<DelresStatus>,
    /// Rejection reason from FTX (populated when `status = Rejected`).
    pub rejection_reason: Option<String>,
    /// Gas day from DTM qualifier 137.
    pub gas_day: Option<String>,
    /// Per-location response lines.
    pub lines: Vec<DeliveryResponseLine>,
}

impl_dvgw_message!(DeliveryResponseMessage);

impl DeliveryResponseMessage {
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Delres);
        let response_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let order_ref = find_all_segments(&core.segments, "RFF")
            .find(|s| s.component_str(0, 0) == Some("Z13"))
            .and_then(|s| s.component_str(0, 1))
            .map(str::to_owned);
        let status = find_segment(&core.segments, "STS")
            .and_then(|s| s.component_str(0, 0))
            .map(DelresStatus::from_sts);
        let rejection_reason = if matches!(status, Some(DelresStatus::Rejected)) {
            find_segment(&core.segments, "FTX")
                .and_then(|s| s.component_str(3, 0))
                .map(str::to_owned)
        } else {
            None
        };
        let gas_day = extract_dtm(&core.segments, "137");
        let lines = extract_lines(&core.segments);
        Self {
            core,
            response_ref,
            order_ref,
            status,
            rejection_reason,
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

fn extract_lines(segs: &[OwnedSegment]) -> Vec<DeliveryResponseLine> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].tag == "LOC" {
            let location_qualifier = segs[i].element_str(0).unwrap_or("").to_owned();
            let location_code = segs[i].component_str(1, 0).unwrap_or("").to_owned();
            let mut quantity = String::new();
            let mut quantity_qualifier = String::new();
            let mut unit = None;
            let mut status = None;
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
                    "STS" => {
                        status = segs[j].component_str(0, 0).map(DelresStatus::from_sts);
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
                result.push(DeliveryResponseLine {
                    location_code,
                    location_qualifier,
                    quantity,
                    quantity_qualifier,
                    unit,
                    status,
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

    fn accepted_segments() -> Vec<OwnedSegment> {
        vec![
            seg(
                "UNH",
                vec![vec!["00001"], vec!["DELRES", "", "", "", "3.0"]],
            ),
            seg("BGM", vec![vec![""], vec!["DRES-001"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001370C"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001368S"]]),
            seg("DTM", vec![vec!["137", "20260115", "102"]]),
            seg("RFF", vec![vec!["Z13", "DORD-001"]]),
            seg("STS", vec![vec!["Z01"]]),
            seg("UNT", vec![vec!["7"], vec!["00001"]]),
        ]
    }

    fn rejected_segments() -> Vec<OwnedSegment> {
        vec![
            seg(
                "UNH",
                vec![vec!["00001"], vec!["DELRES", "", "", "", "3.0"]],
            ),
            seg("BGM", vec![vec![""], vec!["DRES-002"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001370C"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001368S"]]),
            seg("DTM", vec![vec!["137", "20260115", "102"]]),
            seg("RFF", vec![vec!["Z13", "DORD-002"]]),
            seg("STS", vec![vec!["Z03"]]),
            seg(
                "FTX",
                vec![
                    vec!["AAI"],
                    vec![""],
                    vec![""],
                    vec!["Insufficient capacity at delivery point"],
                ],
            ),
            seg("UNT", vec![vec!["8"], vec!["00001"]]),
        ]
    }

    #[test]
    fn from_segments_accepted() {
        let msg = DeliveryResponseMessage::from_segments(accepted_segments());
        assert_eq!(msg.message_type(), DvgwMessageType::Delres);
        assert_eq!(msg.sender_eic(), Some("21X000000001370C"));
        assert_eq!(msg.receiver_eic(), Some("21X000000001368S"));
        assert_eq!(msg.response_ref.as_deref(), Some("DRES-001"));
        assert_eq!(msg.order_ref.as_deref(), Some("DORD-001"));
        assert_eq!(msg.status, Some(DelresStatus::Accepted));
        assert!(msg.rejection_reason.is_none());
    }

    #[test]
    fn from_segments_rejected_with_reason() {
        let msg = DeliveryResponseMessage::from_segments(rejected_segments());
        assert_eq!(msg.status, Some(DelresStatus::Rejected));
        assert_eq!(
            msg.rejection_reason.as_deref(),
            Some("Insufficient capacity at delivery point")
        );
        assert_eq!(msg.order_ref.as_deref(), Some("DORD-002"));
    }

    #[test]
    fn delres_status_display() {
        assert_eq!(DelresStatus::Accepted.to_string(), "Accepted");
        assert_eq!(DelresStatus::Modified.to_string(), "Modified");
        assert_eq!(DelresStatus::Rejected.to_string(), "Rejected");
    }
}
