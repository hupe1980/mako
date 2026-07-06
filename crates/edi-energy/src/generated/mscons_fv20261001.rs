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
        name: "Nachrichtenkopfsegment",
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
            ElementRef::new(3, "1225", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Nachrichtendatum",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "UNS",
        name: "Abschnitts-Kontrollsegment",
        elements: &[ElementRef::new(1, "0081", Status::Mandatory, 1)],
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
        name: "Referenzangaben",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
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
        tag: "LOC",
        name: "Identifikationsangabe",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "CCI",
        name: "Merkmal/Charakteristik",
        elements: &[
            ElementRef::new(1, "7081", Status::Conditional, 1),
            ElementRef::new(2, "C240", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Laufende Position",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "PIA",
        name: "Produktidentifikation (OBIS-Kennzahl)",
        elements: &[
            ElementRef::new(1, "4347", Status::Mandatory, 1),
            ElementRef::new(2, "C212", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "QTY",
        name: "Mengenangaben",
        elements: &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "STS",
        name: "Statusangabe",
        elements: &[
            ElementRef::new(1, "9015", Status::Conditional, 1),
            ElementRef::new(2, "C601", Status::Conditional, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &[
    "270", "35", "7", "Z06", "Z15", "Z16", "Z21", "Z23", "Z27", "Z28", "Z41", "Z42", "Z43", "Z44",
    "Z45", "Z48", "Z83", "Z85",
];
static CODES_1153: &[&str] = &["ACW", "AGI", "AGK", "MG", "Z13", "Z19"];
static CODES_1225: &[&str] = &["1", "9"];
static CODES_2005: &[&str] = &["137", "163", "164", "25", "293", "306", "37", "7"];
static CODES_3035: &[&str] = &["DP", "MR", "MS"];
static CODES_3227: &[&str] = &["172", "237", "Z04", "Z06", "Z98"];
static CODES_4347: &[&str] = &["5"];
static CODES_6063: &[&str] = &["220", "67", "Z10", "Z18", "Z19", "Z47"];
static CODES_7143: &[&str] = &["SRW"];

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
        | ("UNS", 0)
        | ("UNT", 0)
        | ("UNT", 1)
        | ("NAD", 0)
        | ("CTA", 0)
        | ("LOC", 0)
        | ("CCI", 0)
        | ("LIN", 0)
        | ("PIA", 0)
        | ("STS", 0) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "1225" => Some(CODES_1225),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "4347" => Some(CODES_4347),
        "6063" => Some(CODES_6063),
        "7143" => Some(CODES_7143),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_MSCONS_2_5: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-MSCONS-2.5",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_MSCONS_2_5
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

fn rule_loc_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "LOC") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment LOC is missing".to_owned(),
            )
            .with_rule_id("MIG-LOC-REQ")
            .with_segment("LOC".to_owned()),
        );
    }
}

/// Layer 3 — verify the `RFF` segment group appears at most 9 times.
///
/// Each occurrence of the trigger segment `RFF` marks the start of
/// one group instance.  The MIG specifies a maximum of 9 instances.
fn rule_group_sg1_rff_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "RFF").count();
    if count > 9 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by RFF occurs {count} times; maximum is 9"),
            )
            .with_rule_id("MIG-MSCONS-MIG-2.5-GROUP-SG1-RFF-CARD-MAX")
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
            .with_rule_id("MIG-MSCONS-MIG-2.5-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `LOC` segment group appears at most 99999 times.
///
/// Each occurrence of the trigger segment `LOC` marks the start of
/// one group instance.  The MIG specifies a maximum of 99999 instances.
fn rule_group_sg5_loc_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "LOC").count();
    if count > 99_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by LOC occurs {count} times; maximum is 99_999"),
            )
            .with_rule_id("MIG-MSCONS-MIG-2.5-GROUP-SG5-LOC-CARD-MAX")
            .with_segment("LOC".to_owned()),
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
            .with_rule_id("MIG-MSCONS-MIG-2.5-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `LOC` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg5_loc_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "LOC").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by LOC occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-MSCONS-MIG-2.5-GROUP-SG5-LOC-CARD-MIN")
            .with_segment("LOC".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    /// Header segment ordering (before UNS+D).
    const EXPECTED_HEADER_ORDER: &[&str] = &["UNH", "BGM", "DTM", "RFF", "NAD", "CTA", "COM"];
    /// Detail segment ordering (after UNS+D).
    const EXPECTED_DETAIL_ORDER: &[&str] =
        &["LOC", "DTM", "CCI", "LIN", "PIA", "QTY", "STS", "UNT"];

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
        "MIG-MSCONS-MIG-2.5-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-MSCONS-MIG-2.5-ORDER",
        issues,
    );
}

