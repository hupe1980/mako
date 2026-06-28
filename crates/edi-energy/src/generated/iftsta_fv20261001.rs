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
        name: "Dokumentendatum",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
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
        tag: "NAD",
        name: "MP-ID Absender/Empfänger",
        elements: &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "EQD",
        name: "Einzelheiten zu Equipment",
        elements: &[
            ElementRef::new(1, "8053", Status::Mandatory, 1),
            ElementRef::new(2, "C237", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Prüfidentifikator",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "LOC",
        name: "Meldepunkt",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "STS",
        name: "Prüfstatus Antwort auf Summenzeitreihen",
        elements: &[
            ElementRef::new(1, "C601", Status::Conditional, 1),
            ElementRef::new(2, "C555", Status::Conditional, 1),
            ElementRef::new(3, "C556", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CNI",
        name: "Sendungsdaten",
        elements: &[ElementRef::new(1, "1490", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "EFI",
        name: "Ansicht des Senders / Privilegierte Energiemenge",
        elements: &[ElementRef::new(1, "C077", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "QTY",
        name: "Menge",
        elements: &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
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
        tag: "GID",
        name: "Sendungspositionseinzelheiten",
        elements: &[ElementRef::new(1, "1496", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Freier Text",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "4453", Status::Conditional, 1),
            ElementRef::new(3, "C107", Status::Conditional, 1),
            ElementRef::new(4, "C108", Status::Conditional, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["Z03", "Z09", "Z33", "Z49", "Z72", "Z73", "Z86"];
static CODES_1153: &[&str] = &["ACW", "ADY", "AUU", "TN", "Z13"];
static CODES_2005: &[&str] = &["137", "334", "492"];
static CODES_3035: &[&str] = &["BY", "DP", "MR", "MS", "PR", "SU"];
static CODES_3227: &[&str] = &["172"];
static CODES_8053: &[&str] = &["Z01"];

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
        | ("EQD", 0)
        | ("LOC", 0)
        | ("CNI", 0)
        | ("CTA", 0)
        | ("GID", 0)
        | ("FTX", 0)
        | ("FTX", 1) => Some(1),
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
        "8053" => Some(CODES_8053),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_IFTSTA_2_1: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-IFTSTA-2.1",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_IFTSTA_2_1
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

/// Layer 3 — group-window rule for `EQD` groups.
///
/// When a `EQD` group is present, the mandatory inner segments
/// must also be present within each group window.
fn rule_group_eqd_window(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const MANDATORY_INNER: &[&str] = &["RFF"];
    // Find all positions of the trigger segment.
    let trigger_positions: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.tag == "EQD")
        .map(|(i, _)| i)
        .collect();
    for (win_idx, &start) in trigger_positions.iter().enumerate() {
        let end = trigger_positions
            .get(win_idx + 1)
            .copied()
            .unwrap_or(segments.len());
        let window = &segments[start..end];
        for &required_tag in MANDATORY_INNER {
            if !window.iter().any(|s| s.tag == required_tag) {
                issues.push(
                        ValidationIssue::new(
                            ValidationSeverity::Error,
                            format!("mandatory segment {required_tag} missing in EQD group at position {start}"),
                        )
                        .with_rule_id("MIG-IFTSTA-MIG-2.1-GROUP-EQD")
                        .with_segment(required_tag.to_owned())
                    );
            }
        }
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 2 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 2 instances.
fn rule_group_sg1_nad_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "NAD").count();
    if count > 2 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; maximum is 2"),
            )
            .with_rule_id("MIG-IFTSTA-MIG-2.1-GROUP-SG1-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `EQD` segment group appears at most 99999 times.
///
/// Each occurrence of the trigger segment `EQD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99999 instances.
fn rule_group_sg4_eqd_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "EQD").count();
    if count > 99_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by EQD occurs {count} times; maximum is 99_999"),
            )
            .with_rule_id("MIG-IFTSTA-MIG-2.1-GROUP-SG4-EQD-CARD-MAX")
            .with_segment("EQD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CNI` segment group appears at most 99999 times.
///
/// Each occurrence of the trigger segment `CNI` marks the start of
/// one group instance.  The MIG specifies a maximum of 99999 instances.
fn rule_group_sg14_cni_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CNI").count();
    if count > 99_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by CNI occurs {count} times; maximum is 99_999"),
            )
            .with_rule_id("MIG-IFTSTA-MIG-2.1-GROUP-SG14-CNI-CARD-MAX")
            .with_segment("CNI".to_owned()),
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
            .with_rule_id("MIG-IFTSTA-MIG-2.1-GROUP-SG1-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &["UNH", "BGM", "DTM", "NAD", "EQD", "CNI", "UNT"];
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
                .with_rule_id("MIG-IFTSTA-MIG-2.1-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_IFTSTA_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-MIG-2.1")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_group_eqd_window)
            .with_stateless_rule_fn(rule_group_sg1_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_eqd_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg14_cni_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_IFTSTA_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG4",
    trigger: "EQD",
    children: &[GroupDef {
        name: "SG6",
        trigger: "LOC",
        children: &[],
    }],
}];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_21000_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21000")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21000-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21000-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21000-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21000-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21000-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21000-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21000-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21000-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21000-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21000",
                    "21000",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21000_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21000_PACK)
}

static AHB_21001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21001")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21001-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21001-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21001-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21001",
                    "21001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21001_PACK)
}

static AHB_21002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21002")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21002-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21002-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21002-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21002-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21002",
                    "21002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21002_PACK)
}

static AHB_21003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21003")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21003-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21003-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21003-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21003-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21003",
                    "21003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21003_PACK)
}

static AHB_21004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21004")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21004-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21004-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21004-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21004-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21004",
                    "21004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21004_PACK)
}

static AHB_21005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21005")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-EQD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EQD",
                    "AHB-21005-EQD-M",
                    "mandatory segment EQD is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21005-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21005-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21005",
                    "21005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21005_PACK)
}

static AHB_21007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21007")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21007-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21007-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21007-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21007-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21007-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21007-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21007-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21007-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21007-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21007",
                    "21007",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21007_PACK)
}

