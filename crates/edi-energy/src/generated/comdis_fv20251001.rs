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
        "Nachrichten-Kopfsegment",
        &[
            ElementRef::new(1, "0062", Status::Mandatory, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "BGM",
        "Beginn der Nachricht",
        &[
            ElementRef::new(1, "C002", Status::Conditional, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "RFF",
        "Prüfidentifikator",
        &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "DTM",
        "Dokumentendatum",
        &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "CUX",
        "Währungsangaben",
        &[ElementRef::new(1, "C504", Status::Conditional, 1)],
    ),
    SegmentDefinition::new(
        "UNT",
        "Nachrichten-Endesegment",
        &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "NAD",
        "MP-ID Absender/Empfänger",
        &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "CTA",
        "Ansprechpartner",
        &[
            ElementRef::new(1, "3139", Status::Conditional, 1),
            ElementRef::new(2, "C056", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "COM",
        "Kommunikationsverbindung",
        &[ElementRef::new(1, "C076", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "DOC",
        "Dokument-/Nachricht-Einzelheiten",
        &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C503", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "MOA",
        "angeforderter Betrag",
        &[ElementRef::new(1, "C516", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "AJT",
        "Begründung der Korrektheit",
        &[
            ElementRef::new(1, "4465", Status::Mandatory, 1),
            ElementRef::new(2, "1082", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "FTX",
        "Begründung Richtigkeit",
        &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C107", Status::Conditional, 1),
            ElementRef::new(3, "C108", Status::Conditional, 1),
        ],
    ),
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["380", "456", "739", "Z41", "Z42"];
static CODES_1153: &[&str] = &["Z13"];
static CODES_2005: &[&str] = &["137"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_3139: &[&str] = &["IC"];
static CODES_3155: &[&str] = &["AJ", "AL", "EM", "FX", "TE"];
static CODES_4451: &[&str] = &["ACB", "ACD"];
static CODES_5025: &[&str] = &["9"];
static CODES_6343: &[&str] = &["4"];
static CODES_6347: &[&str] = &["2"];

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
        "3139" => Some(CODES_3139),
        "3155" => Some(CODES_3155),
        "4451" => Some(CODES_4451),
        "5025" => Some(CODES_5025),
        "6343" => Some(CODES_6343),
        "6347" => Some(CODES_6347),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_COMDIS_1_0G: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-COMDIS-1.0g",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_COMDIS_1_0G
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

fn rule_doc_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "DOC") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment DOC is missing".to_owned(),
            )
            .with_rule_id("MIG-DOC-REQ")
            .with_segment("DOC".to_owned()),
        );
    }
}

fn rule_ajt_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "AJT") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment AJT is missing".to_owned(),
            )
            .with_rule_id("MIG-AJT-REQ")
            .with_segment("AJT".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg1_nad_max_occurrences(
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
            .with_rule_id("MIG-COMDIS-MIG-1.0g-GROUP-SG1-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at most 9999 times.
///
/// Each occurrence of the trigger segment `DOC` marks the start of
/// one group instance.  The MIG specifies a maximum of 9999 instances.
fn rule_group_sg2_doc_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DOC").count();
    if count > 9_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by DOC occurs {count} times; maximum is 9_999"),
            )
            .with_rule_id("MIG-COMDIS-MIG-1.0g-GROUP-SG2-DOC-CARD-MAX")
            .with_segment("DOC".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg1_nad_min_occurrences(
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
            .with_rule_id("MIG-COMDIS-MIG-1.0g-GROUP-SG1-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg2_doc_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DOC").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by DOC occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-COMDIS-MIG-1.0g-GROUP-SG2-DOC-CARD-MIN")
            .with_segment("DOC".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    /// Per-group expected segment order derived from the MIG.
    ///
    /// Returns an empty slice for groups not covered by the MIG or for the
    /// catch-all arm, which causes those groups to be skipped silently.
    fn group_order(name: &str) -> &'static [&'static str] {
        match name {
            "ROOT" => &["UNH", "BGM", "RFF", "DTM", "CUX", "UNT"],
            "SG1" => &["NAD", "CTA", "COM"],
            "SG2" => &["DOC", "MOA"],
            "SG3" => &["AJT", "FTX"],
            _ => &[],
        }
    }

    /// Recursively verify segment order within a group and all its children.
    ///
    /// Only `direct_segment_indices()` — segments that belong directly to this
    /// group and are not claimed by any child group — are checked.  Child groups
    /// are then visited recursively, so every segment in the message is covered
    /// exactly once.
    fn check_order(
        group: &edifact_rs::group::SegmentGroupIndexed,
        all_segs: &[edifact_rs::Segment<'_>],
        rule_id: &str,
        issues: &mut Vec<ValidationIssue>,
    ) {
        let expected = group_order(group.definition);
        if !expected.is_empty() {
            let mut cursor: usize = 0;
            for idx in group.direct_segment_indices() {
                let seg = &all_segs[idx];
                if let Some(pos) = expected[cursor..].iter().position(|&t| t == seg.tag) {
                    cursor += pos;
                } else if expected.contains(&seg.tag) {
                    // Tag is known for this group but already passed — ordering violation.
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            "segment appears out of order".to_owned(),
                        )
                        .with_rule_id(rule_id)
                        .with_segment(seg.tag.to_owned()),
                    );
                }
                // Tags not in this group's expected order are unknown here;
                // they are either in a child group (checked below) or caught by the DirectoryValidator.
            }
        }
        for child in &group.children {
            check_order(child, all_segs, rule_id, issues);
        }
    }

    let tree = edifact_rs::group::group_segments_indexed(segments, GROUP_SCHEMA, "ROOT");
    check_order(&tree, segments, "MIG-COMDIS-MIG-1.0g-ORDER", issues);
}

static MIG_COMDIS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("COMDIS-MIG-1.0g")
            .for_message_type("COMDIS")
            .for_release("1.0g")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_doc_mandatory)
            .with_stateless_rule_fn(rule_ajt_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_doc_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_doc_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_COMDIS_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_29001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("COMDIS-AHB-1.0g-29001")
            .for_message_type("COMDIS")
            .for_release("1.0g")
            .with_named_stateless_rule_fn("AHB-29001-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-29001-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-29001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-29001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-29001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-29001-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-29001-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-29001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-29001-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-29001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-29001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 29001",
                    "29001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_29001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_29001_PACK)
}

static AHB_29002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("COMDIS-AHB-1.0g-29002")
            .for_message_type("COMDIS")
            .for_release("1.0g")
            .with_named_stateless_rule_fn("AHB-29002-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-29002-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-29002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-29002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-29002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-29002-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-29002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-29002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-29002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-29002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 29002",
                    "29002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_29002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_29002_PACK)
}

static AHB_ALL_PACK_COMDIS_1_0G: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("COMDIS-AHB-1.0g-ALL")
        .for_message_type("COMDIS")
        .for_release("1.0g");
    let pack = pack
        .merge_with_override(ahb_29001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_29002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(29001) => ahb_29001_pack(),
            Some(29002) => ahb_29002_pack(),
            None => Arc::clone(&AHB_ALL_PACK_COMDIS_1_0G),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("COMDIS")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_COMDIS_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("1.0g"));

pub(crate) struct ComdisFv20251001Profile;

impl Profile for ComdisFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Comdis
    }
    fn release(&self) -> &Release {
        &RELEASE_COMDIS_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 09 - 30))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.0g")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("COMDIS AHB 1.0g, Stand 01.10.2025")
    }
    fn pid_source(&self) -> crate::registry::PidSource {
        crate::registry::PidSource::RffZ13
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

pub(crate) static PROFILE: ComdisFv20251001Profile = ComdisFv20251001Profile;
