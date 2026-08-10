//! A data element sits at the same position in every message that carries it.
//!
//! # Why this is a test and not a convention
//!
//! The generated `SegmentDefinition` tables come from the MIG listings, and a MIG
//! lists only the elements *that profile uses*. Element position, however, is
//! fixed by the UN/EDIFACT directory — it is what a counterparty writes on the
//! wire. Deriving positions from the order of the MIG's list therefore shifts
//! every element that follows an omitted one, and the shift is invisible until a
//! real message arrives.
//!
//! It had already happened. REQOTE's MIG lists only `4451` and `C108` for `FTX`,
//! so `C108` was generated at position 2 instead of 4, and mako rejected inbound
//!
//! ```text
//! FTX+ACB+++Zusaetzliche Informationen
//! ```
//!
//! — the correct encoding — with *"segment FTX has 4 elements, expected between 1
//! and 2"*. APERAK, IFTSTA and INSRPT list all four elements and were unaffected,
//! which is exactly why a per-message reading never surfaced it.
//!
//! `xtask::codegen::CANONICAL_ELEMENT_POSITIONS` pins the affected segments. This
//! test is the check that the pinning is complete: it compares every generated
//! message family against every other and fails on any element that appears at
//! two different positions.

use std::collections::{BTreeMap, BTreeSet};

/// Every `ElementRef::new(pos, "id", …)` in the generated tables, as
/// `tag -> element_id -> {positions}`.
fn generated_positions() -> BTreeMap<String, BTreeMap<String, BTreeSet<usize>>> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/generated");
    let mut out: BTreeMap<String, BTreeMap<String, BTreeSet<usize>>> = BTreeMap::new();

    for entry in std::fs::read_dir(dir).expect("generated/ is present") {
        let path = entry.expect("readable entry").path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let src = std::fs::read_to_string(&path).expect("readable source");

        // Walk `SegmentDefinition::new("TAG", "name", &[ ... ])` blocks and read
        // the `ElementRef::new(pos, "id"` entries inside each.
        let mut rest = src.as_str();
        while let Some(start) = rest.find("SegmentDefinition::new(") {
            rest = &rest[start + "SegmentDefinition::new(".len()..];
            let Some(tag) = rest.split('"').nth(1) else {
                break;
            };
            let tag = tag.to_owned();
            let block_end = rest.find("SegmentDefinition::new(").unwrap_or(rest.len());
            let block = &rest[..block_end];

            for piece in block.split("ElementRef::new(").skip(1) {
                let Some((args, _)) = piece.split_once(')') else {
                    continue;
                };
                let mut parts = args.split(',');
                let Some(pos) = parts.next().and_then(|p| p.trim().parse::<usize>().ok()) else {
                    continue;
                };
                let Some(id) = parts.next().map(|p| p.trim().trim_matches('"')) else {
                    continue;
                };
                out.entry(tag.clone())
                    .or_default()
                    .entry(id.to_owned())
                    .or_default()
                    .insert(pos);
            }
        }
    }
    out
}

#[test]
fn an_element_has_one_position_across_every_message_family() {
    let positions = generated_positions();
    assert!(
        positions.len() > 20,
        "expected the generated tables to cover many segments, found {}",
        positions.len()
    );

    let mut conflicts = Vec::new();
    for (tag, elements) in &positions {
        for (id, seen) in elements {
            if seen.len() > 1 {
                conflicts.push(format!("  {tag}.{id} appears at positions {seen:?}"));
            }
        }
    }

    assert!(
        conflicts.is_empty(),
        "a data element must sit at one position in every message that carries it — \
         a MIG may drop an element, but it cannot renumber the ones around it.\n\
         Add the segment to `CANONICAL_ELEMENT_POSITIONS` in xtask/src/codegen.rs \
         with its UN/EDIFACT directory positions, then re-run `cargo xtask codegen`.\n{}",
        conflicts.join("\n")
    );
}

