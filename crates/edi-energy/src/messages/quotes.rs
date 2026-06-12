use edifact_rs::OwnedSegment;

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// QUOTES — Quotation (Angebot) message.
///
/// Used in the German energy market for quotations sent by metering-point
/// operators (MSB) in response to requests for quotation (REQOTE). Covers
/// device takeover offers, general offers, and technical configuration offers.
///
/// | Field      | Segment | Meaning                             |
/// |------------|---------|-------------------------------------|
/// | `bgm`      | BGM     | Document type / message reference   |
/// | `dtm`      | DTM     | Date / time segments (up to 7)      |
/// | `sender`   | NAD+MS  | Message sender                      |
/// | `receiver` | NAD+MR  | Message receiver                    |
///
/// Wire type string: `QUOTES:D:10A:UN:{release}`.
///
/// Supported releases (fv-dated profiles):
/// - `releases::quotes_fv20250401()` (wire: `"1.3b"`, AHB 1.1, valid from 2025-04-01)
/// - `releases::quotes_fv20260401()` (wire: `"1.3c"`, AHB 1.1a, valid from 2026-04-01)
///
/// Both releases share the same 5 Prüfidentifikatoren (15001–15005).
#[derive(Debug, Clone)]
pub struct QuotesMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.
    bgm: Option<Bgm>,
    /// DTM — date/time segments (message date, delivery date, etc.).
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message receiver.
    receiver: Option<Nad>,
}

impl QuotesMessage {
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
                MessageType::Quotes,
            ),
            bgm,
            dtm,
            sender,
            receiver,
        }
    }

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

impl_edi_energy_message!(QuotesMessage);
