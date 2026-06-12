// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate

/// Codegen schema version this module was generated from.
/// Compared against `mig.json` `schema_version` in CI to detect drift.
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
        name: "Datum/Uhrzeit/Zeitspanne",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Produkt-/Leistungsbeschreibung",
        elements: &[
            ElementRef::new(1, "7077", Status::Conditional, 1),
            ElementRef::new(2, "C273", Status::Conditional, 1),
        ],
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
        name: "Beteiligter",
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
        tag: "LOC",
        name: "Meldepunkt",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
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
        tag: "GIN",
        name: "Herstellernummer",
        elements: &[
            ElementRef::new(1, "7405", Status::Mandatory, 1),
            ElementRef::new(2, "C208", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "CCI",
        name: "Merkmal/Eigenschaft",
        elements: &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C240", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CAV",
        name: "Merkmalswert",
        elements: &[ElementRef::new(1, "C889", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "PRI",
        name: "Preisangaben",
        elements: &[ElementRef::new(1, "C509", Status::Conditional, 1)],
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
        tag: "CUX",
        name: "Währungsangaben",
        elements: &[ElementRef::new(1, "C504", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "RNG",
        name: "Angaben zum Wertebereich",
        elements: &[
            ElementRef::new(1, "6167", Status::Mandatory, 1),
            ElementRef::new(2, "C280", Status::Conditional, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["310"];
static CODES_1153: &[&str] = &["AAV", "ACW", "APF", "Z09", "Z13", "Z18"];
static CODES_2005: &[&str] = &["137", "203", "273", "279", "469", "472", "76"];
static CODES_3035: &[&str] = &["DP", "MR", "MS", "VY"];
static CODES_3227: &[&str] = &["172"];
static CODES_4451: &[&str] = &["ACB"];
static CODES_6167: &[&str] = &["10"];
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

fn expected_components(_tag: &str, _idx: usize) -> Option<u8> {
    None
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "4451" => Some(CODES_4451),
        "6167" => Some(CODES_6167),
        "6343" => Some(CODES_6343),
        "6347" => Some(CODES_6347),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_QUOTES_1_3C: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-QUOTES-1.3c",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_QUOTES_1_3C
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
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CUX` segment group appears at most 5 times.
///
/// Each occurrence of the trigger segment `CUX` marks the start of
/// one group instance.  The MIG specifies a maximum of 5 instances.
fn rule_group_sg4_cux_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CUX").count();
    if count > 5 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by CUX occurs {count} times; maximum is 5"),
            )
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG4-CUX-CARD-MAX")
            .with_segment("CUX".to_owned()),
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
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG11-NAD-CARD-MAX")
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
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG27-LIN-CARD-MAX")
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
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG1-RFF-CARD-MIN")
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
            .with_rule_id("MIG-QUOTES-MIG-1.3c-GROUP-SG11-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &[
        "UNH", "BGM", "DTM", "IMD", "FTX", "RFF", "CUX", "NAD", "CTA", "COM", "LOC", "LIN", "PIA",
        "GIN", "CCI", "CAV", "PRI", "UNT",
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
                .with_rule_id("MIG-QUOTES-MIG-1.3c-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_QUOTES_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-MIG-1.3c")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_cux_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg11_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg27_lin_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg11_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_QUOTES_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_15001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-AHB-1.3c-15001")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-15001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-15001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-15001-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-15001-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-15001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-15001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-15001-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-15001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-15001-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-15001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-15001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-15001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-15001-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-15001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 15001",
                    "15001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_15001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_15001_PACK)
}

static AHB_15002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-AHB-1.3c-15002")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-15002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-15002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-15002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-15002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-15002-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-15002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-15002-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-15002-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-15002-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-15002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-15002-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-15002-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-15002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 15002",
                    "15002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_15002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_15002_PACK)
}

static AHB_15003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-AHB-1.3c-15003")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-15003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-15003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-15003-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-15003-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-15003-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-15003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-15003-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-15003-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-15003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-15003-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-15003-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-15003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 15003",
                    "15003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_15003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_15003_PACK)
}

static AHB_15004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-AHB-1.3c-15004")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-15004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-15004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-15004-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-15004-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-15004-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-15004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-15004-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-15004-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-15004-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-15004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-15004-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-15004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 15004",
                    "15004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_15004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_15004_PACK)
}

static AHB_15005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("QUOTES-AHB-1.3c-15005")
            .for_message_type("QUOTES")
            .for_release("1.3c")
            .with_named_stateless_rule_fn("AHB-15005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-15005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-15005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-15005-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-15005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-15005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-15005-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-15005-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-15005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-15005-RNG-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RNG",
                    "AHB-15005-RNG-M",
                    "mandatory segment RNG is missing for Pruefidentifikator 15005",
                    "15005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_15005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_15005_PACK)
}

static AHB_ALL_PACK_QUOTES_1_3C: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("QUOTES-AHB-1.3c-ALL")
        .for_message_type("QUOTES")
        .for_release("1.3c");
    let pack = pack
        .merge_with_override(ahb_15001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_15002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_15003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_15004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_15005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(15001) => ahb_15001_pack(),
            Some(15002) => ahb_15002_pack(),
            Some(15003) => ahb_15003_pack(),
            Some(15004) => ahb_15004_pack(),
            Some(15005) => ahb_15005_pack(),
            None => Arc::clone(&AHB_ALL_PACK_QUOTES_1_3C),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("QUOTES")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_QUOTES_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("1.3c"));

pub(crate) struct QuotesFv20260401Profile;

impl Profile for QuotesFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Quotes
    }
    fn release(&self) -> &Release {
        &RELEASE_QUOTES_FV20260401
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
        Some("QUOTES AHB 1.3c, Stand 01.04.2026")
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

pub(crate) static PROFILE: QuotesFv20260401Profile = QuotesFv20260401Profile;
