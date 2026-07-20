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
    SegmentDefinition::new(
        "UNH",
        "Message Header",
        &[
            ElementRef::new(1, "0062", Status::Mandatory, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "BGM",
        "Beginning of Message",
        &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "DTM",
        "Date/Time/Period (Nachrichtendatum)",
        &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "RFF",
        "Reference (Prüfidentifikator)",
        &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "NAD",
        "Name and Address (Absender)",
        &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "CTA",
        "Contact Information (Absender)",
        &[
            ElementRef::new(1, "3139", Status::Conditional, 1),
            ElementRef::new(2, "C056", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "COM",
        "Communication Contact (Absender)",
        &[ElementRef::new(1, "C076", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "UNS",
        "Section Control",
        &[ElementRef::new(1, "0081", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "FII",
        "Financial Institution Information",
        &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C078", Status::Conditional, 1),
            ElementRef::new(3, "C088", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "FTX",
        "Free Text",
        &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C108", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "CCI",
        "Characteristic/Class Id (Erreichbarkeit)",
        &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C502", Status::Conditional, 1),
            ElementRef::new(3, "C240", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "CAV",
        "Merkmalswert",
        &[ElementRef::new(1, "C889", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "UNT",
        "Message Trailer",
        &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    ),
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["35", "59"];
static CODES_1153: &[&str] = &["ACW", "AGK", "VA", "Z13", "Z25"];
static CODES_2005: &[&str] = &["137", "157", "Z36"];
static CODES_3035: &[&str] = &["MR", "MS", "SU", "Z10"];
static CODES_7059: &[&str] = &["Z19", "Z40"];

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
        "7059" => Some(CODES_7059),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_PARTIN_1_0F: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-PARTIN-1.0f",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_PARTIN_1_0F
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

fn rule_cta_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "CTA") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment CTA is missing".to_owned(),
            )
            .with_rule_id("MIG-CTA-REQ")
            .with_segment("CTA".to_owned()),
        );
    }
}

fn rule_com_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "COM") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment COM is missing".to_owned(),
            )
            .with_rule_id("MIG-COM-REQ")
            .with_segment("COM".to_owned()),
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

fn rule_fii_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "FII") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment FII is missing".to_owned(),
            )
            .with_rule_id("MIG-FII-REQ")
            .with_segment("FII".to_owned()),
        );
    }
}

fn rule_cci_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "CCI") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment CCI is missing".to_owned(),
            )
            .with_rule_id("MIG-CCI-REQ")
            .with_segment("CCI".to_owned()),
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

/// Layer 3 — verify `COM` appears at most 5 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_com_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "COM").count();
    if count > 5 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment COM occurs {count} times; maximum is 5"),
            )
            .with_rule_id("MIG-COM-CARD-MAX")
            .with_segment("COM".to_owned()),
        );
    }
}

/// Layer 3 — verify `FII` appears at most 10 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_fii_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "FII").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment FII occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-FII-CARD-MAX")
            .with_segment("FII".to_owned()),
        );
    }
}

/// Layer 3 — verify `FTX` appears at most 10 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_ftx_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "FTX").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment FTX occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-FTX-CARD-MAX")
            .with_segment("FTX".to_owned()),
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

/// Layer 3 — verify `CAV` appears at most 10 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_cav_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CAV").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment CAV occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-CAV-CARD-MAX")
            .with_segment("CAV".to_owned()),
        );
    }
}

/// Layer 3 — verify `NAD` appears at most 11 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_nad_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "NAD").count();
    if count > 11 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment NAD occurs {count} times; maximum is 11"),
            )
            .with_rule_id("MIG-NAD-CARD-MAX")
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
        "UNH", "BGM", "DTM", "RFF", "DTM", "RFF", "NAD", "CTA", "COM", "NAD",
    ];
    /// Detail segment ordering (after UNS+D).
    const EXPECTED_DETAIL_ORDER: &[&str] = &[
        "NAD", "FII", "RFF", "RFF", "FTX", "CCI", "DTM", "CAV", "CCI", "NAD", "CTA", "COM", "UNT",
    ];

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
        "MIG-PARTIN-MIG-1.0f-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-PARTIN-MIG-1.0f-ORDER",
        issues,
    );
}

