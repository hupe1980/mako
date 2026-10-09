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
//! Both directions run here.
//!
//! **Declared → documented.** Every `WORKFLOW_NAME` constant must appear in the
//! document, spelled in backticks.
//!
//! **Documented → declared.** Every backticked, workflow-shaped name in the
//! document must be a constant some crate declares. Nothing else checks this:
//! `workflow_names.rs` reads the crates and `makod`'s module list and never
//! opens this document, so a row left behind by a renamed or deleted workflow
//! reads as coverage mako does not have — which is the same lie as a missing
//! row, told the other way round.
//!
//! "Workflow-shaped" is calibrated from the constants themselves: a kebab-case
//! token whose first segment is the first segment of some declared name. So
//! `gpke-lieferbeginn` is checked and `mako-markt` is not, without a hard-coded
//! vocabulary that would drift from the crates. Service directory names are
//! excluded — `mabis-syncd` fits the shape and is a daemon.
//!
//! Prose naming a *process* without a workflow is deliberate, and stays prose:
//! only a backticked name is read as a claim about a constant.

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

/// Every directory under `services/` and `crates/`.
///
/// A service or crate name can fit the workflow shape — `mabis-syncd` shares
/// `mabis-zp-lifecycle`'s first segment — and naming one in the document is
/// not a claim about a workflow.
fn workspace_member_names(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for dir in ["services", "crates"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                out.insert(entry.file_name().to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// Backticked workflow-shaped names in `doc` that no crate declares.
///
/// Split from the filesystem so the rule is testable against exact text.
fn undeclared_names(
    doc: &str,
    declared: &BTreeSet<String>,
    members: &BTreeSet<String>,
) -> Vec<String> {
    let prefixes: BTreeSet<&str> = declared
        .iter()
        .filter_map(|n| n.split('-').next())
        .collect();

    let mut out = BTreeSet::new();
    // Backticked spans, one line at a time so an unclosed backtick cannot
    // swallow the rest of the document.
    for line in doc.lines() {
        let mut parts = line.split('`');
        // The text before the first backtick is not inside one.
        parts.next();
        let mut inside = true;
        for part in parts {
            if inside && is_workflow_shaped(part, &prefixes) {
                out.insert(part.to_owned());
            }
            inside = !inside;
        }
    }
    out.retain(|name| !declared.contains(name) && !members.contains(name));
    out.into_iter().collect()
}

/// Whether `token` has the shape of a workflow name in a known family.
fn is_workflow_shaped(token: &str, prefixes: &BTreeSet<&str>) -> bool {
    if !token.contains('-') {
        return false;
    }
    let kebab = token.split('-').all(|seg| {
        !seg.is_empty()
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    });
    if !kebab {
        return false;
    }
    token
        .split('-')
        .next()
        .is_some_and(|first| prefixes.contains(first))
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
        "these workflows run but the process map does not name them, so it understates \
         what mako covers — add a row, spelling the name in backticks:\n  {}",
        missing
            .iter()
            .map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );

    // The reverse: a row surviving a rename claims coverage mako does not have.
    let stale = undeclared_names(&doc, &declared, &workspace_member_names(&root));
    assert!(
        stale.is_empty(),
        "the process map names these as workflows and no crate declares them, so it \
         overstates what mako covers — drop the row, or fix the spelling:\n  {}",
        stale.join("\n  ")
    );
}

/// The reverse scan, against text rather than the tree.
///
/// A row left behind by a renamed workflow is invisible to every other guard:
/// the name still reads as a workflow, and nothing compares it with the
/// constants. The negatives matter as much — a crate name and a daemon name can
/// both fit the shape.
#[test]
fn a_row_for_a_workflow_that_does_not_exist_is_caught() {
    let declared: BTreeSet<String> = ["gpke-lf-anmeldung", "mabis-zp-lifecycle"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    let members: BTreeSet<String> = ["mabis-syncd", "mako-markt"]
        .into_iter()
        .map(str::to_owned)
        .collect();

    let doc = "| `gpke-lf-anmeldung` | runs |\n\
               | `gpke-lieferbeginn` | renamed away |\n\
               | `mabis-zp-lifecycle` | runs, via `mabis-syncd` in `mako-markt` |\n\
               plain gpke-lieferende prose is not a claim\n";
    assert_eq!(
        undeclared_names(doc, &declared, &members),
        vec!["gpke-lieferbeginn".to_owned()]
    );
}