static MIG_MSCONS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("MSCONS-MIG-2.5")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_loc_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_loc_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_loc_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_MSCONS_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[
    GroupDef {
        name: "SG2",
        trigger: "NAD",
        children: &[],
    },
    GroupDef {
        name: "SG5",
        trigger: "LOC",
        children: &[GroupDef {
            name: "SG9",
            trigger: "LIN",
            children: &[GroupDef {
                name: "SG10",
                trigger: "QTY",
                children: &[],
            }],
        }],
    },
];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_13002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13002")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13002-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13002-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13002-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z27']", |q| matches!(q, "7" | "Z27"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13002-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13002-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13002-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13002-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13002-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13002-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13002-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-13002-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-PIA-M", |segs, issues| {
                ahb_check_mandatory(segs, "PIA", "AHB-13002-PIA-M", "mandatory segment PIA is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-PIA-4347-Q", |segs, issues| {
                ahb_check_qualifier(segs, "PIA", "AHB-13002-PIA-4347-Q", "segment PIA DE 4347 (element 0, component 0): qualifier is not one of the allowed values ['5']", |q| matches!(q, "5"), "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13002-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13002", "13002", issues);
            })
            .with_named_stateless_rule_fn("AHB-13002-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13002-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13002", issues);
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Ersatzwertbildungsverfahren (Z32) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13002-SG10-STS-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z32")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z32\") is missing for Pruefidentifikator 13002 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13002-SG10-STS-I0").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when QTY DE[0]="201" is present in SG10 // [94] Wenn QTY DE6063 mit Wert 201 vorhanden, muss STS Ersatzwertbildungsverfahren (Z32) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13002-SG10-STS-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "201")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z32")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z32\") is missing for Pruefidentifikator 13002 (I: when QTY DE[0]=\"201\" is present in SG10)".to_owned()).with_rule_id("AHB-13002-SG10-STS-I1").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Grund der Ersatzwertbildung (Z40) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13002-SG10-STS-I2", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z40")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z40\") is missing for Pruefidentifikator 13002 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13002-SG10-STS-I2").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13002_PACK)
}

static AHB_13003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13003")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13003-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13003-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13003", "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13003-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13003-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13003-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13003", "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13003-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13003-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13003", "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13003-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13003-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13003", "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13003-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13003-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13003", "13003", issues);
            })
            .with_named_stateless_rule_fn("AHB-13003-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13003-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13003", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13003_PACK)
}

static AHB_13005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13005")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13005-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13005-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13005", "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13005-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z15']", |q| matches!(q, "Z15"), "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13005-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13005-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13005", "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13005-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13005-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13005", "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13005-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13005-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13005", "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13005-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['237']", |q| matches!(q, "237"), "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13005-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13005", "13005", issues);
            })
            .with_named_stateless_rule_fn("AHB-13005-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13005-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13005", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13005_PACK)
}

