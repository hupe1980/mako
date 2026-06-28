// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate
#![allow(clippy::doc_markdown)]

/// Codegen schema version this module was generated from.
/// Compared against `mig.json` `schema_version` in CI to detect drift.
#[allow(dead_code)]
pub(crate) const CODEGEN_SCHEMA_VERSION: u32 = 1;

use std::sync::{Arc, LazyLock};

use edifact_rs::directory_validator::{ElementRef, SegmentDefinition, Status};
use edifact_rs::{
    DirectoryValidator, GroupDef, ProfileRulePack, ValidationIssue, ValidationSeverity,
};

use crate::registry::Profile;
use crate::{MessageType, Pruefidentifikator, Release};

static SEGMENTS: &[SegmentDefinition] = &[
    SegmentDefinition {
        tag: "UNH",
        name: "Nachrichten-Kopfsegment",
        elements: &[
            ElementRef::new(1, "0062", Status::Mandatory, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "BGM",
        name: "Beginn der Nachricht",
        elements: &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Nachrichtendatum",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Abonnement / Produkt-/Leistungsbeschreibung",
        elements: &[
            ElementRef::new(1, "7077", Status::Conditional, 1),
            ElementRef::new(2, "C273", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "UNS",
        name: "Abschnitts-Kontrollsegment",
        elements: &[ElementRef::new(1, "0081", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "MOA",
        name: "Monetary Amount (Summe)",
        elements: &[ElementRef::new(1, "C516", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "UNT",
        name: "Nachrichten-Endesegment",
        elements: &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Referenz",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "AJT",
        name: "Antwortkategorie",
        elements: &[ElementRef::new(1, "4465", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Allgemeine Information",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C108", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "NAD",
        name: "MP-ID Absender/Empfänger",
        elements: &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CTA",
        name: "Ansprechpartner",
        elements: &[
            ElementRef::new(1, "3139", Status::Conditional, 1),
            ElementRef::new(2, "C056", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "COM",
        name: "Kommunikationsverbindung",
        elements: &[ElementRef::new(1, "C076", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "CUX",
        name: "Währung",
        elements: &[ElementRef::new(1, "C504", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["7"];
static CODES_1153: &[&str] = &["ACW", "ON", "TN", "Z13"];
static CODES_2005: &[&str] = &["137", "163", "164", "203", "469", "472"];
static CODES_3035: &[&str] = &["MR", "MS", "VY", "Z22"];
static CODES_4451: &[&str] = &["AAP", "ABO", "Z27", "Z28"];
static CODES_6343: &[&str] = &["11"];

pub(crate) fn is_code_valid(de_id: &str, code: &str) -> bool {
    code_list(de_id).is_none_or(|codes| codes.binary_search(&code).is_ok())
}

pub(crate) fn suggest_code(de_id: &str, code: &str) -> Option<&'static str> {
    let codes = code_list(de_id)?;
    // Return the lexicographically nearest valid code.
    // partition_point gives the insertion point for `code` in the sorted slice,
    // so codes[idx] is the first valid code >= code (or last if past end).
    let idx = codes.partition_point(|&c| c < code);
    codes.get(idx).or_else(|| codes.last()).copied()
}

fn expected_components(tag: &str, idx: usize) -> Option<u8> {
    match (tag, idx) {
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "4451" => Some(CODES_4451),
        "6343" => Some(CODES_6343),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_ORDRSP_1_4B: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-ORDRSP-1.4b",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_ORDRSP_1_4B
}

fn rule_unh_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "UNH") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment UNH is missing".to_owned(),
            )
            .with_rule_id("MIG-UNH-REQ")
            .with_segment("UNH".to_owned()),
        );
    }
}

fn rule_bgm_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "BGM") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment BGM is missing".to_owned(),
            )
            .with_rule_id("MIG-BGM-REQ")
            .with_segment("BGM".to_owned()),
        );
    }
}

fn rule_dtm_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "DTM") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment DTM is missing".to_owned(),
            )
            .with_rule_id("MIG-DTM-REQ")
            .with_segment("DTM".to_owned()),
        );
    }
}

fn rule_uns_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "UNS") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment UNS is missing".to_owned(),
            )
            .with_rule_id("MIG-UNS-REQ")
            .with_segment("UNS".to_owned()),
        );
    }
}

