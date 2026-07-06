use edifact_rs::{OwnedSegment, ProfileRulePack, ValidationIssue, ValidationSeverity};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// REMADV — Remittance Advice message.
///
/// | Field      | Segment | Meaning                             |
/// |------------|---------|-------------------------------------|
/// | `bgm`      | BGM     | Document type / message reference   |
/// | `dtm`      | DTM     | Date / time segments                |
/// | `sender`   | NAD+MS  | Message sender                      |
/// | `receiver` | NAD+MR  | Message receiver                    |
#[derive(Debug, Clone)]
pub struct RemadvMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.
    bgm: Option<Bgm>,
    /// DTM — date/time segments.
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message receiver.
    receiver: Option<Nad>,
}

impl RemadvMessage {
    #[must_use]
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        let (bgm, dtm, sender, receiver) = {
            let borrowed: Vec<edifact_rs::Segment<'_>> =
                segments.iter().map(|s| s.as_borrowed()).collect();
            (
                find_bgm(&borrowed),
                collect_dtm(&borrowed),
                find_nad(&borrowed, "MS"),
                find_nad(&borrowed, "MR"),
            )
        };
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Remadv,
            ),
            bgm,
            dtm,
            sender,
            receiver,
        }
    }

    /// The message reference number from UNH (DE 0062).
    /// The EDI@Energy release / association code from UNH DE 0057.
    #[must_use]
    pub fn assoc_code(&self) -> &str {
        &self.core.assoc_code
    }
    /// Raw parsed segments (authoritative for validation and serialization).
    #[must_use]
    pub fn segments(&self) -> &[OwnedSegment] {
        &self.core.segments
    }

    /// BGM — beginning of message.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn bgm(&self) -> Option<&Bgm> {
        self.bgm.as_ref()
    }

    /// DTM — message-level date/time segments.
    #[must_use]
    pub fn dtm(&self) -> &[Dtm] {
        &self.dtm
    }

    /// NAD+MS — message sender.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn sender(&self) -> Option<&Nad> {
        self.sender.as_ref()
    }

    /// NAD+MR — message recipient.  Returns `None` when absent or malformed.
    #[must_use]
    pub fn receiver(&self) -> Option<&Nad> {
        self.receiver.as_ref()
    }
}

impl_edi_energy_message!(RemadvMessage, sem = remadv_semantic_pack());

/// Semantic rule pack for REMADV.
///
/// Checks that are universal across all BDEW REMADV messages:
/// - `SEM-REMADV-DTM-137-REQUIRED`: `DTM+137` (Zahlungsdatum / payment date) must
///   be present.
/// - `SEM-REMADV-PERIOD-ORDER`: When `DTM+163` (Beginn Zahlungszeitraum) and
///   `DTM+164` (Ende Zahlungszeitraum) are both present, start must not be after end.
fn remadv_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("REMADV-SEM")
        .for_message_type("REMADV")
        .with_stateless_rule_fn(
            |segs: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>| {
                // DTM+137 (Zahlungsdatum) is mandatory in BDEW REMADV messages.
                if !segs
                    .iter()
                    .any(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("137"))
                {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            "DTM+137 (Zahlungsdatum / payment date) is missing",
                        )
                        .with_rule_id("SEM-REMADV-DTM-137-REQUIRED")
                        .with_segment("DTM")
                        .with_suggestion(
                            "Add DTM+137:<YYYYMMDD>:102' to specify the payment \
                             date (format 102 = CCYYMMDD per UN/EDIFACT)",
                        ),
                    );
                }
                super::common::check_period_order(segs, "SEM-REMADV-PERIOD-ORDER", issues);
            },
        )
}