static AHB_13006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13006")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13006-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13006-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13006", "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13006-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', '270', 'Z27', 'Z28', 'Z41', 'Z42', 'Z85']", |q| matches!(q, "7" | "270" | "Z27" | "Z28" | "Z41" | "Z42" | "Z85"), "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13006-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['1']", |v| matches!(v, "1"), "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13006-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13006", "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13006-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13006-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13006", "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13006-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-13006-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 13006", "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-RFF-1153-V", |segs, issues| {
                ahb_check_field_value(segs, "RFF", 0, "AHB-13006-RFF-1153-V", "segment RFF DE 1153 (element 0, component 0): value is not one of the allowed values ['ACW']", |v| matches!(v, "ACW"), "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13006-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13006", "13006", issues);
            })
            .with_named_stateless_rule_fn("AHB-13006-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13006-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13006", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13006_PACK)
}

static AHB_13007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13007")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13007-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13007-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13007", "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13007-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z21']", |q| matches!(q, "Z21"), "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13007-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13007-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13007", "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13007-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13007-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13007", "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13007-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13007-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13007", "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13007-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13007-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13007", "13007", issues);
            })
            .with_named_stateless_rule_fn("AHB-13007-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13007-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13007", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13007_PACK)
}

static AHB_13008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13008")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13008-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13008-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13008", "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13008-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13008-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13008-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13008", "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13008-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13008-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13008", "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13008-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13008-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13008", "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13008-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13008-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13008", "13008", issues);
            })
            .with_named_stateless_rule_fn("AHB-13008-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13008-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13008", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13008_PACK)
}

static AHB_13009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13009")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13009-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13009-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13009", "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13009-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z27']", |q| matches!(q, "7" | "Z27"), "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13009-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13009-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13009", "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13009-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13009-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13009", "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13009-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13009-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13009", "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13009-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13009-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13009", "13009", issues);
            })
            .with_named_stateless_rule_fn("AHB-13009-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13009-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13009", issues);
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Ersatzwertbildungsverfahren (Z32) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13009-SG10-STS-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z32")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z32\") is missing for Pruefidentifikator 13009 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13009-SG10-STS-I0").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when QTY DE[0]="201" is present in SG10 // [94] Wenn QTY DE6063 mit Wert 201 vorhanden, muss STS Ersatzwertbildungsverfahren (Z32) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13009-SG10-STS-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "201")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z32")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z32\") is missing for Pruefidentifikator 13009 (I: when QTY DE[0]=\"201\" is present in SG10)".to_owned()).with_rule_id("AHB-13009-SG10-STS-I1").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Grund der Ersatzwertbildung (Z40) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13009-SG10-STS-I2", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z40")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z40\") is missing for Pruefidentifikator 13009 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13009-SG10-STS-I2").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13009_PACK)
}

static AHB_13010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13010")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13010-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13010-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13010", "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13010-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z06', 'Z16']", |q| matches!(q, "Z06" | "Z16"), "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13010-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13010-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13010", "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13010-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13010-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13010", "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13010-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13010-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13010", "13010", issues);
            })
            .with_named_stateless_rule_fn("AHB-13010-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13010-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z04', 'Z06']", |q| matches!(q, "Z04" | "Z06"), "13010", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13010_PACK)
}

static AHB_13011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13011")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13011-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13011-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13011", "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13011-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z06', 'Z16']", |q| matches!(q, "Z06" | "Z16"), "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13011-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13011-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13011", "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13011-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13011-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13011", "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13011-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13011-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13011", "13011", issues);
            })
            .with_named_stateless_rule_fn("AHB-13011-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13011-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z04', 'Z06']", |q| matches!(q, "Z04" | "Z06"), "13011", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13011_PACK)
}

static AHB_13012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13012")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13012-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13012-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13012", "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13012-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z06', 'Z16']", |q| matches!(q, "Z06" | "Z16"), "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13012-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13012-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13012", "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13012-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13012-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13012", "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13012-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13012-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13012", "13012", issues);
            })
            .with_named_stateless_rule_fn("AHB-13012-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13012-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['Z04', 'Z06']", |q| matches!(q, "Z04" | "Z06"), "13012", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13012_PACK)
}

