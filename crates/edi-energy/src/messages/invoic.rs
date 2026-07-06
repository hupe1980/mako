use edifact_rs::{OwnedSegment, ProfileRulePack, ValidationIssue, ValidationSeverity};

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

/// Semantic rule pack for INVOIC.
///
/// Checks that are universal across all BDEW INVOIC PIDs 31001–31011:
/// - `SEM-INVOIC-DTM-137-REQUIRED`: `DTM+137` (Rechnungsdatum) must be present.
/// - `SEM-INVOIC-PERIOD-ORDER`: When `DTM+163` (Beginn Abrechnungszeitraum) and
///   `DTM+164` (Ende Abrechnungszeitraum) are both present, start must not be
///   after end.
fn invoic_semantic_pack() -> ProfileRulePack {
    ProfileRulePack::new("INVOIC-SEM")
        .for_message_type("INVOIC")
        .with_stateless_rule_fn(
            |segs: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>| {
                // DTM+137 (Rechnungsdatum / invoice date) is mandatory in all BDEW
                // INVOIC PIDs 31001–31011: billing, self-billed, and WiM invoices.
                if !segs
                    .iter()
                    .any(|s| s.tag == "DTM" && s.component_str(0, 0) == Some("137"))
                {
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            "DTM+137 (Rechnungsdatum / invoice date) is missing — \
                             mandatory in all BDEW INVOIC PIDs 31001–31011",
                        )
                        .with_rule_id("SEM-INVOIC-DTM-137-REQUIRED")
                        .with_segment("DTM")
                        .with_suggestion(
                            "Add DTM+137:<YYYYMMDD>:102' to specify the invoice \
                             issue date (format 102 = CCYYMMDD per UN/EDIFACT)",
                        ),
                    );
                }
                super::common::check_period_order(segs, "SEM-INVOIC-PERIOD-ORDER", issues);
            },
        )
}
