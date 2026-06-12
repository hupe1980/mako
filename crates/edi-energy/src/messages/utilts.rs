use edifact_rs::OwnedSegment;

use crate::{
    MessageType,
    messages::{
        core::MessageCore,
        segments::{Bgm, Dtm, Nad, collect_dtm, find_bgm, find_nad},
    },
};

/// UTILTS — Übertragung technischer Stammdaten (Technical Master Data) message.
///
/// UTILTS is the EDI@Energy message used to exchange technical master data such
/// as calculation formulas (`Berechnungsformel`, PID 25001), time-switching
/// definitions (`Zählzeitdefinitionen`, PID 25004/25005), switching-time
/// definitions (`Schaltzeitdefinitionen`, PID 25006/25008), and load-curve
/// definitions (`Leistungskurvendefinitionen`, PID 25007/25009).
///
/// **Release**: Both AHB 1.0 (fv20241001) and AHB 1.1 (fv20260401) use the
/// same MIG 1.1e wire format. The profile version selection is driven by the
/// `valid_from` date of each AHB release.
///
/// **Note on Prüfidentifikator**: Like COMDIS and PRICAT, UTILTS stores its
/// Prüfidentifikator in an `SG5 → SG6 RFF+Z13` segment rather than
/// `BGM DE 1004`. The parser extracts the PID from the first `RFF+Z13` found
/// during linear segment scanning via the `rff_z13` `pid_source` strategy.
///
/// **Builder**: Use [`crate::builders::UtiltsBuilder`] to create UTILTS
/// messages programmatically. The builder covers the full MIG 1.1e structure:
/// standard header (UNH, BGM, DTM+137, NAD+MS, NAD+MR), Vorgang header
/// (IDE+24, LOC+172/Z09, DTM+157, STS+Z23), Prüfidentifikator (SG6 RFF+Z13),
/// Verwendungszeitraum periods (SG6 RFF+Z49/Z53 + DTM+Z25/Z26), energy-amount
/// references (SG8 SEQ+Z36), calculation-step components (SG8 SEQ+Z37 with
/// full SG9 CCI/CAV operator chains), and time-switching / load-curve
/// definition blocks (SG8 SEQ+Z42/Z43/Z41/Z69/Z70/Z74 with SG9 properties).
///
/// | Field      | Segment | Meaning                             |
/// |------------|---------|-------------------------------------|
/// | `bgm`      | BGM     | Document type / message reference   |
/// | `dtm`      | DTM     | Date / time segments                |
/// | `sender`   | NAD+MS  | Message sender                      |
/// | `receiver` | NAD+MR  | Message receiver                    |
#[derive(Debug, Clone)]
pub struct UtiltsMessage {
    pub(crate) core: MessageCore,
    /// BGM — beginning of message.
    bgm: Option<Bgm>,
    /// DTM — date/time segments (includes Nachrichtendatum DTM+137 and any
    /// SG5 date/time segments that the linear scan picks up).
    dtm: Vec<Dtm>,
    /// NAD+MS — message sender.
    sender: Option<Nad>,
    /// NAD+MR — message receiver.
    receiver: Option<Nad>,
}

impl UtiltsMessage {
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
                MessageType::Utilts,
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

impl_edi_energy_message!(UtiltsMessage);
