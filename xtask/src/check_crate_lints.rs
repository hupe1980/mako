//! Guard: every crate root denies `unsafe_code`, and `missing_docs` has one
//! policy.
//!
//! `missing_docs` is set once, `warn` in `[workspace.lints.rust]` — a deny
//! under the gate's `-D warnings`. A crate root that states its own
//! `deny`/`warn(missing_docs)` is a second answer to the same question and is
//! refused. A `publish = false` member may opt out — a daemon's library target
//! exists for its binary and its tests, not as an API — and a published crate
//! may not, because its documentation is what crates.io readers get.
//!
//! The workspace holds exactly one `unsafe` block — `makod`'s `main` clearing an
//! empty `MAKOD_DATA_DIR` before clap reads it — and it carries its own
//! `#[allow(unsafe_code)]` and a SAFETY note. Everywhere else the lint is what
//! turns a new one into a build failure instead of something a reviewer has to
//! notice.
//!
//! It has to be checked because it cannot be inherited. `#![deny(...)]` is a
//! crate attribute, and a service with both a `lib.rs` and a `main.rs` is two
//! crates, each of which states the lint for itself or is outside it.
//!
//! `[workspace.lints]` would carry it in one place, but the crates also state
//! their own `pedantic` and per-crate `allow`s, so the roots are the place the
//! rule already lives.
//!
//! The scan covers the trees in [`CRATE_TREES`] (one crate per subdirectory),
//! the roots in [`DIRECT_ROOTS`] and every target in [`TARGET_DIRS`], and the
//! success message names them: a guard that reports a universal has to say
//! which tree that universal is over. `fuzz/` is `exclude`d from the workspace
//! and carries one crate root per `fuzz_targets/*.rs`, which is why it needs a
//! rule of its own rather than a `src/lib.rs` entry.

use std::path::{Path, PathBuf};

/// Trees holding one crate per subdirectory, each rooted at `src/{lib,main}.rs`.
const CRATE_TREES: &[&str] = &["crates", "services"];

/// Crate roots that sit at a fixed path rather than one per subdirectory.
///
/// `makotest/src` is a workspace member published to PyPI as `makotest`; its
/// PyO3 bindings are as much shipped code as any service.
const DIRECT_ROOTS: &[&str] = &["xtask/src", "makotest/src"];

/// Trees where **every** `.rs` file is its own crate root.
///
/// `fuzz/fuzz_targets` is the only one: `cargo-fuzz` compiles each target as a
/// separate `#![no_main]` binary, so each states its own lints. Excluded from
/// the workspace, and reached by `just check-fuzz` rather than by `cargo
/// check` — which is exactly why it needs the guard.
const TARGET_DIRS: &[&str] = &["fuzz/fuzz_targets"];

/// The lint every crate root must state.
const REQUIRED: &[&str] = &["deny(unsafe_code)", "forbid(unsafe_code)"];

/// Whether `src` states the lint as a live crate attribute.
///
/// Matching the bare text would count a commented-out attribute, an
/// `#[allow(unsafe_code)]` on a single item, and this guard's own help line —
/// none of which deny anything. Only an inner attribute (`#![…]`) applies to
/// the whole crate, so only an inner attribute counts.
fn denies_unsafe(src: &str) -> bool {
    src.lines().any(|line| {
        let t = line.trim_start();
        t.starts_with("#![") && REQUIRED.iter().any(|lint| t.contains(lint))
    })
}

/// The inner attributes of `src` (`#![…]`), each joined onto one line.
fn inner_attributes(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in src.lines() {
        let t = line.trim();
        if let Some(buf) = current.as_mut() {
            buf.push(' ');
            buf.push_str(t);
        } else if t.starts_with("#![") {
            current = Some(t.to_owned());
        } else {
            continue;
        }
        let buf = current.as_ref().map_or("", String::as_str);
        if buf.matches('[').count() <= buf.matches(']').count() {
            out.extend(current.take());
        }
    }
    out
}

