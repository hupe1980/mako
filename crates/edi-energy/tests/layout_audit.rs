//! The layouts are held against real messages, not only against each other.
//!
//! `element_positions.rs` holds the hand-authored layouts against the imported
//! MIG layouts, and that is the check for a *disagreement*. It cannot catch a
//! **shared** mistake: if both sources place an element at the same wrong
//! position they agree, the check passes, and mako misreads the wire anyway.
//! The two sources are not independent enough for their agreement to be
//! evidence — the MIG import and the hand-authored table were written from the
//! same documents by the same reading.
//!
//! `edifact-rs`'s `audit_directory` asks the question the corpus can answer:
//! *which of these definitions does real traffic disprove?* A fixture that
//! populates a component the layout does not declare is a contradiction, and no
//! amount of internal agreement reveals it.
//!
//! ## What is and is not a failure
//!
//! A **contradiction** fails: the corpus carries something the layout says
//! cannot be there. `NeverObserved` does not — a corpus that never reaches a
//! slot says nothing about whether the slot is right, and failing on it would
//! only measure fixture coverage. It is counted and printed so the unproven
//! surface is visible rather than silently absent.
//!
//! ## Why only the `valid/` fixtures
//!
//! The `invalid/` fixtures are deliberately wrong — a missing `UCI`, a swapped
//! `LOC` qualifier, an unpermitted `SG8`. A contradiction drawn from one of
//! those would be the fixture's whole point, so auditing them would report the
//! corpus back to itself.

// The hand-authored layouts exist only when a message type is enabled.
#![cfg(any_message)]

use std::path::{Path, PathBuf};

use edi_energy::messages::layouts;
use edifact_rs::{SegmentDefinition, audit_directory, from_bytes};

/// Every hand-authored layout, by tag.
///
/// The same list `element_positions.rs` checks against the MIGs. Kept beside it
/// rather than shared: these two tests ask different questions of it, and a
/// helper crate between them would make the second look like a variant of the
/// first.
fn hand_authored() -> Vec<(&'static str, &'static SegmentDefinition)> {
    vec![
        ("BGM", &layouts::BGM),
        ("DTM", &layouts::DTM),
        ("NAD", &layouts::NAD),
        ("RFF", &layouts::RFF),
        ("IDE", &layouts::IDE),
        ("LOC", &layouts::LOC),
        ("AJT", &layouts::AJT),
        ("ERC", &layouts::ERC),
        ("FTX", &layouts::FTX),
        ("QTY", &layouts::QTY),
        ("LIN", &layouts::LIN),
        ("PIA", &layouts::PIA),
        ("CCI", &layouts::CCI),
        ("CAV", &layouts::CAV),
        ("SEQ", &layouts::SEQ),
        ("STS", &layouts::STS),
        ("CTA", &layouts::CTA),
        ("COM", &layouts::COM),
    ]
}

/// The lowest number of `valid/` fixtures a healthy run may find.
///
/// A corpus that shrinks to nothing proves nothing, and an audit over it passes
/// for the worst possible reason. The floor is under the measured count so an
/// ordinary addition does not trip it, and far enough above zero that a broken
/// path does.
const MIN_FIXTURES: usize = 50;

/// Every `valid/` EDIFACT fixture.
fn valid_fixtures() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("edi")
                && path.components().any(|c| c.as_os_str() == "valid")
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures"),
        &mut out,
    );
    out.sort();
    out
}

#[test]
fn no_hand_authored_layout_is_contradicted_by_real_messages() {
    let fixtures = valid_fixtures();
    assert!(
        fixtures.len() >= MIN_FIXTURES,
        "found {} valid fixture(s), expected at least {MIN_FIXTURES} — an audit over an empty \
         corpus passes for the worst possible reason",
        fixtures.len()
    );

    let layouts = hand_authored();
    let lookup = |tag: &str| {
        layouts
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, def)| *def as &SegmentDefinition)
    };

    let mut contradictions: Vec<String> = Vec::new();
    let mut examined: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();

    for path in &fixtures {
        let bytes = std::fs::read(path).expect("fixture is readable");
        // A fixture that does not parse is a different test's business; this one
        // is about layouts, and reporting a parse error here would move the
        // failure away from the check that owns it.
        let Ok(segments) = from_bytes(&bytes).collect::<Result<Vec<_>, _>>() else {
            continue;
        };
        for audit in audit_directory(lookup, &segments) {
            *examined.entry(audit.tag().to_owned()).or_default() += audit.segments_examined();
            for finding in audit.contradictions() {
                contradictions.push(format!(
                    "{}: {} — {finding:?}",
                    path.file_name().unwrap_or_default().to_string_lossy(),
                    audit.tag(),
                ));
            }
        }
    }

    assert!(
        contradictions.is_empty(),
        "real messages disprove {} hand-authored layout slot(s). Both layout sources can be \
         wrong together — that is what this test exists to catch, and `element_positions` \
         cannot:\n  {}",
        contradictions.len(),
        contradictions.join("\n  ")
    );

    // The corpus has to have exercised something, or the run above proved
    // nothing while reporting success.
    let proven: Vec<&str> = examined
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(t, _)| t.as_str())
        .collect();
    assert!(
        proven.len() >= 8,
        "only {} of {} hand-authored layouts were exercised by {} fixture(s): {proven:?}. \
         An audit proves nothing about a tag the corpus never carries",
        proven.len(),
        layouts.len(),
        fixtures.len()
    );

    // Not a failure — the unproven surface, made visible.
    let unproven: Vec<&str> = layouts
        .iter()
        .map(|(t, _)| *t)
        .filter(|t| examined.get(*t).copied().unwrap_or(0) == 0)
        .collect();
    if !unproven.is_empty() {
        println!(
            "layout audit: {} of {} layouts are not exercised by any valid fixture: {unproven:?}",
            unproven.len(),
            layouts.len()
        );
    }
}
