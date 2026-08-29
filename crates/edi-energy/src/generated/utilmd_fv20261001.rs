// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate
#![allow(clippy::doc_markdown)]
#![allow(clippy::too_many_lines)]

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
            ElementRef::new(1, "C601", Status::Mandatory, 1),
            ElementRef::new(2, "C555", Status::Conditional, 1),
            ElementRef::new(3, "C556", Status::Conditional, 1),
            ElementRef::new(4, "C556", Status::Conditional, 1),
            ElementRef::new(5, "C556", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "FTX",
        "Free Text",
        &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(4, "C108", Status::Conditional, 1),
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
        "PIA",
        "Erforderliches Produkt",
        &[
            ElementRef::new(1, "4347", Status::Mandatory, 1),
            ElementRef::new(2, "C212", Status::Mandatory, 1),
        ],
    ),
    SegmentDefinition::new(
        "CCI",
        "Characteristic/Class Id",
        &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C502", Status::Conditional, 1),
            ElementRef::new(3, "C240", Status::Conditional, 1),
            ElementRef::new(4, "4051", Status::Conditional, 1),
        ],
    ),
    SegmentDefinition::new(
        "CAV",
        "Merkmalswert",
        &[ElementRef::new(1, "C889", Status::Mandatory, 1)],
    ),
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &[
    "E01", "E02", "E03", "E35", "E40", "E44", "Z05", "Z07", "Z14", "Z17", "Z18", "Z37", "Z40",
    "Z71", "Z88", "Z89", "Z90",
];
static CODES_1131: &[&str] = &[
    "S_0054", "S_0055", "S_0056", "S_0059", "S_0060", "S_0063", "S_0064", "S_0090",
];
static CODES_1153: &[&str] = &["ACE", "AGI", "AGL", "MG", "TN", "Z13"];
static CODES_1245: &[&str] = &["Z01", "Z02", "Z03"];
static CODES_2005: &[&str] = &[
    "137", "154", "157", "158", "159", "163", "164", "206", "471", "752", "76", "92", "93", "Z01",
    "Z05", "Z06", "Z07", "Z08", "Z09", "Z10", "Z15", "Z16", "Z21", "Z22", "Z25", "Z26",
];
static CODES_3035: &[&str] = &[
    "BF", "DDO", "DDQ", "DER", "DP", "ELR", "EM", "EO", "MR", "MS", "VY", "Z01", "Z03", "Z04",
    "Z05", "Z07", "Z08", "Z09", "Z25", "Z26", "Z60", "Z63", "Z64", "Z67", "Z68", "Z69", "Z70",
];
static CODES_3227: &[&str] = &[
    "172", "Z01", "Z04", "Z16", "Z17", "Z18", "Z19", "Z20", "Z21", "Z22", "ZST",
];
static CODES_4347: &[&str] = &["5"];
static CODES_7037: &[&str] = &["Z15", "Z18", "ZC9", "ZD0", "ZE3", "ZZD"];
static CODES_7059: &[&str] = &["Z27", "Z28", "Z36"];
static CODES_7495: &[&str] = &["24", "Z18", "Z19", "Z31", "Z32"];
static CODES_9015: &[&str] = &["7", "E01", "E02", "E03", "E04", "E05", "E06", "E07", "E08"];

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
        | ("UNT", 0)
        | ("UNT", 1)
        | ("NAD", 0)
        | ("IDE", 0)
        | ("STS", 0)
        | ("FTX", 0)
        | ("LOC", 0)
        | ("PIA", 0) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1131" => Some(CODES_1131),
        "1153" => Some(CODES_1153),
        "1245" => Some(CODES_1245),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "4347" => Some(CODES_4347),
        "7037" => Some(CODES_7037),
        "7059" => Some(CODES_7059),
        "7495" => Some(CODES_7495),
        "9015" => Some(CODES_9015),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_UTILMD_S2_2: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-UTILMD-S2.2",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_UTILMD_S2_2
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
/// Counted over the group tree, so a nested group sharing the `RFF`
/// trigger is not charged here.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg1_rff_max_occurrences(
    _root: &edifact_rs::SegmentGroupIndexed<'_>,
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    // SG1 is not in GROUP_SCHEMA, so the tree cannot count it.
    let count = segments.iter().filter(|s| s.tag == "RFF").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by RFF occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.2-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Counted over the group tree, so a nested group sharing the `NAD`
/// trigger is not charged here.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg2_nad_max_occurrences(
    root: &edifact_rs::SegmentGroupIndexed<'_>,
    _segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = root.find("SG2").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.2-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at most 9999 times.
///
/// Counted over the group tree, so a nested group sharing the `IDE`
/// trigger is not charged here.  The MIG specifies a maximum of 9999 instances.
fn rule_group_sg4_ide_max_occurrences(
    root: &edifact_rs::SegmentGroupIndexed<'_>,
    _segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = root.find("SG4").count();
    if count > 9_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by IDE occurs {count} times; maximum is 9_999"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.2-GROUP-SG4-IDE-CARD-MAX")
            .with_segment("IDE".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg2_nad_min_occurrences(
    root: &edifact_rs::SegmentGroupIndexed<'_>,
    _segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = root.find("SG2").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.2-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg4_ide_min_occurrences(
    root: &edifact_rs::SegmentGroupIndexed<'_>,
    _segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = root.find("SG4").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by IDE occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-UTILMD-MIG-S2.2-GROUP-SG4-IDE-CARD-MIN")
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
            "SG4" => &["IDE", "DTM", "STS", "FTX", "AGR"],
            "SG5" => &["LOC"],
            "SG8" => &["SEQ", "RFF", "DTM", "QTY", "PIA"],
            "SG9" => &["QTY", "DTM"],
            "SG10" => &["CCI", "CAV"],
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
                } else if expected.contains(&seg.tag.as_ref()) {
                    // Tag is known for this group but already passed — ordering violation.
                    issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            "segment appears out of order".to_owned(),
                        )
                        .with_rule_id(rule_id)
                        .with_segment(seg.tag.as_ref()),
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
    check_order(&tree, segments, "MIG-UTILMD-MIG-S2.2-ORDER", issues);
}

static MIG_UTILMD_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILMD-MIG-S2.2")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_rule_fn(rule_unh_mandatory)
            .with_rule_fn(rule_bgm_mandatory)
            .with_rule_fn(rule_dtm_mandatory)
            .with_rule_fn(rule_unt_mandatory)
            .with_rule_fn(rule_nad_mandatory)
            .with_rule_fn(rule_ide_mandatory)
            .with_rule_fn(rule_rff_mandatory)
            .with_named_group_rule_fn(
                "MIG-UTILMD-MIG-S2.2-GROUP-SG1-RFF-CARD-MAX",
                |g, segs, _ctx, issues| {
                    if g.definition == "ROOT" {
                        rule_group_sg1_rff_max_occurrences(g, segs, issues);
                    }
                },
            )
            .with_named_group_rule_fn(
                "MIG-UTILMD-MIG-S2.2-GROUP-SG2-NAD-CARD-MAX",
                |g, segs, _ctx, issues| {
                    if g.definition == "ROOT" {
                        rule_group_sg2_nad_max_occurrences(g, segs, issues);
                    }
                },
            )
            .with_named_group_rule_fn(
                "MIG-UTILMD-MIG-S2.2-GROUP-SG4-IDE-CARD-MAX",
                |g, segs, _ctx, issues| {
                    if g.definition == "ROOT" {
                        rule_group_sg4_ide_max_occurrences(g, segs, issues);
                    }
                },
            )
            .with_named_group_rule_fn(
                "MIG-UTILMD-MIG-S2.2-GROUP-SG2-NAD-CARD-MIN",
                |g, segs, _ctx, issues| {
                    if g.definition == "ROOT" {
                        rule_group_sg2_nad_min_occurrences(g, segs, issues);
                    }
                },
            )
            .with_named_group_rule_fn(
                "MIG-UTILMD-MIG-S2.2-GROUP-SG4-IDE-CARD-MIN",
                |g, segs, _ctx, issues| {
                    if g.definition == "ROOT" {
                        rule_group_sg4_ide_min_occurrences(g, segs, issues);
                    }
                },
            )
            .with_rule_fn(rule_segment_order),
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
        children: &[
            GroupDef {
                name: "SG5",
                trigger: "LOC",
                children: &[],
            },
            GroupDef {
                name: "SG6",
                trigger: "RFF",
                children: &[],
            },
            GroupDef {
                name: "SG8",
                trigger: "SEQ",
                children: &[GroupDef {
                    name: "SG10",
                    trigger: "CCI",
                    children: &[],
                }],
            },
            GroupDef {
                name: "SG12",
                trigger: "NAD",
                children: &[],
            },
        ],
    },
];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="E01" is present // Kap. 10.3: SG4 DTM+76 (Datum zum geplanten Leistungsbeginn) ist Muss
fn rule_ahb_55168_dtm_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "E01"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "76"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"76\") is missing for Pruefidentifikator 55168 (I: when BGM DE[0]=\"E01\" is present)".to_owned(),
                )
                .with_rule_id("AHB-55168-DTM-I0")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "55168".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z40" is present // Kap. 10.3: SG4 DTM+76 (Datum zum geplanten Leistungsbeginn) ist Muss
