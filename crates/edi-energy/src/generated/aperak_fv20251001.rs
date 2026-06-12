// @generated — do not edit by hand; run `cargo xtask codegen` to regenerate
#![allow(clippy::doc_markdown)]

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
            ElementRef::new(3, "1225", Status::Conditional, 1),
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
        tag: "ERC",
        name: "Fehlercode",
        elements: &[ElementRef::new(1, "C901", Status::Mandatory, 1)],
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

static CODES_1001: &[&str] = &["312", "313"];
static CODES_1153: &[&str] = &["ACD", "ACW", "AWR", "TN", "Z13"];
static CODES_2005: &[&str] = &["137"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_4451: &[&str] = &["AAI", "ABO", "ZZZ"];
static CODES_9321: &[&str] = &[
    "Z10", "Z14", "Z17", "Z18", "Z19", "Z20", "Z21", "Z24", "Z25", "Z26", "Z27", "Z28", "Z29",
    "Z30", "Z31", "Z32", "Z33", "Z34", "Z35", "Z36", "Z37", "Z38", "Z39", "Z40", "Z41", "Z42",
    "Z43", "Z44", "Z45", "Z46", "Z47", "Z48", "Z49", "Z50", "Z51", "Z52", "Z53", "Z54", "Z55",
    "Z56", "Z57", "Z58", "Z59", "Z60", "Z61", "Z62", "Z63", "Z64", "Z65", "Z66", "Z67", "Z68",
    "Z69", "Z70", "Z71", "Z72", "Z73", "Z74", "Z75", "Z76", "Z77", "Z78",
];

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
        "4451" => Some(CODES_4451),
        "9321" => Some(CODES_9321),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_APERAK_2_1I: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-APERAK-2.1i",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_APERAK_2_1I
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

/// Layer 3 — verify the `NAD` segment group appears at most 2 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 2 instances.
fn rule_group_sg3_nad_max_occurrences(
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
            .with_rule_id("MIG-APERAK-MIG-2.1i-GROUP-SG3-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `ERC` segment group appears at most 99999 times.
///
/// Each occurrence of the trigger segment `ERC` marks the start of
/// one group instance.  The MIG specifies a maximum of 99999 instances.
fn rule_group_sg4_erc_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "ERC").count();
    if count > 99_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by ERC occurs {count} times; maximum is 99_999"),
            )
            .with_rule_id("MIG-APERAK-MIG-2.1i-GROUP-SG4-ERC-CARD-MAX")
            .with_segment("ERC".to_owned()),
        );
    }
}

/// Layer 3 — verify the `RFF` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg2_rff_min_occurrences(
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
            .with_rule_id("MIG-APERAK-MIG-2.1i-GROUP-SG2-RFF-CARD-MIN")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg3_nad_min_occurrences(
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
            .with_rule_id("MIG-APERAK-MIG-2.1i-GROUP-SG3-NAD-CARD-MIN")
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
        "UNH", "BGM", "DTM", "RFF", "NAD", "CTA", "COM", "ERC", "FTX", "UNT",
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
                .with_rule_id("MIG-APERAK-MIG-2.1i-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_APERAK_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("APERAK-MIG-2.1i")
            .for_message_type("APERAK")
            .for_release("2.1i")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_group_sg3_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_erc_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_APERAK_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG2",
    trigger: "RFF",
    children: &[],
}];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_29001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("APERAK-AHB-2.1i-29001")
            .for_message_type("APERAK")
            .for_release("2.1i")
            .with_named_stateless_rule_fn("AHB-29001-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-29001-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 29001", "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-29001-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['313']", |q| matches!(q, "313"), "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-29001-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 29001", "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-DTM-2005-Q", |segs, issues| {
                ahb_check_qualifier(segs, "DTM", "AHB-29001-DTM-2005-Q", "segment DTM DE 2005 (element 0, component 0): qualifier is not one of the allowed values ['137']", |q| matches!(q, "137"), "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-29001-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 29001", "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-29001-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-29001-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 29001", "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-29001-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['ACW']", |q| matches!(q, "ACW"), "29001", issues);
            })
            .with_named_stateless_rule_fn("AHB-29001-ERC-M", |segs, issues| {
                ahb_check_mandatory(segs, "ERC", "AHB-29001-ERC-M", "mandatory segment ERC is missing for Pruefidentifikator 29001", "29001", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_29001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_29001_PACK)
}

static AHB_29002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("APERAK-AHB-2.1i-29002")
            .for_message_type("APERAK")
            .for_release("2.1i")
            .with_named_stateless_rule_fn("AHB-29002-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-29002-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 29002", "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-BGM-1001-Q", |segs, issues| {
                ahb_check_qualifier(segs, "BGM", "AHB-29002-BGM-1001-Q", "segment BGM DE 1001 (element 0, component 0): qualifier is not one of the allowed values ['312']", |q| matches!(q, "312"), "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-29002-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 29002", "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-DTM-2005-Q", |segs, issues| {
                ahb_check_qualifier(segs, "DTM", "AHB-29002-DTM-2005-Q", "segment DTM DE 2005 (element 0, component 0): qualifier is not one of the allowed values ['137']", |q| matches!(q, "137"), "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-29002-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 29002", "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-NAD-3035-Q", |segs, issues| {
                ahb_check_qualifier(segs, "NAD", "AHB-29002-NAD-3035-Q", "segment NAD DE 3035 (element 0, component 0): qualifier is not one of the allowed values ['MS', 'MR']", |q| matches!(q, "MS" | "MR"), "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-29002-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 29002", "29002", issues);
            })
            .with_named_stateless_rule_fn("AHB-29002-RFF-1153-Q", |segs, issues| {
                ahb_check_qualifier(segs, "RFF", "AHB-29002-RFF-1153-Q", "segment RFF DE 1153 (element 0, component 0): qualifier is not one of the allowed values ['ACW']", |q| matches!(q, "ACW"), "29002", issues);
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_29002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_29002_PACK)
}

static AHB_ALL_PACK_APERAK_2_1I: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("APERAK-AHB-2.1i-ALL")
        .for_message_type("APERAK")
        .for_release("2.1i");
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
            None => Arc::clone(&AHB_ALL_PACK_APERAK_2_1I),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("APERAK")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_APERAK_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("2.1i"));

pub(crate) struct AperakFv20251001Profile;

impl Profile for AperakFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Aperak
    }
    fn release(&self) -> &Release {
        &RELEASE_APERAK_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 09 - 30))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.1i")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("APERAK AHB 2.1i, Stand 01.10.2025")
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

pub(crate) static PROFILE: AperakFv20251001Profile = AperakFv20251001Profile;
