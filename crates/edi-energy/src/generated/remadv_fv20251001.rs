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
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Date/Time/Period (Dokumentendatum)",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "RFF",
        name: "Reference (Prüfidentifikator)",
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
        tag: "CUX",
        name: "Currencies (Zahlungswährung)",
        elements: &[ElementRef::new(1, "C504", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "DOC",
        name: "Document/Message Details (Rückmeldung)",
        elements: &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C503", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "MOA",
        name: "Monetary Amount",
        elements: &[ElementRef::new(1, "C516", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "AJT",
        name: "Adjustment Details (Abweichungsgrund)",
        elements: &[
            ElementRef::new(1, "4465", Status::Mandatory, 1),
            ElementRef::new(2, "1229", Status::Conditional, 1),
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
        tag: "DLI",
        name: "Document Line Identification",
        elements: &[ElementRef::new(1, "1082", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "UNS",
        name: "Section Control",
        elements: &[ElementRef::new(1, "0081", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "UNT",
        name: "Message Trailer",
        elements: &[
            ElementRef::new(1, "0074", Status::Mandatory, 1),
            ElementRef::new(2, "0062", Status::Mandatory, 1),
        ],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["239"];
static CODES_1153: &[&str] = &["Z13"];
static CODES_2005: &[&str] = &["137"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_6343: &[&str] = &["11"];

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
        "6343" => Some(CODES_6343),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_REMADV_2_9E: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-REMADV-2.9e",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_REMADV_2_9E
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

fn rule_cux_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "CUX") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment CUX is missing".to_owned(),
            )
            .with_rule_id("MIG-CUX-REQ")
            .with_segment("CUX".to_owned()),
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

fn rule_moa_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "MOA") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment MOA is missing".to_owned(),
            )
            .with_rule_id("MIG-MOA-REQ")
            .with_segment("MOA".to_owned()),
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

/// Layer 3 — group-window rule for `DOC` groups (F-004).
///
/// When a `DOC` group is present, the mandatory inner segments
/// must also be present within each group window.
fn rule_group_doc_window(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    const MANDATORY_INNER: &[&str] = &["MOA"];
    // Find all positions of the trigger segment.
    let trigger_positions: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(_, s)| s.tag == "DOC")
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
                            format!("mandatory segment {required_tag} missing in DOC group at position {start}"),
                        )
                        .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-DOC")
                        .with_segment(required_tag.to_owned())
                    );
            }
        }
    }
}

/// Layer 3 — verify the `NAD` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg1_nad_max_occurrences(
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
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG1-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
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
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG4-CUX-CARD-MAX")
            .with_segment("CUX".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at most 999999 times.
///
/// Each occurrence of the trigger segment `DOC` marks the start of
/// one group instance.  The MIG specifies a maximum of 999999 instances.
fn rule_group_sg5_doc_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "DOC").count();
    if count > 999_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by DOC occurs {count} times; maximum is 999_999"),
            )
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG5-DOC-CARD-MAX")
            .with_segment("DOC".to_owned()),
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
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG1-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CUX` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg4_cux_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CUX").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by CUX occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG4-CUX-CARD-MIN")
            .with_segment("CUX".to_owned()),
        );
    }
}

/// Layer 3 — verify the `DOC` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg5_doc_min_occurrences(
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
            .with_rule_id("MIG-REMADV-MIG-2.9e-GROUP-SG5-DOC-CARD-MIN")
            .with_segment("DOC".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    /// Header segment ordering (before UNS+D).
    const EXPECTED_HEADER_ORDER: &[&str] = &[
        "UNH", "BGM", "DTM", "RFF", "NAD", "CTA", "COM", "CUX", "DOC", "MOA", "DTM", "RFF", "AJT",
        "FTX", "DLI",
    ];
    /// Detail segment ordering (after UNS+D).
    const EXPECTED_DETAIL_ORDER: &[&str] = &["MOA", "UNT"];

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
        "MIG-REMADV-MIG-2.9e-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-REMADV-MIG-2.9e-ORDER",
        issues,
    );
}

static MIG_REMADV_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REMADV-MIG-2.9e")
            .for_message_type("REMADV")
            .for_release("2.9e")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_cux_mandatory)
            .with_stateless_rule_fn(rule_doc_mandatory)
            .with_stateless_rule_fn(rule_moa_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_group_doc_window)
            .with_stateless_rule_fn(rule_group_sg1_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_cux_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_doc_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg4_cux_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg5_doc_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_REMADV_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[GroupDef {
    name: "SG5",
    trigger: "DOC",
    children: &[],
}];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_33001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REMADV-AHB-2.9e-33001")
            .for_message_type("REMADV")
            .for_release("2.9e")
            .with_named_stateless_rule_fn("AHB-33001-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-33001-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-33001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-33001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-33001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-33001-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-33001-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-33001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-33001-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-33001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-33001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 33001",
                    "33001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_33001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_33001_PACK)
}

static AHB_33002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REMADV-AHB-2.9e-33002")
            .for_message_type("REMADV")
            .for_release("2.9e")
            .with_named_stateless_rule_fn("AHB-33002-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-33002-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-33002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-33002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-33002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-33002-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-33002-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-33002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-33002-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-33002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-33002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 33002",
                    "33002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_33002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_33002_PACK)
}

static AHB_33003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REMADV-AHB-2.9e-33003")
            .for_message_type("REMADV")
            .for_release("2.9e")
            .with_named_stateless_rule_fn("AHB-33003-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-33003-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-33003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-33003-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-33003-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-33003-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-DLI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DLI",
                    "AHB-33003-DLI-M",
                    "mandatory segment DLI is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-33003-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-33003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-33003-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-33003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-33003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 33003",
                    "33003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_33003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_33003_PACK)
}

static AHB_33004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("REMADV-AHB-2.9e-33004")
            .for_message_type("REMADV")
            .for_release("2.9e")
            .with_named_stateless_rule_fn("AHB-33004-AJT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "AJT",
                    "AHB-33004-AJT-M",
                    "mandatory segment AJT is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-33004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-33004-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-33004-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-33004-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-DLI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DLI",
                    "AHB-33004-DLI-M",
                    "mandatory segment DLI is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-DOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DOC",
                    "AHB-33004-DOC-M",
                    "mandatory segment DOC is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-33004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-33004-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-33004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-33004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-33004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 33004",
                    "33004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_33004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_33004_PACK)
}

static AHB_ALL_PACK_REMADV_2_9E: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("REMADV-AHB-2.9e-ALL")
        .for_message_type("REMADV")
        .for_release("2.9e");
    let pack = pack
        .merge_with_override(ahb_33001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_33002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_33003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_33004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(33001) => ahb_33001_pack(),
            Some(33002) => ahb_33002_pack(),
            Some(33003) => ahb_33003_pack(),
            Some(33004) => ahb_33004_pack(),
            None => Arc::clone(&AHB_ALL_PACK_REMADV_2_9E),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("REMADV")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_REMADV_FV20251001: LazyLock<Release> = LazyLock::new(|| Release::new("2.9e"));

pub(crate) struct RemadvFv20251001Profile;

impl Profile for RemadvFv20251001Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Remadv
    }
    fn release(&self) -> &Release {
        &RELEASE_REMADV_FV20251001
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2025 - 10 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.9e")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("REMADV AHB 2.9e, Stand 01.10.2025")
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

pub(crate) static PROFILE: RemadvFv20251001Profile = RemadvFv20251001Profile;
