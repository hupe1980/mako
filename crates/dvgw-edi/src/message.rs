use edifact_rs::OwnedSegment;

use crate::{DvgwMessageType, DvgwVersion};

// ── Helper: safe element access ───────────────────────────────────────────────

/// Find the first segment with tag `tag` in a flat slice.
pub(crate) fn find_segment<'a>(segs: &'a [OwnedSegment], tag: &str) -> Option<&'a OwnedSegment> {
    segs.iter().find(|s| s.tag == tag)
}

/// Find all segments with tag `tag` in a flat slice.
pub(crate) fn find_all_segments<'a>(
    segs: &'a [OwnedSegment],
    tag: &str,
) -> impl Iterator<Item = &'a OwnedSegment> {
    segs.iter().filter(move |s| s.tag == tag)
}

/// Extract the message reference (UNH DE 0062) from raw segments.
pub(crate) fn extract_message_ref(segs: &[OwnedSegment]) -> &str {
    segs.iter()
        .find(|s| s.tag == "UNH")
        .and_then(|s| s.element_str(0))
        .unwrap_or("")
}

/// Extract the DVGW version string (UNH element 1, component 4 — association code).
pub(crate) fn extract_version(segs: &[OwnedSegment]) -> Option<DvgwVersion> {
    // UNH element 1: S009 composite
    // component 0 = message type, component 2 = version, component 4 = association
    let unh = segs.iter().find(|s| s.tag == "UNH")?;
    // component 4 is the association-assigned code (e.g. "5.11a")
    let assoc = unh.component_str(1, 4)?;
    DvgwVersion::parse(assoc)
}

/// Extract the message type from UNH (element 1, component 0).
pub(crate) fn extract_message_type_code(segs: &[OwnedSegment]) -> Option<String> {
    let unh = segs.iter().find(|s| s.tag == "UNH")?;
    Some(unh.component_str(1, 0)?.to_owned())
}

// ── DvgwMessage trait ─────────────────────────────────────────────────────────

/// Core abstraction for all DVGW message types.
///
/// Every concrete message type (`AlocatMessage`, `NomintMessage`, …) implements
/// this trait. [`AnyDvgwMessage`](crate::AnyDvgwMessage) delegates to it.
///
/// ## Design
///
/// DVGW messages do not use a BGM Prüfidentifikator for routing. Instead,
/// routing is determined by the combination of message type (from UNH) and the
/// sender/receiver role qualifier (NAD+MS/MR). This trait surface reflects that.
pub trait DvgwMessage: Send + Sync {
    /// Returns the DVGW message type discriminant.
    fn message_type(&self) -> DvgwMessageType;

    /// Returns the version extracted from UNH (association code DE 0057).
    fn version(&self) -> Option<&DvgwVersion>;

    /// Returns the UNH message reference (DE 0062).
    fn message_ref(&self) -> &str;

    /// Returns the sender EIC code from NAD+MS (market participant).
    ///
    /// Returns `None` when the NAD+MS segment is absent or malformed.
    fn sender_eic(&self) -> Option<&str>;

    /// Returns the receiver EIC code from NAD+MR.
    ///
    /// Returns `None` when the NAD+MR segment is absent or malformed.
    fn receiver_eic(&self) -> Option<&str>;

    /// Returns the raw parsed segments (including any envelope).
    fn segments(&self) -> &[OwnedSegment];
}

// ── MessageCore ───────────────────────────────────────────────────────────────

/// Internal storage shared by all concrete DVGW message types.
#[derive(Debug, Clone)]
pub(crate) struct MessageCore {
    /// All parsed segments (owned, heap-allocated).
    pub(crate) segments: Vec<OwnedSegment>,
    /// Message type, always `Some` for concrete types.
    pub(crate) message_type: DvgwMessageType,
    /// Version extracted from UNH S009 component 4.
    pub(crate) version: Option<DvgwVersion>,
    /// UNH message reference (DE 0062).
    pub(crate) message_ref: String,
    /// Sender EIC from NAD+MS.
    pub(crate) sender_eic: Option<String>,
    /// Receiver EIC from NAD+MR.
    pub(crate) receiver_eic: Option<String>,
}

impl MessageCore {
    /// Construct from raw segments, extracting metadata eagerly.
    pub(crate) fn from_segments(
        segments: Vec<OwnedSegment>,
        message_type: DvgwMessageType,
    ) -> Self {
        let version = extract_version(&segments);
        let message_ref = extract_message_ref(&segments).to_owned();
        let sender_eic = extract_nad_eic(&segments, "MS");
        let receiver_eic = extract_nad_eic(&segments, "MR");
        Self {
            segments,
            message_type,
            version,
            message_ref,
            sender_eic,
            receiver_eic,
        }
    }
}

/// Extract the EIC identifier from the first NAD segment with the given role qualifier.
///
/// NAD element 0 is the party qualifier (e.g. "MS", "MR").
/// NAD element 1 (composite C082) component 0 is the party identifier (EIC code).
fn extract_nad_eic(segs: &[OwnedSegment], qualifier: &str) -> Option<String> {
    segs.iter()
        .find(|s| s.tag == "NAD" && s.element_str(0) == Some(qualifier))
        .and_then(|s| s.component_str(1, 0))
        .map(str::to_owned)
}

/// Implement `DvgwMessage` for a concrete type that holds `self.core: MessageCore`.
macro_rules! impl_dvgw_message {
    ($T:ty) => {
        impl crate::message::DvgwMessage for $T {
            fn message_type(&self) -> crate::DvgwMessageType {
                self.core.message_type
            }
            fn version(&self) -> Option<&crate::DvgwVersion> {
                self.core.version.as_ref()
            }
            fn message_ref(&self) -> &str {
                &self.core.message_ref
            }
            fn sender_eic(&self) -> Option<&str> {
                self.core.sender_eic.as_deref()
            }
            fn receiver_eic(&self) -> Option<&str> {
                self.core.receiver_eic.as_deref()
            }
            fn segments(&self) -> &[edifact_rs::OwnedSegment] {
                &self.core.segments
            }
        }
    };
}

pub(crate) use impl_dvgw_message;
