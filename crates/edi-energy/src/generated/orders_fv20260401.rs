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
            ElementRef::new(3, "1225", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "DTM",
        name: "Nachrichtendatum",
        elements: &[ElementRef::new(1, "C507", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "IMD",
        name: "Abonnement / Abonnementtyp",
        elements: &[
            ElementRef::new(1, "7077", Status::Conditional, 1),
            ElementRef::new(2, "C272", Status::Conditional, 1),
            ElementRef::new(3, "C273", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "FTX",
        name: "Allgemeine Information",
        elements: &[
            ElementRef::new(1, "4451", Status::Mandatory, 1),
            ElementRef::new(2, "4453", Status::Conditional, 1),
            ElementRef::new(3, "C107", Status::Conditional, 1),
            ElementRef::new(4, "C108", Status::Conditional, 1),
        ],
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
        name: "Referenz Nachrichtennummer",
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
        tag: "LOC",
        name: "Bilanzierungsgebiet/Regelzone",
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
        tag: "LIN",
        name: "Positionsdaten",
        elements: &[
            ElementRef::new(1, "1082", Status::Conditional, 1),
            ElementRef::new(2, "1229", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "PIA",
        name: "OBIS-Kennzahl / Messprodukt",
        elements: &[
            ElementRef::new(1, "4347", Status::Mandatory, 1),
            ElementRef::new(2, "C212", Status::Mandatory, 1),
        ],
    },
    SegmentDefinition {
        tag: "QTY",
        name: "Prozentualer Anteil der Tranche",
        elements: &[ElementRef::new(1, "C186", Status::Mandatory, 1)],
    },
    SegmentDefinition {
        tag: "CCI",
        name: "Profilgruppe",
        elements: &[
            ElementRef::new(1, "7059", Status::Conditional, 1),
            ElementRef::new(2, "C240", Status::Conditional, 1),
            ElementRef::new(3, "C889", Status::Conditional, 1),
        ],
    },
    SegmentDefinition {
        tag: "CAV",
        name: "Prognosegrundlage",
        elements: &[ElementRef::new(1, "C889", Status::Mandatory, 1)],
    },
];

static SEGMENT_MAP: LazyLock<std::collections::HashMap<&'static str, &'static SegmentDefinition>> =
    LazyLock::new(|| SEGMENTS.iter().map(|s| (s.tag, s)).collect());

pub(crate) fn segment_lookup(tag: &str) -> Option<&'static SegmentDefinition> {
    SEGMENT_MAP.get(tag).copied()
}

static CODES_1001: &[&str] = &[
    "Z55", "Z56", "Z57", "Z58", "Z59", "Z60", "Z61", "Z62", "Z63", "Z64",
];
static CODES_1153: &[&str] = &["AGI", "Z13", "Z18", "Z19"];
static CODES_1225: &[&str] = &["9"];
static CODES_1229: &[&str] = &["Z54", "Z55"];
static CODES_2005: &[&str] = &["137", "163", "164", "293"];
static CODES_3035: &[&str] = &["MR", "MS"];
static CODES_3227: &[&str] = &["172", "237", "Z01"];
static CODES_4347: &[&str] = &["5"];
static CODES_4451: &[&str] = &["ZZZ"];

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
            "UNH" | "IMD" | "FTX" | "UNS" | "UNT" | "NAD" | "LOC" | "CTA" | "LIN" | "PIA" | "CCI",
            0,
        )
        | ("BGM", 2)
        | ("IMD" | "FTX" | "UNT" | "LIN", 1) => Some(1),
        _ => None,
    }
}

pub(crate) fn code_list(de_id: &str) -> Option<&'static [&'static str]> {
    match de_id {
        "1001" => Some(CODES_1001),
        "1153" => Some(CODES_1153),
        "1225" => Some(CODES_1225),
        "1229" => Some(CODES_1229),
        "2005" => Some(CODES_2005),
        "3035" => Some(CODES_3035),
        "3227" => Some(CODES_3227),
        "4347" => Some(CODES_4347),
        "4451" => Some(CODES_4451),
        _ => None,
    }
}

