use edifact_rs::{OwnedSegment, ProfileRulePack, ValidationIssue};

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// ORDERS — Purchase Order message.
///
/// | Field      | Segment | Meaning                             |
/// |------------|---------|-------------------------------------|
/// | `bgm`      | BGM     | Document type / message reference   |
/// | `dtm`      | DTM     | Date / time segments                |
/// | `sender`   | NAD+MS  | Message sender                      |
/// | `receiver` | NAD+MR  | Message receiver                    |
#[derive(Debug, Clone)]
pub struct OrdersMessage {
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

impl OrdersMessage {
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
                MessageType::Orders,
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

impl_edi_energy_message!(OrdersMessage, sem = orders_semantic_pack());

/// `SEM-ORDERS-PERIOD-ORDER` — When both a period-start (`DTM+163`) and a
/// period-end (`DTM+164`) are present, the start must not be after the end.
fn orders_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("ORDERS-SEM")
        .for_message_type("ORDERS")
        .with_stateless_rule_fn(
            |segs: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>| {
                super::common::check_period_order(segs, "SEM-ORDERS-PERIOD-ORDER", issues);
            },
        )
}