fn rule_ahb_55168_dtm_cond_1(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z40"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "76"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment DTM (DE[0]=\"76\") is missing for Pruefidentifikator 55168 (I: when BGM DE[0]=\"Z40\" is present)".to_owned(),
                )
                .with_rule_id("AHB-55168-DTM-I1")
                .with_segment("DTM".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "55168".to_owned()));
    }
}

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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55001")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55001-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55001-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55001-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55001-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55001-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55001-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55001-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55001-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55001-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55001-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55001", "55001", issues);
            })
            .with_named_rule_fn("AHB-55001-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55002")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55002-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55002-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55002-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55002-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55002-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55002-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55002-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55002-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55002-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55002-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55002", "55002", issues);
            })
            .with_named_rule_fn("AHB-55002-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55003")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55003-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55003-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55003-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55003-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55003-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55003-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55003-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55003-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55003-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55003-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55003", "55003", issues);
            })
            .with_named_rule_fn("AHB-55003-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55004")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55004-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55004-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55004-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55004-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55004-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55004-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55004-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55004-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55004-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55004-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55004", "55004", issues);
            })
            .with_named_rule_fn("AHB-55004-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55005")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55005-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55005-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55005-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55005-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55005-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55005-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55005-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55005-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55005-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55005-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55005", "55005", issues);
            })
            .with_named_rule_fn("AHB-55005-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55006")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55006-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55006-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55006-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55006-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55006-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55006-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55006-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55006-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55006-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55006-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55006", "55006", issues);
            })
            .with_named_rule_fn("AHB-55006-RFF-1153-Q", |segs, issues| {
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

static AHB_55007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55007")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55007-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55007-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55007-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02', 'Z90']", |q| matches!(q, "E02" | "Z90"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55007-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55007-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55007-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55007-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55007-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55007-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55007-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55007-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55007-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55007-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55007-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55007", "55007", issues);
            })
            .with_named_rule_fn("AHB-55007-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55007-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z22', 'Z21']", |q| matches!(q, "Z16" | "Z22" | "Z21"), "55007", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55007-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55007-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55007-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55007", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55007-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55007-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55007-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55007", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55007-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55007-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55007-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55007", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55007-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55007-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55007-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55007", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55007-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55007-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55007-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z22', 'Z21']", |q| matches!(q, "Z16" | "Z22" | "Z21"), "55007", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55007_PACK)
}

static AHB_55008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55008")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55008-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55008-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55008-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55008-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55008-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55008-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55008-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55008-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55008-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55008-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55008-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55008-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55008", "55008", issues);
            })
            .with_named_rule_fn("AHB-55008-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55008-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55008", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55008-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55008-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55008-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55008", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55008-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55008-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55008-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55008", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55008-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55008-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55008-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55008", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55008-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55008-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55008-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55008", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55008_PACK)
}

static AHB_55009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55009")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55009-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55009-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55009-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55009-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55009-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55009-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55009-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55009-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55009-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55009-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55009-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55009-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55009", "55009", issues);
            })
            .with_named_rule_fn("AHB-55009-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55009-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55009", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55009-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55009-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55009-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55009", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55009-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55009-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55009-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55009", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55009-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55009-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55009-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55009", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55009-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55009-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55009-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55009", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55009_PACK)
}

static AHB_55010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55010")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55010-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55010-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55010", "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55010-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55010-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55010", "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55010-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55010-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55010", "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55010-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55010-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55010", "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55010-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55010-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55010", "55010", issues);
            })
            .with_named_rule_fn("AHB-55010-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55010-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55010", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55010-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55010-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55010-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55010", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55010-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55010-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55010-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55010", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55010_PACK)
}

static AHB_55011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55011")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55011-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55011-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55011", "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55011-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55011-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55011", "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55011-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55011-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55011", "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55011-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55011-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55011", "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55011-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55011-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55011", "55011", issues);
            })
            .with_named_rule_fn("AHB-55011-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55011-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55011", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55011-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55011-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55011-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55011", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55011-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55011-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55011-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55011", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55011_PACK)
}

static AHB_55012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55012")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55012-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55012-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55012", "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55012-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55012-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55012", "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55012-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55012-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55012", "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55012-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55012-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55012", "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55012-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55012-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55012", "55012", issues);
            })
            .with_named_rule_fn("AHB-55012-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55012-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55012", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55012-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55012-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55012-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55012", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55012-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55012-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55012-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55012", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55012_PACK)
}

static AHB_55013_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55013")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55013-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55013-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55013", "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55013-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55013-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55013", "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55013-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55013-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55013", "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55013-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55013-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55013", "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55013-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55013-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55013", "55013", issues);
            })
            .with_named_rule_fn("AHB-55013-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55013-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55013", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55013-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55013-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55013-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55013", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55013-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55013-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55013-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55013", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55013_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55013_PACK)
}

static AHB_55014_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55014")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55014-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55014-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55014", "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55014-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55014-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55014", "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55014-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55014-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55014", "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55014-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55014-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55014", "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55014-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55014-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55014", "55014", issues);
            })
            .with_named_rule_fn("AHB-55014-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55014-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55014", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55014-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55014-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55014-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55014", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55014-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55014-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55014-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55014", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55014_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55014_PACK)
}

static AHB_55015_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55015")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55015-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55015-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55015", "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55015-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55015-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55015", "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55015-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55015-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55015", "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55015-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55015-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55015", "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55015-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55015-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55015", "55015", issues);
            })
            .with_named_rule_fn("AHB-55015-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55015-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55015", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55015-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55015-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55015-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55015", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55015-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55015-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55015-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55015", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55015_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55015_PACK)
}

