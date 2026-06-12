/// Concrete EDI@Energy message type implementations.
///
/// Each sub-module is feature-gated. Only the modules corresponding to enabled
/// Cargo features are compiled.
pub(crate) mod core;

// ── impl_edi_energy_message! ──────────────────────────────────────────────────

#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
/// Generate all [`crate::EdiEnergyMessage`] trait methods for a message type
/// that delegates to an internal `self.core: MessageCore` field.
///
/// # Variants
///
/// ```text
/// // No message-type-specific semantic rule pack (stub / simple types):
/// impl_edi_energy_message!(InvoicMessage);
///
/// // With a message-type-specific semantic rule pack:
/// impl_edi_energy_message!(UtilmdMessage, sem = utilmd_semantic_pack());
/// ```
///
/// The `sem = expr` expression is evaluated on **every validate call** —
/// keep it cheap (function-pointer construction, not data loading).
macro_rules! impl_edi_energy_message {
    // ── no semantic pack ───────────────────────────────────────────────────
    ($ty:ty) => {
        impl crate::EdiEnergyMessage for $ty {
            fn try_message_type(&self) -> Option<crate::MessageType> {
                Some(self.core.message_type())
            }
            fn detect_release(&self) -> Result<&crate::Release, crate::Error> {
                self.core.detect_release()
            }
            fn message_ref(&self) -> &str {
                &self.core.message_ref
            }
            fn detect_pruefidentifikator(&self) -> Result<crate::Pruefidentifikator, crate::Error> {
                self.core.detect_pruefidentifikator()
            }
            fn validate(&self) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core.validate_against_with_semantic(&release, None)
            }
            fn validate_against(
                &self,
                release: &crate::Release,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                self.core.validate_against_with_semantic(release, None)
            }
            fn validate_with_pack(
                &self,
                extra: crate::CustomRulePack,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                self.core.validate_with_extra_pack(None, extra.into_inner())
            }
            fn validate_on_date(
                &self,
                reference_date: time::Date,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core
                    .validate_against_with_semantic_and_registry_on_date(
                        &release,
                        None,
                        crate::registry::ReleaseRegistry::global(),
                        Some(reference_date),
                    )
            }
            fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
                self.core.serialize()
            }
            fn segments(&self) -> &[edifact_rs::OwnedSegment] {
                &self.core.segments
            }
        }
    };

    // ── with message-type-specific semantic rule pack ──────────────────────
    ($ty:ty, sem = $pack:expr) => {
        impl crate::EdiEnergyMessage for $ty {
            fn try_message_type(&self) -> Option<crate::MessageType> {
                Some(self.core.message_type())
            }
            fn detect_release(&self) -> Result<&crate::Release, crate::Error> {
                self.core.detect_release()
            }
            fn message_ref(&self) -> &str {
                &self.core.message_ref
            }
            fn detect_pruefidentifikator(&self) -> Result<crate::Pruefidentifikator, crate::Error> {
                self.core.detect_pruefidentifikator()
            }
            fn validate(&self) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core
                    .validate_against_with_semantic(&release, Some($pack))
            }
            fn validate_against(
                &self,
                release: &crate::Release,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                self.core
                    .validate_against_with_semantic(release, Some($pack))
            }
            fn validate_with_pack(
                &self,
                extra: crate::CustomRulePack,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                self.core
                    .validate_with_extra_pack(Some($pack), extra.into_inner())
            }
            fn validate_on_date(
                &self,
                reference_date: time::Date,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core
                    .validate_against_with_semantic_and_registry_on_date(
                        &release,
                        Some($pack),
                        crate::registry::ReleaseRegistry::global(),
                        Some(reference_date),
                    )
            }
            fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
                self.core.serialize()
            }
            fn segments(&self) -> &[edifact_rs::OwnedSegment] {
                &self.core.segments
            }
        }
    };
}

/// Typed EDIFACT segment structs shared across all message types.
///
/// This module is only compiled when at least one message-type feature is enabled.
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
pub mod segments;

/// APERAK — Application Error and Acknowledgement message.
#[cfg(feature = "aperak")]
pub mod aperak;
/// COMDIS — Commercial Dispute (Handelsunstimmigkeit) message.
#[cfg(feature = "comdis")]
pub mod comdis;
/// CONTRL — Interchange Control Structure (syntax acknowledgement) message.
#[cfg(feature = "contrl")]
pub mod contrl;
/// IFTSTA — International Multimodal Status Report message.
#[cfg(feature = "iftsta")]
pub mod iftsta;
/// INSRPT — Inspection Report message.
#[cfg(feature = "insrpt")]
pub mod insrpt;
/// INVOIC — Invoice message.
#[cfg(feature = "invoic")]
pub mod invoic;
/// MSCONS — Metered Services Consumption Report message.
#[cfg(feature = "mscons")]
pub mod mscons;
/// ORDCHG — Purchase Order Change message.
#[cfg(feature = "ordchg")]
pub mod ordchg;
/// ORDERS — Purchase Order message.
#[cfg(feature = "orders")]
pub mod orders;
/// ORDRSP — Purchase Order Response message.
#[cfg(feature = "ordrsp")]
pub mod ordrsp;
/// PARTIN — Party Information message.
#[cfg(feature = "partin")]
pub mod partin;
/// PRICAT — Price/Sales Catalogue (Preisliste) message.
#[cfg(feature = "pricat")]
pub mod pricat;
/// QUOTES — Quotation message.
#[cfg(feature = "quotes")]
pub mod quotes;
/// REMADV — Remittance Advice message.
#[cfg(feature = "remadv")]
pub mod remadv;
/// REQOTE — Request for Quotation message.
#[cfg(feature = "reqote")]
pub mod reqote;
/// UTILMD — Utilities Master Data message.
#[cfg(feature = "utilmd")]
pub mod utilmd;
/// UTILTS — Übertragung technischer Stammdaten (Technical Master Data) message.
#[cfg(feature = "utilts")]
pub mod utilts;