// Layer 2 scope: mandatory segment presence, element/component counts,
// code-list validity. Does NOT check segment sequence or repetition
// cardinality — those are Layer 3 (MIG ProfileRulePack) responsibilities.
// Cached in a LazyLock so construction happens once per profile (F-019 fix).
static DIRECTORY_VALIDATOR_ORDERS_1_4C: LazyLock<DirectoryValidator> = LazyLock::new(|| {
    DirectoryValidator::new(
        "EDI@Energy-ORDERS-1.4c",
        segment_lookup,
        is_code_valid,
        suggest_code,
        expected_components,
        None,
    )
});

pub(crate) fn directory_validator() -> &'static DirectoryValidator {
    &DIRECTORY_VALIDATOR_ORDERS_1_4C
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

/// Layer 3 — verify the `NAD` segment group appears at most 3 times.
///
/// Each occurrence of the trigger segment `NAD` marks the start of
/// one group instance.  The MIG specifies a maximum of 3 instances.
fn rule_group_sg2_nad_max_occurrences(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let count = segments.iter().filter(|s| s.tag == "NAD").count();
    if count > 3 {
        issues.push(
            ValidationIssue::new(
                ValidationSeverity::Error,
                format!("segment group triggered by NAD occurs {count} times; maximum is 3"),
            )
            .with_rule_id("MIG-ORDERS-MIG-1.4c-GROUP-SG2-NAD-CARD-MAX")
            .with_segment("NAD".to_owned()),
        );
    }
}

/// Layer 3 — verify the `LIN` segment group appears at most 200000 times.
///
/// Each occurrence of the trigger segment `LIN` marks the start of
/// one group instance.  The MIG specifies a maximum of 200000 instances.
fn rule_group_sg29_lin_max_occurrences(
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
            .with_rule_id("MIG-ORDERS-MIG-1.4c-GROUP-SG29-LIN-CARD-MAX")
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
            .with_rule_id("MIG-ORDERS-MIG-1.4c-GROUP-SG1-RFF-CARD-MIN")
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
            .with_rule_id("MIG-ORDERS-MIG-1.4c-GROUP-SG2-NAD-CARD-MIN")
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
    const EXPECTED_HEADER_ORDER: &[&str] = &[
        "UNH", "BGM", "DTM", "IMD", "FTX", "RFF", "NAD", "CTA", "COM", "LOC", "LIN", "PIA", "QTY",
    ];
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
        "MIG-ORDERS-MIG-1.4c-ORDER",
        issues,
    );
    check_detail_section(
        detail_segs,
        EXPECTED_DETAIL_ORDER,
        "MIG-ORDERS-MIG-1.4c-ORDER",
        issues,
    );
}

static MIG_ORDERS_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-MIG-1.4c")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_stateless_rule_fn(rule_unh_mandatory)
            .with_stateless_rule_fn(rule_bgm_mandatory)
            .with_stateless_rule_fn(rule_dtm_mandatory)
            .with_stateless_rule_fn(rule_uns_mandatory)
            .with_stateless_rule_fn(rule_unt_mandatory)
            .with_stateless_rule_fn(rule_rff_mandatory)
            .with_stateless_rule_fn(rule_nad_mandatory)
            .with_stateless_rule_fn(rule_group_sg2_nad_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg29_lin_max_occurrences)
            .with_stateless_rule_fn(rule_group_sg1_rff_min_occurrences)
            .with_stateless_rule_fn(rule_group_sg2_nad_min_occurrences)
            .with_stateless_rule_fn(rule_segment_order),
    )
});

pub(crate) fn mig_rule_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&MIG_ORDERS_PACK)
}

