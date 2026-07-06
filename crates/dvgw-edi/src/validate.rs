//! Per-message-type semantic validation for DVGW EDIFACT formats.
//!
//! Each `*_pack()` function builds a [`edifact_rs::ProfileRulePack`] for one
//! DVGW message type.  The packs are handed to a
//! [`edifact_rs::ValidationContext`] and run via
//! [`validate_lenient_owned`](edifact_rs::ValidationContext::validate_lenient_owned)
//! — the same machinery used by the `edi-energy` crate's five-layer pipeline,
//! just without the MIG/AHB directory layers that DVGW formats do not yet have
//! compiled-in profiles for.
//!
//! ## Rules
//!
//! | Rule ID | Message types | Severity | Description |
//! |---|---|---|---|
//! | `SEM-DVGW-BGM-REQUIRED` | All | Error | BGM must be present |
//! | `SEM-DVGW-NAD-MS-REQUIRED` | All | Error | NAD+MS (sender) must be present |
//! | `SEM-DVGW-NAD-MR-REQUIRED` | All | Error | NAD+MR (receiver) must be present |
//! | `SEM-ALOCAT-DTM-137-REQUIRED` | ALOCAT | Error | Gas day DTM+137 must be present |
//! | `SEM-ALOCAT-LOC-EXPECTED` | ALOCAT | Warning | At least one LOC expected |
//! | `SEM-NOMINT-DTM-137-REQUIRED` | NOMINT | Error | Gas day DTM+137 must be present |
//! | `SEM-NOMRES-DTM-137-REQUIRED` | NOMRES | Error | Gas day DTM+137 must be present |
//! | `SEM-NOMRES-RFF-Z13-EXPECTED` | NOMRES | Warning | NOMINT correlation RFF+Z13 expected |
//! | `SEM-SCHEDL-DTM-137-REQUIRED` | SCHEDL | Error | Gas day/transport day DTM+137 must be present |
//! | `SEM-IMBNOT-DTM-137-REQUIRED` | IMBNOT | Error | Imbalance gas day DTM+137 must be present |
//! | `SEM-TRANOT-DTM-137-REQUIRED` | TRANOT | Error | Gas transport notification date DTM+137 must be present |
//! | `SEM-DELORD-DTM-137-REQUIRED` | DELORD | Error | Gas delivery order date DTM+137 must be present |
//! | `SEM-DELRES-DTM-137-REQUIRED` | DELRES | Error | Gas delivery response date DTM+137 must be present |

use edifact_rs::{ProfileRulePack, Segment, ValidationIssue, ValidationSeverity};

// ── Segment-level helpers (borrowed `Segment<'_>` — matches stateless-rule closure signature)

#[inline]
fn has_segment(segs: &[Segment<'_>], tag: &str) -> bool {
    segs.iter().any(|s| s.tag == tag)
}

/// Returns `true` when at least one NAD segment has the given role qualifier in
/// element 0 (party function code qualifier, DE 3035).
#[inline]
fn has_nad_role(segs: &[Segment<'_>], qualifier: &str) -> bool {
    segs.iter()
        .any(|s| s.tag == "NAD" && s.element_str(0) == Some(qualifier))
}

/// Returns `true` when at least one DTM segment has the given qualifier in
/// composite element 0 (C507), component 0 (DE 2005).
#[inline]
fn has_dtm_qualifier(segs: &[Segment<'_>], qualifier: &str) -> bool {
    segs.iter()
        .any(|s| s.tag == "DTM" && s.component_str(0, 0).is_some_and(|q| q == qualifier))
}

/// Returns `true` when at least one RFF segment has the given qualifier in
/// composite element 0 (C506), component 0 (DE 1153).
#[inline]
fn has_rff_qualifier(segs: &[Segment<'_>], qualifier: &str) -> bool {
    segs.iter()
        .any(|s| s.tag == "RFF" && s.component_str(0, 0).is_some_and(|q| q == qualifier))
}

// ── Shared rule closures ──────────────────────────────────────────────────────

/// Emits `SEM-DVGW-BGM-REQUIRED` when no BGM segment is present.
fn require_bgm(segs: &[Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !has_segment(segs, "BGM") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "BGM segment is missing — every DVGW message must carry a document \
                 reference in the Business Group Message segment",
            )
            .with_rule_id("SEM-DVGW-BGM-REQUIRED")
            .with_segment("BGM")
            .with_suggestion(
                "Add BGM+<function-code>+<document-number>' to identify the message; \
                 the document number (element 1 component 0, DE 1004) is used for \
                 correlation (NOMRES RFF+Z13) and duplicate detection",
            ),
        );
    }
}

/// Emits `SEM-DVGW-NAD-MS-REQUIRED` when no NAD+MS segment is present.
fn require_nad_ms(segs: &[Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !has_nad_role(segs, "MS") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "NAD+MS (Marktteilnehmer Absender / sending market participant) is missing",
            )
            .with_rule_id("SEM-DVGW-NAD-MS-REQUIRED")
            .with_segment("NAD")
            .with_suggestion(
                "Add NAD+MS+<EIC-code>::293' to identify the sending market participant; \
                 qualifier 293 (ENTSOE) is the standard code list for EIC codes in DVGW messages",
            ),
        );
    }
}

