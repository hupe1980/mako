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
            ElementRef::new(3, "1225", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "DTM",
        "Date/Time/Period",
        &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "UNT",
        "Message Trailer",
        &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "RFF",
        "Reference",
        &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "NAD",
        "Name and Address",
        &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "CTA",
        "Contact Information",
        &[
            ElementRef::new(1, "3139", Status::Conditional, 1),
            ElementRef::new(2, "C056", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "COM",
        "Communication Contact",
        &[ElementRef::new(1, "C076", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "IDE",
        "Identity",
        &[
            ElementRef::new(1, "7495", Status::Mandatory, 1),
            ElementRef::new(2, "C206", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "STS",
        "Status",
        &[
            ElementRef::new(1, "9015", Status::Mandatory, 1),
            ElementRef::new(2, "9013", Status::Conditional, 1),
            ElementRef::new(3, "9011", Status::Conditional, 1),
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
        "AGR",
        "Agreement Identification",
        &[ElementRef::new(1, "C543", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "LOC",
        "Place/Location Identification",
        &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "SEQ",
        "Sequence Details",
        &[
            ElementRef::new(1, "1245", Status::Conditional, 1),
            ElementRef::new(2, "C286", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "QTY",
        "Quantity",
        &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
    ),
    SegmentDefinition::new(
        "CCI",
        "Characteristic/Class Id",
        &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C502", Status::Conditional, 1),
            ElementRef::new(3, "C240", Status::Conditional, 1),
        ],
    ),
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["E01", "E03", "E0F", "E1A", "E44"];
static CODES_1153: &[&str] = &["ACE", "AGI", "AGL", "MG", "TN", "Z13"];
static CODES_1245: &[&str] = &["Z01", "Z02", "Z03"];
static CODES_2005: &[&str] = &["137", "163", "164", "165", "166", "203"];
static CODES_3035: &[&str] = &["BF", "DDQ", "DER", "ELR", "EM", "MR", "MS", "Z01"];
static CODES_3227: &[&str] = &["172", "Z01", "Z04", "ZST"];
static CODES_7495: &[&str] = &["Z18", "Z19", "Z31", "Z32"];
static CODES_9015: &[&str] = &["E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08"];

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
        ("UNH", 0)
        | ("BGM", 2)
        | ("UNT", 0)
        | ("UNT", 1)
        | ("NAD", 0)
        | ("CTA", 0)
        | ("IDE", 0)
        | ("STS", 0)
        | ("STS", 1)
        | ("STS", 2)
        | ("FTX", 0)
        | ("LOC", 0)
        | ("SEQ", 0)
        | ("CCI", 0) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "1245" => Some(CODES_1245),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "7495" => Some(CODES_7495),
        "9015" => Some(CODES_9015),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_UTILMD_S2_1: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-UTILMD-S2.1",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_UTILMD_S2_1
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

fn rule_ide_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "IDE") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment IDE is missing".to_owned(),
            )
            .with_rule_id("MIG-IDE-REQ")
            .with_segment("IDE".to_owned()),
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

/// Layer 3 — verify the `RFF` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `RFF` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg1_rff_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "RFF").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by RFF occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.1-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg2_nad_max_occurrences(
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
            .with_rule_id("MIG-UTILMD-MIG-S2.1-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at most 9999 times.
///
/// Each occurrence of the trigger segment `IDE` marks the start of
/// one group instance.  The MIG specifies a maximum of 9999 instances.
fn rule_group_sg4_ide_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "IDE").count();
    if count > 9_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by IDE occurs {count} times; maximum is 9_999"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.1-GROUP-SG4-IDE-CARD-MAX")
            .with_segment("IDE".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg2_nad_min_occurrences(
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
            .with_rule_id("MIG-UTILMD-MIG-S2.1-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg4_ide_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "IDE").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by IDE occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.1-GROUP-SG4-IDE-CARD-MIN")
            .with_segment("IDE".to_owned()),
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
            "ROOT" => &["UNH", "BGM", "DTM", "UNT"],
            "SG1" | "SG6" => &["RFF"],
            "SG2" | "SG12" => &["NAD"],
            "SG3" => &["CTA", "COM"],
            "SG4" => &["IDE", "STS", "DTM", "FTX", "AGR"],
            "SG5" => &["LOC"],
            "SG8" => &["SEQ", "RFF", "DTM", "QTY"],
            "SG9" => &["QTY", "DTM"],
            "SG10" => &["CCI"],
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
    check_order(&tree, segments, "MIG-UTILMD-MIG-S2.1-ORDER", issues);
}

static MIG_UTILMD_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILMD-MIG-S2.1")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_ide_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_ide_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_ide_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_UTILMD_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[
    GroupDef {
        name: "SG2",
        trigger: "NAD",
        children: &[],
    },
    GroupDef {
        name: "SG4",
        trigger: "IDE",
        children: &[GroupDef {
            name: "SG6",
            trigger: "RFF",
            children: &[],
        }],
    },
];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="E03" is present
fn rule_ahb_55555_sts_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "E03"));
    if condition_holds && !segments.iter().any(|s| s.tag == "STS") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment STS is missing for Pruefidentifikator 55555 (I: when BGM DE[0]=\"E03\" is present)".to_owned(),
                )
                .with_rule_id("AHB-55555-STS-I0")
                .with_segment("STS".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "55555".to_owned()));
    }
}

static AHB_55001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55001")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55001-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55001-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55001-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55001-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55001-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55001-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55001-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55001-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55001-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55001-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_stateless_rule_fn("AHB-55001-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55001-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55001", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55001-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55001-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55001-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55001", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55001-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55001-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55001-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55001", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A06" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55001-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A06")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "Z07")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"Z07\") is missing for Pruefidentifikator 55001 (I: when STS DE[0]=\"E01\"+DE[2]=\"A06\" is present in SG4)".to_owned()).with_rule_id("AHB-55001-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55001-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55001 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55001-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55001_PACK)
}

static AHB_55002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55002")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55002-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55002-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55002-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55002-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55002-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55002-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55002-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55002-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55002-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55002-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_stateless_rule_fn("AHB-55002-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55002-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55002", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55002-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55002-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55002-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55002", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55002-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55002-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55002-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55002", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A06" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55002-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A06")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "Z07")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"Z07\") is missing for Pruefidentifikator 55002 (I: when STS DE[0]=\"E01\"+DE[2]=\"A06\" is present in SG4)".to_owned()).with_rule_id("AHB-55002-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55002-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55002 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55002-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55002_PACK)
}