/// The specific layouts that were wrong, pinned by value so a regenerate that
/// silently drops the canonical table is caught even if it happens to stay
/// self-consistent across families.
#[test]
fn the_known_edifact_layouts_are_exact() {
    let positions = generated_positions();
    let expect = |tag: &str, id: &str, want: usize| {
        let Some(seen) = positions.get(tag).and_then(|m| m.get(id)) else {
            return; // this element is not used by any generated profile
        };
        assert_eq!(
            seen.iter().copied().collect::<Vec<_>>(),
            vec![want],
            "{tag}.{id} must sit at EDIFACT position {want}"
        );
    };

    // FTX — Free Text
    expect("FTX", "4451", 1);
    expect("FTX", "4453", 2);
    expect("FTX", "C107", 3);
    expect("FTX", "C108", 4);
    // CCI — Characteristic/Class ID
    expect("CCI", "7059", 1);
    expect("CCI", "C502", 2);
    expect("CCI", "C240", 3);
    // IMD — Item Description
    expect("IMD", "7077", 1);
    expect("IMD", "C272", 2);
    expect("IMD", "C273", 3);
    // STS — Status
    expect("STS", "C601", 1);
    expect("STS", "C555", 2);
    expect("STS", "C556", 3);
}

/// The hand-authored layouts and the generated tables must agree.
///
/// mako carries **two** descriptions of where a data element sits, filled from
/// the same MIGs by different routes:
///
/// - `messages::layouts` — hand-authored, and what the `EdifactDeserialize`
///   derive resolves `#[edifact(element = "4440")]` against when *reading* a
///   typed segment.
/// - the generated `SEGMENTS` tables — emitted by `xtask codegen` from the MIG
///   JSON via `CANONICAL_ELEMENT_POSITIONS`, and what *validates* an inbound
///   segment's arity.
///
/// Each is separately guarded against a hand-written MIG table
/// (`segment_layout_guard.rs` and the two tests above), but nothing checked
/// them against **each other** — so a fix applied to one source could leave the
/// other pointing at the old slot. That splits the failure in two: the parser
/// reads the wrong component while the validator still accepts the segment, or
/// the validator rejects a segment the parser would have read correctly. This
/// closes the triangle.
///
/// # Why the `cfg`
///
/// `messages::layouts` is itself gated on "at least one message type", so with
/// `--no-default-features` this test referenced a module that does not exist and
/// the whole test binary failed to compile — which `just test-features` runs and
/// nothing else does. The list must stay identical to the one on `pub mod
/// layouts` in `src/messages/mod.rs`.
#[cfg(any(
    feature = "utilmd",
    feature = "mscons",
    feature = "aperak",
    feature = "contrl",
    feature = "invoic",
    feature = "remadv",
    feature = "orders",
    feature = "iftsta",
    feature = "insrpt",
    feature = "reqote",
    feature = "partin",
    feature = "ordchg",
    feature = "ordrsp",
    feature = "quotes",
    feature = "comdis",
    feature = "pricat",
    feature = "utilts",
))]
#[test]
fn the_hand_authored_layouts_agree_with_the_generated_tables() {
    use edi_energy::messages::layouts;

    let generated = generated_positions();

    // Every layout `messages::segments` addresses. A composite is named by its
    // own identifier (`C108`) in both sources, so the two are directly
    // comparable without expanding components.
    let layouts: &[&'static edifact_rs::SegmentDefinition] = &[
        &layouts::BGM,
        &layouts::DTM,
        &layouts::NAD,
        &layouts::RFF,
        &layouts::IDE,
        &layouts::LOC,
        &layouts::ERC,
        &layouts::FTX,
        &layouts::QTY,
        &layouts::LIN,
        &layouts::PIA,
        &layouts::CCI,
        &layouts::STS,
        &layouts::CTA,
        &layouts::COM,
    ];

    let mut offenders = Vec::new();
    let mut compared = 0usize;

    for def in layouts {
        let Some(gen_tag) = generated.get(def.tag) else {
            // No generated profile carries this segment — nothing to compare.
            continue;
        };
        for elem in def.elements {
            let id = elem.data_element();
            let Some(gen_positions) = gen_tag.get(id) else {
                // The MIGs of the generated profiles do not use this element.
                continue;
            };
            compared += 1;
            let want = usize::from(elem.position());
            if !gen_positions.contains(&want) {
                offenders.push(format!(
                    "{}.{id}: messages::layouts says position {want}, \
                     the generated tables say {gen_positions:?}",
                    def.tag
                ));
            }
        }
    }

    assert!(
        compared > 20,
        "expected the two sources to overlap on many elements, compared only {compared}"
    );
    assert!(
        offenders.is_empty(),
        "the hand-authored segment layouts and the generated MIG tables disagree \
         on where a data element sits. The parser resolves named elements against \
         `messages::layouts`; the validator checks arity against the generated \
         tables — they must describe the same segment.\n  {}",
        offenders.join("\n  ")
    );
}