/// Emits `SEM-DVGW-NAD-MR-REQUIRED` when no NAD+MR segment is present.
fn require_nad_mr(segs: &[Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !has_nad_role(segs, "MR") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "NAD+MR (Marktteilnehmer Empfänger / receiving market participant) is missing",
            )
            .with_rule_id("SEM-DVGW-NAD-MR-REQUIRED")
            .with_segment("NAD")
            .with_suggestion(
                "Add NAD+MR+<EIC-code>::293' to identify the receiving market participant",
            ),
        );
    }
}

/// Emits `<rule_id>` when no `DTM+<qualifier>` segment is present.
///
/// DVGW messages carry gas-day / transport-day references as mandatory timing
/// qualifiers.  Missing timing makes the message unprocessable for the receiving
/// side's scheduling or billing system.
fn require_dtm(
    segs: &[Segment<'_>],
    qualifier: &'static str,
    rule_id: &'static str,
    description: &'static str,
    issues: &mut Vec<ValidationIssue>,
) {
    if !has_dtm_qualifier(segs, qualifier) {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("DTM+{qualifier} ({description}) is missing"),
            )
            .with_rule_id(rule_id)
            .with_segment("DTM")
            .with_suggestion(format!(
                "Add DTM+{qualifier}:<timestamp>:203' to specify the {description}; \
                 format 203 = CCYYMMDDHHmm (ISO 8601 combined date/time)",
            )),
        );
    }
}

// ── Per-message-type ProfileRulePack builders ─────────────────────────────────

/// Semantic rule pack for ALOCAT (Allokationsnachricht) messages.
///
/// Checks common DVGW mandatory fields (BGM, NAD+MS, NAD+MR) plus:
/// - `DTM+137` (Gasdatum / Abrechnungstag) must be present.
/// - At least one `LOC` segment (allocation quantity line item) is expected.
#[cfg(feature = "alocat")]
pub(crate) fn alocat_pack() -> ProfileRulePack {
    ProfileRulePack::new("ALOCAT-SEM")
        .for_message_type("ALOCAT")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-ALOCAT-DTM-137-REQUIRED",
                "Gasdatum / Abrechnungstag (gas accounting day)",
                issues,
            );
            if !has_segment(segs, "LOC") {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "ALOCAT contains no LOC segment — at least one allocation \
                         quantity line item (LOC + QTY) is expected per DVGW ALOCAT specification",
                    )
                    .with_rule_id("SEM-ALOCAT-LOC-EXPECTED")
                    .with_segment("LOC")
                    .with_suggestion(
                        "Add at least one LOC+<qualifier>+<location-code>::ZZZ' segment \
                         followed by a QTY segment for each allocation entry",
                    ),
                );
            }
        })
}