static GROUP_SCHEMA: &[GroupDef] = &[];
#[allow(unused_imports)]
use super::ahb_helpers::{
    ahb_check_conditional, ahb_check_field_value, ahb_check_mandatory, ahb_check_not_used,
    ahb_check_qualifier, ahb_check_required_qualifier, ahb_check_soll,
};

/// Bedingungsoperator I — I: when BGM DE[0]="7" is present // [2] Wenn BGM+7 (Lieferschein) vorhanden, ist IMD Pflicht
fn rule_ahb_17102_imd_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "7"));
    if condition_holds && !segments.iter().any(|s| s.tag == "IMD") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment IMD is missing for Pruefidentifikator 17102 (I: when BGM DE[0]=\"7\" is present)".to_owned(),
                )
                .with_rule_id("AHB-17102-IMD-I0")
                .with_segment("IMD".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "17102".to_owned()));
    }
}

/// Bedingungsoperator I — I: when BGM DE[0]="7" is present // [2] Wenn BGM+7 (Lieferschein) vorhanden, ist IMD Pflicht
fn rule_ahb_17301_imd_cond_0(
    segments: &[edifact_rs::Segment<'_>],
    issues: &mut Vec<ValidationIssue>,
) {
    let __start = issues.len();
    let condition_holds = segments
        .iter()
        .any(|s| s.tag == "BGM" && s.element_str(0).is_some_and(|v| v == "7"));
    if condition_holds && !segments.iter().any(|s| s.tag == "IMD") {
        issues.push(
                ValidationIssue::new(
                    ValidationSeverity::Error,
                    "conditional segment IMD is missing for Pruefidentifikator 17301 (I: when BGM DE[0]=\"7\" is present)".to_owned(),
                )
                .with_rule_id("AHB-17301-IMD-I0")
                .with_segment("IMD".to_owned())
            );
    }
    for __i in &mut issues[__start..] {
        __i.context.push(("pid".to_owned(), "17301".to_owned()));
    }
}

static AHB_17001_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17001")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17001-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17001-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-17001-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-17001-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17001-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17001-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17001-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17001-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17001-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17001-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17001",
                    "17001",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17001_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17001_PACK)
}

static AHB_17002_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17002")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17002-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17002-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17002",
                    "17002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17002-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17002-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17002",
                    "17002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17002-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17002-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17002",
                    "17002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17002-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17002-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17002",
                    "17002",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17002-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17002-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17002",
                    "17002",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17002_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17002_PACK)
}

static AHB_17004_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17004")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17004-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17004-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17004-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17004-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17004-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17004-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17004-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17004-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17004-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17004",
                    "17004",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17004_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17004_PACK)
}

static AHB_17005_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17005")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17005-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17005-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17005-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17005-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17005-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17005-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17005-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17005-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17005-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17005-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17005-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17005-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17005",
                    "17005",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17005_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17005_PACK)
}

static AHB_17006_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17006")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17006-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17006-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17006-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17006-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17006-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17006-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17006-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17006-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17006-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17006",
                    "17006",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17006_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17006_PACK)
}

static AHB_17007_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17007")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17007-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17007-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17007",
                    "17007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17007-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17007-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17007",
                    "17007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17007-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17007-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17007",
                    "17007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17007-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17007-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17007",
                    "17007",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17007-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17007-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17007",
                    "17007",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17007_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17007_PACK)
}

static AHB_17009_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17009")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17009-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17009-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17009-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17009-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17009-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17009-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17009-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17009-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17009-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17009-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17009",
                    "17009",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17009_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17009_PACK)
}

static AHB_17011_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17011")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17011-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17011-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-17011-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-17011-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17011-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17011-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17011-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17011-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17011-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17011-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17011-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17011",
                    "17011",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17011_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17011_PACK)
}

static AHB_17101_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17101")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17101-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17101-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17101-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17101-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17101-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17101-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17101-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17101-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17101-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17101-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17101-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17101-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17101",
                    "17101",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17101_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17101_PACK)
}

