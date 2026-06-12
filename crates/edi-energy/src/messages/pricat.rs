use edifact_rs::OwnedSegment;

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// PRICAT — Price/Sales Catalogue (Preisliste) message.
///
/// PRICAT is the EDI@Energy message used to transmit price lists and tariff
/// information, for example Ausgleichsenergiepreise (PID 27001),
/// Preisblatt MSB-Leistungen (PID 27002), and Preisblatt NB-Leistungen
/// (PID 27003).
///
/// **Note on Prüfidentifikator**: Like COMDIS, PRICAT stores its
/// Prüfidentifikator in a top-level `SG1 RFF+Z13` segment rather than
/// `BGM DE 1004`. The parser extracts the PID from `RFF+Z13` at parse time;
/// `detect_pruefidentifikator()` returns the correct value when the segment
/// is present.
///
/// **Annual format change**: The SG4 (CTA/COM contact details) sub-group of
/// SG2 was removed in MIG 2.1 (fv20260401) compared to MIG 2.0e (fv20250401).
/// Both profile versions are supported via the profile registry.
///
/// **Builder**: Use `crate::PricatBuilder` to create PRICAT messages
/// programmatically. The builder covers the header (UNH, BGM, RFF+Z13,
/// DTM+137, NAD+MR/MS, UNT); SG17/SG36/SG40 price body segments are not
/// yet supported (see REFACTOR.md F-030).
///
/// | Field      | Segment | Meaning                             |
/// |------------|---------|-------------------------------------|
/// | `bgm`      | BGM     | Document type / message reference   |
/// | `dtm`      | DTM     | Date / time segments                |
/// | `sender`   | NAD+MS  | Message sender                      |
/// | `receiver` | NAD+MR  | Message receiver                    |
#[derive(Debug, Clone)]
pub struct PricatMessage {
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

impl PricatMessage {
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
                MessageType::Pricat,
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

impl_edi_energy_message!(PricatMessage);