/// Semantic rule pack for NOMINT (Nominierungsintegration) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum).
#[cfg(feature = "nomint")]
pub(crate) fn nomint_pack() -> ProfileRulePack {
    ProfileRulePack::new("NOMINT-SEM")
        .for_message_type("NOMINT")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-NOMINT-DTM-137-REQUIRED",
                "Gasdatum (gas day for nomination)",
                issues,
            );
        })
}

/// Semantic rule pack for NOMRES (Nominierungsantwort) messages.
///
/// Checks common DVGW mandatory fields, `DTM+137` (Gasdatum), and emits a
/// warning when `RFF+Z13` (Nominierungsreferenz) is absent — this reference is
/// required for correlating the response back to the originating NOMINT workflow.
#[cfg(feature = "nomres")]
pub(crate) fn nomres_pack() -> ProfileRulePack {
    ProfileRulePack::new("NOMRES-SEM")
        .for_message_type("NOMRES")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-NOMRES-DTM-137-REQUIRED",
                "Gasdatum (gas day for nomination response)",
                issues,
            );
            if !has_rff_qualifier(segs, "Z13") {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "NOMRES has no RFF+Z13 (Nominierungsreferenz) — the document number \
                         of the corresponding NOMINT should be cited here for correlation",
                    )
                    .with_rule_id("SEM-NOMRES-RFF-Z13-EXPECTED")
                    .with_segment("RFF")
                    .with_suggestion(
                        "Add RFF+Z13:<nomination_ref>' where <nomination_ref> is the BGM \
                         element-1 document number from the originating NOMINT; this field \
                         is used by the ProcessRegistry to route the response to the correct \
                         outbound nomination workflow",
                    ),
                );
            }
        })
}

/// Semantic rule pack for SCHEDL (Schedulingnachricht) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum / Transporttag).
#[cfg(feature = "schedl")]
pub(crate) fn schedl_pack() -> ProfileRulePack {
    ProfileRulePack::new("SCHEDL-SEM")
        .for_message_type("SCHEDL")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-SCHEDL-DTM-137-REQUIRED",
                "Gasdatum / Transporttag (gas transport day)",
                issues,
            );
        })
}

/// Semantic rule pack for IMBNOT (Imbalance Notification) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum).
#[cfg(feature = "imbnot")]
pub(crate) fn imbnot_pack() -> ProfileRulePack {
    ProfileRulePack::new("IMBNOT-SEM")
        .for_message_type("IMBNOT")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-IMBNOT-DTM-137-REQUIRED",
                "Gasdatum (imbalance gas day)",
                issues,
            );
        })
}

/// Semantic rule pack for TRANOT (Transport Notification) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum / Transportdatum).
#[cfg(feature = "tranot")]
pub(crate) fn tranot_pack() -> ProfileRulePack {
    ProfileRulePack::new("TRANOT-SEM")
        .for_message_type("TRANOT")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-TRANOT-DTM-137-REQUIRED",
                "Gasdatum / Transportdatum (gas transport notification date)",
                issues,
            );
        })
}

/// Semantic rule pack for DELORD (Delivery Order) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum / Lieferdatum).
#[cfg(feature = "delord")]
pub(crate) fn delord_pack() -> ProfileRulePack {
    ProfileRulePack::new("DELORD-SEM")
        .for_message_type("DELORD")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-DELORD-DTM-137-REQUIRED",
                "Gasdatum / Lieferdatum (gas delivery order date)",
                issues,
            );
        })
}

/// Semantic rule pack for DELRES (Delivery Response) messages.
///
/// Checks common DVGW mandatory fields plus `DTM+137` (Gasdatum / Lieferdatum).
#[cfg(feature = "delres")]
pub(crate) fn delres_pack() -> ProfileRulePack {
    ProfileRulePack::new("DELRES-SEM")
        .for_message_type("DELRES")
        .with_stateless_rule_fn(|segs, issues| {
            require_bgm(segs, issues);
            require_nad_ms(segs, issues);
            require_nad_mr(segs, issues);
            require_dtm(
                segs,
                "137",
                "SEM-DELRES-DTM-137-REQUIRED",
                "Gasdatum / Lieferdatum (gas delivery response date)",
                issues,
            );
        })
}