static AHB_55016_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55016")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55016-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55016-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55016-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55016-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55016-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55016-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55016-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55016-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55016-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55016-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55016", "55016", issues);
            })
            .with_named_rule_fn("AHB-55016-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55017")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55017-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55017-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55017-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55017-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55017-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55017-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55017-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55017-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55017-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55017-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55017", "55017", issues);
            })
            .with_named_rule_fn("AHB-55017-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55018")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55018-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55018-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55018-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55018-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55018-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55018-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55018-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55018-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55018-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55018-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55018", "55018", issues);
            })
            .with_named_rule_fn("AHB-55018-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55022")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55022-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55022-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55022-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55022-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55022-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55022-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55022-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55022-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55022-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55022-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55022-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55022", "55022", issues);
            })
            .with_named_rule_fn("AHB-55022-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55023")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55023-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55023-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55023-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55023-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55023-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55023-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55023-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55023-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55023-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55023-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55023-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55023", "55023", issues);
            })
            .with_named_rule_fn("AHB-55023-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55024")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55024-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55024-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55024-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'E02', 'E35']", |q| matches!(q, "E01" | "E02" | "E35"), "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55024-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55024-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55024-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55024-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55024-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55024-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55024-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55024-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55024", "55024", issues);
            })
            .with_named_rule_fn("AHB-55024-RFF-1153-Q", |segs, issues| {
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

static AHB_55036_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55036")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55036-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55036-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55036-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55036-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55036-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55036-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55036-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR', 'VY']", |q| matches!(q, "MS" | "MR" | "VY"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-NAD-3035-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "NAD", "AHB-55036-NAD-3035-RQ", "mandatory segment NAD with DE 3035 qualifier 'MS', 'MR' is missing", |q| matches!(q, "MS" | "MR"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55036-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55036-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55036-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55036-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55036-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55036-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13', 'TN']", |q| matches!(q, "Z13" | "TN"), "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55036-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55036", "55036", issues);
            })
            .with_named_rule_fn("AHB-55036-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55036-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55036", issues);
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55036-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55036-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55036-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55036", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55036-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55036-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55036-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55036", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55036-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55036-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55036-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55036", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55036-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55036-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55036-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13', 'TN']", |q| matches!(q, "Z13" | "TN"), "55036", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG12", "NAD", "AHB-55036-SG12-NAD-M")
            .with_named_group_rule_fn("AHB-55036-SG12-PRESENT", |group, _segs, _ctx, issues| {
                if group.definition == "ROOT" && group.find("SG12").next().is_none() {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, "mandatory segment group SG12 is missing for Pruefidentifikator 55036".to_owned())
                            .with_rule_id("AHB-55036-SG12-PRESENT")
                            .with_segment("NAD")
                            .with_context_entry("pid", "55036"),
                    );
                }
            })
            .with_scoped_group_rule_fn("SG12", "AHB-55036-SG12-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55036-SG12-NAD-3035-Q", "in group SG12: segment NAD DE 3035 qualifier is not one of ['VY']", |q| matches!(q, "VY"), "55036", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55036_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55036_PACK)
}

static AHB_55037_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55037")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55037-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55037-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55037-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55037-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55037-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55037-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55037-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55037-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55037-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55037-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55037-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55037-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55037-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55037-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55037", "55037", issues);
            })
            .with_named_rule_fn("AHB-55037-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55037-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55037", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55037-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55037-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55037-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "DTM", "AHB-55037-SG4-DTM-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55037-SG4-DTM-2005-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "DTM", "AHB-55037-SG4-DTM-2005-Q", "in group SG4: segment DTM DE 2005 qualifier is not one of ['93']", |q| matches!(q, "93"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55037-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55037-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55037-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55037-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55037-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55037-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55037-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55037-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55037-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55037-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55037-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55037-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55037", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55037_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55037_PACK)
}

static AHB_55038_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55038")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55038-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55038-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55038-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55038-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55038-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55038-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55038-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR', 'VY']", |q| matches!(q, "MS" | "MR" | "VY"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-NAD-3035-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "NAD", "AHB-55038-NAD-3035-RQ", "mandatory segment NAD with DE 3035 qualifier 'MS', 'MR' is missing", |q| matches!(q, "MS" | "MR"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55038-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55038-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55038-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55038-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55038-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55038-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55038-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55038", "55038", issues);
            })
            .with_named_rule_fn("AHB-55038-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55038-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55038", issues);
            })
            .require_segment_in_group("SG4", "DTM", "AHB-55038-SG4-DTM-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55038-SG4-DTM-2005-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "DTM", "AHB-55038-SG4-DTM-2005-Q", "in group SG4: segment DTM DE 2005 qualifier is not one of ['92']", |q| matches!(q, "92"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55038-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55038-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55038-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55038-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55038-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55038-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55038-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55038-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55038-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55038-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55038-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55038-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_named_group_rule_fn("AHB-55038-SG12-PRESENT", |group, _segs, _ctx, issues| {
                if group.definition == "ROOT" && group.find("SG12").next().is_none() {
                    issues.push(
                        ValidationIssue::new(ValidationSeverity::Error, "mandatory segment group SG12 is missing for Pruefidentifikator 55038".to_owned())
                            .with_rule_id("AHB-55038-SG12-PRESENT")
                            .with_segment("NAD")
                            .with_context_entry("pid", "55038"),
                    );
                }
            })
            .with_scoped_group_rule_fn("SG12", "AHB-55038-SG12-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55038-SG12-NAD-3035-Q", "in group SG12: segment NAD DE 3035 qualifier is not one of ['VY']", |q| matches!(q, "VY"), "55038", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55038_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55038_PACK)
}

static AHB_55039_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55039")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55039-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55039-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55039-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55039-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55039-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55039-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55039-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55039-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55039-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55039-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55039", "55039", issues);
            })
            .with_named_rule_fn("AHB-55039-RFF-1153-Q", |segs, issues| {
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

static AHB_55040_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55040")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55040-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55040-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55040-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55040-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55040-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55040-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55040-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55040-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55040-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55040-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55040-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55040-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55040-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55040", "55040", issues);
            })
            .with_named_rule_fn("AHB-55040-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55040-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55040", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55040-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55040-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55040-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55040", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55040-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55040-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55040-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55040", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55040_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55040_PACK)
}

static AHB_55041_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55041")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55041-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55041-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55041-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E35']", |q| matches!(q, "E35"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55041-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55041-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55041-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55041-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55041-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55041-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55041-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55041-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55041-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55041-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55041", "55041", issues);
            })
            .with_named_rule_fn("AHB-55041-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55041-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55041", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55041-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55041-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55041-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55041", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55041-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55041-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55041-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55041", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55041_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55041_PACK)
}

static AHB_55042_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55042")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55042-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55042-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55042-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55042-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55042-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55042-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55042-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55042-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55042-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55042-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55042", "55042", issues);
            })
            .with_named_rule_fn("AHB-55042-RFF-1153-Q", |segs, issues| {
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

static AHB_55043_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55043")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55043-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55043-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55043-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55043-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55043-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55043-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55043-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55043-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55043-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55043-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55043-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55043-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55043-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55043", "55043", issues);
            })
            .with_named_rule_fn("AHB-55043-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55043-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55043", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55043-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55043-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55043-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55043", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55043-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55043-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55043-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55043", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55043_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55043_PACK)
}

static AHB_55044_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55044")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55044-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55044-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55044-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55044-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55044-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55044-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55044-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55044-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55044-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55044-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55044-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55044-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55044-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55044", "55044", issues);
            })
            .with_named_rule_fn("AHB-55044-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55044-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55044", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55044-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55044-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55044-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55044", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55044-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55044-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55044-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55044", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55044_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55044_PACK)
}

static AHB_55051_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55051")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55051-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55051-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55051-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55051-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55051-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55051-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55051-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55051-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55051-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55051-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55051", "55051", issues);
            })
            .with_named_rule_fn("AHB-55051-RFF-1153-Q", |segs, issues| {
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

static AHB_55052_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55052")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55052-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55052-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55052-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55052-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55052-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55052-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55052-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55052-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55052-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55052-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55052-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55052-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55052-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55052", "55052", issues);
            })
            .with_named_rule_fn("AHB-55052-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55052-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55052", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55052-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55052-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55052-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55052", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55052-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55052-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55052-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55052", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55052_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55052_PACK)
}

static AHB_55053_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55053")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55053-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55053-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55053-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55053-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55053-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55053-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55053-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55053-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55053-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55053-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55053-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55053-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55053-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55053", "55053", issues);
            })
            .with_named_rule_fn("AHB-55053-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55053-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55053", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55053-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55053-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55053-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55053", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55053-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55053-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55053-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55053", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55053_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55053_PACK)
}

static AHB_55065_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55065")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55065-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55065-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_rule_fn("AHB-55065-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55065-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55065", issues);
            })
            .with_named_rule_fn("AHB-55065-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55065-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_rule_fn("AHB-55065-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55065-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55065", issues);
            })
            .with_named_rule_fn("AHB-55065-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55065-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55065", "55065", issues);
            })
            .with_named_rule_fn("AHB-55065-NAD-3035-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55069")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55069-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55069-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_rule_fn("AHB-55069-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55069-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55069", issues);
            })
            .with_named_rule_fn("AHB-55069-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55069-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_rule_fn("AHB-55069-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55069-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55069", issues);
            })
            .with_named_rule_fn("AHB-55069-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55069-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55069", "55069", issues);
            })
            .with_named_rule_fn("AHB-55069-NAD-3035-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55070")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55070-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55070-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_rule_fn("AHB-55070-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55070-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z05']", |q| matches!(q, "Z05"), "55070", issues);
            })
            .with_named_rule_fn("AHB-55070-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55070-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_rule_fn("AHB-55070-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55070-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55070", issues);
            })
            .with_named_rule_fn("AHB-55070-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55070-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55070", "55070", issues);
            })
            .with_named_rule_fn("AHB-55070-NAD-3035-Q", |segs, issues| {
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

static AHB_55109_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55109")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55109-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55109-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55109-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55109-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55109-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55109-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55109-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55109-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55109-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55109-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55109-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55109-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55109", "55109", issues);
            })
            .with_named_rule_fn("AHB-55109-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55109-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55109", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55109-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55109-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55109-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55109", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55109-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55109-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55109-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55109", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55109_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55109_PACK)
}

