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
        tag: "UCI",
        name: "Übertragungsdatei-Antwort",
        elements: &[
            ElementRef::new(1, "0020", Status::Mandatory, 1),
            ElementRef::new(2, "S002", Status::Mandatory, 1),
            ElementRef::new(3, "S003", Status::Mandatory, 1),
            ElementRef::new(4, "0083", Status::Mandatory, 1),
            ElementRef::new(5, "0085", Status::Conditional, 1),
            ElementRef::new(6, "0013", Status::Conditional, 1),
            ElementRef::new(7, "S011", Status::Conditional, 1),
        ],
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
        tag: "UCM",
        name: "Nachrichtenantwort",
        elements: &[
            ElementRef::new(1, "0062", Status::Conditional, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
            ElementRef::new(3, "0083", Status::Mandatory, 1),
            ElementRef::new(4, "0085", Status::Conditional, 1),
            ElementRef::new(5, "0013", Status::Conditional, 1),
            ElementRef::new(6, "S011", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "UCS",
        name: "Segment-Fehleranzeige",
        elements: &[
            ElementRef::new(1, "0096", Status::Mandatory, 1),
            ElementRef::new(2, "0085", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "UCD",
        name: "Datenelement-Fehleranzeige",
        elements: &[
            ElementRef::new(1, "0085", Status::Mandatory, 1),
            ElementRef::new(2, "S011", Status::Mandatory, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_0013: &[&str] = &["UNA", "UNB", "UNH", "UNT", "UNZ"];
static CODES_0083: &[&str] = &["4", "7"];
static CODES_0085: &[&str] = &[
    "10", "11", "12", "13", "14", "15", "16", "17", "18", "19", "2", "20", "21", "22", "23", "24",
    "25", "26", "27", "28", "29", "30", "31", "32", "33", "34", "35", "36", "37", "38", "39", "40",
    "41", "42", "43", "44", "45", "5", "6", "7",
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
        ("UNH" | "UCI" | "UNT" | "UCM" | "UCS" | "UCD", 0)
        | ("UCI" | "UCM", 3 | 4)
        | ("UCI", 5)
        | ("UNT" | "UCS", 1)
        | ("UCM", 2) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "0013" => Some(CODES_0013),
        "0083" => Some(CODES_0083),
        "0085" => Some(CODES_0085),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_CONTRL_2_0B: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-CONTRL-2.0b",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_CONTRL_2_0B
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

fn rule_uci_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "UCI") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment UCI is missing".to_owned(),
            )
            .with_rule_id("MIG-UCI-REQ")
            .with_segment("UCI".to_owned()),
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

/// Layer 3 — verify the `UCM` segment group appears at most 999999 times.
///
/// Each occurrence of the trigger segment `UCM` marks the start of
/// one group instance.  The MIG specifies a maximum of 999999 instances.
fn rule_group_sg1_ucm_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "UCM").count();
    if count > 999_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by UCM occurs {count} times; maximum is 999_999"),
            )
            .with_rule_id("MIG-CONTRL-MIG-2.0b-GROUP-SG1-UCM-CARD-MAX")
            .with_segment("UCM".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const EXPECTED_ORDER: &[&str] = &["UNH", "UCI", "UCM", "UNT"];
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
                .with_rule_id("MIG-CONTRL-MIG-2.0b-ORDER")
                .with_segment(seg.tag.to_owned()),
            );
        }
        // Unknown tags are passed through — they get caught by the DirectoryValidator.
    }
}

static MIG_CONTRL_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("CONTRL-MIG-2.0b")
            .for_message_type("CONTRL")
            .for_release("2.0b")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_uci_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_group_sg1_ucm_max_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_CONTRL_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_ALL_PACK_CONTRL_2_0B: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("CONTRL-AHB-2.0b-ALL")
            .for_message_type("CONTRL")
            .for_release("2.0b"),
    )
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            None => Arc::clone(&AHB_ALL_PACK_CONTRL_2_0B),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("CONTRL")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_CONTRL_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("2.0b"));

pub(crate) struct ContrlFv20251001Profile;

impl Profile for ContrlFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Contrl
    }
    fn release(&self) -> &Release {
        &RELEASE_CONTRL_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 12 - 31))
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.0b")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("CONTRL AHB 2.0b, Stand 01.10.2025")
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

pub(crate) static PROFILE: ContrlFv20251001Profile = ContrlFv20251001Profile;
