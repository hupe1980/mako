//! Every fixture's validation verdict, pinned.
//!
//! The conformance tests assert a *direction*: a `valid/` fixture is clean, an
//! `invalid/` one fires the rules its `.expected.json` names. Neither notices a
//! rule that stops firing, an `Error` reclassified to a `Warning`, or a rule the
//! corpus never asked about — the shapes a dependency upgrade takes.
//!
//! This records the whole verdict instead: rule ID and severity per fixture,
//! sorted and deduplicated. It says nothing about whether the verdict is
//! *right* — the conformance tests do that — only that it did not move
//! unnoticed. Messages and spans are excluded: wording churns, and a span moves
//! whenever a fixture is reformatted.
//!
//! # Updating
//!
//! ```bash
//! BLESS_VALIDATION_SNAPSHOT=1 cargo test -p edi-energy --all-features \
//!     --test validation_snapshot
//! ```
//!
//! Read the diff first. A line that disappears is a check that was lost.

#![cfg(all(
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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use edi_energy::{EdiEnergyMessage as _, Platform};

/// Where the snapshot lives.
fn snapshot_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/validation_snapshot.txt")
}

/// The same reference date the conformance tests use, so the two agree on which
/// profile is in force. Deriving it from the registry rather than hard-coding it
/// means a new release does not silently re-date the corpus.
fn reference_date() -> time::Date {
    use edi_energy::registry::ReleaseRegistry;
    ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter_map(|p| p.valid_from())
        .max()
        .map(|d| d.saturating_add(time::Duration::days(365)))
        .unwrap_or_else(|| {
            time::Date::from_calendar_date(2027, time::Month::January, 1)
                .expect("hard-coded fallback date is valid")
        })
}

/// Every `*.edi` under `dir`, recursively, as a repo-relative path.
fn collect_fixtures(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures(&path, out);
        } else if path.extension().is_some_and(|e| e == "edi") {
            out.push(path);
        }
    }
}

/// One fixture's verdict: the sorted, deduplicated `(severity, rule_id)` set.
///
/// A rule that fires more than once contributes one line — the count depends on
/// how many segments a fixture happens to carry, which is not what this guards.
fn verdict(path: &Path, root: &Path) -> Vec<String> {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"));

    let Ok(msg) = Platform::with_all_profiles().parse(&bytes) else {
        return vec![format!("{rel}\tPARSE-ERROR")];
    };
    let Ok(report) = msg.validate_on_date(reference_date()) else {
        return vec![format!("{rel}\tVALIDATE-ERROR")];
    };

    let mut lines: BTreeSet<String> = BTreeSet::new();
    for (severity, issues) in [
        ("error", report.errors()),
        ("warning", report.warnings()),
        ("info", report.infos()),
    ] {
        for issue in issues {
            let rule = issue.rule_id.as_deref().unwrap_or("<no-rule-id>");
            lines.insert(format!("{rel}\t{severity}\t{rule}"));
        }
    }
    if lines.is_empty() {
        lines.insert(format!("{rel}\tclean"));
    }
    lines.into_iter().collect()
}

/// The verdict of every fixture in the corpus, in one comparable document.
#[test]
fn the_corpus_verdict_has_not_moved() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths = Vec::new();
    collect_fixtures(&root, &mut paths);
    paths.sort();
    assert!(
        paths.len() > 50,
        "expected the full fixture corpus, found {} files — is the path right?",
        paths.len()
    );

    let mut lines = Vec::new();
    for path in &paths {
        lines.extend(verdict(path, &root));
    }
    let actual = format!(
        "# Validation verdict per fixture: <path>\\t<severity>\\t<rule id>.\n\
         # Regenerate with BLESS_VALIDATION_SNAPSHOT=1; read the diff before committing.\n\
         # {} fixtures, {} lines.\n{}",
        paths.len(),
        lines.len(),
        lines.join("\n")
    );

    let path = snapshot_path();
    if std::env::var_os("BLESS_VALIDATION_SNAPSHOT").is_some() {
        std::fs::write(&path, format!("{actual}\n")).expect("write snapshot");
        return;
    }

    let expected = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}\nRun with BLESS_VALIDATION_SNAPSHOT=1 to create it.",
            path.display()
        )
    });
    let expected = expected.trim_end();

    if expected == actual {
        return;
    }

    // Report the difference as lines gained and lost — a diff of 6000 lines is
    // unreadable, and the interesting part is always which checks moved.
    let old: BTreeSet<&str> = expected.lines().filter(|l| !l.starts_with('#')).collect();
    let new: BTreeSet<&str> = actual.lines().filter(|l| !l.starts_with('#')).collect();
    let lost: Vec<&&str> = old.difference(&new).take(40).collect();
    let gained: Vec<&&str> = new.difference(&old).take(40).collect();
    panic!(
        "the corpus verdict moved.\n\n\
         no longer reported ({} total, first {} shown):\n  {}\n\n\
         newly reported ({} total, first {} shown):\n  {}\n\n\
         A lost line is a check that stopped firing — confirm that is intended.\n\
         Re-bless with BLESS_VALIDATION_SNAPSHOT=1 once the diff is understood.",
        old.difference(&new).count(),
        lost.len(),
        lost.iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  "),
        new.difference(&old).count(),
        gained.len(),
        gained
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n  "),
    );
}

/// Every fixture must at least *parse* and reach the rule layers.
///
/// `PARSE-ERROR` and `VALIDATE-ERROR` are not verdicts about a message's
/// content — they mean the entry never got as far as being judged, typically a
/// malformed envelope such as a `UNT` DE 0074 count that does not match the
/// segments carried. A receiver answers that with a CONTRL rejection, while
/// `validate-pruefids` still counts the fixture as PID coverage.
///
/// The snapshot cannot catch it: an unchanging failure is an unchanged verdict.
#[test]
fn every_fixture_parses_and_is_judged() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths = Vec::new();
    collect_fixtures(&root, &mut paths);
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no fixtures found under {}",
        root.display()
    );

    let unjudged: Vec<String> = paths
        .iter()
        .flat_map(|p| verdict(p, &root))
        .filter(|line| line.ends_with("PARSE-ERROR") || line.ends_with("VALIDATE-ERROR"))
        .collect();

    assert!(
        unjudged.is_empty(),
        "{} fixture(s) never reached the rule layers — the envelope or the \
         syntax is malformed, so nothing about their content is being checked:\n  {}",
        unjudged.len(),
        unjudged.join("\n  ")
    );
}