static AHB_13013_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13013")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13013-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13013-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13013", "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13013-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z23']", |q| matches!(q, "Z23"), "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13013-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13013-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13013", "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13013-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13013-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13013", "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13013-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13013-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13013", "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13013-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13013-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13013", "13013", issues);
            })
            .with_named_stateless_rule_fn("AHB-13013-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13013-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', 'Z10']", |v| matches!(v, "220" | "Z10"), "13013", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13013_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13013_PACK)
}

static AHB_13014_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13014")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13014-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13014-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13014", "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13014-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z23']", |q| matches!(q, "Z23"), "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13014-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13014-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13014", "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13014-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13014-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13014", "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13014-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13014-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13014", "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13014-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13014-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13014", "13014", issues);
            })
            .with_named_stateless_rule_fn("AHB-13014-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13014-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', 'Z10']", |v| matches!(v, "220" | "Z10"), "13014", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13014_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13014_PACK)
}

static AHB_13015_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13015")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13015-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13015-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13015", "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13015-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z27']", |q| matches!(q, "7" | "Z27"), "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13015-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13015-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13015", "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13015-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13015-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13015", "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13015-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13015-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13015", "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13015-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13015-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13015", "13015", issues);
            })
            .with_named_stateless_rule_fn("AHB-13015-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13015-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13015", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13015_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13015_PACK)
}

static AHB_13016_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13016")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13016-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13016-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13016", "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13016-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z27', 'Z28']", |q| matches!(q, "7" | "Z27" | "Z28"), "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13016-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13016-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13016", "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13016-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13016-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13016", "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13016-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13016-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13016", "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13016-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13016-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13016", "13016", issues);
            })
            .with_named_stateless_rule_fn("AHB-13016-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13016-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13016", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13016_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13016_PACK)
}

static AHB_13017_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13017")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13017-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13017-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13017-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13017-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13017-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13017-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13017-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13017-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13017-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13017-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-13017-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-PIA-M", |segs, issues| {
                ahb_check_mandatory(segs, "PIA", "AHB-13017-PIA-M", "mandatory segment PIA is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-PIA-4347-Q", |segs, issues| {
                ahb_check_qualifier(segs, "PIA", "AHB-13017-PIA-4347-Q", "segment PIA DE 4347 (element 0, component 0): qualifier is not one of the allowed values ['5']", |q| matches!(q, "5"), "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13017-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13017", "13017", issues);
            })
            .with_named_stateless_rule_fn("AHB-13017-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13017-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67', 'Z18']", |v| matches!(v, "220" | "67" | "Z18"), "13017", issues);
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Ersatzwertbildungsverfahren (Z32) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13017-SG10-STS-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z32")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z32\") is missing for Pruefidentifikator 13017 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13017-SG10-STS-I0").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when QTY DE[0]="67" is present in SG10 // [92] Wenn QTY DE6063 mit Wert 67 vorhanden, muss STS Grund der Ersatzwertbildung (Z40) vorhanden sein
            .with_scoped_group_rule_fn("SG10", "AHB-13017-SG10-STS-I1", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "QTY" && s.element_str(0).is_some_and(|v| v == "67")) && !segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z40")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG10: conditional segment STS (DE[0]=\"Z40\") is missing for Pruefidentifikator 13017 (I: when QTY DE[0]=\"67\" is present in SG10)".to_owned()).with_rule_id("AHB-13017-SG10-STS-I1").with_segment("STS".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13017_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13017_PACK)
}

static AHB_13018_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13018")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13018-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13018-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13018", "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13018-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z48']", |q| matches!(q, "7" | "Z48"), "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13018-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13018-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13018", "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13018-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13018-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13018", "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13018-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13018-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13018", "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13018-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13018-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13018", "13018", issues);
            })
            .with_named_stateless_rule_fn("AHB-13018-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13018-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13018", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13018_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13018_PACK)
}