static AHB_55003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55003")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55003-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55003-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55003-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55003-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55003-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55003-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55003-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55003-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55003-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55003-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_stateless_rule_fn("AHB-55003-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55003-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55003", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55003-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55003-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55003-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55003", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55003-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55003-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55003-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55003", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A06" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55003-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A06")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "Z07")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"Z07\") is missing for Pruefidentifikator 55003 (I: when STS DE[0]=\"E01\"+DE[2]=\"A06\" is present in SG4)".to_owned()).with_rule_id("AHB-55003-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55003-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55003 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55003-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55003_PACK)
}

static AHB_55004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55004")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55004-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55004-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55004-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E0F']", |q| matches!(q, "E0F"), "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55004-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55004-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55004-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55004-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55004-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55004-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55004-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_stateless_rule_fn("AHB-55004-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55004-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55004", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55004-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55004-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55004-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55004", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55004-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55004-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55004-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55004", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55004-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55004 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55004-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55004-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 55004 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55004-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55004-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55004 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55004-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55004_PACK)
}

static AHB_55005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55005")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55005-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55005-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55005-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55005-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55005-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55005-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55005-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55005-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55005-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55005-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_stateless_rule_fn("AHB-55005-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55005-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55005", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55005-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55005-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55005-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55005", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55005-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55005-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55005-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55005", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55005-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55005 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55005-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55005-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 55005 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55005-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55005-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55005 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55005-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55005_PACK)
}

static AHB_55006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55006")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55006-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55006-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55006-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E0F']", |q| matches!(q, "E0F"), "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55006-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55006-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55006-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55006-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55006-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55006-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55006-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_stateless_rule_fn("AHB-55006-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55006-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55006", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55006-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55006-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55006-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55006", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55006-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55006-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55006-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55006", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55006-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55006 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55006-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55006-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 55006 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-55006-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="A99" is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55006-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A99")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 55006 (I: when STS DE[0]=\"E01\"+DE[2]=\"A99\" is present in SG4)".to_owned()).with_rule_id("AHB-55006-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55006_PACK)
}

