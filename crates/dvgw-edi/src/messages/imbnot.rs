//! IMBNOT — Imbalance Notification message.
//!
//! The IMBNOT message is sent by an FNB or MGV to a BKV to notify them of an
//! imbalance in their balance group for a given gas day.
//!
//! **Format versions:** DVGW G685 / G2000
//! **UN/EDIFACT base:** D03A

use edifact_rs::OwnedSegment;

use crate::{
    DvgwMessageType,
    message::{MessageCore, find_all_segments, find_segment, impl_dvgw_message},
};

// ── Typed field types ─────────────────────────────────────────────────────────

/// An imbalance quantity for a specific balance group entry.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub struct ImbalanceEntry {
    /// Balance group code from LOC or NAD.
    pub balance_group: String,
    /// Imbalance quantity value from QTY C186 component 1 (negative = short).
    pub quantity: String,
    /// Quantity qualifier from QTY C186 component 0 (e.g. "136").
    pub quantity_qualifier: String,
    /// Measurement unit from QTY C186 component 2 (e.g. "KWH").
    pub unit: Option<String>,
    /// Direction qualifier: "Z03" = short, "Z04" = long.
    pub direction: Option<String>,
}

impl ImbalanceEntry {
    /// Parse the `quantity` string as a `Decimal`.
    ///
    /// Returns `None` when the string is empty or not a valid decimal number.
    /// EDIFACT decimal notation uses `.` as the decimal mark.
    ///
    /// Gas quantities are settled to at least three decimal places (DVGW G 685
    /// §7), and binary floating point cannot represent those decimal fractions
    /// exactly, so `Decimal` is the only accessor offered.
    #[cfg(feature = "decimal")]
    #[must_use]
    pub fn quantity_decimal(&self) -> Option<rust_decimal::Decimal> {
        use std::str::FromStr;
        if self.quantity.is_empty() {
            None
        } else {
            rust_decimal::Decimal::from_str(&self.quantity).ok()
        }
    }

    /// Returns `true` if the direction is explicitly marked as short (Z03).
    #[must_use]
    pub fn is_short(&self) -> bool {
        self.direction.as_deref() == Some("Z03")
    }

    /// Returns `true` if the direction is explicitly marked as long (Z04).
    #[must_use]
    pub fn is_long(&self) -> bool {
        self.direction.as_deref() == Some("Z04")
    }
}

// ── Main message struct ───────────────────────────────────────────────────────

/// A parsed IMBNOT message (Imbalance Notification).
///
/// Access typed fields via the public properties, or use [`DvgwMessage`] trait
/// methods for common operations.
///
/// [`DvgwMessage`]: crate::message::DvgwMessage
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ImbalanceMessage {
    pub(crate) core: MessageCore,
    /// Document reference from BGM element 1, component 0.
    pub document_ref: Option<String>,
    /// Gas day from DTM qualifier 137.
    pub gas_day: Option<String>,
    /// Imbalance entries (one per balance group or measurement point).
    pub entries: Vec<ImbalanceEntry>,
}

impl_dvgw_message!(ImbalanceMessage);

impl ImbalanceMessage {
    pub(crate) fn from_segments(segments: Vec<OwnedSegment>) -> Self {
        let core = MessageCore::from_segments(segments, DvgwMessageType::Imbnot);
        let document_ref = find_segment(&core.segments, "BGM")
            .and_then(|s| s.component_str(1, 0))
            .map(str::to_owned);
        let gas_day = extract_dtm(&core.segments, "137");
        let entries = extract_entries(&core.segments);
        Self {
            core,
            document_ref,
            gas_day,
            entries,
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

fn extract_entries(segs: &[OwnedSegment]) -> Vec<ImbalanceEntry> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < segs.len() {
        let seg = &segs[i];
        // Each QTY segment with an imbalance qualifier starts an entry.
        // The balance group comes from the preceding LOC or NAD segment.
        if seg.tag == "QTY" {
            let quantity_qualifier = seg.component_str(0, 0).unwrap_or("").to_owned();
            let quantity = seg.component_str(0, 1).unwrap_or("").to_owned();
            let unit = seg.component_str(0, 2).map(str::to_owned);
            // Look back for balance group identifier in preceding LOC or NAD
            let balance_group = segs[..i]
                .iter()
                .rev()
                .find(|s| s.tag == "LOC" || (s.tag == "NAD" && s.element_str(0) == Some("Z01")))
                .map(|s| s.component_str(1, 0).unwrap_or("").to_owned())
                .unwrap_or_default();
            // Look ahead for STS segment with direction qualifier
            let direction = segs
                .get(i + 1)
                .filter(|s| s.tag == "STS")
                .and_then(|s| s.component_str(0, 0))
                .map(str::to_owned);
            if !quantity.is_empty() {
                result.push(ImbalanceEntry {
                    balance_group,
                    quantity,
                    quantity_qualifier,
                    unit,
                    direction,
                });
            }
        }
        i += 1;
    }
    result
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::{DvgwMessageType, message::DvgwMessage};

    use super::*;

    fn seg(tag: &str, elements: Vec<Vec<&str>>) -> OwnedSegment {
        use edifact_rs::OwnedElement;
        OwnedSegment::new(
            tag,
            elements
                .into_iter()
                .map(|comps| OwnedElement::of(&comps))
                .collect(),
        )
    }

    fn minimal_segments() -> Vec<OwnedSegment> {
        vec![
            seg(
                "UNH",
                vec![vec!["00001"], vec!["IMBNOT", "", "", "", "3.0"]],
            ),
            seg("BGM", vec![vec![""], vec!["IMBN-001"], vec!["9"]]),
            seg("NAD", vec![vec!["MS"], vec!["21X000000001368W"]]),
            seg("NAD", vec![vec!["MR"], vec!["21X000000001369U"]]),
            seg("DTM", vec![vec!["137", "20260115", "102"]]),
            seg("UNT", vec![vec!["5"], vec!["00001"]]),
        ]
    }

    #[test]
    fn from_segments_extracts_core_fields() {
        let msg = ImbalanceMessage::from_segments(minimal_segments());
        assert_eq!(msg.message_type(), DvgwMessageType::Imbnot);
        assert_eq!(msg.sender_eic(), Some("21X000000001368W"));
        assert_eq!(msg.receiver_eic(), Some("21X000000001369U"));
        assert_eq!(msg.document_ref.as_deref(), Some("IMBN-001"));
        assert_eq!(msg.gas_day.as_deref(), Some("20260115"));
    }

    #[test]
    fn is_short_and_is_long_helpers() {
        let short = ImbalanceEntry {
            balance_group: "BG001".to_owned(),
            quantity: "-5000".to_owned(),
            quantity_qualifier: "136".to_owned(),
            unit: Some("KWH".to_owned()),
            direction: Some("Z03".to_owned()),
        };
        assert!(short.is_short());
        assert!(!short.is_long());

        let long = ImbalanceEntry {
            direction: Some("Z04".to_owned()),
            ..short.clone()
        };
        assert!(long.is_long());
        assert!(!long.is_short());
    }
}