static AHB_55110_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55110")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55110-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55110-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55110-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55110-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55110-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55110-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55110-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55110-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55110-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55110-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55110-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55110-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55110", "55110", issues);
            })
            .with_named_rule_fn("AHB-55110-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55110-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55110", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55110-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55110-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55110-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55110", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55110-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55110-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55110-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55110", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55110_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55110_PACK)
}

static AHB_55136_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55136")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55136-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55136-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55136-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55136-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55136-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55136-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55136-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55136-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55136-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55136-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55136-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55136-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55136", "55136", issues);
            })
            .with_named_rule_fn("AHB-55136-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55136-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55136", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55136-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55136-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55136-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55136", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55136-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55136-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55136-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55136", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55136_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55136_PACK)
}

static AHB_55137_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55137")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55137-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55137-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55137-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55137-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55137-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55137-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55137-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55137-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55137-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55137-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55137-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55137-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55137", "55137", issues);
            })
            .with_named_rule_fn("AHB-55137-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55137-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55137", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55137-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55137-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55137-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55137", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55137-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55137-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55137-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55137", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55137_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55137_PACK)
}

static AHB_55168_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55168")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55168-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55168-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55168-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01', 'Z40']", |q| matches!(q, "E01" | "Z40"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55168-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_rule_fn(rule_ahb_55168_dtm_cond_0)
            .with_rule_fn(rule_ahb_55168_dtm_cond_1)
            .with_named_rule_fn("AHB-55168-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55168-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55168-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55168-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55168-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55168-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55168-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55168-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55168-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55168-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z17', 'Z18', 'Z19', 'Z20', 'Z21', 'Z22']", |q| matches!(q, "Z16" | "Z17" | "Z18" | "Z19" | "Z20" | "Z21" | "Z22"), "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55168-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55168", "55168", issues);
            })
            .with_named_rule_fn("AHB-55168-RFF-1153-Q", |segs, issues| {
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

static AHB_55169_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55169")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55169-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55169-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55169-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55169-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55169-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55169-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55169-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55169-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55169-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55169-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55169-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55169-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55169-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55169-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z17', 'Z18', 'Z19', 'Z20', 'Z21', 'Z22']", |q| matches!(q, "Z16" | "Z17" | "Z18" | "Z19" | "Z20" | "Z21" | "Z22"), "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55169-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55169", "55169", issues);
            })
            .with_named_rule_fn("AHB-55169-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55169-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55169", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55169-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55169-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55169-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55169", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55169-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55169-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55169-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55169", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55169_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55169_PACK)
}

static AHB_55170_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55170")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55170-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55170-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55170-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55170-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55170-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55170-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55170-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55170-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55170-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55170-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55170-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-STS-9015-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "STS", "AHB-55170-STS-9015-RQ", "mandatory segment STS with DE 9015 qualifier 'E01' is missing", |q| matches!(q, "E01"), "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55170-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55170", "55170", issues);
            })
            .with_named_rule_fn("AHB-55170-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55170-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55170", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55170-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55170-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55170-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55170", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55170-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55170-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55170-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55170", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55170_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55170_PACK)
}

static AHB_55555_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55555")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55555-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55555-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55555-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55555-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55555-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55555-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55555-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55555-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55555-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55555-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_rule_fn(rule_ahb_55555_sts_cond_0)
            .with_named_rule_fn("AHB-55555-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55555-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['E07', 'E08']", |q| matches!(q, "E07" | "E08"), "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55555-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55555", "55555", issues);
            })
            .with_named_rule_fn("AHB-55555-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55600")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55600-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55600-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55600-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55600-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55600-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55600-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55600-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55600-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55600-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55600-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55600", "55600", issues);
            })
            .with_named_rule_fn("AHB-55600-RFF-1153-Q", |segs, issues| {
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
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55601")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55601-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55601-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55601-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55601-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55601-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55601-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55601-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55601-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55601-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55601-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55601", "55601", issues);
            })
            .with_named_rule_fn("AHB-55601-RFF-1153-Q", |segs, issues| {
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

static AHB_55607_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55607")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55607-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55607-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55607-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55607-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55607-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55607-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55607-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55607-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55607-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55607-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55607-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55607-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55607-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55607-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55607-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55607", issues);
            })
            .with_named_rule_fn("AHB-55607-CCI-M", |segs, issues| {
                ahb_check_mandatory(segs, "CCI", "AHB-55607-CCI-M", "mandatory segment CCI is missing for Pruefidentifikator 55607", "55607", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55607-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55607-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55607-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55607", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55607-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55607-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55607-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55607", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55607-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55607-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55607-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55607", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55607-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55607-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55607-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55607", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55607-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55607-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55607-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z21']", |q| matches!(q, "Z16" | "Z21"), "55607", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG10", "CCI", "AHB-55607-SG10-CCI-M")
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55607_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55607_PACK)
}

static AHB_55608_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55608")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55608-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55608-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55608-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55608-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55608-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55608-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55608-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55608-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55608-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55608-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55608-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55608-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55608", "55608", issues);
            })
            .with_named_rule_fn("AHB-55608-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55608-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55608", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55608-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55608-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55608-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55608", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55608-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55608-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55608-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55608", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55608-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55608-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55608-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55608", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55608-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55608-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55608-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55608", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55608_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55608_PACK)
}

static AHB_55609_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55609")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55609-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55609-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55609-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55609-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55609-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55609-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55609-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55609-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55609-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55609-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55609-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55609-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55609", "55609", issues);
            })
            .with_named_rule_fn("AHB-55609-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55609-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55609", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55609-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55609-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55609-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55609", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55609-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55609-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55609-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55609", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55609-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55609-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55609-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7', 'E01']", |q| matches!(q, "7" | "E01"), "55609", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55609-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55609-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55609-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55609", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55609_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55609_PACK)
}

static AHB_55611_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55611")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55611-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55611-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55611-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E02']", |q| matches!(q, "E02"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55611-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55611-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55611-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55611-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55611-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55611-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-55611-STS-M", "mandatory segment STS is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-55611-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55611-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55611-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55611-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55611", "55611", issues);
            })
            .with_named_rule_fn("AHB-55611-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-55611-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z16', 'Z17']", |q| matches!(q, "Z16" | "Z17"), "55611", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55611-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55611-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55611-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "DTM", "AHB-55611-SG4-DTM-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55611-SG4-DTM-2005-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "DTM", "AHB-55611-SG4-DTM-2005-Q", "in group SG4: segment DTM DE 2005 qualifier is not one of ['92', '93']", |q| matches!(q, "92" | "93"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "IDE", "AHB-55611-SG4-IDE-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55611-SG4-IDE-7495-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "IDE", "AHB-55611-SG4-IDE-7495-Q", "in group SG4: segment IDE DE 7495 qualifier is not one of ['24']", |q| matches!(q, "24"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG4", "STS", "AHB-55611-SG4-STS-M")
            .with_scoped_group_rule_fn("SG4", "AHB-55611-SG4-STS-9015-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "STS", "AHB-55611-SG4-STS-9015-Q", "in group SG4: segment STS DE 9015 qualifier is not one of ['7']", |q| matches!(q, "7"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG5", "LOC", "AHB-55611-SG5-LOC-M")
            .with_scoped_group_rule_fn("SG5", "AHB-55611-SG5-LOC-3227-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "LOC", "AHB-55611-SG5-LOC-3227-Q", "in group SG5: segment LOC DE 3227 qualifier is not one of ['Z16', 'Z17']", |q| matches!(q, "Z16" | "Z17"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55611-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55611-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55611-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55611", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55611_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55611_PACK)
}

static AHB_55615_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55615")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55615-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55615-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55615-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55615-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55615-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55615-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55615-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55615-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55615-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55615-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55615-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55615-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55615", "55615", issues);
            })
            .with_named_rule_fn("AHB-55615-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55615-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55615", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55615-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55615-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55615-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55615", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55615-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55615-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55615-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55615", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55615_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55615_PACK)
}