static AHB_55016_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55016")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55016-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55016-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55016-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55016-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55016-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55016-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55016-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55016-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55016-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55016-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_stateless_rule_fn("AHB-55016-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55016-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55016", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55016-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55016-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55016-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55016", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55016-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55016-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55016-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55016", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]∈{A04|A05} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55016-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A04" || v == "A05")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55016 (I: when STS DE[0]=\"E01\"+DE[2]∈{A04|A05} is present in SG4)".to_owned()).with_rule_id("AHB-55016-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55016_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55016_PACK)
}

static AHB_55017_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55017")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55017-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55017-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55017-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55017-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55017-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55017-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55017-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55017-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55017-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55017-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_stateless_rule_fn("AHB-55017-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55017-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55017", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55017-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55017-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55017-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55017", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55017-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55017-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55017-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55017", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]∈{A04|A05} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55017-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A04" || v == "A05")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55017 (I: when STS DE[0]=\"E01\"+DE[2]∈{A04|A05} is present in SG4)".to_owned()).with_rule_id("AHB-55017-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55017_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55017_PACK)
}

static AHB_55018_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55018")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55018-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55018-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55018-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55018-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55018-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55018-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55018-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55018-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55018-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55018-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_stateless_rule_fn("AHB-55018-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55018-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55018", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55018-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55018-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55018-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55018", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55018-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55018-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55018-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55018", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]∈{A04|A05} is present in SG4
            .with_scoped_group_rule_fn("SG4", "AHB-55018-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "A04" || v == "A05")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 55018 (I: when STS DE[0]=\"E01\"+DE[2]∈{A04|A05} is present in SG4)".to_owned()).with_rule_id("AHB-55018-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55018_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55018_PACK)
}

static AHB_55022_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55022")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55022-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55022-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55022-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55022-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55022-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55022-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55022-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55022-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55022-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55022-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55022-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_stateless_rule_fn("AHB-55022-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55022-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55022", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55022-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55022-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55022-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55022", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55022-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55022-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55022-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55022", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55022_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55022_PACK)
}

static AHB_55023_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55023")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55023-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55023-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55023-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55023-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55023-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55023-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55023-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55023-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55023-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55023-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55023-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_stateless_rule_fn("AHB-55023-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55023-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55023", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55023-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55023-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55023-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55023", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55023-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55023-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55023-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55023", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55023_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55023_PACK)
}

static AHB_55024_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55024")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55024-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55024-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55024-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55024-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55024-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55024-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55024-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55024-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55024-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55024-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55024-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_stateless_rule_fn("AHB-55024-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55024-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55024", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55024-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55024-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55024-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55024", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55024-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55024-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55024-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55024", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55024_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55024_PACK)
}

static AHB_55039_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55039")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55039-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55039-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55039-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55039-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55039-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55039-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55039-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55039-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55039-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55039-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_stateless_rule_fn("AHB-55039-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55039-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55039", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55039-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55039-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55039-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55039", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55039-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55039-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55039-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55039", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55039_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55039_PACK)
}

static AHB_55042_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55042")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55042-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55042-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55042-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55042-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55042-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55042-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55042-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55042-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55042-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55042-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_stateless_rule_fn("AHB-55042-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55042-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55042", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55042-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55042-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55042-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55042", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55042-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55042-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55042-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55042", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55042_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55042_PACK)
}

static AHB_55051_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55051")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55051-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55051-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55051-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55051-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55051-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55051-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55051-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55051-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55051-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55051-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_stateless_rule_fn("AHB-55051-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55051-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55051", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55051-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55051-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55051-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55051", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55051-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55051-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55051-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55051", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55051_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55051_PACK)
}

static AHB_55065_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55065")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55065-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55065-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_stateless_rule_fn("AHB-55065-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55065-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55065", issues);
            })
            .with_named_stateless_rule_fn("AHB-55065-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55065-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_stateless_rule_fn("AHB-55065-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55065-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55065", issues);
            })
            .with_named_stateless_rule_fn("AHB-55065-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55065-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_stateless_rule_fn("AHB-55065-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55065-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55065", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55065-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55065-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55065-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55065", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55065_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55065_PACK)
}

static AHB_55069_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55069")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55069-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55069-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_stateless_rule_fn("AHB-55069-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55069-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55069", issues);
            })
            .with_named_stateless_rule_fn("AHB-55069-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55069-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_stateless_rule_fn("AHB-55069-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55069-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55069", issues);
            })
            .with_named_stateless_rule_fn("AHB-55069-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55069-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_stateless_rule_fn("AHB-55069-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55069-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55069", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55069-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55069-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55069-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55069", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55069_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55069_PACK)
}