static AHB_21009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21009")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21009-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21009-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21009-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21009-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21009",
                    "21009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21009_PACK)
}

static AHB_21010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21010")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21010-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21010-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21010-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21010-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21010-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21010-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21010-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21010-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21010-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21010",
                    "21010",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21010_PACK)
}

static AHB_21011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21011")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21011-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21011-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21011-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21011-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21011-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21011-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21011-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21011-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21011-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21011",
                    "21011",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21011_PACK)
}

static AHB_21012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21012")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21012-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21012-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21012-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21012-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21012-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21012-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21012-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21012-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21012-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21012",
                    "21012",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21012_PACK)
}

static AHB_21013_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21013")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21013-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21013-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21013-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21013-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21013-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21013-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21013-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21013-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21013-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21013",
                    "21013",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21013_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21013_PACK)
}

static AHB_21015_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21015")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21015-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21015-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21015-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21015-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21015-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21015-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21015-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21015-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21015-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21015-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21015-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21015",
                    "21015",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21015_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21015_PACK)
}

static AHB_21018_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21018")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21018-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21018-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21018-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21018-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21018-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21018-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21018-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21018-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21018-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21018",
                    "21018",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21018_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21018_PACK)
}

static AHB_21024_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21024")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21024-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21024-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21024-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21024-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21024-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21024-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21024-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21024-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21024-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21024-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21024-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21024",
                    "21024",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21024_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21024_PACK)
}

static AHB_21025_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21025")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21025-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21025-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21025-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21025-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21025-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21025-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21025-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21025-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21025-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21025-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21025-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21025-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21025-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21025",
                    "21025",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21025_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21025_PACK)
}

static AHB_21026_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21026")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21026-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21026-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21026-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21026-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21026-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21026-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21026-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21026-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21026-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21026-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21026-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21026",
                    "21026",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21026_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21026_PACK)
}

static AHB_21027_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21027")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21027-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21027-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21027-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21027-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21027-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21027-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21027-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21027-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21027-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21027-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21027-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21027-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21027-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21027",
                    "21027",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21027_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21027_PACK)
}

static AHB_21028_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21028")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21028-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21028-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21028-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21028-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21028-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21028-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21028-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21028-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21028-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21028",
                    "21028",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21028_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21028_PACK)
}

static AHB_21029_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21029")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21029-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21029-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21029-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21029-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21029-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21029-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21029-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21029-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21029-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21029",
                    "21029",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21029_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21029_PACK)
}

static AHB_21030_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21030")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21030-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21030-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21030-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21030-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21030-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21030-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21030-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21030-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21030-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21030",
                    "21030",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21030_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21030_PACK)
}

static AHB_21031_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21031")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21031-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21031-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21031-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21031-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21031-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21031-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21031-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21031-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21031-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21031",
                    "21031",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21031_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21031_PACK)
}

static AHB_21032_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21032")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21032-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21032-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21032-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21032-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21032-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21032-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21032-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21032-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21032-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21032-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21032",
                    "21032",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21032_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21032_PACK)
}

static AHB_21033_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21033")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21033-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21033-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21033-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21033-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21033-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21033-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21033-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21033-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21033-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21033-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21033",
                    "21033",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21033_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21033_PACK)
}

static AHB_21035_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21035")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21035-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21035-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21035-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21035-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-EFI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EFI",
                    "AHB-21035-EFI-M",
                    "mandatory segment EFI is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21035-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-21035-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21035-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21035-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21035-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21035",
                    "21035",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21035_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21035_PACK)
}

static AHB_21036_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21036")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21036-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21036-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21036-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21036-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21036-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21036-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21036-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21036-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21036-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21036",
                    "21036",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21036_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21036_PACK)
}

static AHB_21037_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21037")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21037-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21037-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21037-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21037-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21037-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21037-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21037-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21037-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21037-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21037-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21037-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21037-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21037",
                    "21037",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21037_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21037_PACK)
}

