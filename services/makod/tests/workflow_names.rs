//! Guard: a process is spawned and resumed under the same workflow name.
//!
//! `spawn_or_resume`, `resume_by_key` and `dispatch_to_process` all take the
//! workflow name as a **string argument**, and the domain crate that owns the
//! process declares it as a `pub const WORKFLOW_NAME`. Nothing links the two.
//!
//! A literal that disagrees with the constant does not fail: the stream is
//! written under one name and looked up under another, so every resume misses
//! and each inbound message starts a *new* process. The counterparty's answer
//! never finds the conversation it belongs to, and the original process sits
//! until its Frist expires — with no error anywhere, because both halves did
//! exactly what they were told.
//!
//! This is the workflow-name twin of rule 2 in `deadline_labels.rs`, and it
//! exists for the same reason: the sources are the only place both ends of the
//! string are visible.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("makod lives two levels below the workspace root")
        .to_path_buf()
}

fn rust_sources(root: &Path, rel: &str) -> Vec<PathBuf> {
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
    let mut out = Vec::new();
    walk(&root.join(rel), &mut out);
    out.sort();
    out
}

/// Every `pub const WORKFLOW_NAME: &str = "…"` across the domain crates.
fn workflow_name_constants(root: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for path in rust_sources(root, "crates") {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in src.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("pub const WORKFLOW_NAME") else {
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

/// Every kebab-case string literal passed to a process-spawning helper.
fn spawned_names(root: &Path) -> Vec<(PathBuf, String)> {
    const HELPERS: &[&str] = &["spawn_or_resume", "resume_by_key", "dispatch_to_process"];
    let mut out = Vec::new();
    for path in rust_sources(root, "services/makod/src") {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for helper in HELPERS {
            let mut from = 0;
            while let Some(at) = src[from..].find(helper) {
                let at = from + at;
                from = at + helper.len();
                // Skip past the turbofish to the argument list.
                let Some(open) = src[at..].find('(') else {
                    break;
                };
                let start = at + open + 1;
                let mut depth = 1_i32;
                let mut end = start;
                for (i, ch) in src[start..].char_indices() {
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                end = start + i;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                for lit in string_literals(&src[start..end]) {
                    // Workflow names are kebab-case with at least one hyphen;
                    // anything else in the argument list is a different kind of
                    // string (a business key, a reason, a header name).
                    if lit.contains('-')
                        && lit
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
                    {
                        out.push((path.clone(), lit));
                    }
                }
            }
        }
    }
    out
}

fn string_literals(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes: Vec<char> = segment.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == '"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != '"' {
                if bytes[j] == '\\' {
                    j += 1;
                }
                j += 1;
            }
            if j < bytes.len() {
                out.push(bytes[start..j].iter().collect());
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    out
}

/// Every workflow name `makod` spawns under is one a domain crate declares.
#[test]
fn every_spawned_workflow_name_is_a_declared_constant() {
    let root = workspace_root();
    let declared = workflow_name_constants(&root);
    assert!(
        declared.len() > 40,
        "the scanner found only {} WORKFLOW_NAME constants — has the layout changed?",
        declared.len()
    );

    let spawned = spawned_names(&root);
    assert!(
        spawned.len() > 20,
        "the scanner found only {} spawned names — has the layout changed?",
        spawned.len()
    );

    let unknown: BTreeSet<String> = spawned
        .iter()
        .filter(|(_, name)| !declared.contains(name))
        .map(|(path, name)| {
            format!(
                "{name:?} ({})",
                path.strip_prefix(&root).unwrap_or(path).display()
            )
        })
        .collect();

    assert!(
        unknown.is_empty(),
        "these workflow names are spawned but no domain crate declares them, so the \
         stream is written under one name and looked up under another — every resume \
         misses, each inbound message starts a new process, and nothing errors.\n  {}",
        unknown.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}

/// Where a `WORKFLOW_NAME` constant is declared, and how `makod` could name it.
struct Declaration {
    name: String,
    /// Module file stem — the `mako_gpke::comdis::WORKFLOW_NAME` form.
    module: String,
    /// Enclosing `impl` type, when the constant is an associated one — the
    /// `EmobAbmeldungWorkflow::WORKFLOW_NAME` form.
    impl_type: Option<String>,
    path: PathBuf,
}

/// Every `WORKFLOW_NAME` declaration, with the paths `makod` can reach it by.
fn declarations(root: &Path) -> Vec<Declaration> {
    let mut out = Vec::new();
    for path in rust_sources(root, "crates") {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let module = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut impl_type: Option<String> = None;
        for line in src.lines() {
            let trimmed = line.trim();
            // Track the innermost `impl X {` seen so far; an associated
            // constant belongs to it.
            if let Some(rest) = trimmed.strip_prefix("impl ")
                && rest.ends_with('{')
            {
                impl_type = rest
                    .trim_end_matches('{')
                    .split_whitespace()
                    .next_back()
                    .map(str::to_owned);
            }
            let Some(rest) = trimmed.strip_prefix("pub const WORKFLOW_NAME") else {
                continue;
            };
            let Some(open) = rest.find('"') else { continue };
            let Some(close) = rest[open + 1..].find('"') else {
                continue;
            };
            out.push(Declaration {
                name: rest[open + 1..open + 1 + close].to_owned(),
                module: module.clone(),
                impl_type: impl_type.clone(),
                path: path.clone(),
            });
        }
    }
    out
}

/// Every workflow a domain crate declares is reachable from `makod`.
///
/// Nothing about declaring a `WORKFLOW_NAME` connects it to a router. An
/// unregistered workflow compiles, ships in the crate's public API, reads as a
/// covered process, and can never run — no Prüfidentifikator resolves to it and
/// no command moves it.
///
/// Three routers can reach one and the check does not care which
/// (`EdifactIngestDispatcher`, `mako_redispatch::router`, the API-Webdienste
/// REST channel). What it refuses is a name **no** part of `makod` mentions.
#[test]
fn every_declared_workflow_is_reachable_from_makod() {
    let root = workspace_root();
    let declared = declarations(&root);
    assert!(
        declared.len() > 40,
        "the scanner found only {} WORKFLOW_NAME declarations — has the layout changed?",
        declared.len()
    );

    let makod: String = rust_sources(&root, "services/makod/src")
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();

    let names_it = |needle: &str| {
        makod.match_indices(needle).any(|(i, _)| {
            makod[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric() && c != '_')
        })
    };

    let stranded: Vec<String> = declared
        .iter()
        .filter(|d| {
            let literal = format!("\"{}\"", d.name);
            let by_module = format!("{}::WORKFLOW_NAME", d.module);
            let by_type = d.impl_type.as_ref().map(|t| format!("{t}::WORKFLOW_NAME"));
            !(makod.contains(&literal)
                || names_it(&by_module)
                || by_type.is_some_and(|t| names_it(&t)))
        })
        .map(|d| {
            format!(
                "{:?} ({})",
                d.name,
                d.path.strip_prefix(&root).unwrap_or(&d.path).display()
            )
        })
        .collect();

    assert!(
        stranded.is_empty(),
        "these workflows are declared but no router in makod can reach them, so they \
         are unrunnable code shipped in a crate's public API — register them, or delete \
         them:\n  {}",
        stranded.join("\n  ")
    );
}