static AHB_55616_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55616")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55616-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55616-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55616-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55616-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55616-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55616-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55616-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55616-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55616-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55616-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55616-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55616-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55616", "55616", issues);
            })
            .with_named_rule_fn("AHB-55616-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55616-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55616", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55616-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55616-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55616-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55616", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55616-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55616-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55616-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55616", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55616_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55616_PACK)
}

static AHB_55617_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55617")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55617-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55617-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55617-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55617-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55617-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55617-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55617-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55617-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55617-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55617-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55617-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55617-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55617", "55617", issues);
            })
            .with_named_rule_fn("AHB-55617-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55617-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55617", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55617-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55617-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55617-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55617", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55617-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55617-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55617-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55617", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55617_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55617_PACK)
}

static AHB_55618_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55618")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55618-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55618-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55618-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55618-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55618-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55618-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55618-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55618-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55618-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55618-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55618-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55618-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55618", "55618", issues);
            })
            .with_named_rule_fn("AHB-55618-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55618-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55618", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55618-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55618-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55618-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55618", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55618-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55618-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55618-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55618", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55618_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55618_PACK)
}

static AHB_55619_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55619")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55619-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55619-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55619-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55619-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55619-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55619-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55619-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55619-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55619-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55619-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55619-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55619-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55619", "55619", issues);
            })
            .with_named_rule_fn("AHB-55619-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55619-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55619", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55619-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55619-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55619-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55619", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55619-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55619-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55619-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55619", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55619_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55619_PACK)
}

static AHB_55620_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55620")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55620-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55620-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55620-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55620-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55620-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55620-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55620-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55620-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55620-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55620-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55620-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55620-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55620", "55620", issues);
            })
            .with_named_rule_fn("AHB-55620-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55620-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55620", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55620-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55620-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55620-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55620", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55620-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55620-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55620-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55620", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55620_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55620_PACK)
}

static AHB_55621_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55621")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55621-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55621-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55621-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55621-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55621-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55621-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55621-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55621-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55621-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55621-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55621-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55621-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55621", "55621", issues);
            })
            .with_named_rule_fn("AHB-55621-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55621-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55621", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55621-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55621-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55621-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55621", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55621-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55621-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55621-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55621", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55621_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55621_PACK)
}

static AHB_55622_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55622")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55622-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55622-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55622-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55622-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55622-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55622-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55622-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55622-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55622-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55622-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55622-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55622-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55622", "55622", issues);
            })
            .with_named_rule_fn("AHB-55622-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55622-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55622", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55622-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55622-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55622-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55622", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55622-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55622-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55622-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55622", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55622_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55622_PACK)
}

static AHB_55623_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55623")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55623-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55623-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55623-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55623-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55623-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55623-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55623-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55623-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55623-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55623-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55623-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55623-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55623", "55623", issues);
            })
            .with_named_rule_fn("AHB-55623-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55623-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55623", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55623-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55623-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55623-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55623", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55623-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55623-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55623-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55623", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55623_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55623_PACK)
}

static AHB_55624_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55624")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55624-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55624-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55624-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55624-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55624-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55624-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55624-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55624-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55624-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55624-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55624-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55624-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55624", "55624", issues);
            })
            .with_named_rule_fn("AHB-55624-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55624-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55624", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55624-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55624-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55624-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55624", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55624-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55624-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55624-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55624", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55624_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55624_PACK)
}

static AHB_55625_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55625")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55625-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55625-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55625-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55625-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55625-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55625-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55625-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55625-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55625-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55625-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55625-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55625-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55625", "55625", issues);
            })
            .with_named_rule_fn("AHB-55625-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55625-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55625", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55625-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55625-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55625-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55625", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55625-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55625-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55625-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55625", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55625_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55625_PACK)
}

static AHB_55626_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55626")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55626-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55626-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55626-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55626-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55626-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55626-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55626-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55626-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55626-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55626-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55626-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55626-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55626", "55626", issues);
            })
            .with_named_rule_fn("AHB-55626-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55626-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55626", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55626-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55626-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55626-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55626", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55626-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55626-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55626-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55626", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55626_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55626_PACK)
}

static AHB_55627_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55627")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55627-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55627-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55627-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55627-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55627-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55627-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55627-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55627-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55627-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55627-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55627-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55627-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55627", "55627", issues);
            })
            .with_named_rule_fn("AHB-55627-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55627-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55627", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55627-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55627-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55627-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55627", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55627-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55627-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55627-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55627", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55627_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55627_PACK)
}

static AHB_55628_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55628")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55628-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55628-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55628-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55628-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55628-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55628-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55628-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55628-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55628-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55628-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55628-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55628-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55628", "55628", issues);
            })
            .with_named_rule_fn("AHB-55628-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55628-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55628", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55628-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55628-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55628-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55628", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55628-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55628-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55628-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55628", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55628_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55628_PACK)
}

static AHB_55629_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55629")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55629-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55629-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55629-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55629-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55629-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55629-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55629-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55629-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55629-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55629-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55629-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55629-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55629", "55629", issues);
            })
            .with_named_rule_fn("AHB-55629-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55629-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55629", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55629-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55629-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55629-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55629", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55629-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55629-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55629-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55629", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55629_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55629_PACK)
}

static AHB_55630_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55630")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55630-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55630-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55630-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55630-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55630-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55630-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55630-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55630-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55630-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55630-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55630-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55630-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55630", "55630", issues);
            })
            .with_named_rule_fn("AHB-55630-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55630-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55630", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55630-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55630-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55630-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55630", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55630-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55630-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55630-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55630", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55630_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55630_PACK)
}

static AHB_55632_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55632")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55632-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55632-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55632-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55632-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55632-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55632-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55632-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55632-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55632-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55632-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55632-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55632-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55632", "55632", issues);
            })
            .with_named_rule_fn("AHB-55632-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55632-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55632", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55632-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55632-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55632-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55632", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55632-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55632-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55632-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55632", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55632_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55632_PACK)
}

static AHB_55633_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55633")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55633-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55633-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55633-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55633-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55633-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55633-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55633-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55633-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55633-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55633-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55633-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55633-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55633", "55633", issues);
            })
            .with_named_rule_fn("AHB-55633-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55633-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55633", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55633-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55633-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55633-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55633", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55633-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55633-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55633-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55633", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55633_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55633_PACK)
}

static AHB_55634_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55634")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55634-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55634-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55634-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55634-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55634-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55634-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55634-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55634-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55634-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55634-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55634-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55634-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55634", "55634", issues);
            })
            .with_named_rule_fn("AHB-55634-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55634-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55634", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55634-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55634-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55634-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55634", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55634-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55634-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55634-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55634", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55634_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55634_PACK)
}

static AHB_55635_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55635")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55635-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55635-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55635-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55635-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55635-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55635-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55635-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55635-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55635-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55635-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55635-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55635-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55635", "55635", issues);
            })
            .with_named_rule_fn("AHB-55635-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55635-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55635", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55635-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55635-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55635-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55635", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55635-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55635-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55635-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55635", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55635_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55635_PACK)
}

static AHB_55636_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55636")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55636-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55636-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55636-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55636-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55636-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55636-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55636-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55636-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55636-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55636-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55636-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55636-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55636", "55636", issues);
            })
            .with_named_rule_fn("AHB-55636-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55636-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55636", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55636-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55636-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55636-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55636", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55636-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55636-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55636-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55636", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55636_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55636_PACK)
}

static AHB_55638_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55638")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55638-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55638-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55638-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55638-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55638-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55638-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55638-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55638-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55638-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55638-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55638-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55638-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55638", "55638", issues);
            })
            .with_named_rule_fn("AHB-55638-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55638-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55638", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55638-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55638-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55638-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55638", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55638-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55638-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55638-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55638", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55638_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55638_PACK)
}

