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
        name: "Message Header",
        elements: &[
            ElementRef::new(1, "0062", Status::Mandatory, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "BGM",
        name: "Beginning of Message",
        elements: &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Date/Time/Period",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Item Description",
        elements: &[
            ElementRef::new(1, "7077", Status::Conditional, 1),
            ElementRef::new(2, "C273", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Free Text",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C108", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "NAD",
        name: "Name and Address",
        elements: &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CTA",
        name: "Contact Information",
        elements: &[
            ElementRef::new(1, "3139", Status::Conditional, 1),
            ElementRef::new(2, "C056", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "COM",
        name: "Communication Contact",
        elements: &[ElementRef::new(1, "C076", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "LOC",
        name: "Place/Location Identification",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Line Item",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "UNT",
        name: "Message Trailer",
        elements: &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Reference (Prüfidentifikator/Referenz)",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "PIA",
        name: "Zusätzliche Produktidentifikation",
        elements: &[
            ElementRef::new(1, "4347", Status::Mandatory, 1),
            ElementRef::new(2, "C212", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "CCI",
        name: "Characteristic/Class Id",
        elements: &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C502", Status::Conditional, 1),
            ElementRef::new(3, "C240", Status::Conditional, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["311", "Z29", "Z57", "Z74", "Z93"];
static CODES_1153: &[&str] = &["AEP", "AGK", "AGO", "Z13"];
static CODES_2005: &[&str] = &["137", "163", "164", "203", "469", "472"];
static CODES_3035: &[&str] = &["BY", "DP", "MR", "MS"];
static CODES_3227: &[&str] = &["172"];
static CODES_4451: &[&str] = &["REG", "ZZZ"];

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
        ("CCI", 0) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "4451" => Some(CODES_4451),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_REQOTE_1_3C: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-REQOTE-1.3c",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_REQOTE_1_3C
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

/// Layer 3 — verify `DTM` appears at most 5 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_dtm_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DTM").count();
    if count > 5 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment DTM occurs {count} times; maximum is 5"),
            )
            .with_rule_id("MIG-DTM-CARD-MAX")
            .with_segment("DTM".to_owned()),
        );
    }
}

/// Layer 3 — verify `FTX` appears at most 2 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead.
fn rule_ftx_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "FTX").count();
    if count > 2 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment FTX occurs {count} times; maximum is 2"),
            )
            .with_rule_id("MIG-FTX-CARD-MAX")
            .with_segment("FTX".to_owned()),
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
            .with_rule_id("MIG-REQOTE-MIG-1.3c-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg11_nad_max_occurrences(
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
            .with_rule_id("MIG-REQOTE-MIG-1.3c-GROUP-SG11-NAD-CARD-MAX")
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
            .with_rule_id("MIG-REQOTE-MIG-1.3c-GROUP-SG27-LIN-CARD-MAX")
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
            .with_rule_id("MIG-REQOTE-MIG-1.3c-GROUP-SG1-RFF-CARD-MIN")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg11_nad_min_occurrences(
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
            .with_rule_id("MIG-REQOTE-MIG-1.3c-GROUP-SG11-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
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
            "ROOT" => &[
                "UNH", "BGM", "DTM", "IMD", "FTX", "NAD", "CTA", "COM", "LOC", "LIN", "UNT",
            ],
            "SG1" => &["RFF"],
            "SG11" => &["NAD", "LOC"],
            "SG14" => &["CTA", "COM"],
            "SG27" => &["LIN", "PIA"],
            "SG28" => &["CCI"],
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
    check_order(&tree, segments, "MIG-REQOTE-MIG-1.3c-ORDER", issues);
}

static MIG_REQOTE_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-MIG-1.3c")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_dtm_max_occurrences)
            .with_stateless_rule_fn(rule_ftx_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg11_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg27_lin_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg11_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_REQOTE_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when RFF DE[0]="Z41" is present // [17] Wenn SG1 RFF+Z41 (Referenznummer des Vorgangs der Anmeldung nach WiM) vorhanden, ist SG11 NAD (NB) Pflicht
fn rule_ahb_35004_nad_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "RFF" && s.element_str(0).is_some_and(|v| v == "Z41"));
    if condition_holds && !segments.iter().any(|s| s.tag == "NAD") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment NAD is missing for Pruefidentifikator 35004 (I: when RFF DE[0]=\"Z41\" is present)".to_owned(),
                )
                .with_rule_id("AHB-35004-NAD-I0")
                .with_segment("NAD".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "35004".to_owned()));
    }
}

static AHB_35001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-AHB-1.3c-35001")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-35001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-35001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-35001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-35001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-35001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-35001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-35001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-35001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-35001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 35001",
                    "35001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_35001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_35001_PACK)
}

static AHB_35002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-AHB-1.3c-35002")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-35002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-35002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-35002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-35002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-35002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-35002-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-35002-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-35002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-35002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 35002",
                    "35002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_35002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_35002_PACK)
}

static AHB_35003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-AHB-1.3c-35003")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-35003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-35003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-35003-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-35003-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-35003-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-35003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-35003-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-35003-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-35003-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-35003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-35003-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-35003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 35003",
                    "35003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_35003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_35003_PACK)
}

static AHB_35004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-AHB-1.3c-35004")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-35004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-35004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-35004-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-35004-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-35004-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-35004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-35004-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-35004-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-35004-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-35004-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-35004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_35004_nad_cond_0)
            .with_named_stateless_rule_fn("AHB-35004-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-35004-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-35004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 35004",
                    "35004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_35004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_35004_PACK)
}

static AHB_35005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REQOTE-AHB-1.3c-35005")
            .for_message_type("REQOTE")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-35005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-35005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-35005-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-35005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-35005-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-35005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-35005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-35005-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-35005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-35005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 35005",
                    "35005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_35005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_35005_PACK)
}

static AHB_ALL_PACK_REQOTE_1_3C: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("REQOTE-AHB-1.3c-ALL")
        .for_message_type("REQOTE")
        .for_release("1.3c");
    let pack = pack
        .merge_with_override(ahb_35001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_35002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_35003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_35004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_35005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(35001) => ahb_35001_pack(),
            Some(35002) => ahb_35002_pack(),
            Some(35003) => ahb_35003_pack(),
            Some(35004) => ahb_35004_pack(),
            Some(35005) => ahb_35005_pack(),
            None => Arc::clone(&AHB_ALL_PACK_REQOTE_1_3C),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("REQOTE")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_REQOTE_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("1.3c"));

pub(crate) struct ReqoteFv20260401Profile;

impl Profile for ReqoteFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Reqote
    }
    fn release(&self) -> &Release {
        &RELEASE_REQOTE_FV20260401
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 04 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.3c")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("REQOTE AHB 1.3c, Stand 01.04.2026")
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

pub(crate) static PROFILE: ReqoteFv20260401Profile = ReqoteFv20260401Profile;