static AHB_13019_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13019")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13019-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13019-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13019", "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13019-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z27']", |q| matches!(q, "7" | "Z27"), "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13019-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13019-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13019", "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13019-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13019-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13019", "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13019-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13019-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13019", "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13019-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13019-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13019", "13019", issues);
            })
            .with_named_stateless_rule_fn("AHB-13019-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13019-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13019", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13019_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13019_PACK)
}

static AHB_13020_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13020")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13020-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13020-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13020", "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13020-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z43']", |q| matches!(q, "Z43"), "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13020-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13020-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13020", "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13020-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13020-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13020", "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13020-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13020-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13020", "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13020-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13020-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13020", "13020", issues);
            })
            .with_named_stateless_rule_fn("AHB-13020-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13020-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220']", |v| matches!(v, "220"), "13020", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13020_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13020_PACK)
}

static AHB_13021_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13021")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13021-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13021-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13021", "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13021-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z44']", |q| matches!(q, "Z44"), "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13021-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13021-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13021", "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13021-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13021-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13021", "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13021-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13021-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13021", "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13021-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13021-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13021", "13021", issues);
            })
            .with_named_stateless_rule_fn("AHB-13021-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13021-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220']", |v| matches!(v, "220"), "13021", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13021_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13021_PACK)
}

static AHB_13022_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13022")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13022-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13022-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13022", "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13022-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z45']", |q| matches!(q, "Z45"), "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13022-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13022-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13022", "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13022-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13022-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13022", "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13022-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13022-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13022", "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13022-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13022-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13022", "13022", issues);
            })
            .with_named_stateless_rule_fn("AHB-13022-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13022-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220']", |v| matches!(v, "220"), "13022", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13022_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13022_PACK)
}

static AHB_13023_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13023")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13023-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13023-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13023", "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13023-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7']", |q| matches!(q, "7"), "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13023-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13023-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13023", "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13023-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13023-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13023", "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13023-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13023-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13023", "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13023-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13023-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13023", "13023", issues);
            })
            .with_named_stateless_rule_fn("AHB-13023-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13023-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13023", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13023_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13023_PACK)
}

static AHB_13025_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13025")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13025-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13025-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13025", "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13025-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['7', 'Z48']", |q| matches!(q, "7" | "Z48"), "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13025-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13025-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13025", "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13025-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13025-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13025", "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13025-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13025-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13025", "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13025-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13025-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13025", "13025", issues);
            })
            .with_named_stateless_rule_fn("AHB-13025-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13025-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67']", |v| matches!(v, "220" | "67"), "13025", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13025_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13025_PACK)
}

static AHB_13026_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13026")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13026-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13026-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13026", "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13026-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z15']", |q| matches!(q, "Z15"), "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13026-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13026-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13026", "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13026-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13026-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13026", "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13026-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13026-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13026", "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13026-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['237']", |q| matches!(q, "237"), "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13026-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13026", "13026", issues);
            })
            .with_named_stateless_rule_fn("AHB-13026-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13026-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220']", |v| matches!(v, "220"), "13026", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13026_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13026_PACK)
}

static AHB_13027_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13027")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13027-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13027-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13027", "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13027-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z83']", |q| matches!(q, "Z83"), "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13027-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13027-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13027", "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13027-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13027-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13027", "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13027-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13027-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13027", "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13027-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13027-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13027", "13027", issues);
            })
            .with_named_stateless_rule_fn("AHB-13027-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13027-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['220', '67', 'Z18']", |v| matches!(v, "220" | "67" | "Z18"), "13027", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13027_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13027_PACK)
}