fn rule_unt_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "UNT") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment UNT is missing".to_owned(),
            )
            .with_rule_id("MIG-UNT-REQ")
            .with_segment("UNT".to_owned()),
        );
    }
}

fn rule_rff_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "RFF") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment RFF is missing".to_owned(),
            )
            .with_rule_id("MIG-RFF-REQ")
            .with_segment("RFF".to_owned()),
        );
    }
}

fn rule_nad_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "NAD") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment NAD is missing".to_owned(),
            )
            .with_rule_id("MIG-NAD-REQ")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify `DTM` appears at most 6 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_dtm_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DTM").count();
    if count > 6 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment DTM occurs {count} times; maximum is 6"),
            )
            .with_rule_id("MIG-DTM-CARD-MAX")
            .with_segment("DTM".to_owned()),
        );
    }
}

/// Layer 3 — verify `IMD` appears at most 2 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_imd_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "IMD").count();
    if count > 2 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment IMD occurs {count} times; maximum is 2"),
            )
            .with_rule_id("MIG-IMD-CARD-MAX")
            .with_segment("IMD".to_owned()),
        );
    }
}

/// Layer 3 — verify `MOA` appears at most 5 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_moa_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "MOA").count();
    if count > 5 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment MOA occurs {count} times; maximum is 5"),
            )
            .with_rule_id("MIG-MOA-CARD-MAX")
            .with_segment("MOA".to_owned()),
        );
    }
}

/// Layer 3 — verify the `RFF` segment group appears at most 9999 times.
///
/// Each occurrence of the trigger segment `RFF` marks the start of
/// one group instance.  The MIG specifies a maximum of 9999 instances.
fn rule_group_sg1_rff_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "RFF").count();
    if count > 9_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by RFF occurs {count} times; maximum is 9_999"),
            )
            .with_rule_id("MIG-ORDRSP-MIG-1.4b-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg3_nad_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "NAD").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-ORDRSP-MIG-1.4b-GROUP-SG3-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `LIN` segment group appears at most 200000 times.
///
/// Each occurrence of the trigger segment `LIN` marks the start of
/// one group instance.  The MIG specifies a maximum of 200000 instances.
fn rule_group_sg27_lin_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "LIN").count();
    if count > 200_000 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by LIN occurs {count} times; maximum is 200_000"),
            )
            .with_rule_id("MIG-ORDRSP-MIG-1.4b-GROUP-SG27-LIN-CARD-MAX")
            .with_segment("LIN".to_owned()),
        );
    }
}

/// Layer 3 — verify the `RFF` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg1_rff_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "RFF").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by RFF occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-ORDRSP-MIG-1.4b-GROUP-SG1-RFF-CARD-MIN")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg3_nad_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "NAD").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-ORDRSP-MIG-1.4b-GROUP-SG3-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    /// Header segment ordering (before UNS+D).
    const EXPECTED_HEADER_ORDER: &[&str] = &[
        "UNH", "BGM", "DTM", "IMD", "RFF", "AJT", "FTX", "NAD", "CTA", "COM", "LIN", "CUX",
    ];
    /// Detail segment ordering (after UNS+D).
    const EXPECTED_DETAIL_ORDER: &[&str] = &["MOA", "UNT"];

    /// Strict order check for the header section (no group repetition expected).
    fn check_header_section(
        segs: &[edifact_rs::Segment<'_>],
        expected: &[&str],
        rule_id: &str,
        issues: &mut Vec<ValidationIssue>,
    ) {
        let mut cursor: usize = 0;
        for seg in segs {
            if let Some(pos) = expected[cursor..].iter().position(|&t| t == seg.tag) {
                cursor += pos;
            } else if expected.contains(&seg.tag) {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "segment appears out of order".to_owned(),
                    )
                    .with_rule_id(rule_id)
                    .with_segment(seg.tag.to_owned()),
                );
            }
            // Unknown tags are passed through — they get caught by the DirectoryValidator.
        }
    }

    /// Group-trigger-aware order check for the detail section (post-UNS).
    ///
    /// When the first tag in `expected` is seen again after the cursor has
    /// already advanced, this indicates a new group-repetition occurrence
    /// (e.g. a second `LOC` group in MSCONS).  The cursor is silently reset
    /// to that position instead of reporting an ordering violation.
    fn check_detail_section(
        segs: &[edifact_rs::Segment<'_>],
        expected: &[&str],
        rule_id: &str,
        issues: &mut Vec<ValidationIssue>,
    ) {
        let group_trigger = expected.first().copied().unwrap_or("");
        let mut cursor: usize = 0;
        for seg in segs {
            // A repeated group-trigger tag resets the cursor to allow multiple group occurrences.
            if cursor > 0 && seg.tag == group_trigger {
                cursor = 0;
            }
            if let Some(pos) = expected[cursor..].iter().position(|&t| t == seg.tag) {
                cursor += pos;
            } else if expected.contains(&seg.tag) {
                issues.push(
                    ValidationIssue::new(
                        ValidationSeverity::Error,
                        "segment appears out of order".to_owned(),
                    )
                    .with_rule_id(rule_id)
                    .with_segment(seg.tag.to_owned()),
                );
            }
            // Unknown tags are passed through — they get caught by the DirectoryValidator.
        }
    }

    let uns_pos = segments.iter().position(|s| s.tag == "UNS");
    let (header_segs, detail_segs) = match uns_pos {
        Some(pos) => (&segments[..pos], &segments[pos + 1..]),
        None => (segments, &[][..]),
    };
    check_header_section(
        header_segs,
        EXPECTED_HEADER_ORDER,
        "MIG-ORDRSP-MIG-1.4b-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-ORDRSP-MIG-1.4b-ORDER",
        issues,
    );
}