/// What is wrong with a crate root's `missing_docs` attributes, if anything.
///
/// `published` is whether the crate's manifest admits crates.io (it carries no
/// `publish = false`).
fn docs_policy_finding(src: &str, published: bool) -> Option<&'static str> {
    for attr in inner_attributes(src) {
        if !attr.contains("missing_docs") {
            continue;
        }
        let compact: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
        if compact.contains("deny(missing_docs")
            || compact.contains("warn(missing_docs")
            || compact.contains("forbid(missing_docs")
        {
            return Some(
                "states its own `missing_docs` level — `[workspace.lints.rust]` holds the one policy",
            );
        }
        if published {
            return Some(
                "opts a published crate out of `missing_docs` — only a `publish = false` member may",
            );
        }
    }
    None
}

/// Whether the manifest beside `root` (`<crate>/src/<file>.rs`) lets the crate
/// reach crates.io.
fn is_published(root: &Path) -> bool {
    let Some(manifest) = root
        .parent()
        .and_then(Path::parent)
        .map(|d| d.join("Cargo.toml"))
    else {
        return true;
    };
    !std::fs::read_to_string(manifest)
        .unwrap_or_default()
        .lines()
        .any(|l| {
            let l: String = l.chars().filter(|c| !c.is_whitespace()).collect();
            l == "publish=false"
        })
}

/// Whether the workspace manifest sets `missing_docs` once, as a warning the
/// gate's `-D warnings` turns into a deny.
fn workspace_sets_missing_docs(manifest: &str) -> bool {
    let mut in_rust = false;
    for line in manifest.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_rust = t == "[workspace.lints.rust]";
            continue;
        }
        let compact: String = t.chars().filter(|c| !c.is_whitespace()).collect();
        if in_rust && compact == "missing_docs=\"warn\"" {
            return true;
        }
    }
    false
}

/// Check every crate root in the workspace.
///
/// Returns `true` when each one denies `unsafe_code` and none departs from the
/// workspace's `missing_docs` policy.
pub fn run(workspace_root: &Path) -> bool {
    let docs_ok = check_docs_policy(workspace_root);
    check_unsafe(workspace_root) && docs_ok
}

/// The `missing_docs` half: the workspace sets it, no crate root restates it,
/// no published crate opts out.
fn check_docs_policy(workspace_root: &Path) -> bool {
    let manifest = std::fs::read_to_string(workspace_root.join("Cargo.toml")).unwrap_or_default();
    let mut ok = true;
    if !workspace_sets_missing_docs(&manifest) {
        eprintln!(
            "check-crate-lints: `[workspace.lints.rust]` does not set `missing_docs = \"warn\"`"
        );
        ok = false;
    }
    let mut roots = Vec::new();
    for dir in CRATE_TREES {
        collect(&workspace_root.join(dir), &mut roots);
    }
    for dir in DIRECT_ROOTS {
        for name in ["lib.rs", "main.rs"] {
            let root = workspace_root.join(dir).join(name);
            if root.is_file() {
                roots.push(root);
            }
        }
    }
    roots.sort();
    for root in &roots {
        let src = std::fs::read_to_string(root).unwrap_or_default();
        if let Some(why) = docs_policy_finding(&src, is_published(root)) {
            eprintln!(
                "check-crate-lints: {} {why}",
                root.strip_prefix(workspace_root).unwrap_or(root).display()
            );
            ok = false;
        }
    }
    if ok {
        println!(
            "check-crate-lints: `missing_docs` is set once in `[workspace.lints.rust]`; \
             none of {} crate root(s) restates it, and only `publish = false` ones opt out",
            roots.len()
        );
    }
    ok
}