static AHB_13028_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("MSCONS-AHB-2.5-13028")
            .for_message_type("MSCONS")
            .for_release("2.5")
            .with_named_stateless_rule_fn("AHB-13028-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-13028-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 13028", "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-13028-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['Z85']", |q| matches!(q, "Z85"), "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-BGM-1225-V", |segs, issues| {
                ahb_check_field_value(segs, "BGM", 2, "AHB-13028-BGM-1225-V", "segment BGM DE 1225 (element 2, component 0): value is not one of the allowed values ['9']", |v| matches!(v, "9"), "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-13028-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 13028", "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-DTM-2005-RQ", |segs, issues| {
                ahb_check_required_qualifier(segs, "DTM", "AHB-13028-DTM-2005-RQ", "mandatory segment DTM with DE 2005 qualifier '137' is missing", |q| matches!(q, "137"), "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-13028-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 13028", "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-13028-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-13028-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 13028", "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-LOC-3227-Q", |segs, issues| {
                ahb_check_qualifier(segs, "LOC", "AHB-13028-LOC-3227-Q", "segment LOC DE 3227 (element 0, component 0): qualifier is not one of the allowed values ['172']", |q| matches!(q, "172"), "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-QTY-M", |segs, issues| {
                ahb_check_mandatory(segs, "QTY", "AHB-13028-QTY-M", "mandatory segment QTY is missing for Pruefidentifikator 13028", "13028", issues);
            })
            .with_named_stateless_rule_fn("AHB-13028-QTY-6063-V", |segs, issues| {
                ahb_check_field_value(segs, "QTY", 0, "AHB-13028-QTY-6063-V", "segment QTY DE 6063 (element 0, component 0): value is not one of the allowed values ['Z47']", |v| matches!(v, "Z47"), "13028", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_13028_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_13028_PACK)
}

static AHB_ALL_PACK_MSCONS_2_5: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("MSCONS-AHB-2.5-ALL")
        .for_message_type("MSCONS")
        .for_release("2.5");
    let pack = pack
        .merge_with_override(ahb_13002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13013_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13014_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13015_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13016_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13017_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13018_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13019_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13020_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13021_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13022_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13023_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13025_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13026_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13027_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_13028_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(13002) => ahb_13002_pack(),
            Some(13003) => ahb_13003_pack(),
            Some(13005) => ahb_13005_pack(),
            Some(13006) => ahb_13006_pack(),
            Some(13007) => ahb_13007_pack(),
            Some(13008) => ahb_13008_pack(),
            Some(13009) => ahb_13009_pack(),
            Some(13010) => ahb_13010_pack(),
            Some(13011) => ahb_13011_pack(),
            Some(13012) => ahb_13012_pack(),
            Some(13013) => ahb_13013_pack(),
            Some(13014) => ahb_13014_pack(),
            Some(13015) => ahb_13015_pack(),
            Some(13016) => ahb_13016_pack(),
            Some(13017) => ahb_13017_pack(),
            Some(13018) => ahb_13018_pack(),
            Some(13019) => ahb_13019_pack(),
            Some(13020) => ahb_13020_pack(),
            Some(13021) => ahb_13021_pack(),
            Some(13022) => ahb_13022_pack(),
            Some(13023) => ahb_13023_pack(),
            Some(13025) => ahb_13025_pack(),
            Some(13026) => ahb_13026_pack(),
            Some(13027) => ahb_13027_pack(),
            Some(13028) => ahb_13028_pack(),
            None => Arc::clone(&AHB_ALL_PACK_MSCONS_2_5),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("MSCONS")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_MSCONS_FV20261001: LazyLock<Release> = LazyLock::new(|| Release::new("2.5"));

pub(crate) struct MsconsFv20261001Profile;

impl Profile for MsconsFv20261001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Mscons
    }
    fn release(&self) -> &Release {
        &RELEASE_MSCONS_FV20261001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.5")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("MSCONS AHB 2.5, Stand 01.10.2026")
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

pub(crate) static PROFILE: MsconsFv20261001Profile = MsconsFv20261001Profile;