static AHB_55639_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55639")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55639-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55639-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55639-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55639-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55639-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55639-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55639-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55639-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55639-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55639-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55639-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55639-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55639", "55639", issues);
            })
            .with_named_rule_fn("AHB-55639-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55639-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55639", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55639-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55639-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55639-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55639", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55639-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55639-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55639-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55639", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55639_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55639_PACK)
}

static AHB_55640_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55640")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55640-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55640-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55640-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55640-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55640-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55640-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55640-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55640-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55640-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55640-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55640-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55640-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55640", "55640", issues);
            })
            .with_named_rule_fn("AHB-55640-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55640-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55640", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55640-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55640-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55640-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55640", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55640-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55640-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55640-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55640", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55640_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55640_PACK)
}

static AHB_55641_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55641")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55641-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55641-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55641-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55641-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55641-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55641-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55641-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55641-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55641-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55641-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55641-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55641-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55641", "55641", issues);
            })
            .with_named_rule_fn("AHB-55641-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55641-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55641", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55641-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55641-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55641-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55641", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55641-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55641-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55641-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55641", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55641_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55641_PACK)
}

static AHB_55642_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55642")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55642-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55642-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55642-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55642-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55642-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55642-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55642-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55642-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55642-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55642-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55642-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55642-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55642", "55642", issues);
            })
            .with_named_rule_fn("AHB-55642-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55642-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55642", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55642-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55642-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55642-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55642", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55642-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55642-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55642-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55642", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55642_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55642_PACK)
}

static AHB_55643_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55643")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55643-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55643-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55643-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55643-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55643-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55643-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55643-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55643-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55643-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55643-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55643-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55643-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55643", "55643", issues);
            })
            .with_named_rule_fn("AHB-55643-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55643-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55643", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55643-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55643-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55643-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55643", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55643-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55643-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55643-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55643", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55643_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55643_PACK)
}

static AHB_55644_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55644")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55644-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55644-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55644-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55644-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55644-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55644-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55644-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55644-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55644-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55644-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55644-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55644-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55644", "55644", issues);
            })
            .with_named_rule_fn("AHB-55644-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55644-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55644", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55644-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55644-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55644-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55644", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55644-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55644-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55644-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55644", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55644_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55644_PACK)
}

static AHB_55645_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55645")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55645-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55645-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55645-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55645-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55645-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55645-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55645-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55645-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55645-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55645-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55645-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55645-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55645", "55645", issues);
            })
            .with_named_rule_fn("AHB-55645-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55645-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55645", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55645-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55645-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55645-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55645", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55645-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55645-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55645-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55645", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55645_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55645_PACK)
}

static AHB_55646_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55646")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55646-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55646-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55646-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55646-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55646-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55646-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55646-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55646-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55646-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55646-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55646-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55646-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55646", "55646", issues);
            })
            .with_named_rule_fn("AHB-55646-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55646-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55646", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55646-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55646-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55646-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55646", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55646-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55646-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55646-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55646", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55646_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55646_PACK)
}

static AHB_55647_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55647")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55647-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55647-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55647-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55647-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55647-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55647-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55647-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55647-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55647-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55647-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55647-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55647-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55647", "55647", issues);
            })
            .with_named_rule_fn("AHB-55647-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55647-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55647", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55647-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55647-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55647-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55647", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55647-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55647-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55647-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55647", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55647_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55647_PACK)
}

static AHB_55648_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55648")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55648-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55648-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55648-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55648-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55648-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55648-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55648-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55648-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55648-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55648-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55648-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55648-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55648", "55648", issues);
            })
            .with_named_rule_fn("AHB-55648-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55648-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55648", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55648-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55648-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55648-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55648", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55648-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55648-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55648-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55648", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55648_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55648_PACK)
}

static AHB_55649_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55649")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55649-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55649-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55649-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55649-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55649-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55649-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55649-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55649-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55649-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55649-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55649-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55649-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55649", "55649", issues);
            })
            .with_named_rule_fn("AHB-55649-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55649-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55649", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55649-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55649-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55649-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55649", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55649-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55649-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55649-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55649", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55649_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55649_PACK)
}

static AHB_55650_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55650")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55650-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55650-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55650-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55650-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55650-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55650-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55650-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55650-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55650-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55650-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55650-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55650-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55650", "55650", issues);
            })
            .with_named_rule_fn("AHB-55650-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55650-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55650", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55650-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55650-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55650-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55650", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55650-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55650-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55650-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55650", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55650_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55650_PACK)
}

static AHB_55651_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55651")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55651-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55651-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55651-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55651-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55651-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55651-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55651-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55651-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55651-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55651-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55651-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55651-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55651", "55651", issues);
            })
            .with_named_rule_fn("AHB-55651-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55651-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55651", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55651-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55651-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55651-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55651", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55651-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55651-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55651-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55651", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55651_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55651_PACK)
}

static AHB_55652_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55652")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55652-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55652-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55652-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55652-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55652-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55652-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55652-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55652-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55652-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55652-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55652-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55652-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55652", "55652", issues);
            })
            .with_named_rule_fn("AHB-55652-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55652-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55652", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55652-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55652-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55652-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55652", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55652-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55652-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55652-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55652", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55652_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55652_PACK)
}

static AHB_55653_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55653")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55653-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55653-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55653-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55653-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55653-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55653-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55653-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55653-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55653-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55653-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55653-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55653-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55653", "55653", issues);
            })
            .with_named_rule_fn("AHB-55653-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55653-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55653", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55653-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55653-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55653-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55653", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55653-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55653-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55653-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55653", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55653_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55653_PACK)
}

static AHB_55654_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55654")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55654-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55654-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55654-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55654-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55654-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55654-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55654-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55654-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55654-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55654-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55654-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55654-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55654", "55654", issues);
            })
            .with_named_rule_fn("AHB-55654-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55654-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55654", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55654-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55654-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55654-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55654", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55654-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55654-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55654-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55654", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55654_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55654_PACK)
}

static AHB_55655_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55655")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55655-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55655-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55655-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55655-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55655-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55655-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55655-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55655-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55655-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55655-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55655-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55655-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55655", "55655", issues);
            })
            .with_named_rule_fn("AHB-55655-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55655-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55655", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55655-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55655-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55655-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55655", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55655-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55655-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55655-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55655", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55655_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55655_PACK)
}

static AHB_55656_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55656")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55656-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55656-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55656-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55656-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55656-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55656-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55656-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55656-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55656-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55656-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55656-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55656-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55656", "55656", issues);
            })
            .with_named_rule_fn("AHB-55656-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55656-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55656", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55656-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55656-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55656-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55656", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55656-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55656-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55656-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55656", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55656_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55656_PACK)
}

static AHB_55657_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55657")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55657-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55657-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55657-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55657-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55657-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55657-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55657-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55657-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55657-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55657-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55657-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55657-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55657", "55657", issues);
            })
            .with_named_rule_fn("AHB-55657-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55657-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55657", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55657-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55657-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55657-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55657", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55657-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55657-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55657-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55657", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55657_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55657_PACK)
}

static AHB_55658_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55658")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55658-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55658-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55658-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55658-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55658-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55658-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55658-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55658-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55658-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55658-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55658-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55658-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55658", "55658", issues);
            })
            .with_named_rule_fn("AHB-55658-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55658-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55658", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55658-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55658-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55658-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55658", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55658-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55658-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55658-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55658", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55658_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55658_PACK)
}

static AHB_55659_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55659")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55659-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55659-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55659-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55659-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55659-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55659-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55659-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55659-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55659-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55659-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55659-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55659-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55659", "55659", issues);
            })
            .with_named_rule_fn("AHB-55659-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55659-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55659", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55659-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55659-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55659-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55659", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55659-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55659-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55659-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55659", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55659_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55659_PACK)
}

static AHB_55660_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55660")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55660-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55660-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55660-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55660-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55660-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55660-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55660-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55660-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55660-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55660-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55660-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55660-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55660", "55660", issues);
            })
            .with_named_rule_fn("AHB-55660-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55660-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55660", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55660-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55660-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55660-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55660", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55660-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55660-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55660-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55660", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55660_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55660_PACK)
}

