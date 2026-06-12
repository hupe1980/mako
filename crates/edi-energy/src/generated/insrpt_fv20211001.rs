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
        tag: "DOC",
        name: "Dokument-/Nachricht-Einzelheiten",
        elements: &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C503", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Bezugsnummer des Dokuments",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
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
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "STS",
        name: "Gerätestatus",
        elements: &[
            ElementRef::new(1, "C601", Status::Conditional, 1),
            ElementRef::new(2, "C555", Status::Conditional, 1),
            ElementRef::new(3, "C556", Status::Conditional, 1),
        ],
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
    SegmentDefinition {
        tag: "LOC",
        name: "Meldepunkt",
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

static CODES_1001: &[&str] = &["21", "22", "23", "293", "4"];
static CODES_1153: &[&str] = &["AAV", "Z13", "Z21"];
static CODES_2005: &[&str] = &["137", "163", "164", "292", "9"];
static CODES_3035: &[&str] = &["CC", "DP", "MR", "MS"];
static CODES_3139: &[&str] = &["IC"];
static CODES_3155: &[&str] = &["AJ", "AL", "EM", "FX", "TE"];
static CODES_3227: &[&str] = &["172"];
static CODES_4451: &[&str] = &["AAO", "ACD"];

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
        | ("CTA", 0)
        | ("LIN", 0)
        | ("FTX", 0)
        | ("FTX", 1)
        | ("LOC", 0) => Some(1),
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
        "3227" => Some(CODES_3227),
        "4451" => Some(CODES_4451),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_INSRPT_1_1A: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-INSRPT-1.1a",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_INSRPT_1_1A
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

fn rule_lin_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "LIN") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment LIN is missing".to_owned(),
            )
            .with_rule_id("MIG-LIN-REQ")
            .with_segment("LIN".to_owned()),
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
            .with_rule_id("MIG-INSRPT-MIG-1.1a-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `DOC` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg3_doc_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DOC").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by DOC occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-INSRPT-MIG-1.1a-GROUP-SG3-DOC-CARD-MAX")
            .with_segment("DOC".to_owned()),
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
            .with_rule_id("MIG-INSRPT-MIG-1.1a-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg3_doc_min_occurrences(
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
            .with_rule_id("MIG-INSRPT-MIG-1.1a-GROUP-SG3-DOC-CARD-MIN")
            .with_segment("DOC".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &["UNH", "BGM", "DTM", "NAD", "DOC", "UNT"];
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
                .with_rule_id("MIG-INSRPT-MIG-1.1a-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_INSRPT_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INSRPT-MIG-1.1a")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_doc_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_lin_mandatory)
            .with_stateless_rule_fn(rule_loc_mandatory)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_doc_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_doc_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_INSRPT_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG3",
    trigger: "DOC",
    children: &[GroupDef {
        name: "SG7",
        trigger: "LIN",
        children: &[],
    }],
}];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_23001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23001")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23001-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23001-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23001-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23001-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23001-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23001-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23001-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23001-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23001", "23001", issues);
            })
            .with_named_stateless_rule_fn("AHB-23001-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23001-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23001", "23001", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23001-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23001 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23001-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23001-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23001 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23001-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23001_PACK)
}

static AHB_23003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23003")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23003-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23003-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23003-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23003-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23003-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23003-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23003-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23003-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23003", "23003", issues);
            })
            .with_named_stateless_rule_fn("AHB-23003-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23003-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23003", "23003", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23003-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23003 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23003-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23003-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23003 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23003-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23003_PACK)
}

static AHB_23004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23004")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23004-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23004-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23004-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23004-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23004-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23004-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23004-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23004-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23004", "23004", issues);
            })
            .with_named_stateless_rule_fn("AHB-23004-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23004-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23004", "23004", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23004-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23004 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23004-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23004-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23004 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23004-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23004_PACK)
}

static AHB_23005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23005")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23005-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23005-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23005-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23005-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23005-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23005-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23005-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23005-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23005", "23005", issues);
            })
            .with_named_stateless_rule_fn("AHB-23005-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23005-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23005", "23005", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23005-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23005 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23005-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23005_PACK)
}

static AHB_23008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23008")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23008-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23008-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-COM-M", |segs, issues| {
                ahb_check_mandatory(segs, "COM", "AHB-23008-COM-M", "mandatory segment COM is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-CTA-M", |segs, issues| {
                ahb_check_mandatory(segs, "CTA", "AHB-23008-CTA-M", "mandatory segment CTA is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23008-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23008-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23008-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23008-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23008-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23008-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23008", "23008", issues);
            })
            .with_named_stateless_rule_fn("AHB-23008-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23008-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23008", "23008", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23008-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23008 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23008-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23008-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23008 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23008-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23008_PACK)
}

static AHB_23009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23009")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23009-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23009-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23009-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23009-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23009-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23009-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23009-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23009-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23009", "23009", issues);
            })
            .with_named_stateless_rule_fn("AHB-23009-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23009-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23009", "23009", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23009-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23009 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23009-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23009_PACK)
}