static AHB_55070_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55070")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55070-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55070-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_stateless_rule_fn("AHB-55070-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55070-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55070", issues);
            })
            .with_named_stateless_rule_fn("AHB-55070-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55070-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_stateless_rule_fn("AHB-55070-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55070-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55070", issues);
            })
            .with_named_stateless_rule_fn("AHB-55070-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55070-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_stateless_rule_fn("AHB-55070-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55070-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55070", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55070-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55070-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55070-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55070", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55070_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55070_PACK)
}

static AHB_55168_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55168")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55168-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55168-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55168-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55168-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55168-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55168-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55168-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55168-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55168-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55168-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_stateless_rule_fn("AHB-55168-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55168-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55168", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55168-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55168-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55168-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55168", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55168-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55168-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55168-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55168", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55168_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55168_PACK)
}

static AHB_55555_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55555")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55555-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55555-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55555-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55555-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55555-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55555-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55555-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55555-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55555-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55555-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_stateless_rule_fn(rule_ahb_55555_sts_cond_0)
            .with_named_stateless_rule_fn("AHB-55555-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55555-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['E07', 'E08']", |q| matches!(q, "E07" | "E08"), "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55555-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_stateless_rule_fn("AHB-55555-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55555-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55555", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55555-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55555-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55555-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55555", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55555-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55555-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55555-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55555", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55555_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55555_PACK)
}

static AHB_55600_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55600")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55600-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55600-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55600-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55600-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55600-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55600-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55600-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55600-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55600-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55600-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_stateless_rule_fn("AHB-55600-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55600-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55600", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55600-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55600-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55600-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55600", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55600-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55600-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55600-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55600", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55600_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55600_PACK)
}

static AHB_55601_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.1-55601")
            .for_message_type("UTILMD")
            .for_release("S2.1")
            .with_named_stateless_rule_fn("AHB-55601-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55601-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55601-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55601-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55601-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55601-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55601-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55601-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55601-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55601-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_stateless_rule_fn("AHB-55601-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55601-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55601", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55601-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55601-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55601-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55601", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55601-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55601-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55601-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55601", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55601_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55601_PACK)
}

static AHB_ALL_PACK_UTILMD_S2_1: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("UTILMD-AHB-S2.1-ALL")
        .for_message_type("UTILMD")
        .for_release("S2.1");
    let pack = pack
        .merge_with_override(ahb_55001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55016_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55017_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55018_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55022_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55023_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55024_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55039_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55042_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55051_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55065_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55069_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55070_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55168_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55555_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55600_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55601_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(55001) => ahb_55001_pack(),
            Some(55002) => ahb_55002_pack(),
            Some(55003) => ahb_55003_pack(),
            Some(55004) => ahb_55004_pack(),
            Some(55005) => ahb_55005_pack(),
            Some(55006) => ahb_55006_pack(),
            Some(55016) => ahb_55016_pack(),
            Some(55017) => ahb_55017_pack(),
            Some(55018) => ahb_55018_pack(),
            Some(55022) => ahb_55022_pack(),
            Some(55023) => ahb_55023_pack(),
            Some(55024) => ahb_55024_pack(),
            Some(55039) => ahb_55039_pack(),
            Some(55042) => ahb_55042_pack(),
            Some(55051) => ahb_55051_pack(),
            Some(55065) => ahb_55065_pack(),
            Some(55069) => ahb_55069_pack(),
            Some(55070) => ahb_55070_pack(),
            Some(55168) => ahb_55168_pack(),
            Some(55555) => ahb_55555_pack(),
            Some(55600) => ahb_55600_pack(),
            Some(55601) => ahb_55601_pack(),
            None => Arc::clone(&AHB_ALL_PACK_UTILMD_S2_1),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("UTILMD")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_UTILMD_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("S2.1"));

pub(crate) struct UtilmdFv20251001Profile;

impl Profile for UtilmdFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Utilmd
    }
    fn release(&self) -> &Release {
        &RELEASE_UTILMD_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 09 - 30))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("S2.1")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("UTILMD MIG S2.1 konsolidierte Lesefassung Stand 02.03.2026")
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

pub(crate) static PROFILE: UtilmdFv20251001Profile = UtilmdFv20251001Profile;