/// The `unsafe_code` half.
fn check_unsafe(workspace_root: &Path) -> bool {
    let mut roots = Vec::new();
    for dir in CRATE_TREES {
        collect(&workspace_root.join(dir), &mut roots);
    }
    for dir in DIRECT_ROOTS {
        for name in ["lib.rs", "main.rs"] {
            let root = workspace_root.join(dir).join(name);
            if root.is_file() {
                roots.push(root);
            }
        }
    }
    for dir in TARGET_DIRS {
        let Ok(entries) = std::fs::read_dir(workspace_root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                roots.push(path);
            }
        }
    }
    roots.sort();

    // A guard that scanned nothing reports nothing, and "all 0 crate root(s)"
    // reads exactly like a pass. Refuse instead.
    if roots.is_empty() {
        eprintln!(
            "check-crate-lints: the scan found no crate root under {} — \
             the layout has probably changed",
            scope(),
        );
        return false;
    }

    let missing: Vec<&PathBuf> = roots
        .iter()
        .filter(|root| !denies_unsafe(&std::fs::read_to_string(root).unwrap_or_default()))
        .collect();

    if missing.is_empty() {
        println!(
            "check-crate-lints: all {} crate root(s) under {} deny `unsafe_code`",
            roots.len(),
            scope(),
        );
        return true;
    }

    eprintln!(
        "check-crate-lints: {} crate root(s) do not deny `unsafe_code`:",
        missing.len()
    );
    for root in &missing {
        eprintln!(
            "  {}",
            root.strip_prefix(workspace_root).unwrap_or(root).display()
        );
    }
    eprintln!("\nAdd `#![deny(unsafe_code)]` below the crate's module docs.");
    false
}

/// The scanned trees, as the message names them.
fn scope() -> String {
    CRATE_TREES
        .iter()
        .map(|d| format!("{d}/"))
        .chain(DIRECT_ROOTS.iter().map(|d| (*d).to_owned()))
        .chain(TARGET_DIRS.iter().map(|d| format!("{d}/*.rs")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every `<dir>/*/src/{lib,main}.rs`.
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        for name in ["lib.rs", "main.rs"] {
            let root = entry.path().join("src").join(name);
            if root.is_file() {
                out.push(root);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Only a live inner attribute denies anything: a commented-out one, an
    /// `#[allow]` on a single item and a mention in prose all leave the crate
    /// outside the lint.
    #[test]
    fn only_a_live_inner_attribute_counts() {
        assert!(super::denies_unsafe("//! docs\n#![deny(unsafe_code)]\n"));
        assert!(super::denies_unsafe("#![forbid(unsafe_code)]"));
        assert!(!super::denies_unsafe("// #![deny(unsafe_code)]"));
        assert!(!super::denies_unsafe(
            "//! Every crate root denies `unsafe_code`"
        ));
        assert!(!super::denies_unsafe("#[allow(unsafe_code)]\nunsafe { }"));
    }

    /// A crate root restating the level is refused in any crate; an opt-out is
    /// refused in a published crate and admitted in a `publish = false` one.
    #[test]
    fn missing_docs_has_one_policy() {
        use super::docs_policy_finding as f;
        assert!(f("#![deny(missing_docs)]\n", true).is_some());
        assert!(f("#![warn(missing_docs)]\n", false).is_some());
        assert!(f("#![deny(unsafe_code)]\n", true).is_none());
        let opt_out = "#![expect(missing_docs, reason = \"x\")]\n";
        assert!(f(opt_out, true).is_some());
        assert!(f(opt_out, false).is_none());
        assert!(
            f(
                "//! a crate without #![deny(missing_docs)] in prose\n",
                true
            )
            .is_none()
        );
        assert!(super::workspace_sets_missing_docs(
            "[workspace.lints.rust]\nmissing_docs = \"warn\"\n\n[workspace.lints.clippy]\n"
        ));
        assert!(!super::workspace_sets_missing_docs(
            "[workspace.lints.clippy]\nmissing_docs = \"warn\"\n"
        ));
    }

    /// A scan that reaches no file has checked nothing, and must say so rather
    /// than report the clean line.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!super::run(std::path::Path::new(
            "/nonexistent/mako/workspace/root"
        )));
    }
}