static AHB_17102_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17102")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17102-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17102-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17102-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17102-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_17102_imd_cond_0)
            .with_named_stateless_rule_fn("AHB-17102-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17102-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17102-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17102-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17102-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17102-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17102-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17102-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17102",
                    "17102",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17102_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17102_PACK)
}

static AHB_17103_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17103")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17103-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17103-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17103-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17103-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17103-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17103-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17103-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17103-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17103-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17103",
                    "17103",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17103_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17103_PACK)
}

static AHB_17104_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17104")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17104-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17104-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-17104-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-17104-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17104-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17104-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17104-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17104-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17104-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17104",
                    "17104",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17104_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17104_PACK)
}

static AHB_17110_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17110")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17110-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17110-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17110",
                    "17110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17110-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17110-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17110",
                    "17110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17110-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17110-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17110",
                    "17110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17110-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17110-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17110",
                    "17110",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17110-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17110-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17110",
                    "17110",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17110_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17110_PACK)
}

static AHB_17113_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17113")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17113-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17113-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-17113-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-17113-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17113-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17113-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17113-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17113-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17113-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17113-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17113-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17113-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17113-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17113",
                    "17113",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17113_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17113_PACK)
}

static AHB_17115_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17115")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17115-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17115-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-COM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "COM",
                    "AHB-17115-COM-M",
                    "mandatory segment COM is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-CTA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CTA",
                    "AHB-17115-CTA-M",
                    "mandatory segment CTA is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17115-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17115-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17115-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17115-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17115-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17115-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17115-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17115",
                    "17115",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17115_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17115_PACK)
}

static AHB_17118_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17118")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17118-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17118-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-17118-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17118-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17118-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17118-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17118-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17118-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17118-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17118-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17118-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17118",
                    "17118",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17118_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17118_PACK)
}

static AHB_17120_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17120")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17120-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17120-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-17120-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17120-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17120-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17120-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17120-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17120-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17120-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17120-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17120",
                    "17120",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17120_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17120_PACK)
}

static AHB_17121_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17121")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17121-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17121-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-17121-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17121-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17121-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17121-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17121-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17121-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17121-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17121-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17121-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17121-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17121",
                    "17121",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17121_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17121_PACK)
}

static AHB_17122_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17122")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17122-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17122-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17122-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17122-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17122-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17122-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17122-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17122-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17122-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17122",
                    "17122",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17122_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17122_PACK)
}

static AHB_17123_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17123")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17123-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17123-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17123-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17123-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17123-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17123-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17123-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17123-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17123-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17123-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17123-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17123",
                    "17123",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17123_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17123_PACK)
}

static AHB_17128_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17128")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17128-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17128-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17128-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17128-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17128-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17128-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17128-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17128-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17128-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17128-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17128",
                    "17128",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17128_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17128_PACK)
}

static AHB_17129_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17129")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17129-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17129-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17129",
                    "17129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17129-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17129-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17129",
                    "17129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17129-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17129-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17129",
                    "17129",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17129-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17129-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17129",
                    "17129",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17129_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17129_PACK)
}

static AHB_17130_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17130")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17130-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17130-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17130-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17130-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17130-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17130-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17130-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17130-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17130-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17130-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17130-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17130",
                    "17130",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17130_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17130_PACK)
}

static AHB_17131_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17131")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17131-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17131-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17131",
                    "17131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17131-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17131-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17131",
                    "17131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17131-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17131-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17131",
                    "17131",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17131-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17131-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17131",
                    "17131",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17131_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17131_PACK)
}

static AHB_17132_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17132")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17132-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17132-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17132",
                    "17132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17132-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17132-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17132",
                    "17132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17132-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17132-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17132",
                    "17132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17132-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17132-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17132",
                    "17132",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17132-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17132-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17132",
                    "17132",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17132_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17132_PACK)
}

