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
            ElementRef::new(1, "C002", Status::Conditional, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Nachrichtendatum",
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
        tag: "IDE",
        name: "Vorgang",
        elements: &[
            ElementRef::new(1, "7495", Status::Mandatory, 1),
            ElementRef::new(2, "C206", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LOC",
        name: "Standort/Definition",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "STS",
        name: "Statusinformation",
        elements: &[
            ElementRef::new(1, "C601", Status::Conditional, 1),
            ElementRef::new(2, "C555", Status::Conditional, 1),
            ElementRef::new(3, "C556", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Bemerkung",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C107", Status::Conditional, 1),
            ElementRef::new(3, "C108", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Referenz",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "SEQ",
        name: "Folge",
        elements: &[
            ElementRef::new(1, "1229", Status::Conditional, 1),
            ElementRef::new(2, "C286", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CCI",
        name: "Charakteristikum/Klasse-Information",
        elements: &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C502", Status::Conditional, 1),
            ElementRef::new(3, "C240", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CAV",
        name: "Charakteristikum-Wert",
        elements: &[ElementRef::new(1, "C889", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "QTY",
        name: "Mengendaten",
        elements: &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["Z36", "Z59", "Z60", "Z78", "Z79", "Z80", "Z81"];
static CODES_1153: &[&str] = &["AGI", "TN", "Z13", "Z19", "Z23", "Z28", "Z46", "Z49", "Z53"];
static CODES_1229: &[&str] = &["Z36", "Z37", "Z41", "Z42", "Z43", "Z69", "Z70", "Z74"];
static CODES_2005: &[&str] = &["137", "157", "293", "Z25", "Z26", "Z34", "Z35"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_3139: &[&str] = &["IC"];
static CODES_3155: &[&str] = &["AJ", "AL", "EM", "FX", "TE"];
static CODES_3227: &[&str] = &["172", "Z09"];
static CODES_4451: &[&str] = &["ACB"];
static CODES_7495: &[&str] = &["24"];
static CODES_9015: &[&str] = &["E01", "Z23", "Z36"];

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
        "1229" => Some(CODES_1229),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3139" => Some(CODES_3139),
        "3155" => Some(CODES_3155),
        "3227" => Some(CODES_3227),
        "4451" => Some(CODES_4451),
        "7495" => Some(CODES_7495),
        "9015" => Some(CODES_9015),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_UTILTS_1_1E: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-UTILTS-1.1e",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_UTILTS_1_1E
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

/// Layer 3 — verify the `NAD` segment group appears at most 2 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 2 instances.
fn rule_group_sg2_nad_max_occurrences(
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
            .with_rule_id("MIG-UTILTS-MIG-1.1e-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at most 99999 times.
///
/// Each occurrence of the trigger segment `IDE` marks the start of
/// one group instance.  The MIG specifies a maximum of 99999 instances.
fn rule_group_sg5_ide_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "IDE").count();
    if count > 99_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by IDE occurs {count} times; maximum is 99_999"),
            )
            .with_rule_id("MIG-UTILTS-MIG-1.1e-GROUP-SG5-IDE-CARD-MAX")
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
            .with_rule_id("MIG-UTILTS-MIG-1.1e-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `IDE` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg5_ide_min_occurrences(
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
            .with_rule_id("MIG-UTILTS-MIG-1.1e-GROUP-SG5-IDE-CARD-MIN")
            .with_segment("IDE".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &[
        "UNH", "BGM", "DTM", "NAD", "CTA", "COM", "IDE", "LOC", "STS", "FTX", "RFF", "SEQ", "CCI",
        "CAV", "QTY", "UNT",
    ];
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
                .with_rule_id("MIG-UTILTS-MIG-1.1e-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_UTILTS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-MIG-1.1e")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_ide_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_ide_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_ide_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_UTILTS_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_25001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25001")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-25001-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-25001-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-25001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-25001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25001-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-25001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25001-SEQ-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "SEQ",
                    "AHB-25001-SEQ-M",
                    "mandatory segment SEQ is missing for Pruefidentifikator 25001",
                    "25001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25001_PACK)
}

static AHB_25004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25004")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-25004-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-25004-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25004-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-SEQ-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "SEQ",
                    "AHB-25004-SEQ-M",
                    "mandatory segment SEQ is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25004-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-25004-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 25004",
                    "25004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25004_PACK)
}

static AHB_25005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25005")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-25005-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25005-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-25005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-25005-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25005-SEQ-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "SEQ",
                    "AHB-25005-SEQ-M",
                    "mandatory segment SEQ is missing for Pruefidentifikator 25005",
                    "25005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25005_PACK)
}

static AHB_25006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25006")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25006-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25006-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25006-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25006-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25006-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25006-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25006-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25006-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25006-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25006-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25006-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-25006-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 25006",
                    "25006",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25006_PACK)
}

static AHB_25007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25007")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25007-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25007-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25007-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25007-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25007-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25007-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25007-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25007-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25007-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25007-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25007-STS-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "STS",
                    "AHB-25007-STS-M",
                    "mandatory segment STS is missing for Pruefidentifikator 25007",
                    "25007",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25007_PACK)
}

static AHB_25008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25008")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25008-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25008-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25008-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25008-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25008-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25008-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25008-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-25008-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25008-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25008-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25008-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25008-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25008",
                    "25008",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25008_PACK)
}

static AHB_25009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25009")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25009-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25009-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25009-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-25009-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25009",
                    "25009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25009_PACK)
}

static AHB_25010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("UTILTS-AHB-1.1e-25010")
            .for_message_type("UTILTS")
            .for_release("1.1e")
            .with_named_stateless_rule_fn("AHB-25010-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-25010-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-25010-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-25010-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-25010-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-IDE-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IDE",
                    "AHB-25010-IDE-M",
                    "mandatory segment IDE is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-25010-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-25010-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-25010-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 25010",
                    "25010",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_25010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_25010_PACK)
}

static AHB_ALL_PACK_UTILTS_1_1E: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("UTILTS-AHB-1.1e-ALL")
        .for_message_type("UTILTS")
        .for_release("1.1e");
    let pack = pack
        .merge_with_override(ahb_25001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_25010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(25001) => ahb_25001_pack(),
            Some(25004) => ahb_25004_pack(),
            Some(25005) => ahb_25005_pack(),
            Some(25006) => ahb_25006_pack(),
            Some(25007) => ahb_25007_pack(),
            Some(25008) => ahb_25008_pack(),
            Some(25009) => ahb_25009_pack(),
            Some(25010) => ahb_25010_pack(),
            None => Arc::clone(&AHB_ALL_PACK_UTILTS_1_1E),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("UTILTS")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_UTILTS_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("1.1e"));

pub(crate) struct UtiltsFv20260401Profile;

impl Profile for UtiltsFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Utilts
    }
    fn release(&self) -> &Release {
        &RELEASE_UTILTS_FV20260401
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 04 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.1e")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("UTILTS AHB 1.1e, Stand 01.04.2026")
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

pub(crate) static PROFILE: UtiltsFv20260401Profile = UtiltsFv20260401Profile;