static AHB_55661_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55661")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55661-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55661-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55661-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55661-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55661-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55661-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55661-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55661-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55661-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55661-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55661-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55661-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55661", "55661", issues);
            })
            .with_named_rule_fn("AHB-55661-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55661-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55661", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55661-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55661-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55661-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55661", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55661-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55661-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55661-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55661", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55661_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55661_PACK)
}

static AHB_55662_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55662")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55662-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55662-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55662-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55662-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55662-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55662-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55662-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55662-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55662-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55662-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55662-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55662-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55662", "55662", issues);
            })
            .with_named_rule_fn("AHB-55662-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55662-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55662", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55662-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55662-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55662-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55662", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55662-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55662-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55662-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55662", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55662_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55662_PACK)
}

static AHB_55663_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55663")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55663-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55663-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55663-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55663-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55663-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55663-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55663-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55663-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55663-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55663-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55663-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55663-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55663", "55663", issues);
            })
            .with_named_rule_fn("AHB-55663-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55663-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55663", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55663-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55663-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55663-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55663", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55663-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55663-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55663-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55663", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55663_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55663_PACK)
}

static AHB_55664_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55664")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55664-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55664-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55664-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55664-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55664-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55664-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55664-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55664-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55664-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55664-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55664-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z18' is missing", |q| matches!(q, "Z18"), "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55664-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55664", "55664", issues);
            })
            .with_named_rule_fn("AHB-55664-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55664-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55664", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55664-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55664-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55664-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55664", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55664-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55664-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55664-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55664", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55664_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55664_PACK)
}

static AHB_55665_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55665")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55665-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55665-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55665-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55665-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55665-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55665-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55665-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55665-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55665-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55665-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55665-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55665-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55665", "55665", issues);
            })
            .with_named_rule_fn("AHB-55665-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55665-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55665", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55665-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55665-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55665-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55665", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55665-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55665-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55665-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55665", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55665_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55665_PACK)
}

static AHB_55666_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55666")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55666-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55666-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55666-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55666-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55666-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55666-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55666-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55666-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55666-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55666-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55666-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z19' is missing", |q| matches!(q, "Z19"), "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55666-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55666", "55666", issues);
            })
            .with_named_rule_fn("AHB-55666-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55666-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55666", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55666-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55666-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55666-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55666", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55666-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55666-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55666-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55666", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55666_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55666_PACK)
}

static AHB_55667_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55667")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55667-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55667-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55667-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55667-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55667-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55667-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55667-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55667-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55667-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55667-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55667-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55667-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55667", "55667", issues);
            })
            .with_named_rule_fn("AHB-55667-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55667-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55667", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55667-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55667-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55667-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55667", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55667-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55667-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55667-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55667", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55667_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55667_PACK)
}

static AHB_55669_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55669")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55669-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55669-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55669-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55669-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55669-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55669-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55669-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55669-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55669-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55669-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55669-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z17' is missing", |q| matches!(q, "Z17"), "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55669-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55669", "55669", issues);
            })
            .with_named_rule_fn("AHB-55669-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55669-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55669", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55669-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55669-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55669-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55669", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55669-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55669-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55669-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55669", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55669_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55669_PACK)
}

static AHB_55670_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55670")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55670-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55670-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55670-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55670-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55670-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55670-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55670-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55670-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55670-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55670-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55670-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55670-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55670", "55670", issues);
            })
            .with_named_rule_fn("AHB-55670-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55670-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55670", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55670-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55670-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55670-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55670", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55670-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55670-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55670-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55670", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55670_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55670_PACK)
}

static AHB_55671_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55671")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55671-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55671-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55671-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55671-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55671-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55671-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55671-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55671-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55671-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55671-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55671-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55671-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55671", "55671", issues);
            })
            .with_named_rule_fn("AHB-55671-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55671-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55671", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55671-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55671-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55671-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55671", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55671-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55671-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55671-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55671", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55671_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55671_PACK)
}

static AHB_55684_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55684")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55684-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55684-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55684-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55684-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55684-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55684-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55684-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55684-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55684-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55684-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55684-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55684-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55684", "55684", issues);
            })
            .with_named_rule_fn("AHB-55684-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55684-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55684", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55684-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55684-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55684-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55684", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55684-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55684-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55684-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55684", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55684_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55684_PACK)
}

static AHB_55685_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55685")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55685-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55685-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55685-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55685-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55685-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55685-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55685-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55685-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55685-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55685-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55685-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55685-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55685", "55685", issues);
            })
            .with_named_rule_fn("AHB-55685-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55685-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55685", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55685-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55685-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55685-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55685", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55685-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55685-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55685-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55685", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55685_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55685_PACK)
}

static AHB_55686_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55686")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55686-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55686-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55686-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55686-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55686-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55686-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55686-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55686-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55686-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55686-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55686-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55686-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55686", "55686", issues);
            })
            .with_named_rule_fn("AHB-55686-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55686-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55686", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55686-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55686-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55686-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55686", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55686-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55686-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55686-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55686", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55686_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55686_PACK)
}

static AHB_55687_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55687")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55687-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55687-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55687-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55687-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55687-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55687-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55687-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55687-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55687-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55687-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55687-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z21' is missing", |q| matches!(q, "Z21"), "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55687-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55687", "55687", issues);
            })
            .with_named_rule_fn("AHB-55687-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55687-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55687", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55687-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55687-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55687-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55687", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55687-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55687-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55687-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55687", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55687_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55687_PACK)
}

static AHB_55688_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55688")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55688-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55688-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55688-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55688-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55688-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55688-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55688-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55688-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55688-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55688-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55688-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55688-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55688", "55688", issues);
            })
            .with_named_rule_fn("AHB-55688-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55688-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55688", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55688-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55688-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55688-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55688", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55688-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55688-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55688-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55688", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55688_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55688_PACK)
}

static AHB_55689_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55689")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55689-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55689-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55689-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55689-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55689-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55689-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55689-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55689-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55689-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55689-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55689-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55689-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55689", "55689", issues);
            })
            .with_named_rule_fn("AHB-55689-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55689-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55689", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55689-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55689-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55689-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55689", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55689-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55689-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55689-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55689", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55689_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55689_PACK)
}

static AHB_55691_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55691")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55691-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55691-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55691-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55691-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55691-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55691-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55691-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55691-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55691-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55691-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55691-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55691-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55691", "55691", issues);
            })
            .with_named_rule_fn("AHB-55691-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55691-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55691", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55691-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55691-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55691-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55691", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55691-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55691-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55691-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55691", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55691_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55691_PACK)
}

static AHB_55692_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55692")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55692-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55692-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55692-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55692-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55692-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55692-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55692-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55692-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55692-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55692-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55692-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z16' is missing", |q| matches!(q, "Z16"), "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55692-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55692", "55692", issues);
            })
            .with_named_rule_fn("AHB-55692-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55692-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55692", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55692-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55692-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55692-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55692", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55692-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55692-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55692-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55692", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55692_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55692_PACK)
}

static AHB_55693_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55693")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55693-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55693-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55693-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55693-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55693-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55693-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55693-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55693-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55693-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55693-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55693-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55693-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55693", "55693", issues);
            })
            .with_named_rule_fn("AHB-55693-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55693-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55693", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55693-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55693-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55693-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55693", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55693-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55693-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55693-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55693", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55693_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55693_PACK)
}

static AHB_55694_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-S2.2-55694")
            .for_message_type("UTILMD")
            .for_release("S2.2")
            .with_named_rule_fn("AHB-55694-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-55694-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-55694-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-55694-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-55694-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-55694-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-55694-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-55694-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-55694-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['24']", |q| matches!(q, "24"), "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-55694-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-LOC-3227-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "LOC", "AHB-55694-LOC-3227-RQ", "mandatory segment LOC with DE 3227 qualifier 'Z20' is missing", |q| matches!(q, "Z20"), "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-55694-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 55694", "55694", issues);
            })
            .with_named_rule_fn("AHB-55694-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-55694-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "55694", issues);
            })
            .require_segment_in_group("SG2", "NAD", "AHB-55694-SG2-NAD-M")
            .with_scoped_group_rule_fn("SG2", "AHB-55694-SG2-NAD-3035-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "NAD", "AHB-55694-SG2-NAD-3035-Q", "in group SG2: segment NAD DE 3035 qualifier is not one of ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "55694", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .require_segment_in_group("SG6", "RFF", "AHB-55694-SG6-RFF-M")
            .with_scoped_group_rule_fn("SG6", "AHB-55694-SG6-RFF-1153-Q", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                ahb_check_qualifier(segs, "RFF", "AHB-55694-SG6-RFF-1153-Q", "in group SG6: segment RFF DE 1153 qualifier is not one of ['Z13']", |q| matches!(q, "Z13"), "55694", issues);
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_55694_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_55694_PACK)
}