static MIG_ORDRSP_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-MIG-1.4b")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_dtm_max_occurrences)
            .with_stateless_rule_fn(rule_imd_max_occurrences)
            .with_stateless_rule_fn(rule_moa_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg27_lin_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_ORDRSP_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="Z12" is present // [83] Wenn BGM+Z12 (Änderung der Technik der Lokation) vorhanden, ist DTM+203 (Ausführungsdatum) Pflicht
fn rule_ahb_19005_dtm_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z12"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "203"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"203\") is missing for Pruefidentifikator 19005 (I: when BGM DE[0]=\"Z12\" is present)".to_owned(),
                )
                .with_rule_id("AHB-19005-DTM-I0")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "19005".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z93" is present // [82] Wenn BGM+Z93 (Bestellung eines Angebots Änderung der Technik) vorhanden, ist DTM+469 (Startdatum) Pflicht
fn rule_ahb_19005_dtm_cond_1(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z93"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "469"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"469\") is missing for Pruefidentifikator 19005 (I: when BGM DE[0]=\"Z93\" is present)".to_owned(),
                )
                .with_rule_id("AHB-19005-DTM-I1")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "19005".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z12" is present // [83] Wenn BGM+Z12 (Änderung der Technik der Lokation) vorhanden, ist DTM+203 (Ausführungsdatum) Pflicht
fn rule_ahb_19006_dtm_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z12"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "203"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"203\") is missing for Pruefidentifikator 19006 (I: when BGM DE[0]=\"Z12\" is present)".to_owned(),
                )
                .with_rule_id("AHB-19006-DTM-I0")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "19006".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z93" is present // [82] Wenn BGM+Z93 (Bestellung eines Angebots Änderung der Technik) vorhanden, ist DTM+469 (Startdatum) Pflicht
fn rule_ahb_19006_dtm_cond_1(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z93"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "469"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"469\") is missing for Pruefidentifikator 19006 (I: when BGM DE[0]=\"Z93\" is present)".to_owned(),
                )
                .with_rule_id("AHB-19006-DTM-I1")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "19006".to_owned()));
    }
}

static AHB_19001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19001")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19001-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19001-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19001-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-19001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19001",
                    "19001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19001_PACK)
}

static AHB_19002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19002")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19002-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19002-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19002",
                    "19002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19002_PACK)
}

static AHB_19003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19003")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19003-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19003-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19003",
                    "19003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19003",
                    "19003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19003",
                    "19003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19003",
                    "19003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19003",
                    "19003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19003_PACK)
}

static AHB_19004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19004")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19004-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19004-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19004",
                    "19004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19004",
                    "19004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19004",
                    "19004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19004",
                    "19004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19004",
                    "19004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19004_PACK)
}

