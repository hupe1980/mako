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
        name: "Datum/Uhrzeit",
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
        name: "Referenz/Prüfidentifikator",
        elements: &[ElementRef::new(1, "C506", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "NAD",
        name: "MP-ID Empfänger/Absender",
        elements: &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LOC",
        name: "Regelzone",
        elements: &[
            ElementRef::new(1, "3227", Status::Mandatory, 1),
            ElementRef::new(2, "C517", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CUX",
        name: "Währungsangaben",
        elements: &[ElementRef::new(1, "C504", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "PGI",
        name: "Produktgruppen-Information",
        elements: &[ElementRef::new(1, "5379", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "PIA",
        name: "Preisschlüsselstamm",
        elements: &[
            ElementRef::new(1, "4347", Status::Mandatory, 1),
            ElementRef::new(2, "C212", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Produktbeschreibung",
        elements: &[
            ElementRef::new(1, "7077", Status::Conditional, 1),
            ElementRef::new(2, "C272", Status::Conditional, 1),
            ElementRef::new(3, "C273", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "PRI",
        name: "Preisangaben",
        elements: &[ElementRef::new(1, "C509", Status::Conditional, 1)],
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

static CODES_1001: &[&str] = &["Z04", "Z32", "Z54", "Z64", "Z67", "Z70", "Z77", "Z94"];
static CODES_1153: &[&str] = &["ACW", "Z13", "Z56"];
static CODES_2005: &[&str] = &["137", "157", "163", "164", "492"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_5125: &[&str] = &["CAL"];
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

fn expected_components(tag: &str, idx: usize) -> Option<u8> {
    match (tag, idx) {
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "5125" => Some(CODES_5125),
        "6167" => Some(CODES_6167),
        "6343" => Some(CODES_6343),
        "6347" => Some(CODES_6347),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile.
static DIRECTORY_VALIDATOR_PRICAT_2_1: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-PRICAT-2.1",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_PRICAT_2_1
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
            .with_rule_id("MIG-PRICAT-MIG-2.1-GROUP-SG1-RFF-CARD-MAX")
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
            .with_rule_id("MIG-PRICAT-MIG-2.1-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CUX` segment group appears at most 20 times.
///
/// Each occurrence of the trigger segment `CUX` marks the start of
/// one group instance.  The MIG specifies a maximum of 20 instances.
fn rule_group_sg6_cux_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CUX").count();
    if count > 20 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by CUX occurs {count} times; maximum is 20"),
            )
            .with_rule_id("MIG-PRICAT-MIG-2.1-GROUP-SG6-CUX-CARD-MAX")
            .with_segment("CUX".to_owned()),
        );
    }
}

/// Layer 3 — verify the `PGI` segment group appears at most 1000 times.
///
/// Each occurrence of the trigger segment `PGI` marks the start of
/// one group instance.  The MIG specifies a maximum of 1000 instances.
fn rule_group_sg17_pgi_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "PGI").count();
    if count > 1_000 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by PGI occurs {count} times; maximum is 1_000"),
            )
            .with_rule_id("MIG-PRICAT-MIG-2.1-GROUP-SG17-PGI-CARD-MAX")
            .with_segment("PGI".to_owned()),
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
            .with_rule_id("MIG-PRICAT-MIG-2.1-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
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
            "SG1" => &["RFF"],
            "SG2" => &["NAD", "LOC"],
            "SG6" => &["CUX"],
            "SG17" => &["PGI"],
            "SG36" => &["LIN", "PIA", "IMD"],
            "SG40" => &["PRI", "RNG", "DTM"],
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
    check_order(&tree, segments, "MIG-PRICAT-MIG-2.1-ORDER", issues);
}

static MIG_PRICAT_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PRICAT-MIG-2.1")
            .for_message_type("PRICAT")
            .for_release("2.1")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg6_cux_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg17_pgi_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_PRICAT_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="Z94" is present // [57] wenn BGM DE1001=Z94 (Preisblatt Technik) vorhanden, ist IMD Pflicht
fn rule_ahb_27001_imd_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z94"));
    if condition_holds && !segments.iter().any(|s| s.tag == "IMD") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment IMD is missing for Pruefidentifikator 27001 (I: when BGM DE[0]=\"Z94\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27001-IMD-I0")
                .with_segment("IMD".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27001".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z32" is present // [31] wenn BGM DE1001=Z32 (Preisblatt Messstellenbetrieb) vorhanden, ist PIA Pflicht
fn rule_ahb_27001_pia_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z32"));
    if condition_holds && !segments.iter().any(|s| s.tag == "PIA") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment PIA is missing for Pruefidentifikator 27001 (I: when BGM DE[0]=\"Z32\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27001-PIA-I0")
                .with_segment("PIA".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27001".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z77" is present // [34] wenn BGM DE1001=Z77 (Preisblatt Konfigurationen) vorhanden, ist RNG Pflicht
fn rule_ahb_27001_rng_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z77"));
    if condition_holds && !segments.iter().any(|s| s.tag == "RNG") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RNG is missing for Pruefidentifikator 27001 (I: when BGM DE[0]=\"Z77\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27001-RNG-I0")
                .with_segment("RNG".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27001".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z94" is present // [57] wenn BGM DE1001=Z94 (Preisblatt Technik) vorhanden, ist RNG Pflicht
fn rule_ahb_27001_rng_cond_1(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z94"));
    if condition_holds && !segments.iter().any(|s| s.tag == "RNG") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RNG is missing for Pruefidentifikator 27001 (I: when BGM DE[0]=\"Z94\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27001-RNG-I1")
                .with_segment("RNG".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27001".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z32" is present // [31] wenn BGM DE1001=Z32 (Preisblatt Messstellenbetrieb) vorhanden, ist PIA Pflicht
fn rule_ahb_27002_pia_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z32"));
    if condition_holds && !segments.iter().any(|s| s.tag == "PIA") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment PIA is missing for Pruefidentifikator 27002 (I: when BGM DE[0]=\"Z32\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27002-PIA-I0")
                .with_segment("PIA".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27002".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z94" is present // [57] wenn BGM DE1001=Z94 (Preisblatt Technik) vorhanden, ist IMD Pflicht
fn rule_ahb_27002_imd_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z94"));
    if condition_holds && !segments.iter().any(|s| s.tag == "IMD") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment IMD is missing for Pruefidentifikator 27002 (I: when BGM DE[0]=\"Z94\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27002-IMD-I0")
                .with_segment("IMD".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27002".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z77" is present // [34] wenn BGM DE1001=Z77 (Preisblatt Konfigurationen) vorhanden, ist RNG Pflicht
fn rule_ahb_27002_rng_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z77"));
    if condition_holds && !segments.iter().any(|s| s.tag == "RNG") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RNG is missing for Pruefidentifikator 27002 (I: when BGM DE[0]=\"Z77\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27002-RNG-I0")
                .with_segment("RNG".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27002".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="Z94" is present // [57] wenn BGM DE1001=Z94 (Preisblatt Technik) vorhanden, ist RNG Pflicht
fn rule_ahb_27002_rng_cond_1(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "Z94"));
    if condition_holds && !segments.iter().any(|s| s.tag == "RNG") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment RNG is missing for Pruefidentifikator 27002 (I: when BGM DE[0]=\"Z94\" is present)".to_owned(),
                )
                .with_rule_id("AHB-27002-RNG-I1")
                .with_segment("RNG".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "27002".to_owned()));
    }
}

static AHB_27001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PRICAT-AHB-2.1-27001")
            .for_message_type("PRICAT")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-27001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-27001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-27001-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-27001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_27001_imd_cond_0)
            .with_named_stateless_rule_fn("AHB-27001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-27001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-27001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-27001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-PGI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PGI",
                    "AHB-27001-PGI-M",
                    "mandatory segment PGI is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_27001_pia_cond_0)
            .with_named_stateless_rule_fn("AHB-27001-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-27001-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-27001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 27001",
                    "27001",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_27001_rng_cond_0)
            .with_stateless_rule_fn(rule_ahb_27001_rng_cond_1)
            .with_max_issues_per_rule(50),
    )
});

fn ahb_27001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_27001_PACK)
}

static AHB_27002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PRICAT-AHB-2.1-27002")
            .for_message_type("PRICAT")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-27002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-27002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-27002-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-27002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-27002-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-27002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-PGI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PGI",
                    "AHB-27002-PGI-M",
                    "mandatory segment PGI is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-27002-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-27002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 27002",
                    "27002",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_27002_pia_cond_0)
            .with_stateless_rule_fn(rule_ahb_27002_imd_cond_0)
            .with_stateless_rule_fn(rule_ahb_27002_rng_cond_0)
            .with_stateless_rule_fn(rule_ahb_27002_rng_cond_1)
            .with_max_issues_per_rule(50),
    )
});

fn ahb_27002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_27002_PACK)
}

static AHB_27003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("PRICAT-AHB-2.1-27003")
            .for_message_type("PRICAT")
            .for_release("2.1")
            .with_named_stateless_rule_fn("AHB-27003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-27003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-27003-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-27003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-27003-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-27003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-PGI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PGI",
                    "AHB-27003-PGI-M",
                    "mandatory segment PGI is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-27003-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-27003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-27003-RNG-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RNG",
                    "AHB-27003-RNG-M",
                    "mandatory segment RNG is missing for Pruefidentifikator 27003",
                    "27003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_27003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_27003_PACK)
}

static AHB_ALL_PACK_PRICAT_2_1: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("PRICAT-AHB-2.1-ALL")
        .for_message_type("PRICAT")
        .for_release("2.1");
    let pack = pack
        .merge_with_override(ahb_27001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_27002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_27003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(27001) => ahb_27001_pack(),
            Some(27002) => ahb_27002_pack(),
            Some(27003) => ahb_27003_pack(),
            None => Arc::clone(&AHB_ALL_PACK_PRICAT_2_1),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("PRICAT")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_PRICAT_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("2.1"));

pub(crate) struct PricatFv20260401Profile;

impl Profile for PricatFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Pricat
    }
    fn release(&self) -> &Release {
        &RELEASE_PRICAT_FV20260401
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 04 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.1")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("PRICAT AHB 2.1, Stand 01.04.2026")
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

pub(crate) static PROFILE: PricatFv20260401Profile = PricatFv20260401Profile;
