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
        name: "Referenz",
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
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["Z51", "Z52", "Z57"];
static CODES_1153: &[&str] = &["ON", "TN", "Z13"];
static CODES_1225: &[&str] = &["1"];
static CODES_2005: &[&str] = &["137"];
static CODES_3035: &[&str] = &["MR", "MS"];

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
        "1225" => Some(CODES_1225),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_ORDCHG_1_1: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-ORDCHG-1.1",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_ORDCHG_1_1
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
            .with_rule_id("MIG-ORDCHG-MIG-1.1-GROUP-SG1-RFF-CARD-MAX")
            .with_segment("RFF".to_owned()),
        );
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg3_nad_max_occurrences(
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
            .with_rule_id("MIG-ORDCHG-MIG-1.1-GROUP-SG3-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
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
            .with_rule_id("MIG-ORDCHG-MIG-1.1-GROUP-SG1-RFF-CARD-MIN")
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
            .with_rule_id("MIG-ORDCHG-MIG-1.1-GROUP-SG3-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
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
    const EXPECTED_DETAIL_ORDER: &[&str] = &["UNT"];

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
        "MIG-ORDCHG-MIG-1.1-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-ORDCHG-MIG-1.1-ORDER",
        issues,
    );
}

static MIG_ORDCHG_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDCHG-MIG-1.1")
            .for_message_type("ORDCHG")
            .for_release("1.1")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg3_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_ORDCHG_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="Z52" is present // [3] Wenn BGM+Z52 (Entsperrung) vorhanden, ist RFF+TN (Transaktions-Referenznummer) Pflicht
fn rule_ahb_39000_rff_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z52"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "RFF" && s.element_str(0).is_some_and(|v| v == "TN"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RFF (DE[0]=\"TN\") is missing for Pruefidentifikator 39000 (I: when BGM DE[0]=\"Z52\" is present)".to_owned(),
                )
                .with_rule_id("AHB-39000-RFF-I0")
                .with_segment("RFF".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "39000".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z52" is present // [3] Wenn BGM+Z52 (Entsperrung) vorhanden, ist RFF+TN (Transaktions-Referenznummer) Pflicht
fn rule_ahb_39001_rff_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z52"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "RFF" && s.element_str(0).is_some_and(|v| v == "TN"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RFF (DE[0]=\"TN\") is missing for Pruefidentifikator 39001 (I: when BGM DE[0]=\"Z52\" is present)".to_owned(),
                )
                .with_rule_id("AHB-39001-RFF-I0")
                .with_segment("RFF".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "39001".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z52" is present // [3] Wenn BGM+Z52 (Entsperrung) vorhanden, ist RFF+TN (Transaktions-Referenznummer) Pflicht
fn rule_ahb_39002_rff_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z52"));
    if condition_holds
        && !segments
            .iter()
            .any(|s| s.tag == "RFF" && s.element_str(0).is_some_and(|v| v == "TN"))
    {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RFF (DE[0]=\"TN\") is missing for Pruefidentifikator 39002 (I: when BGM DE[0]=\"Z52\" is present)".to_owned(),
                )
                .with_rule_id("AHB-39002-RFF-I0")
                .with_segment("RFF".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "39002".to_owned()));
    }
}

static AHB_39000_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDCHG-AHB-1.1-39000")
            .for_message_type("ORDCHG")
            .for_release("1.1")
            .with_named_stateless_rule_fn("AHB-39000-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-39000-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 39000",
                    "39000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39000-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-39000-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 39000",
                    "39000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39000-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-39000-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 39000",
                    "39000",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39000-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-39000-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 39000",
                    "39000",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_39000_rff_cond_0)
            .with_max_issues_per_rule(50),
    )
});

fn ahb_39000_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_39000_PACK)
}

static AHB_39001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDCHG-AHB-1.1-39001")
            .for_message_type("ORDCHG")
            .for_release("1.1")
            .with_named_stateless_rule_fn("AHB-39001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-39001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 39001",
                    "39001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-39001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 39001",
                    "39001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-39001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 39001",
                    "39001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-39001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 39001",
                    "39001",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_39001_rff_cond_0)
            .with_max_issues_per_rule(50),
    )
});

fn ahb_39001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_39001_PACK)
}

static AHB_39002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDCHG-AHB-1.1-39002")
            .for_message_type("ORDCHG")
            .for_release("1.1")
            .with_named_stateless_rule_fn("AHB-39002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-39002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 39002",
                    "39002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-39002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 39002",
                    "39002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-39002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 39002",
                    "39002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-39002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-39002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 39002",
                    "39002",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_39002_rff_cond_0)
            .with_max_issues_per_rule(50),
    )
});

fn ahb_39002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_39002_PACK)
}

static AHB_ALL_PACK_ORDCHG_1_1: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("ORDCHG-AHB-1.1-ALL")
        .for_message_type("ORDCHG")
        .for_release("1.1");
    let pack = pack
        .merge_with_override(ahb_39000_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_39001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_39002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(39000) => ahb_39000_pack(),
            Some(39001) => ahb_39001_pack(),
            Some(39002) => ahb_39002_pack(),
            None => Arc::clone(&AHB_ALL_PACK_ORDCHG_1_1),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("ORDCHG")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_ORDCHG_FV20241001: LazyLock<Release> = LazyLock::new(|| Release::new("1.1"));

pub(crate) struct OrdchgFv20241001Profile;

impl Profile for OrdchgFv20241001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Ordchg
    }
    fn release(&self) -> &Release {
        &RELEASE_ORDCHG_FV20241001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2024 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 09 - 30))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.1")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("ORDCHG AHB 1.1, Stand 01.10.2024")
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

pub(crate) static PROFILE: OrdchgFv20241001Profile = OrdchgFv20241001Profile;
