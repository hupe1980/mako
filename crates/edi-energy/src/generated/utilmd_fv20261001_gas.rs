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
            ElementRef::new(3, "1225", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Date/Time/Period",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
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
        name: "Reference",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
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
        tag: "IDE",
        name: "Identity",
        elements: &[
            ElementRef::new(1, "7495", Status::Mandatory, 1),
            ElementRef::new(2, "C206", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "STS",
        name: "Status",
        elements: &[
            ElementRef::new(1, "9015", Status::Mandatory, 1),
            ElementRef::new(2, "9013", Status::Conditional, 1),
            ElementRef::new(3, "9011", Status::Conditional, 1),
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
        tag: "AGR",
        name: "Agreement Identification",
        elements: &[ElementRef::new(1, "C543", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "LOC",
        name: "Place/Location Identification",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
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
        | ("LOC", 0) => Some(1),
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
static DIRECTORY_VALIDATOR_UTILMD_G1_2: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-UTILMD-G1.2",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_UTILMD_G1_2
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
            .with_rule_id("MIG-UTILMD-MIG-G1.2-GROUP-SG1-RFF-CARD-MAX")
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
            .with_rule_id("MIG-UTILMD-MIG-G1.2-GROUP-SG2-NAD-CARD-MAX")
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
            .with_rule_id("MIG-UTILMD-MIG-G1.2-GROUP-SG4-IDE-CARD-MAX")
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
            .with_rule_id("MIG-UTILMD-MIG-G1.2-GROUP-SG2-NAD-CARD-MIN")
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
            .with_rule_id("MIG-UTILMD-MIG-G1.2-GROUP-SG4-IDE-CARD-MIN")
            .with_segment("IDE".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &["UNH", "BGM", "DTM", "RFF", "NAD", "IDE", "UNT"];
    let mut cursor: usize = 0;
    for seg in segments {
        if let Some(pos) = EXPECTED_ORDER[cursor..].iter().position(|&t| t == seg.tag) {
            cursor += pos;
        } else if EXPECTED_ORDER.contains(&seg.tag) {
            // Tag is known but already passed — ordering violation.
            issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "segment appears out of order".to_owned(),
                )
                .with_rule_id("MIG-UTILMD-MIG-G1.2-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_UTILMD_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILMD-MIG-G1.2")
            .for_message_type("UTILMD")
            .for_release("G1.2")
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

static GROUP_SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG4",
    trigger: "IDE",
    children: &[],
}];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_44001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44001")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44001-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44001-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44001", "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44001-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44001-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44001", "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44001-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44001-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44001", "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44001-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44001-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44001", "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44001-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44001-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44001", "44001", issues);
            })
            .with_named_stateless_rule_fn("AHB-44001-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44001-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44001", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44001-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44001 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44001-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="Z35" is present in SG4 // [84] Wenn STS+E01++Z35 (Ablehnung Abmeldeanfrage) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44001-SG4-FTX-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "Z35")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44001 (I: when STS DE[0]=\"E01\"+DE[2]=\"Z35\" is present in SG4)".to_owned()).with_rule_id("AHB-44001-SG4-FTX-I1").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44001_PACK)
}

static AHB_44002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44002")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44002-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44002-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44002", "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44002-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44002-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44002", "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44002-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44002-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44002", "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44002-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44002-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44002", "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44002-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44002-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44002", "44002", issues);
            })
            .with_named_stateless_rule_fn("AHB-44002-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44002-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44002", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44002-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44002 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44002-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="Z35" is present in SG4 // [84] Wenn STS+E01++Z35 (Ablehnung Abmeldeanfrage) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44002-SG4-FTX-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "Z35")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44002 (I: when STS DE[0]=\"E01\"+DE[2]=\"Z35\" is present in SG4)".to_owned()).with_rule_id("AHB-44002-SG4-FTX-I1").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44002_PACK)
}

static AHB_44003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44003")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44003-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44003-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44003", "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44003-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44003-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44003", "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44003-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44003-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44003", "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44003-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44003-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44003", "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44003-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44003-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44003", "44003", issues);
            })
            .with_named_stateless_rule_fn("AHB-44003-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44003-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44003", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44003-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44003 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44003-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="Z35" is present in SG4 // [84] Wenn STS+E01++Z35 (Ablehnung Abmeldeanfrage) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44003-SG4-FTX-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "Z35")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44003 (I: when STS DE[0]=\"E01\"+DE[2]=\"Z35\" is present in SG4)".to_owned()).with_rule_id("AHB-44003-SG4-FTX-I1").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44003_PACK)
}

static AHB_44004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44004")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44004-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44004-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44004", "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44004-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E0F']", |q| matches!(q, "E0F"), "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44004-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44004", "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44004-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44004-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44004", "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44004-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44004-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44004", "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44004-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44004-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44004", "44004", issues);
            })
            .with_named_stateless_rule_fn("AHB-44004-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44004-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44004", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [7] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+Beginn Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44004-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 44004 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44004-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [11] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+36 (Ende) Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44004-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 44004 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44004-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44004-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44004 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44004-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44004_PACK)
}

static AHB_44005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44005")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44005-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44005-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44005", "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44005-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44005-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44005", "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44005-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44005-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44005", "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44005-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44005-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44005", "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44005-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44005-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44005", "44005", issues);
            })
            .with_named_stateless_rule_fn("AHB-44005-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44005-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44005", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [7] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+Beginn Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44005-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 44005 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44005-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [11] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+36 (Ende) Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44005-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 44005 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44005-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44005-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44005 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44005-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44005_PACK)
}