static AHB_23011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23011")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23011-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23011-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23011-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23011-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23011-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23011-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23011-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23011-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23011", "23011", issues);
            })
            .with_named_stateless_rule_fn("AHB-23011-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23011-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23011", "23011", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23011-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23011 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23011-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23011-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23011 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23011-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23011_PACK)
}

static AHB_23012_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(ProfileRulePack::new("INSRPT-AHB-1.1a-23012")
            .for_message_type("INSRPT")
            .for_release("1.1a")
            .with_named_stateless_rule_fn("AHB-23012-BGM-M", |segs, issues| {
                ahb_check_mandatory(segs, "BGM", "AHB-23012-BGM-M", "mandatory segment BGM is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-DOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "DOC", "AHB-23012-DOC-M", "mandatory segment DOC is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-DTM-M", |segs, issues| {
                ahb_check_mandatory(segs, "DTM", "AHB-23012-DTM-M", "mandatory segment DTM is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-LIN-M", |segs, issues| {
                ahb_check_mandatory(segs, "LIN", "AHB-23012-LIN-M", "mandatory segment LIN is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-LOC-M", |segs, issues| {
                ahb_check_mandatory(segs, "LOC", "AHB-23012-LOC-M", "mandatory segment LOC is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-NAD-M", |segs, issues| {
                ahb_check_mandatory(segs, "NAD", "AHB-23012-NAD-M", "mandatory segment NAD is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-RFF-M", |segs, issues| {
                ahb_check_mandatory(segs, "RFF", "AHB-23012-RFF-M", "mandatory segment RFF is missing for Pruefidentifikator 23012", "23012", issues);
            })
            .with_named_stateless_rule_fn("AHB-23012-STS-M", |segs, issues| {
                ahb_check_mandatory(segs, "STS", "AHB-23012-STS-M", "mandatory segment STS is missing for Pruefidentifikator 23012", "23012", issues);
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10" is present in SG7 // [8] Wenn SG7 STS+Z06+Z10 vorhanden, DTM 164 (Verarbeitung Endedatum) ist Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23012-SG7-DTM-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10")) && !segs.iter().any(|s| s.tag == "DTM" && s.element_str(0).is_some_and(|v| v == "164")) {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment DTM (DE[0]=\"164\") is missing for Pruefidentifikator 23012 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\" is present in SG7)".to_owned()).with_rule_id("AHB-23012-SG7-DTM-I0").with_segment("DTM".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })

            // Bedingungsoperator I — I: when STS DE[0]="Z06"+DE[1]="Z10"+DE[2]="ZC1" is present in SG7 // [2] Wenn SG7 STS+Z06+Z10+ZC1 (Ablehnung wegen bestimmter Ursache) vorhanden, ist FTX (AAO Fehlerbeschreibung) Pflicht
            .with_scoped_group_rule_fn("SG7", "AHB-23012-SG7-FTX-I0", |group, segs, _ctx, issues| {
                let __gs_start = issues.len();
                if segs.iter().any(|s| s.tag == "STS" && s.element_str(0).is_some_and(|v| v == "Z06") && s.element_str(1).is_some_and(|v| v == "Z10") && s.element_str(2).is_some_and(|v| v == "ZC1")) && !segs.iter().any(|s| s.tag == "FTX") {
                    issues.push(ValidationIssue::new(ValidationSeverity::Error, "in SG7: conditional segment FTX is missing for Pruefidentifikator 23012 (I: when STS DE[0]=\"Z06\"+DE[1]=\"Z10\"+DE[2]=\"ZC1\" is present in SG7)".to_owned()).with_rule_id("AHB-23012-SG7-FTX-I0").with_segment("FTX".to_owned()));
                }
                for __gi in &mut issues[__gs_start..] {
                    __gi.context.push(("group_occurrence".to_owned(), group.occurrence_index.to_string()));
                }
            })
            .with_max_issues_per_rule(50)
        )
});

fn ahb_23012_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_23012_PACK)
}

static AHB_ALL_PACK_INSRPT_1_1A: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("INSRPT-AHB-1.1a-ALL")
        .for_message_type("INSRPT")
        .for_release("1.1a");
    let pack = pack
        .merge_with_override(ahb_23001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_23012_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(23001) => ahb_23001_pack(),
            Some(23003) => ahb_23003_pack(),
            Some(23004) => ahb_23004_pack(),
            Some(23005) => ahb_23005_pack(),
            Some(23008) => ahb_23008_pack(),
            Some(23009) => ahb_23009_pack(),
            Some(23011) => ahb_23011_pack(),
            Some(23012) => ahb_23012_pack(),
            None => Arc::clone(&AHB_ALL_PACK_INSRPT_1_1A),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("INSRPT")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_INSRPT_FV20211001: LazyLock<Release> = LazyLock::new(|| Release::new("1.1a"));

pub(crate) struct InsrptFv20211001Profile;

impl Profile for InsrptFv20211001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Insrpt
    }
    fn release(&self) -> &Release {
        &RELEASE_INSRPT_FV20211001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2021 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 12 - 31))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.1a")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("INSRPT AHB 1.1a, Stand 01.10.2021")
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

pub(crate) static PROFILE: InsrptFv20211001Profile = InsrptFv20211001Profile;