static MIG_PARTIN_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-MIG-1.0f")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_cta_mandatory)
            .with_stateless_rule_fn(rule_com_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_fii_mandatory)
            .with_stateless_rule_fn(rule_cci_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_com_max_occurrences)
            .with_stateless_rule_fn(rule_fii_max_occurrences)
            .with_stateless_rule_fn(rule_ftx_max_occurrences)
            .with_stateless_rule_fn(rule_dtm_max_occurrences)
            .with_stateless_rule_fn(rule_cav_max_occurrences)
            .with_stateless_rule_fn(rule_nad_max_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_PARTIN_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_37000_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37000")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37000-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37000-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-37000-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37000-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37000-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37000-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37000-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37000-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37000-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37000-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37000-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37000-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37000",
                    "37000",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37000_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37000_PACK)
}

static AHB_37001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37001")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37001-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37001-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37001-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37001",
                    "37001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37001_PACK)
}

static AHB_37002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37002")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37002-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37002-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37002-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37002",
                    "37002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37002_PACK)
}

static AHB_37003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37003")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37003-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37003-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37003-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37003-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37003-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37003",
                    "37003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37003_PACK)
}

static AHB_37004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37004")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37004-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37004-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37004-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37004-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37004-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37004",
                    "37004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37004_PACK)
}

static AHB_37005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37005")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37005-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37005-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37005-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37005-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37005-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37005",
                    "37005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37005_PACK)
}

static AHB_37006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37006")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37006-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37006-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37006-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37006-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37006-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37006-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37006-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37006-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37006-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37006-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37006-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37006",
                    "37006",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37006_PACK)
}

static AHB_37008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37008")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37008-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37008-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37008-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37008-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37008-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37008-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37008-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37008-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37008-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37008-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37008-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37008",
                    "37008",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37008_PACK)
}

static AHB_37009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37009")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37009-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37009-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37009-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37009-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37009-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37009",
                    "37009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37009_PACK)
}

static AHB_37010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37010")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37010-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37010-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37010-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37010-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37010-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37010-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37010-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37010-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37010-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37010-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37010-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37010",
                    "37010",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37010_PACK)
}

static AHB_37011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37011")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37011-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37011-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37011-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37011-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37011-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37011-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37011-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37011-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37011-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37011-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37011-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37011",
                    "37011",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37011_PACK)
}

static AHB_37012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37012")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37012-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37012-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37012-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37012-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37012-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37012-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37012-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37012-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37012-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37012-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37012-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37012",
                    "37012",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37012_PACK)
}

static AHB_37013_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37013")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37013-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37013-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37013-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37013-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37013-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37013-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37013-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37013-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37013-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37013-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37013-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37013",
                    "37013",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37013_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37013_PACK)
}

static AHB_37014_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PARTIN-AHB-1.0f-37014")
            .for_message_type("PARTIN")
            .for_release("1.0f")
            .with_named_stateless_rule_fn("AHB-37014-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-37014-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-37014-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-37014-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-37014-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-37014-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-FII-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FII",
                    "AHB-37014-FII-M",
                    "mandatory segment FII is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-37014-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-37014-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-37014-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-37014-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 37014",
                    "37014",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_37014_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_37014_PACK)
}

static AHB_ALL_PACK_PARTIN_1_0F: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("PARTIN-AHB-1.0f-ALL")
        .for_message_type("PARTIN")
        .for_release("1.0f");
    let pack = pack
        .merge_with_override(ahb_37000_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37013_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_37014_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(37000) => ahb_37000_pack(),
            Some(37001) => ahb_37001_pack(),
            Some(37002) => ahb_37002_pack(),
            Some(37003) => ahb_37003_pack(),
            Some(37004) => ahb_37004_pack(),
            Some(37005) => ahb_37005_pack(),
            Some(37006) => ahb_37006_pack(),
            Some(37008) => ahb_37008_pack(),
            Some(37009) => ahb_37009_pack(),
            Some(37010) => ahb_37010_pack(),
            Some(37011) => ahb_37011_pack(),
            Some(37012) => ahb_37012_pack(),
            Some(37013) => ahb_37013_pack(),
            Some(37014) => ahb_37014_pack(),
            None => Arc::clone(&AHB_ALL_PACK_PARTIN_1_0F),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("PARTIN")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_PARTIN_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("1.0f"));

pub(crate) struct PartinFv20251001Profile;

impl Profile for PartinFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Partin
    }
    fn release(&self) -> &Release {
        &RELEASE_PARTIN_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 03 - 31))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.0f")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("PARTIN AHB 1.0f, Stand 01.10.2025")
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

pub(crate) static PROFILE: PartinFv20251001Profile = PartinFv20251001Profile;