static AHB_44006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44006")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44006-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44006-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44006", "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44006-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E0F']", |q| matches!(q, "E0F"), "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44006-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44006", "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44006-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44006-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44006", "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44006-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44006-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44006", "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44006-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44006-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44006", "44006", issues);
            })
            .with_named_stateless_rule_fn("AHB-44006-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44006-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44006", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [7] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+Beginn Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44006-SG4-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM is missing for Pruefidentifikator 44006 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44006-SG4-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="7"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4 // [11] Wenn STS+7++ZG9/ZH1/ZH2 (Transaktionsgrund: Aufhebung zukünftiger Zuordnung) vorhanden, ist DTM+36 (Ende) Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44006-SG4-DTM-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "7") && s.element_str(2).is_some_and(|v| v == "ZG9" || v == "ZH1" || v == "ZH2")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "36")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment DTM (DE[0]=\"36\") is missing for Pruefidentifikator 44006 (I: when STS DE[0]=\"7\"+DE[2]∈{ZG9|ZH1|ZH2} is present in SG4)".to_owned()).with_rule_id("AHB-44006-SG4-DTM-I1").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44006-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44006 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44006-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44006_PACK)
}

static AHB_44017_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44017")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44017-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44017-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44017", "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44017-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44017-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44017", "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44017-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44017-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44017", "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44017-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44017-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44017", "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44017-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44017-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44017", "44017", issues);
            })
            .with_named_stateless_rule_fn("AHB-44017-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44017-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44017", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44017-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44017 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44017-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44017_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44017_PACK)
}

static AHB_44018_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44018")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44018-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44018-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44018", "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44018-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E01']", |q| matches!(q, "E01"), "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44018-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44018", "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44018-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44018-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44018", "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44018-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44018-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44018", "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44018-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44018-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44018", "44018", issues);
            })
            .with_named_stateless_rule_fn("AHB-44018-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44018-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44018", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="E01"+DE[2]="E14" is present in SG4 // [48] Wenn STS+E01++E14 (Status: Ablehnung Sonstiges) vorhanden, ist FTX Pflicht
            .with_scoped_group_rule_fn("SG4", "AHB-44018-SG4-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "E01") && s.element_str(2).is_some_and(|v| v == "E14")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG4: conditional segment FTX is missing for Pruefidentifikator 44018 (I: when STS DE[0]=\"E01\"+DE[2]=\"E14\" is present in SG4)".to_owned()).with_rule_id("AHB-44018-SG4-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44018_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44018_PACK)
}

static AHB_44555_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("UTILMD-AHB-G1.2-44555")
            .for_message_type("UTILMD")
            .for_release("G1.2")
            .with_named_stateless_rule_fn("AHB-44555-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-44555-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-44555-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['E03']", |q| matches!(q, "E03"), "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-44555-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-44555-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-44555-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-44555-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-IDE-M", |segs, issues| {
                ahb_check_mandatory(segs, "IDE", "AHB-44555-IDE-M", "mandatory segment IDE is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-IDE-7495-Q", |segs, issues| {
                ahb_check_qualifier(segs, "IDE", "AHB-44555-IDE-7495-Q", "segment IDE DE 7495 (element 0, component 0): qualifier is not one of the allowed values ['Z19']", |q| matches!(q, "Z19"), "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-44555-STS-M", "mandatory segment STS is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-STS-9015-Q", |segs, issues| {
                ahb_check_qualifier(segs, "STS", "AHB-44555-STS-9015-Q", "segment STS DE 9015 (element 0, component 0): qualifier is not one of the allowed values ['E07', 'E08']", |q| matches!(q, "E07" | "E08"), "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-44555-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 44555", "44555", issues);
            })
            .with_named_stateless_rule_fn("AHB-44555-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-44555-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['Z13']", |q| matches!(q, "Z13"), "44555", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_44555_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_44555_PACK)
}

static AHB_ALL_PACK_UTILMD_G1_2: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("UTILMD-AHB-G1.2-ALL")
        .for_message_type("UTILMD")
        .for_release("G1.2");
    let pack = pack
        .merge_with_override(ahb_44001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44017_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44018_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_44555_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(44001) => ahb_44001_pack(),
            Some(44002) => ahb_44002_pack(),
            Some(44003) => ahb_44003_pack(),
            Some(44004) => ahb_44004_pack(),
            Some(44005) => ahb_44005_pack(),
            Some(44006) => ahb_44006_pack(),
            Some(44017) => ahb_44017_pack(),
            Some(44018) => ahb_44018_pack(),
            Some(44555) => ahb_44555_pack(),
            None => Arc::clone(&AHB_ALL_PACK_UTILMD_G1_2),
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

static RELEASE_UTILMD_FV20261001_GAS: LazyLock<Release> = LazyLock::new(|| Release::new("G1.2"));

pub(crate) struct UtilmdFv20261001GasProfile;

impl Profile for UtilmdFv20261001GasProfile {
    fn message_type(&self) -> MessageType {
        MessageType::Utilmd
    }
    fn release(&self) -> &Release {
        &RELEASE_UTILMD_FV20261001_GAS
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("G1.2")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("UTILMD AHB G1.2, Stand 01.10.2026")
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

pub(crate) static PROFILE: UtilmdFv20261001GasProfile = UtilmdFv20261001GasProfile;