static AHB_ALL_PACK_UTILMD_S2_2: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("UTILMD-AHB-S2.2-ALL")
        .for_message_type("UTILMD")
        .for_release("S2.2");
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
        .merge_with_override(ahb_55007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55013_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55014_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55015_pack().as_ref().clone())
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
        .merge_with_override(ahb_55036_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55037_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55038_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55039_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55040_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55041_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55042_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55043_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55044_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55051_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55052_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55053_pack().as_ref().clone())
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
        .merge_with_override(ahb_55109_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55110_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55136_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55137_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55168_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55169_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55170_pack().as_ref().clone())
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
    let pack = pack
        .merge_with_override(ahb_55607_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55608_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55609_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55611_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55615_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55616_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55617_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55618_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55619_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55620_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55621_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55622_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55623_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55624_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55625_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55626_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55627_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55628_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55629_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55630_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55632_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55633_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55634_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55635_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55636_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55638_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55639_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55640_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55641_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55642_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55643_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55644_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55645_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55646_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55647_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55648_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55649_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55650_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55651_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55652_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55653_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55654_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55655_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55656_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55657_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55658_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55659_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55660_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55661_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55662_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55663_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55664_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55665_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55666_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55667_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55669_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55670_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55671_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55684_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55685_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55686_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55687_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55688_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55689_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55691_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55692_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55693_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_55694_pack().as_ref().clone())
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
            Some(55007) => ahb_55007_pack(),
            Some(55008) => ahb_55008_pack(),
            Some(55009) => ahb_55009_pack(),
            Some(55010) => ahb_55010_pack(),
            Some(55011) => ahb_55011_pack(),
            Some(55012) => ahb_55012_pack(),
            Some(55013) => ahb_55013_pack(),
            Some(55014) => ahb_55014_pack(),
            Some(55015) => ahb_55015_pack(),
            Some(55016) => ahb_55016_pack(),
            Some(55017) => ahb_55017_pack(),
            Some(55018) => ahb_55018_pack(),
            Some(55022) => ahb_55022_pack(),
            Some(55023) => ahb_55023_pack(),
            Some(55024) => ahb_55024_pack(),
            Some(55036) => ahb_55036_pack(),
            Some(55037) => ahb_55037_pack(),
            Some(55038) => ahb_55038_pack(),
            Some(55039) => ahb_55039_pack(),
            Some(55040) => ahb_55040_pack(),
            Some(55041) => ahb_55041_pack(),
            Some(55042) => ahb_55042_pack(),
            Some(55043) => ahb_55043_pack(),
            Some(55044) => ahb_55044_pack(),
            Some(55051) => ahb_55051_pack(),
            Some(55052) => ahb_55052_pack(),
            Some(55053) => ahb_55053_pack(),
            Some(55065) => ahb_55065_pack(),
            Some(55069) => ahb_55069_pack(),
            Some(55070) => ahb_55070_pack(),
            Some(55109) => ahb_55109_pack(),
            Some(55110) => ahb_55110_pack(),
            Some(55136) => ahb_55136_pack(),
            Some(55137) => ahb_55137_pack(),
            Some(55168) => ahb_55168_pack(),
            Some(55169) => ahb_55169_pack(),
            Some(55170) => ahb_55170_pack(),
            Some(55555) => ahb_55555_pack(),
            Some(55600) => ahb_55600_pack(),
            Some(55601) => ahb_55601_pack(),
            Some(55607) => ahb_55607_pack(),
            Some(55608) => ahb_55608_pack(),
            Some(55609) => ahb_55609_pack(),
            Some(55611) => ahb_55611_pack(),
            Some(55615) => ahb_55615_pack(),
            Some(55616) => ahb_55616_pack(),
            Some(55617) => ahb_55617_pack(),
            Some(55618) => ahb_55618_pack(),
            Some(55619) => ahb_55619_pack(),
            Some(55620) => ahb_55620_pack(),
            Some(55621) => ahb_55621_pack(),
            Some(55622) => ahb_55622_pack(),
            Some(55623) => ahb_55623_pack(),
            Some(55624) => ahb_55624_pack(),
            Some(55625) => ahb_55625_pack(),
            Some(55626) => ahb_55626_pack(),
            Some(55627) => ahb_55627_pack(),
            Some(55628) => ahb_55628_pack(),
            Some(55629) => ahb_55629_pack(),
            Some(55630) => ahb_55630_pack(),
            Some(55632) => ahb_55632_pack(),
            Some(55633) => ahb_55633_pack(),
            Some(55634) => ahb_55634_pack(),
            Some(55635) => ahb_55635_pack(),
            Some(55636) => ahb_55636_pack(),
            Some(55638) => ahb_55638_pack(),
            Some(55639) => ahb_55639_pack(),
            Some(55640) => ahb_55640_pack(),
            Some(55641) => ahb_55641_pack(),
            Some(55642) => ahb_55642_pack(),
            Some(55643) => ahb_55643_pack(),
            Some(55644) => ahb_55644_pack(),
            Some(55645) => ahb_55645_pack(),
            Some(55646) => ahb_55646_pack(),
            Some(55647) => ahb_55647_pack(),
            Some(55648) => ahb_55648_pack(),
            Some(55649) => ahb_55649_pack(),
            Some(55650) => ahb_55650_pack(),
            Some(55651) => ahb_55651_pack(),
            Some(55652) => ahb_55652_pack(),
            Some(55653) => ahb_55653_pack(),
            Some(55654) => ahb_55654_pack(),
            Some(55655) => ahb_55655_pack(),
            Some(55656) => ahb_55656_pack(),
            Some(55657) => ahb_55657_pack(),
            Some(55658) => ahb_55658_pack(),
            Some(55659) => ahb_55659_pack(),
            Some(55660) => ahb_55660_pack(),
            Some(55661) => ahb_55661_pack(),
            Some(55662) => ahb_55662_pack(),
            Some(55663) => ahb_55663_pack(),
            Some(55664) => ahb_55664_pack(),
            Some(55665) => ahb_55665_pack(),
            Some(55666) => ahb_55666_pack(),
            Some(55667) => ahb_55667_pack(),
            Some(55669) => ahb_55669_pack(),
            Some(55670) => ahb_55670_pack(),
            Some(55671) => ahb_55671_pack(),
            Some(55684) => ahb_55684_pack(),
            Some(55685) => ahb_55685_pack(),
            Some(55686) => ahb_55686_pack(),
            Some(55687) => ahb_55687_pack(),
            Some(55688) => ahb_55688_pack(),
            Some(55689) => ahb_55689_pack(),
            Some(55691) => ahb_55691_pack(),
            Some(55692) => ahb_55692_pack(),
            Some(55693) => ahb_55693_pack(),
            Some(55694) => ahb_55694_pack(),
            None => Arc::clone(&AHB_ALL_PACK_UTILMD_S2_2),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("UTILMD")
                .with_named_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_UTILMD_FV20261001: LazyLock<Release> = LazyLock::new(|| Release::new("S2.2"));

pub(crate) struct UtilmdFv20261001Profile;

impl Profile for UtilmdFv20261001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Utilmd
    }
    fn release(&self) -> &Release {
        &RELEASE_UTILMD_FV20261001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("S2.2")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("UTILMD MIG S2.2, Stand 01.10.2026")
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
    fn group_schema(&self) -> &'static [GroupDef<'static>] {
        GROUP_SCHEMA
    }
}

pub(crate) static PROFILE: UtilmdFv20261001Profile = UtilmdFv20261001Profile;
