//! Guard: every crate root denies `unsafe_code`.
//!
//! The workspace holds exactly one `unsafe` block — `makod`'s `main` clearing an
//! empty `MAKOD_DATA_DIR` before clap reads it — and it carries its own
//! `#[allow(unsafe_code)]` and a SAFETY note. Everywhere else the lint is what
//! turns a new one into a build failure instead of something a reviewer has to
//! notice.
//!
//! It has to be checked because it cannot be inherited. `#![deny(...)]` is a
//! crate attribute, and a service with both a `lib.rs` and a `main.rs` is two
//! crates: `makod` denied it in `main.rs` and left seventy thousand lines of
//! `lib.rs` outside the lint. Fifteen of the workspace's forty-five crate roots
//! were outside it that way.
//!
//! `[workspace.lints]` would carry it in one place, but only for lints Cargo
//! propagates — and every crate here also states its own `missing_docs`,
//! `pedantic` and per-crate `allow`s, so the roots are the place the rules
//! already live.

use std::path::{Path, PathBuf};

/// The lint every crate root must state.
const REQUIRED: &[&str] = &["deny(unsafe_code)", "forbid(unsafe_code)"];

/// Check every crate root in the workspace.
///
/// Returns `true` when each one denies `unsafe_code`.
pub fn run(workspace_root: &Path) -> bool {
    let mut roots = Vec::new();
    for dir in ["crates", "services"] {
        collect(&workspace_root.join(dir), &mut roots);
    }
    for name in ["lib.rs", "main.rs"] {
        let root = workspace_root.join("xtask/src").join(name);
        if root.is_file() {
            roots.push(root);
        }
    }
    roots.sort();

    let missing: Vec<&PathBuf> = roots
        .iter()
        .filter(|root| {
            let src = std::fs::read_to_string(root).unwrap_or_default();
            !REQUIRED.iter().any(|lint| src.contains(lint))
        })
        .collect();

    if missing.is_empty() {
        println!(
            "check-crate-lints: all {} crate root(s) deny `unsafe_code`",
            roots.len()
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
