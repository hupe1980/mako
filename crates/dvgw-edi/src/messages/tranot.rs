//! TRANOT — Transport Notification message.
//!
//! The TRANOT message notifies market participants about transport conditions,
//! capacity restrictions, or force majeure events affecting their delivery points.
//!
//! **Format versions:** DVGW G685 / G2000
//! **UN/EDIFACT base:** D03A

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// An affected delivery point or location cited in the transport notification.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct AffectedPoint {
    /// Location code from LOC element 1, component 0.
    pub location_code: String,
    /// Location qualifier from LOC element 0.
    pub location_qualifier: String,
    /// Capacity restriction quantity from QTY (if present).
    pub restricted_capacity: Option<String>,
    /// Unit of the restricted capacity (e.g. "KWH", "KW").
    pub capacity_unit: Option<String>,
}

// ── Main message struct ───────────────────────────────────────────────────────

/// A parsed TRANOT message (Transport Notification).
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TransportNotificationMessage {
    pub(crate) core: MessageCore,
    /// Document reference from BGM element 1, component 0.
    pub document_ref: Option<String>,
    /// Notification type qualifier from BGM element 0, component 0 (e.g. "Z01" = restriction).
    pub notification_type: Option<String>,
    /// Affected period start from DTM qualifier 2.
    pub period_start: Option<String>,
    /// Affected period end from DTM qualifier 3.
    pub period_end: Option<String>,
    /// Free-text reason / description from FTX segments.
    pub reason: Option<String>,
    /// Affected delivery points or locations.
    pub affected_points: Vec<AffectedPoint>,
}

impl_dvgw_message!(TransportNotificationMessage);

impl TransportNotificationMessage {
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Tranot);
        let document_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let notification_type = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(0, 0))
            .map(str::to_owned);
        let period_start = extract_dtm(&core.segments, "2");
        let period_end = extract_dtm(&core.segments, "3");
        let reason = find_segment(&core.segments, "FTX")
            .and_then(|s| s.component_str(3, 0))
            .map(str::to_owned);
        let affected_points = extract_affected_points(&core.segments);
        Self {
            core,
            document_ref,
            notification_type,
            period_start,
            period_end,
            reason,
            affected_points,
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

fn extract_affected_points(segs: &[OwnedSegment]) -> Vec<AffectedPoint> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        if segs[i].tag == "LOC" {
            let location_qualifier = segs[i].element_str(0).unwrap_or("").to_owned();
            let location_code = segs[i].component_str(1, 0).unwrap_or("").to_owned();
            let mut restricted_capacity = None;
            let mut capacity_unit = None;
            let mut j = i + 1;
            while j < segs.len() && segs[j].tag != "LOC" {
                if segs[j].tag == "QTY" {
                    restricted_capacity = segs[j].component_str(0, 1).map(str::to_owned);
                    capacity_unit = segs[j].component_str(0, 2).map(str::to_owned);
                }
                j += 1;
            }
            if !location_code.is_empty() {
                result.push(AffectedPoint {
                    location_code,
                    location_qualifier,
                    restricted_capacity,
                    capacity_unit,
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
                vec![vec!["00001"], vec!["TRANOT", "", "", "", "3.0"]],
            ),
            seg("BGM", vec![vec!["Z01"], vec!["TRAN-001"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001368S"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001369Q"]]),
            seg("DTM", vec![vec!["2", "202601150600", "203"]]),
            seg("DTM", vec![vec!["3", "202601151800", "203"]]),
            seg(
                "FTX",
                vec![
                    vec!["AAI"],
                    vec![""],
                    vec![""],
                    vec!["Maintenance work on compressor station"],
                ],
            ),
            seg("UNT", vec![vec!["7"], vec!["00001"]]),
        ]
    }

    #[test]
    fn from_segments_extracts_core_fields() {
        let msg = TransportNotificationMessage::from_segments(minimal_segments());
        assert_eq!(msg.message_type(), DvgwMessageType::Tranot);
        assert_eq!(msg.sender_eic(), Some("21X000000001368S"));
        assert_eq!(msg.receiver_eic(), Some("21X000000001369Q"));
        assert_eq!(msg.document_ref.as_deref(), Some("TRAN-001"));
        assert_eq!(msg.notification_type.as_deref(), Some("Z01"));
        assert_eq!(msg.period_start.as_deref(), Some("202601150600"));
        assert_eq!(msg.period_end.as_deref(), Some("202601151800"));
        assert_eq!(
            msg.reason.as_deref(),
            Some("Maintenance work on compressor station")
        );
    }
}
