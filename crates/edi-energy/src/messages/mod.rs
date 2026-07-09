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

// ── Shared semantic validation helpers ────────────────────────────────────────

/// Helpers shared across multiple message-type semantic rule packs.
///
/// Gated on the union of all consuming features so they compile away in minimal
/// feature builds.
#[cfg(any(
    feature = "mscons",
    feature = "utilmd",
    feature = "orders",
    feature = "insrpt",
    feature = "invoic",
    feature = "remadv",
))]
pub(super) mod common {
    /// Returns `true` when `id` is exactly 11 ASCII upper-case letters or digits.
    ///
    /// Used by MSCONS `SEM-MSCONS-MELO-FORMAT` and UTILMD `SEM-UTILMD-MALO-FORMAT`
    /// to validate Marktlokations-IDs and Messlokations-IDs.
    #[inline]
    pub(super) fn is_valid_location_id(id: &str) -> bool {
        id.len() == 11
            && id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }

    /// Emit a period-order error when DTM+163 (start) is lexicographically after
    /// DTM+164 (end) in `segments`.
    ///
    /// Shared by MSCONS, ORDERS (flat message-level rule) and INSRPT (group rule
    /// via `with_scoped_group_rule_fn`).  The `rule_id` parameter lets each
    /// caller stamp their own `SEM-<TYPE>-PERIOD-ORDER` identifier.
    #[cfg(any(
        feature = "insrpt",
        feature = "remadv",
        feature = "orders",
        feature = "invoic",
    ))]
    pub(super) fn check_period_order(
        segments: &[edifact_rs::Segment<'_>],
        rule_id: &'static str,
        issues: &mut Vec<edifact_rs::ValidationIssue>,
    ) {
        let mut start: Option<(&str, edifact_rs::Span)> = None;
        let mut end: Option<(&str, edifact_rs::Span)> = None;

        for seg in segments.iter().filter(|s| s.tag == "DTM") {
            let Some(c507) = seg.get_element(0) else {
                continue;
            };
            let qualifier = c507.get_component(0).unwrap_or("");
            let value = c507.get_component(1).unwrap_or("");
            match qualifier {
                "163" => start = Some((value, seg.span)),
                "164" => end = Some((value, seg.span)),
                _ => {}
            }
        }

        if let (Some((start_val, start_span)), Some((end_val, _))) = (start, end) {
            if !start_val.is_empty() && !end_val.is_empty() && start_val > end_val {
                issues.push(
                    edifact_rs::ValidationIssue::new(
                        edifact_rs::ValidationSeverity::Error,
                        "DTM: period-start (qualifier 163) is after period-end (qualifier 164)"
                            .to_owned(),
                    )
                    .with_span(start_span)
                    .with_rule_id(rule_id)
                    .with_segment("DTM")
                    .with_suggestion(
                        "Ensure DTM+163 (Beginn Lieferzeitraum) is not later than \
                         DTM+164 (Ende Lieferzeitraum) \u{2014} date values must be in \
                         ascending chronological order",
                    ),
                );
            }
        }
    }
}