static AHB_19005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19005")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19005-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19005-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19005-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19005-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_19005_dtm_cond_0)
            .with_stateless_rule_fn(rule_ahb_19005_dtm_cond_1)
            .with_named_stateless_rule_fn("AHB-19005-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19005-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19005",
                    "19005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19005_PACK)
}

static AHB_19006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19006")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19006-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19006-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19006-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19006-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19006-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19006-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19006-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19006-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19006-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19006-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_19006_dtm_cond_0)
            .with_stateless_rule_fn(rule_ahb_19006_dtm_cond_1)
            .with_named_stateless_rule_fn("AHB-19006-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19006-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19006-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19006-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19006",
                    "19006",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19006_PACK)
}

static AHB_19007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19007")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19007-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19007-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19007-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19007-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19007-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19007-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19007-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19007-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19007-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19007-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19007-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19007-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19007",
                    "19007",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19007_PACK)
}

static AHB_19009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19009")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19009-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19009-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19009-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19009-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19009",
                    "19009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19009_PACK)
}

static AHB_19010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19010")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19010-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19010-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19010-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19010-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19010-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19010-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19010-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19010-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19010-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19010-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19010-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19010-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19010",
                    "19010",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19010_PACK)
}

static AHB_19011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19011")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19011-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19011-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19011-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19011-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19011-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19011-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-19011-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19011-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19011-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19011-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19011",
                    "19011",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19011_PACK)
}

static AHB_19012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19012")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19012-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19012-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19012-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19012-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19012-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19012-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19012-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19012-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19012-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19012-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19012-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19012-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19012",
                    "19012",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19012_PACK)
}

static AHB_19013_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19013")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19013-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19013-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19013",
                    "19013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19013-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19013-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19013",
                    "19013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19013-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19013-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19013",
                    "19013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19013-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19013-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19013",
                    "19013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19013-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19013-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19013",
                    "19013",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19013_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19013_PACK)
}

static AHB_19014_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19014")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19014-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19014-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19014",
                    "19014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19014-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19014-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19014",
                    "19014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19014-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19014-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19014",
                    "19014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19014-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19014-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19014",
                    "19014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19014-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19014-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19014",
                    "19014",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19014_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19014_PACK)
}

static AHB_19015_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19015")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19015-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19015-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19015",
                    "19015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19015-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19015-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19015",
                    "19015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19015-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19015-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19015",
                    "19015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19015-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19015-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19015",
                    "19015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19015-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19015-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19015",
                    "19015",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19015_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19015_PACK)
}

static AHB_19016_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19016")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19016-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19016-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19016",
                    "19016",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19016-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19016-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19016",
                    "19016",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19016-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19016-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19016",
                    "19016",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19016-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19016-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19016",
                    "19016",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19016-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19016-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19016",
                    "19016",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19016_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19016_PACK)
}

static AHB_19101_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19101")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19101-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19101-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19101",
                    "19101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19101-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19101-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19101",
                    "19101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19101-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19101-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19101",
                    "19101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19101-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19101-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19101",
                    "19101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19101-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19101-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19101",
                    "19101",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19101_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19101_PACK)
}

static AHB_19102_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19102")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19102-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19102-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19102-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19102-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19102-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19102-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19102-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19102-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19102-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19102-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19102-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19102-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19102",
                    "19102",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19102_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19102_PACK)
}

static AHB_19103_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19103")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19103-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19103-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19103-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19103-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19103-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19103-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19103-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19103-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19103-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19103-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19103-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19103-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19103",
                    "19103",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19103_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19103_PACK)
}

static AHB_19104_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19104")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19104-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19104-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19104-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19104-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19104-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19104-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19104-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19104-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19104-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19104",
                    "19104",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19104_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19104_PACK)
}

static AHB_19110_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19110")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19110-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19110-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19110-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19110-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19110-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19110-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19110-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19110-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19110-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19110-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19110-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19110-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19110",
                    "19110",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19110_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19110_PACK)
}

static AHB_19114_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19114")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19114-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19114-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19114-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19114-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19114-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19114-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19114-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19114-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19114-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19114-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19114",
                    "19114",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19114_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19114_PACK)
}

static AHB_19115_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19115")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19115-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19115-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19115-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19115-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19115-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19115-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19115-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19115-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19115-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19115",
                    "19115",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19115_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19115_PACK)
}

