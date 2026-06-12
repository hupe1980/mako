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
        name: "Nachrichtenanfang",
        elements: &[
            ElementRef::new(1, "0062", Status::Mandatory, 1),
            ElementRef::new(2, "S009", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "BGM",
        name: "Rechnungsnummer",
        elements: &[
            ElementRef::new(1, "C002", Status::Mandatory, 1),
            ElementRef::new(2, "C106", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Datums-/Zeitangaben",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Rechnungstyp",
        elements: &[
            ElementRef::new(1, "C272", Status::Conditional, 1),
            ElementRef::new(2, "C273", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Meldeinformationen",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "C108", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "GEI",
        name: "Spezifikation der Sonderrechnung",
        elements: &[
            ElementRef::new(1, "7365", Status::Mandatory, 1),
            ElementRef::new(2, "C529", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "UNS",
        name: "Abschnitts-Kontrollsegment",
        elements: &[ElementRef::new(1, "0081", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "UNT",
        name: "Nachrichtenende",
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
        name: "Name und Adresse",
        elements: &[
            ElementRef::new(1, "3035", Status::Mandatory, 1),
            ElementRef::new(2, "C082", Status::Conditional, 1),
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
        tag: "CUX",
        name: "Währungsangaben",
        elements: &[ElementRef::new(1, "C504", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "PYT",
        name: "Zahlungsbedingungen",
        elements: &[
            ElementRef::new(1, "4279", Status::Mandatory, 1),
            ElementRef::new(2, "C019", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[ElementRef::new(1, "1082", Status::Conditional, 1)],
    },
    SegmentDefinition {
        tag: "QTY",
        name: "Mengenangaben",
        elements: &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "MOA",
        name: "Positionsnettobetrag",
        elements: &[ElementRef::new(1, "C516", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "PRI",
        name: "Preis",
        elements: &[ElementRef::new(1, "C509", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "TAX",
        name: "Umsatzsteuer der Position",
        elements: &[
            ElementRef::new(1, "5283", Status::Mandatory, 1),
            ElementRef::new(2, "C241", Status::Conditional, 1),
            ElementRef::new(3, "C243", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "ALC",
        name: "Abschlag / Zuschlag",
        elements: &[
            ElementRef::new(1, "5463", Status::Mandatory, 1),
            ElementRef::new(2, "C552", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "PCD",
        name: "Prozentangabe",
        elements: &[ElementRef::new(1, "C501", Status::Mandatory, 1)],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &["380", "389", "457", "Z25"];
static CODES_1153: &[&str] = &["ACE", "OI", "VA", "Z13", "Z56"];
static CODES_1225: &[&str] = &["7", "9"];
static CODES_2005: &[&str] = &[
    "137", "155", "156", "203", "265", "3", "9", "Z01", "Z11", "Z12", "Z42", "Z43",
];
static CODES_3035: &[&str] = &["DP", "MR", "MS", "ZSH"];
static CODES_3227: &[&str] = &["172"];
static CODES_4279: &[&str] = &["3"];
static CODES_4451: &[&str] = &["REG"];
static CODES_5153: &[&str] = &["VAT"];
static CODES_5189: &[&str] = &["Z01", "Z02"];
static CODES_5463: &[&str] = &["A", "C"];
static CODES_6063: &[&str] = &["136", "47", "Z17"];
static CODES_7009: &[&str] = &["Z06", "Z07"];
static CODES_7081: &[&str] = &[
    "13I", "13R", "ABR", "ABS", "JVR", "KON", "MMM", "MSB", "MVR", "NAP", "SOR", "TEC", "WIM",
    "Z43", "Z44", "Z45", "ZVR",
];
static CODES_7365: &[&str] = &["Z01", "Z02", "Z03"];

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
        (
            "UNH" | "FTX" | "GEI" | "UNS" | "UNT" | "NAD" | "LOC" | "CTA" | "PYT" | "LIN" | "TAX"
            | "ALC",
            0,
        )
        | ("UNT", 1) => Some(1),
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
        "4279" => Some(CODES_4279),
        "4451" => Some(CODES_4451),
        "5153" => Some(CODES_5153),
        "5189" => Some(CODES_5189),
        "5463" => Some(CODES_5463),
        "6063" => Some(CODES_6063),
        "7009" => Some(CODES_7009),
        "7081" => Some(CODES_7081),
        "7365" => Some(CODES_7365),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_INVOIC_2_8E: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-INVOIC-2.8e",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_INVOIC_2_8E
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

fn rule_imd_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "IMD") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment IMD is missing".to_owned(),
            )
            .with_rule_id("MIG-IMD-REQ")
            .with_segment("IMD".to_owned()),
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

fn rule_pyt_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "PYT") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment PYT is missing".to_owned(),
            )
            .with_rule_id("MIG-PYT-REQ")
            .with_segment("PYT".to_owned()),
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

fn rule_tax_mandatory(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    if !segments.iter().any(|s| s.tag == "TAX") {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                "mandatory segment TAX is missing".to_owned(),
            )
            .with_rule_id("MIG-TAX-REQ")
            .with_segment("TAX".to_owned()),
        );
    }
}

/// Layer 3 — verify `GEI` appears at most 10 times in the message header.
///
/// This rule only fires for segment tags that appear exclusively in the
/// message header (not in any segment group).  Tags shared between the
/// header and groups use per-group window rules instead (F-010 fix).
fn rule_gei_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "GEI").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment GEI occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-GEI-CARD-MAX")
            .with_segment("GEI".to_owned()),
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
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG1-RFF-CARD-MAX")
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
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CUX` segment group appears at most 99 times.
///
/// Each occurrence of the trigger segment `CUX` marks the start of
/// one group instance.  The MIG specifies a maximum of 99 instances.
fn rule_group_sg7_cux_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "CUX").count();
    if count > 99 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by CUX occurs {count} times; maximum is 99"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG7-CUX-CARD-MAX")
            .with_segment("CUX".to_owned()),
        );
    }
}

/// Layer 3 — verify the `PYT` segment group appears at most 10 times.
///
/// Each occurrence of the trigger segment `PYT` marks the start of
/// one group instance.  The MIG specifies a maximum of 10 instances.
fn rule_group_sg8_pyt_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "PYT").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by PYT occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG8-PYT-CARD-MAX")
            .with_segment("PYT".to_owned()),
        );
    }
}

/// Layer 3 — verify the `LIN` segment group appears at most 9999999 times.
///
/// Each occurrence of the trigger segment `LIN` marks the start of
/// one group instance.  The MIG specifies a maximum of 9999999 instances.
fn rule_group_sg26_lin_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "LIN").count();
    if count > 9_999_999 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!(
                    "segment group triggered by LIN occurs {count} times; maximum is 9_999_999"
                ),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG26-LIN-CARD-MAX")
            .with_segment("LIN".to_owned()),
        );
    }
}

/// Layer 3 — verify the `MOA` segment group appears at most 100 times.
///
/// Each occurrence of the trigger segment `MOA` marks the start of
/// one group instance.  The MIG specifies a maximum of 100 instances.
fn rule_group_sg50_moa_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "MOA").count();
    if count > 100 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by MOA occurs {count} times; maximum is 100"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG50-MOA-CARD-MAX")
            .with_segment("MOA".to_owned()),
        );
    }
}

/// Layer 3 — verify the `TAX` segment group appears at most 10 times.
///
/// Each occurrence of the trigger segment `TAX` marks the start of
/// one group instance.  The MIG specifies a maximum of 10 instances.
fn rule_group_sg52_tax_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "TAX").count();
    if count > 10 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by TAX occurs {count} times; maximum is 10"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG52-TAX-CARD-MAX")
            .with_segment("TAX".to_owned()),
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
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG1-RFF-CARD-MIN")
            .with_segment("RFF".to_owned()),
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
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG2-NAD-CARD-MIN")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `CUX` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg7_cux_min_occurrences(
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
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG7-CUX-CARD-MIN")
            .with_segment("CUX".to_owned()),
        );
    }
}

/// Layer 3 — verify the `PYT` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg8_pyt_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "PYT").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by PYT occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG8-PYT-CARD-MIN")
            .with_segment("PYT".to_owned()),
        );
    }
}

/// Layer 3 — verify the `MOA` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg50_moa_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "MOA").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by MOA occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG50-MOA-CARD-MIN")
            .with_segment("MOA".to_owned()),
        );
    }
}

/// Layer 3 — verify the `TAX` segment group appears at least 1 time(s).
///
/// The MIG specifies a minimum of 1 occurrence(s) for this group.
fn rule_group_sg52_tax_min_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "TAX").count();
    if count < 1 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by TAX occurs {count} times; minimum is 1"),
            )
            .with_rule_id("MIG-INVOIC-MIG-2.8e-GROUP-SG52-TAX-CARD-MIN")
            .with_segment("TAX".to_owned()),
        );
    }
}

/// Layer 3.5 — verify that segment tags appear in the normative sequence.
///
/// The rule does NOT require every tag to be present (that is Layer 3's job);
/// it only checks that tag positions are non-decreasing w.r.t. the expected order.
fn rule_segment_order(segments: &[edifact_rs::Segment<'_>], issues: &mut Vec<ValidationIssue>) {
    /// Header segment ordering (before UNS+D).
    const EXPECTED_HEADER_ORDER: &[&str] = &["UNH", "BGM", "DTM", "IMD", "FTX", "GEI"];
    /// Detail segment ordering (after UNS+D).
    const EXPECTED_DETAIL_ORDER: &[&str] =
        &["RFF", "NAD", "CUX", "PYT", "LIN", "MOA", "TAX", "UNT"];

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
        "MIG-INVOIC-MIG-2.8e-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-INVOIC-MIG-2.8e-ORDER",
        issues,
    );
}

static MIG_INVOIC_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-MIG-2.8e")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_imd_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_cux_mandatory)
            .with_stateless_rule_fn(rule_pyt_mandatory)
            .with_stateless_rule_fn(rule_moa_mandatory)
            .with_stateless_rule_fn(rule_tax_mandatory)
            .with_stateless_rule_fn(rule_gei_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg7_cux_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg8_pyt_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg26_lin_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg50_moa_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg52_tax_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg7_cux_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg8_pyt_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg50_moa_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg52_tax_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_INVOIC_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

static AHB_31001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31001")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31001-ALC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "ALC",
                    "AHB-31001-ALC-M",
                    "mandatory segment ALC is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31001-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-GEI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "GEI",
                    "AHB-31001-GEI-M",
                    "mandatory segment GEI is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31001-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31001-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31001-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-PCD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PCD",
                    "AHB-31001-PCD-M",
                    "mandatory segment PCD is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31001-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31001-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31001-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31001-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31001-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31001",
                    "31001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31001_PACK)
}

static AHB_31002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31002")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31002-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31002-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31002-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31002-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31002-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31002-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31002-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31002-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31002-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31002-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31002-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31002-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31002",
                    "31002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31002_PACK)
}

static AHB_31003_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31003")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31003-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31003-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31003-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31003-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31003-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31003-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31003-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31003-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31003-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31003-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31003-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31003-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31003-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31003-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31003-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31003-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31003-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31003",
                    "31003",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31003_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31003_PACK)
}

static AHB_31004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31004")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31004-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31004-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31004-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31004-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31004-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31004-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31004-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31004-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31004-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31004",
                    "31004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31004_PACK)
}

static AHB_31005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31005")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31005-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31005-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31005-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31005-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31005-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31005-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31005-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31005-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31005-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31005-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31005-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31005",
                    "31005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31005_PACK)
}

static AHB_31006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31006")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31006-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31006-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31006-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31006-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31006-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31006-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31006-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31006-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31006-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31006-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31006-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31006-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31006-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31006-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31006-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31006-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31006-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31006",
                    "31006",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31006_PACK)
}

static AHB_31007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31007")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31007-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31007-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31007-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31007-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31007-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31007-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31007-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31007-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31007-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31007-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31007-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31007-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31007-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31007-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31007-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31007-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31007",
                    "31007",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31007_PACK)
}

static AHB_31008_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31008")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31008-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31008-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31008-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31008-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31008-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31008-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31008-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31008-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31008-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31008-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31008-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31008-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31008-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31008-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31008-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31008-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31008",
                    "31008",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31008_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31008_PACK)
}

static AHB_31009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31009")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31009-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31009-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31009-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31009-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31009-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31009-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31009-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31009-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31009-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31009-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31009-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31009-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31009",
                    "31009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31009_PACK)
}

static AHB_31010_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31010")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31010-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31010-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31010-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31010-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31010-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31010-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31010-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31010-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31010-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31010-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31010-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31010-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31010-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31010-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31010",
                    "31010",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31010_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31010_PACK)
}

static AHB_31011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("INVOIC-AHB-2.8e-31011")
            .for_message_type("INVOIC")
            .for_release("2.8e")
            .with_named_stateless_rule_fn("AHB-31011-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-31011-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-31011-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-31011-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-CUX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CUX",
                    "AHB-31011-CUX-M",
                    "mandatory segment CUX is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-31011-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-31011-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-31011-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-31011-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-MOA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "MOA",
                    "AHB-31011-MOA-M",
                    "mandatory segment MOA is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-31011-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-PRI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PRI",
                    "AHB-31011-PRI-M",
                    "mandatory segment PRI is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-PYT-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PYT",
                    "AHB-31011-PYT-M",
                    "mandatory segment PYT is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-31011-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-31011-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-31011-TAX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "TAX",
                    "AHB-31011-TAX-M",
                    "mandatory segment TAX is missing for Pruefidentifikator 31011",
                    "31011",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_31011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_31011_PACK)
}

static AHB_ALL_PACK_INVOIC_2_8E: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("INVOIC-AHB-2.8e-ALL")
        .for_message_type("INVOIC")
        .for_release("2.8e");
    let pack = pack
        .merge_with_override(ahb_31001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31003_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31008_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31010_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_31011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(31001) => ahb_31001_pack(),
            Some(31002) => ahb_31002_pack(),
            Some(31003) => ahb_31003_pack(),
            Some(31004) => ahb_31004_pack(),
            Some(31005) => ahb_31005_pack(),
            Some(31006) => ahb_31006_pack(),
            Some(31007) => ahb_31007_pack(),
            Some(31008) => ahb_31008_pack(),
            Some(31009) => ahb_31009_pack(),
            Some(31010) => ahb_31010_pack(),
            Some(31011) => ahb_31011_pack(),
            None => Arc::clone(&AHB_ALL_PACK_INVOIC_2_8E),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("INVOIC")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_INVOIC_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("2.8e"));

pub(crate) struct InvoicFv20260401Profile;

impl Profile for InvoicFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Invoic
    }
    fn release(&self) -> &Release {
        &RELEASE_INVOIC_FV20260401
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 04 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("2.8e")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("INVOIC AHB 2.8e, Stand 01.04.2026")
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

pub(crate) static PROFILE: InvoicFv20260401Profile = InvoicFv20260401Profile;