static AHB_17133_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17133")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17133-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17133-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-CAV-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CAV",
                    "AHB-17133-CAV-M",
                    "mandatory segment CAV is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17133-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17133-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17133-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17133-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17133-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17133-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17133-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17133-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17133",
                    "17133",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17133_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17133_PACK)
}

static AHB_17134_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17134")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17134-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17134-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17134-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17134-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17134-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17134-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17134-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17134-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17134-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-QTY-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "QTY",
                    "AHB-17134-QTY-M",
                    "mandatory segment QTY is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17134-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17134-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17134",
                    "17134",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17134_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17134_PACK)
}

static AHB_17135_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17135")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17135-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17135-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-CCI-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "CCI",
                    "AHB-17135-CCI-M",
                    "mandatory segment CCI is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17135-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17135-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17135-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17135-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-PIA-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "PIA",
                    "AHB-17135-PIA-M",
                    "mandatory segment PIA is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17135-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17135-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17135",
                    "17135",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17135_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17135_PACK)
}

static AHB_17209_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17209")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17209-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17209-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17209-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17209-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17209-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17209-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17209-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17209-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17209-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17209-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17209-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17209-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17209",
                    "17209",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17209_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17209_PACK)
}

static AHB_17210_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17210")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17210-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17210-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17210-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-IMD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "IMD",
                    "AHB-17210-IMD-M",
                    "mandatory segment IMD is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-LIN-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LIN",
                    "AHB-17210-LIN-M",
                    "mandatory segment LIN is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17210-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17210-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17210-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17210-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17210",
                    "17210",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17210_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17210_PACK)
}

static AHB_17211_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17211")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17211-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17211-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17211",
                    "17211",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17211-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17211-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17211",
                    "17211",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17211-FTX-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "FTX",
                    "AHB-17211-FTX-M",
                    "mandatory segment FTX is missing for Pruefidentifikator 17211",
                    "17211",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17211-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17211-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17211",
                    "17211",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17211-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17211-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17211",
                    "17211",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17211_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17211_PACK)
}

static AHB_17301_PACK: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    Arc::new(
        ProfileRulePack::new("ORDERS-AHB-1.4c-17301")
            .for_message_type("ORDERS")
            .for_release("1.4c")
            .with_named_stateless_rule_fn("AHB-17301-BGM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "BGM",
                    "AHB-17301-BGM-M",
                    "mandatory segment BGM is missing for Pruefidentifikator 17301",
                    "17301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17301-DTM-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "DTM",
                    "AHB-17301-DTM-M",
                    "mandatory segment DTM is missing for Pruefidentifikator 17301",
                    "17301",
                    issues,
                );
            })
            .with_stateless_rule_fn(rule_ahb_17301_imd_cond_0)
            .with_named_stateless_rule_fn("AHB-17301-LOC-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "LOC",
                    "AHB-17301-LOC-M",
                    "mandatory segment LOC is missing for Pruefidentifikator 17301",
                    "17301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17301-NAD-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "NAD",
                    "AHB-17301-NAD-M",
                    "mandatory segment NAD is missing for Pruefidentifikator 17301",
                    "17301",
                    issues,
                );
            })
            .with_named_stateless_rule_fn("AHB-17301-RFF-M", |segs, issues| {
                ahb_check_mandatory(
                    segs,
                    "RFF",
                    "AHB-17301-RFF-M",
                    "mandatory segment RFF is missing for Pruefidentifikator 17301",
                    "17301",
                    issues,
                );
            })
            .with_max_issues_per_rule(50),
    )
});

fn ahb_17301_pack() -> Arc<ProfileRulePack> {
    Arc::clone(&AHB_17301_PACK)
}