static AHB_19116_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19116")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19116-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19116-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19116-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-19116-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19116-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19116-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-19116-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-19116-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19116-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19116-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19116-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19116",
                    "19116",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19116_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19116_PACK)
}

static AHB_19117_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19117")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19117-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19117-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19117",
                    "19117",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19117-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19117-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19117",
                    "19117",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19117-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19117-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19117",
                    "19117",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19117-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19117-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19117",
                    "19117",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19117-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19117-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19117",
                    "19117",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19117_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19117_PACK)
}

static AHB_19118_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19118")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19118-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19118-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19118",
                    "19118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19118-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19118-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19118",
                    "19118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19118-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19118-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19118",
                    "19118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19118-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19118-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19118",
                    "19118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19118-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19118-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19118",
                    "19118",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19118_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19118_PACK)
}

static AHB_19119_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19119")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19119-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19119-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19119",
                    "19119",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19119-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19119-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19119",
                    "19119",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19119-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19119-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19119",
                    "19119",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19119-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19119-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19119",
                    "19119",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19119-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19119-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19119",
                    "19119",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19119_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19119_PACK)
}

static AHB_19120_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19120")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19120-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19120-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19120-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19120-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19120-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19120-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19120-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19120-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19120-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19120-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19120-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19120-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19120",
                    "19120",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19120_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19120_PACK)
}

static AHB_19121_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19121")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19121-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19121-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19121-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19121-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19121-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19121-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19121-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19121-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19121-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19121-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19121-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19121-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19121",
                    "19121",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19121_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19121_PACK)
}

static AHB_19123_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19123")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19123-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19123-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19123-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19123-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19123-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19123-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19123-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19123-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19123-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19123-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19123-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19123-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19123",
                    "19123",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19123_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19123_PACK)
}

static AHB_19124_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19124")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19124-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19124-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19124-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19124-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19124-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19124-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19124-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19124-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19124-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19124-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19124-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19124-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19124",
                    "19124",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19124_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19124_PACK)
}

static AHB_19127_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19127")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19127-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19127-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19127-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19127-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19127-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19127-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19127-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19127-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19127-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19127-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19127-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19127-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19127",
                    "19127",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19127_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19127_PACK)
}

static AHB_19128_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19128")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19128-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19128-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19128",
                    "19128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19128-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19128-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19128",
                    "19128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19128-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19128-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19128",
                    "19128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19128-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19128-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19128",
                    "19128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19128-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19128-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19128",
                    "19128",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19128_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19128_PACK)
}

static AHB_19129_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19129")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19129-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19129-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19129",
                    "19129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19129-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19129-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19129",
                    "19129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19129-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19129-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19129",
                    "19129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19129-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19129-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19129",
                    "19129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19129-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19129-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19129",
                    "19129",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19129_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19129_PACK)
}

static AHB_19130_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19130")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19130-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19130-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19130-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19130-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19130-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19130-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19130-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19130-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19130-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19130-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19130-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19130-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19130",
                    "19130",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19130_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19130_PACK)
}

static AHB_19131_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19131")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19131-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19131-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19131-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19131-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19131-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19131-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19131-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19131-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19131-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19131-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19131-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19131-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19131",
                    "19131",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19131_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19131_PACK)
}

static AHB_19132_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19132")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19132-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19132-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19132-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19132-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19132-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19132-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19132-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19132-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19132-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19132-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19132-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19132-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19132",
                    "19132",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19132_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19132_PACK)
}

static AHB_19133_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19133")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19133-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19133-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19133-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19133-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19133-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19133-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19133-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-19133-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19133-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19133-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19133-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19133-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19133",
                    "19133",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19133_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19133_PACK)
}

static AHB_19204_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19204")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19204-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19204-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19204",
                    "19204",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19204-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19204-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19204",
                    "19204",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19204-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19204-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19204",
                    "19204",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19204-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19204-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19204",
                    "19204",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19204-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19204-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19204",
                    "19204",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19204_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19204_PACK)
}

static AHB_19301_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19301")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19301-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19301-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19301-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19301-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19301-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19301-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19301-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19301-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19301-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19301-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19301",
                    "19301",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19301_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19301_PACK)
}

