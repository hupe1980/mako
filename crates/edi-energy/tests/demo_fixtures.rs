//! Every EDIFACT fixture the runnable demos post must validate.
//!
//! `demos/` is the first mako anyone runs, and its fixtures get copied into
//! tickets as "this is what a 55001 looks like" — so a demo must not post a
//! message mako itself rejects.
//!
//! They stay outside `tests/fixtures/` on purpose: under the conformance suite
//! they would stop being the files the READMEs tell people to
//! `curl --data-binary`.

use std::path::{Path, PathBuf};

use edi_energy::{EdiEnergyMessage, Platform};

/// The repository's `demos/` directory.
fn demos_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../demos")
        .canonicalize()
        .expect("demos/ is part of the repository")
}

/// Every `.edi` file under `demos/`, sorted for a deterministic failure order.
fn demo_edi_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            demo_edi_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "edi") {
            out.push(path);
        }
    }
}

#[test]
fn every_demo_edifact_fixture_validates() {
    let root = demos_dir();
    let mut files = Vec::new();
    demo_edi_files(&root, &mut files);
    assert!(
        !files.is_empty(),
        "no .edi fixtures found under {} — the glob or the demos moved",
        root.display()
    );

    let mut failures = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .display()
            .to_string();
        let raw = std::fs::read(path).expect("fixture is readable");
        let msg = match Platform::with_all_profiles().parse(&raw) {
            Ok(m) => m,
            Err(e) => {
                failures.push(format!("{rel}: parse failed: {e}"));
                continue;
            }
        };
        match msg.validate() {
            Err(e) => failures.push(format!("{rel}: validation could not run: {e}")),
            Ok(report) => {
                for issue in report.errors() {
                    failures.push(format!(
                        "{rel}: [{}] {}",
                        issue.rule_id.as_deref().unwrap_or("-"),
                        issue.message
                    ));
                }
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} demo fixture error(s) — the demos post messages mako itself rejects:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
