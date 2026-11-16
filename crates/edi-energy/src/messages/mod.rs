/// Concrete EDI@Energy message type implementations.
///
/// Each sub-module is feature-gated. Only the modules corresponding to enabled
/// Cargo features are compiled.
pub(crate) mod core;

// ── impl_edi_energy_message! ──────────────────────────────────────────────────

#[cfg(any_message)]
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
#[cfg(any(
    feature = "aperak",
    feature = "comdis",
    feature = "iftsta",
    feature = "insrpt",
    feature = "invoic",
    feature = "mscons",
    feature = "ordchg",
    feature = "orders",
    feature = "ordrsp",
    feature = "partin",
    feature = "pricat",
    feature = "quotes",
    feature = "remadv",
    feature = "reqote",
    feature = "utilmd",
    feature = "utilts",
))]
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
            fn decimal_mark(&self) -> char {
                self.core.decimal_mark
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
            fn validate_on_date_with_rollen(
                &self,
                reference_date: time::Date,
                rollen: &crate::profile::Parteirollen,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core
                    .validate_against_with_semantic_and_registry_on_date(
                        &release,
                        None,
                        crate::registry::ReleaseRegistry::global(),
                        Some(reference_date),
                        rollen,
                    )
            }
            fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
                self.core.serialize()
            }
            fn segments(&self) -> &[edifact::OwnedSegment] {
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
            fn decimal_mark(&self) -> char {
                self.core.decimal_mark
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
            fn validate_on_date_with_rollen(
                &self,
                reference_date: time::Date,
                rollen: &crate::profile::Parteirollen,
            ) -> Result<crate::EdiEnergyReport, crate::Error> {
                let release = self.core.detect_release()?;
                self.core
                    .validate_against_with_semantic_and_registry_on_date(
                        &release,
                        Some($pack),
                        crate::registry::ReleaseRegistry::global(),
                        Some(reference_date),
                        rollen,
                    )
            }
            fn serialize(&self) -> Result<Vec<u8>, crate::Error> {
                self.core.serialize()
            }
            fn segments(&self) -> &[edifact::OwnedSegment] {
                &self.core.segments
            }
        }
    };
}

/// Typed EDIFACT segment structs shared across all message types.
///
/// This module is only compiled when at least one message-type feature is enabled.
#[cfg(any_message)]
pub mod segments;

#[cfg(any_message)]
pub mod layouts;

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
    /// Returns `true` when `id` has the BDEW **Marktlokations-ID** shape:
    /// exactly 11 ASCII upper-case letters or digits.
    ///
    /// A real MaLo-ID is 11 *digits* carrying a BDEW check digit in the 11th
    /// position. This check is deliberately looser — it is a wire-format screen,
    /// not a checksum test, so that a transposed digit still parses and can be
    /// rejected by the domain layer with a precise error.
    #[inline]
    #[cfg(any(feature = "mscons", feature = "utilmd",))]
    pub(super) fn is_valid_malo_id(id: &str) -> bool {
        id.len() == 11
            && id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    }

    /// Returns `true` when `id` has the BDEW **Messlokations-ID** shape:
    /// 33 characters, opening with a two-letter ISO 3166-1 country code and
    /// continuing with ASCII alphanumerics.
    #[inline]
    #[cfg(any(feature = "mscons", feature = "utilmd",))]
    pub(super) fn is_valid_melo_id(id: &str) -> bool {
        id.len() == 33
            && id.bytes().take(2).all(|b| b.is_ascii_uppercase())
            && id.bytes().skip(2).all(|b| b.is_ascii_alphanumeric())
    }

    /// Returns `true` when `id` is acceptable in a **Meldepunkt** field
    /// (`LOC+172`, UTILMD `IDE` C206).
    ///
    /// Per the MSCONS AHB (SG6 LOC — "ID der Messlokation oder ID der
    /// Marktlokation oder ID des Netzkopplungspunktes") such a field is
    /// polymorphic: the qualifier fixes the *role* of the point, not the ID
    /// scheme, so both the 11-character `MaLo` and the 33-character `MeLo` form
    /// are legal and must be accepted.
    #[inline]
    #[cfg(any(feature = "mscons", feature = "utilmd",))]
    pub(super) fn is_valid_location_id(id: &str) -> bool {
        is_valid_malo_id(id) || is_valid_melo_id(id)
    }

    /// Emit a period-order error for every `DTM+163` (start) that is not before
    /// the `DTM+164` (end) it pairs with.
    ///
    /// The values are decoded by DE 2379 through `edifact::DtmValue`, so a
    /// `303` start at `+02` is compared as the instant it names, not as a
    /// string. A start pairs with the next end that follows it, before any
    /// further start: in every EDI@Energy MIG a period's two `DTM` sit together
    /// in one segment group, so an MSCONS carrying one period per `SG10` is
    /// checked period by period rather than its last start against its last
    /// end. A value its code does not decode is `SEM-DTM-VALUE`'s finding,
    /// not this rule's.
    ///
    /// Shared by MSCONS, ORDERS, REMADV, INVOIC (message-level rules) and INSRPT
    /// (group rule via `with_scoped_group_rule_fn`). The `rule_id` parameter
    /// lets each caller stamp their own `SEM-<TYPE>-PERIOD-ORDER` identifier.
    #[cfg(any(
        feature = "insrpt",
        feature = "remadv",
        feature = "orders",
        feature = "invoic",
        feature = "mscons",
    ))]
    pub(super) fn check_period_order(
        segments: &[edifact::Segment<'_>],
        rule_id: &'static str,
        issues: &mut Vec<edifact::ValidationIssue>,
    ) {
        let decode = |seg: &edifact::Segment<'_>| {
            edifact::DtmValue::parse(seg.component_str(0, 1)?, seg.component_str(0, 2)?).ok()
        };
        let mut start: Option<(edifact::DtmValue, edifact::Span)> = None;
        for seg in segments.iter().filter(|s| s.tag == "DTM") {
            match seg.component_str(0, 0) {
                Some("163") => start = decode(seg).map(|v| (v, seg.span)),
                Some("164") => {
                    let (Some((begin, span)), Some(end)) = (start.take(), decode(seg)) else {
                        continue;
                    };
                    let reason = match begin.compare(&end) {
                        edifact::DtmRelation::Before
                        | edifact::DtmRelation::Equal
                        | edifact::DtmRelation::Overlapping => continue,
                        edifact::DtmRelation::Indeterminate => {
                            "cannot be ordered against period-end (qualifier 164)"
                        }
                        _ => "is after period-end (qualifier 164)",
                    };
                    issues.push(
                        edifact::ValidationIssue::new(
                            edifact::ValidationSeverity::Error,
                            format!("DTM: period-start (qualifier 163) {reason}"),
                        )
                        .with_span(span)
                        .with_rule_id(rule_id)
                        .with_segment("DTM")
                        .with_suggestion(
                            "Ensure DTM+163 (Beginn) is not later than DTM+164 (Ende) \
                             \u{2014} compared as the instants their DE 2379 formats name",
                        ),
                    );
                }
                _ => {}
            }
        }
    }
}