static AHB_ALL_PACK_ORDERS_1_4C: LazyLock<Arc<ProfileRulePack>> = LazyLock::new(|| {
    let pack = ProfileRulePack::new("ORDERS-AHB-1.4c-ALL")
        .for_message_type("ORDERS")
        .for_release("1.4c");
    let pack = pack
        .merge_with_override(ahb_17001_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17002_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17004_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17005_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17006_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17007_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17009_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17011_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17101_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17102_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17103_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17104_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17110_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17113_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17115_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17118_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17120_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17121_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17122_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17123_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17128_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17129_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17130_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17131_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17132_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17133_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17134_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17135_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17209_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17210_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17211_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    let pack = pack
        .merge_with_override(ahb_17301_pack().as_ref().clone())
        .expect("AHB union pack merge_with_override failed");
    Arc::new(pack)
});

pub(crate) fn ahb_rule_pack(pid: Option<Pruefidentifikator>) -> Arc<ProfileRulePack> {
    match pid.map(super::super::pruefidentifikator::Pruefidentifikator::as_u32) {
            Some(17001) => ahb_17001_pack(),
            Some(17002) => ahb_17002_pack(),
            Some(17004) => ahb_17004_pack(),
            Some(17005) => ahb_17005_pack(),
            Some(17006) => ahb_17006_pack(),
            Some(17007) => ahb_17007_pack(),
            Some(17009) => ahb_17009_pack(),
            Some(17011) => ahb_17011_pack(),
            Some(17101) => ahb_17101_pack(),
            Some(17102) => ahb_17102_pack(),
            Some(17103) => ahb_17103_pack(),
            Some(17104) => ahb_17104_pack(),
            Some(17110) => ahb_17110_pack(),
            Some(17113) => ahb_17113_pack(),
            Some(17115) => ahb_17115_pack(),
            Some(17118) => ahb_17118_pack(),
            Some(17120) => ahb_17120_pack(),
            Some(17121) => ahb_17121_pack(),
            Some(17122) => ahb_17122_pack(),
            Some(17123) => ahb_17123_pack(),
            Some(17128) => ahb_17128_pack(),
            Some(17129) => ahb_17129_pack(),
            Some(17130) => ahb_17130_pack(),
            Some(17131) => ahb_17131_pack(),
            Some(17132) => ahb_17132_pack(),
            Some(17133) => ahb_17133_pack(),
            Some(17134) => ahb_17134_pack(),
            Some(17135) => ahb_17135_pack(),
            Some(17209) => ahb_17209_pack(),
            Some(17210) => ahb_17210_pack(),
            Some(17211) => ahb_17211_pack(),
            Some(17301) => ahb_17301_pack(),
            None => Arc::clone(&AHB_ALL_PACK_ORDERS_1_4C),
            Some(_unknown) => Arc::new(ProfileRulePack::new("unknown-pid")
                .for_message_type("ORDERS")
                .with_named_stateless_rule_fn("AHB-UNKNOWN-PID", |_segs, issues| {
                    issues.push(ValidationIssue::new(
                        ValidationSeverity::Warning,
                        "Pruefidentifikator is not registered for this release — AHB rules were not applied",
                    ).with_rule_id("AHB-UNKNOWN-PID"));
                })),
        }
}

static RELEASE_ORDERS_FV20260401: LazyLock<Release> = LazyLock::new(|| Release::new("1.4c"));

pub(crate) struct OrdersFv20260401Profile;

impl Profile for OrdersFv20260401Profile {
    fn message_type(&self) -> MessageType {
        MessageType::Orders
    }
    fn release(&self) -> &Release {
        &RELEASE_ORDERS_FV20260401
    }
    fn valid_from(&self) -> Option<::time::Date> {
        Some(::time::macros::date!(2026 - 04 - 01))
    }
    fn valid_until(&self) -> Option<::time::Date> {
        None
    }
    fn ahb_revision(&self) -> Option<&'static str> {
        Some("1.4c")
    }
    fn source_document(&self) -> Option<&'static str> {
        Some("ORDERS AHB 1.4c, Stand 01.04.2026")
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

pub(crate) static PROFILE: OrdersFv20260401Profile = OrdersFv20260401Profile;
