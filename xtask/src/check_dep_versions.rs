//! Guard: the documented dependency versions are the ones the build resolves.
//!
//! `site/content/docs/architecture/_index.md` carries a table of the external
//! crates mako's domain rests on, each with the version it is pinned to and a
//! description of what that version provides. A reader plans an upgrade from it
//! and an auditor reads it as a statement about the deployed artefact.
//!
//! Nothing tied the two together, and four of six rows had drifted — `sepa`,
//! `metering`, `meterstore` and `doubleentry` each named a version older than
//! the one in the manifests, by one or two breaking minors. A version in prose
//! is a claim like any other count this workspace pins.
//!
//! ## What it compares
//!
//! Every row of the form `| [`name`](https://crates.io/crates/name) | `X.Y` |`
//! against the requirement the workspace states for that crate — the
//! `[workspace.dependencies]` entry, or a member manifest for a crate only one
//! service takes. The claim must be a **prefix** of the requirement, so a table
//! saying `0.21` matches a manifest saying `0.21.3`, and `0.2` does not match
//! `0.21`.

use std::path::Path;

/// Where the documented table lives.
const DOC: &str = "site/content/docs/architecture/_index.md";

/// Scan the docs table against the manifests.
///
/// Returns `true` when every documented version matches what the build pins.
pub fn run(workspace_root: &Path) -> bool {
    let doc_path = workspace_root.join(DOC);
    let Ok(doc) = std::fs::read_to_string(&doc_path) else {
        eprintln!("check-dep-versions: cannot read {DOC}");
        return false;
    };
    let manifests = collect_manifests(workspace_root);

    let claims = documented_versions(&doc);
    if claims.is_empty() {
        eprintln!("check-dep-versions: no dependency rows found in {DOC} — has the table moved?");
        return false;
    }

    let mut problems = Vec::new();
    for (name, claimed) in &claims {
        match manifests.iter().find_map(|m| requirement(m, name)) {
            Some(req) if req.starts_with(claimed.as_str()) => {}
            Some(req) => problems.push(format!("  {name}: documented `{claimed}`, pinned `{req}`")),
            None => problems.push(format!(
                "  {name}: documented `{claimed}`, but no manifest requires it"
            )),
        }
    }

    if problems.is_empty() {
        println!(
            "check-dep-versions: {} documented dependency version(s) match the manifests",
            claims.len()
        );
        return true;
    }
    eprintln!("check-dep-versions: the architecture page states versions the build does not pin:");
    for p in &problems {
        eprintln!("{p}");
    }
    eprintln!(
        "\nUpdate the table in {DOC} — and its description, if the new version changed what mako uses."
    );
    false
}

/// Every `Cargo.toml` that can state a version requirement.
fn collect_manifests(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
        out.push(s);
    }
    for dir in ["crates", "services"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            if let Ok(s) = std::fs::read_to_string(e.path().join("Cargo.toml")) {
                out.push(s);
            }
        }
    }
    out
}

/// The `(crate, version)` pairs the table claims.
#[must_use]
pub fn documented_versions(doc: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in doc.lines() {
        let line = line.trim_start();
        if !line.starts_with("| [`") {
            continue;
        }
        let Some((name, rest)) = line.strip_prefix("| [`").and_then(|r| r.split_once("`]")) else {
            continue;
        };
        if !rest.contains("https://crates.io/crates/") {
            continue;
        }
        // The version is the next cell, itself in backticks.
        let Some(after) = rest.split_once("| `").map(|(_, a)| a) else {
            continue;
        };
        let Some((version, _)) = after.split_once('`') else {
            continue;
        };
        if version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            out.push((name.to_owned(), version.to_owned()));
        }
    }
    out
}

/// The version requirement `manifest` states for `name`, if any.
///
/// Reads both `name = "1.2"` and `name = { version = "1.2", … }`. A `workspace
/// = true` entry states no version of its own and is skipped, so the workspace
/// manifest answers for it.
#[must_use]
pub fn requirement(manifest: &str, name: &str) -> Option<String> {
    for line in manifest.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix(name) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(v) = rest.strip_prefix('"') {
            return v.split('"').next().map(str::to_owned);
        }
        if rest.starts_with('{')
            && let Some((_, after)) = rest.split_once("version")
            && let Some((_, after)) = after.split_once('"')
        {
            return after.split('"').next().map(str::to_owned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{documented_versions, requirement};

    #[test]
    fn reads_a_table_row() {
        let doc = "| [`metering`](https://crates.io/crates/metering) | `0.21` | German … |";
        assert_eq!(
            documented_versions(doc),
            vec![("metering".to_owned(), "0.21".to_owned())]
        );
    }

    /// A row without a crates.io link is prose, not a dependency claim.
    #[test]
    fn ignores_rows_that_are_not_dependencies() {
        let doc = "| [`edmd`](@/docs/services/edmd.md) | `:8380` | the daemon |";
        assert!(documented_versions(doc).is_empty());
    }

    #[test]
    fn reads_both_manifest_spellings() {
        assert_eq!(
            requirement("sepa             = { version = \"0.6.0\" }", "sepa").as_deref(),
            Some("0.6.0")
        );
        assert_eq!(
            requirement("metering = \"0.21\"", "metering").as_deref(),
            Some("0.21")
        );
        assert_eq!(requirement("other = \"1\"", "metering"), None);
    }

    /// A member taking the workspace pin states no version of its own.
    #[test]
    fn a_workspace_entry_states_no_version() {
        assert_eq!(
            requirement("metering = { workspace = true }", "metering"),
            None
        );
    }
}
