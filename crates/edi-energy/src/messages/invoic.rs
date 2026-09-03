use edifact_rs::{OwnedSegment, ProfileRulePack, ValidationIssue};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// INVOIC — Invoice message.
///
/// Typed access to key fields of an INVOIC message in the German energy market.
///
/// | Field      | Segment | Meaning                          |
/// |------------|---------|----------------------------------|
/// | `bgm`      | BGM     | Document type / invoice reference |
/// | `dtm`      | DTM     | Date / time segments             |
/// | `sender`   | NAD+MS  | Message sender                   |
/// | `receiver` | NAD+MR  | Message receiver                 |
#[derive(Debug, Clone)]
pub struct InvoicMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message (document type and invoice number).
    bgm: Option<Bgm>,
    /// DTM — date/time segments.
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message receiver.
    receiver: Option<Nad>,
}

impl InvoicMessage {
    #[must_use]
    pub(crate) fn from_parts(
        segments: Vec<OwnedSegment>,
        message_ref: impl Into<Box<str>>,
        assoc_code: impl Into<Box<str>>,
        pruefidentifikator: Option<u32>,
    ) -> Self {
        let (bgm, dtm, sender, receiver) = (
            find_bgm(&segments),
            collect_dtm(&segments),
            find_nad(&segments, "MS"),
            find_nad(&segments, "MR"),
        );
        Self {
            core: MessageCore::new(
                segments,
                message_ref,
                assoc_code,
                pruefidentifikator,
                MessageType::Invoic,
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

impl_edi_energy_message!(InvoicMessage, sem = invoic_semantic_pack());

/// Semantic rule pack for INVOIC: `SEM-INVOIC-PERIOD-ORDER` — when `DTM+163`
/// (Beginn Abrechnungszeitraum) and `DTM+164` (Ende Abrechnungszeitraum) are both
/// present, the start must not be after the end. Presence of dates is the
/// AHB's business.
fn invoic_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("INVOIC-SEM")
        .for_message_type("INVOIC")
        .with_rule_fn(
            |segs: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>| {
                super::common::check_period_order(segs, "SEM-INVOIC-PERIOD-ORDER", issues);
            },
        )
}