static AHB_19302_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDRSP-AHB-1.4b-19302")
            .for_message_type("ORDRSP")
            .for_release("1.4b")
            .with_named_stateless_rule_fn("AHB-19302-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-19302-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-19302-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-19302-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-19302-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-19302-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-19302-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-19302-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-19302-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-19302-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 19302",
                    "19302",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_19302_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_19302_PACK)
}

static AHB_ALL_PACK_ORDRSP_1_4B: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("ORDRSP-AHB-1.4b-ALL")
        .for_message_type("ORDRSP")
        .for_release("1.4b");
    let pack = pack
        .merge_with_override(ahb_19001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19013_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19014_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19015_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19016_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19101_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19102_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19103_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19104_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19110_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19114_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19115_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19116_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19117_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19118_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19119_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19120_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19121_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19123_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19124_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19127_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19128_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19129_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19130_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19131_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19132_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19133_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19204_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19301_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_19302_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(19001) => ahb_19001_pack(),
            Some(19002) => ahb_19002_pack(),
            Some(19003) => ahb_19003_pack(),
            Some(19004) => ahb_19004_pack(),
            Some(19005) => ahb_19005_pack(),
            Some(19006) => ahb_19006_pack(),
            Some(19007) => ahb_19007_pack(),
            Some(19009) => ahb_19009_pack(),
            Some(19010) => ahb_19010_pack(),
            Some(19011) => ahb_19011_pack(),
            Some(19012) => ahb_19012_pack(),
            Some(19013) => ahb_19013_pack(),
            Some(19014) => ahb_19014_pack(),
            Some(19015) => ahb_19015_pack(),
            Some(19016) => ahb_19016_pack(),
            Some(19101) => ahb_19101_pack(),
            Some(19102) => ahb_19102_pack(),
            Some(19103) => ahb_19103_pack(),
            Some(19104) => ahb_19104_pack(),
            Some(19110) => ahb_19110_pack(),
            Some(19114) => ahb_19114_pack(),
            Some(19115) => ahb_19115_pack(),
            Some(19116) => ahb_19116_pack(),
            Some(19117) => ahb_19117_pack(),
            Some(19118) => ahb_19118_pack(),
            Some(19119) => ahb_19119_pack(),
            Some(19120) => ahb_19120_pack(),
            Some(19121) => ahb_19121_pack(),
            Some(19123) => ahb_19123_pack(),
            Some(19124) => ahb_19124_pack(),
            Some(19127) => ahb_19127_pack(),
            Some(19128) => ahb_19128_pack(),
            Some(19129) => ahb_19129_pack(),
            Some(19130) => ahb_19130_pack(),
            Some(19131) => ahb_19131_pack(),
            Some(19132) => ahb_19132_pack(),
            Some(19133) => ahb_19133_pack(),
            Some(19204) => ahb_19204_pack(),
            Some(19301) => ahb_19301_pack(),
            Some(19302) => ahb_19302_pack(),
            None => Arc::clone(&AHB_ALL_PACK_ORDRSP_1_4B),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("ORDRSP")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_ORDRSP_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("1.4b"));

pub(crate) struct OrdrspFv20251001Profile;

impl Profile for OrdrspFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Ordrsp
    }
    fn release(&self) -> &Release {
        &RELEASE_ORDRSP_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 03 - 31))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.4b")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("ORDRSP AHB 1.4b, Stand 01.10.2025")
    }
    fn mig_rule_pack(&self) -> Arc<ProfileRulePack> {
        mig_rule_pack()
    }
    fn ahb_rule_pack(&self, pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
        ahb_rule_pack(pid)
    }
    fn is_code_valid(&self, de_id: &str, code: &str) -> bool {
        is_code_valid(de_id, code)
    }
    fn suggest_code(&self, de_id: &str, code: &str) -> Option<&'static str> {
        suggest_code(de_id, code)
    }
    fn segment_lookup(&self, tag: &str) -> Option<&'static SegmentDefinition> {
        segment_lookup(tag)
    }
    fn code_list(&self, de_id: &str) -> Option<&'static [&'static str]> {
        code_list(de_id)
    }
    fn directory_validator(&self) -> &'static DirectoryValidator {
        directory_validator()
    }
    fn group_schema(&self) -> &'static [GroupDef] {
        GROUP_SCHEMA
    }
}

pub(crate) static PROFILE: OrdrspFv20251001Profile = OrdrspFv20251001Profile;