static AHB_21038_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21038")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21038-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21038-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21038-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-21038-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-21038-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21038-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21038-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21038-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21038-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21038-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21038-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21038-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21038",
                    "21038",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21038_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21038_PACK)
}

static AHB_21039_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21039")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21039-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21039-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21039-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21039-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21039-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21039-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21039-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21039-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21039-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21039-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21039-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21039",
                    "21039",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21039_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21039_PACK)
}

static AHB_21040_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21040")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21040-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21040-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21040-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21040-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21040-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21040-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21040-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21040-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21040-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21040",
                    "21040",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21040_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21040_PACK)
}

static AHB_21042_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21042")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21042-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21042-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21042-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21042-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21042-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21042-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21042-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21042-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21042-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21042-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21042-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21042-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21042",
                    "21042",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21042_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21042_PACK)
}

static AHB_21043_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21043")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21043-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21043-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21043-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21043-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21043-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21043-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21043-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21043-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21043-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21043-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21043",
                    "21043",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21043_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21043_PACK)
}

static AHB_21044_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21044")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21044-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21044-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21044-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21044-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21044-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21044-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21044-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21044-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21044-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21044-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21044-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21044-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21044",
                    "21044",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21044_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21044_PACK)
}

static AHB_21045_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21045")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21045-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21045-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21045-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21045-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-EFI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "EFI",
                    "AHB-21045-EFI-M",
                    "mandatory segment EFI is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21045-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21045-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-21045-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21045-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-21045-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21045-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21045-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21045-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21045",
                    "21045",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21045_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21045_PACK)
}

static AHB_21047_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("IFTSTA-AHB-2.1-21047")
            .for_message_type("IFTSTA")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-21047-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-21047-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-CNI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CNI",
                    "AHB-21047-CNI-M",
                    "mandatory segment CNI is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-21047-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-21047-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-GID-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GID",
                    "AHB-21047-GID-M",
                    "mandatory segment GID is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-21047-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-21047-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-21047-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-21047-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 21047",
                    "21047",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_21047_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_21047_PACK)
}

static AHB_ALL_PACK_IFTSTA_2_1: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("IFTSTA-AHB-2.1-ALL")
        .for_message_type("IFTSTA")
        .for_release("2.1");
    let pack = pack
        .merge_with_override(ahb_21000_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21013_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21015_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21018_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21024_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21025_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21026_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21027_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21028_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21029_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21030_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21031_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21032_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21033_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21035_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21036_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21037_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21038_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21039_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21040_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21042_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21043_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21044_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21045_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_21047_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(21000) => ahb_21000_pack(),
            Some(21001) => ahb_21001_pack(),
            Some(21002) => ahb_21002_pack(),
            Some(21003) => ahb_21003_pack(),
            Some(21004) => ahb_21004_pack(),
            Some(21005) => ahb_21005_pack(),
            Some(21007) => ahb_21007_pack(),
            Some(21009) => ahb_21009_pack(),
            Some(21010) => ahb_21010_pack(),
            Some(21011) => ahb_21011_pack(),
            Some(21012) => ahb_21012_pack(),
            Some(21013) => ahb_21013_pack(),
            Some(21015) => ahb_21015_pack(),
            Some(21018) => ahb_21018_pack(),
            Some(21024) => ahb_21024_pack(),
            Some(21025) => ahb_21025_pack(),
            Some(21026) => ahb_21026_pack(),
            Some(21027) => ahb_21027_pack(),
            Some(21028) => ahb_21028_pack(),
            Some(21029) => ahb_21029_pack(),
            Some(21030) => ahb_21030_pack(),
            Some(21031) => ahb_21031_pack(),
            Some(21032) => ahb_21032_pack(),
            Some(21033) => ahb_21033_pack(),
            Some(21035) => ahb_21035_pack(),
            Some(21036) => ahb_21036_pack(),
            Some(21037) => ahb_21037_pack(),
            Some(21038) => ahb_21038_pack(),
            Some(21039) => ahb_21039_pack(),
            Some(21040) => ahb_21040_pack(),
            Some(21042) => ahb_21042_pack(),
            Some(21043) => ahb_21043_pack(),
            Some(21044) => ahb_21044_pack(),
            Some(21045) => ahb_21045_pack(),
            Some(21047) => ahb_21047_pack(),
            None => Arc::clone(&AHB_ALL_PACK_IFTSTA_2_1),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("IFTSTA")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_IFTSTA_FV20261001: LazyLock<Release> = LazyLock::new(|| Release::new("2.1"));

pub(crate) struct IftstaFv20261001Profile;

impl Profile for IftstaFv20261001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Iftsta
    }
    fn release(&self) -> &Release {
        &RELEASE_IFTSTA_FV20261001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.1")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("IFTSTA AHB 2.1, Stand 01.10.2026")
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

pub(crate) static PROFILE: IftstaFv20261001Profile = IftstaFv20261001Profile;
