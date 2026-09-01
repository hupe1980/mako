//! Guard: every workflow a domain crate declares has a row in the process map.
//!
//! `concepts/PROCESS_COVERAGE.md` is the cross-cutting inventory — one row per
//! regulated process, regardless of which Marktrolle initiates it. Its value is
//! entirely in being complete: a reader asking „does mako run the
//! Abrechnungsdaten process" reads a *silence* as a no.
//!
//! Every other check runs inside the code — `validate_dispatch_completeness`
//! that a routed Prüfidentifikator reaches an arm, `workflow_names.rs` that a
//! spawned name is declared — so a workflow can route, spawn, answer and settle
//! while the document mapping the processes never mentions it.
//!
//! One direction only: every declared name must appear. A row for a workflow
//! that no longer exists is caught by `workflow_names.rs`, and prose naming a
//! *process* without a workflow is deliberate.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("makod lives two levels below the workspace root")
        .to_path_buf()
}

/// Every `pub const WORKFLOW_NAME … = "…"` across the domain crates.
///
/// The same scan `workflow_names.rs` runs, so the two guards agree on what a
/// workflow name is. It matches both the free-standing `&str` constant and the
/// `&'static str` associated constant the eMob workflows declare.
fn workflow_name_constants(root: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);

    let mut out = BTreeSet::new();
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in src.lines() {
            let Some(rest) = line.trim().strip_prefix("pub const WORKFLOW_NAME") else {
                continue;
            };
            let Some(open) = rest.find('"') else { continue };
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            out.insert(rest[open + 1..open + 1 + close].to_owned());
        }
    }
    out
}

#[test]
fn every_declared_workflow_has_a_row_in_the_process_map() {
    let root = workspace_root();
    let doc_path = root.join("concepts/PROCESS_COVERAGE.md");
    let Ok(doc) = std::fs::read_to_string(&doc_path) else {
        // `concepts/` is not in git, so a checkout without it is normal and only
        // costs this guard.
        eprintln!("skipping: {} is not present", doc_path.display());
        return;
    };

    let declared = workflow_name_constants(&root);
    assert!(
        declared.len() > 40,
        "the scanner found only {} WORKFLOW_NAME constants — has the layout changed?",
        declared.len()
    );

    // Backticked, so a workflow name is credited only where it is written as
    // code. A shorthand continuation (`-nomination` after `gabi-gas-allocation`)
    // reads as a name to a human and matches nothing a reader can grep for.
    let missing: Vec<&String> = declared
        .iter()
        .filter(|name| !doc.contains(&format!("`{name}`")))
        .collect();

    assert!(
        missing.is_empty(),
        "these workflows run but concepts/PROCESS_COVERAGE.md does not name them, so \
         the process map understates what mako covers — add a row, spelling the name \
         in backticks:\n  {}",
        missing
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}
