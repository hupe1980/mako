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
//! **Doc → manifest.** Every row of the form
//! `| [`name`](https://crates.io/crates/name) | `X.Y` |` against the
//! requirement the workspace states for that crate — the
//! `[workspace.dependencies]` entry, or a member manifest for a crate only one
//! service takes. The claim must be a **prefix** of the requirement, so a table
//! saying `0.21` matches a manifest saying `0.21.3`, and `0.2` does not match
//! `0.21`.
//!
//! **Manifest → doc.** One direction only catches a row that goes stale. It
//! says nothing about a dependency that was never written down, and an
//! undocumented domain crate is the more expensive gap: the table is what an
//! auditor reads as the list of what mako rests on, so a crate missing from it
//! is a dependency nobody planned an upgrade for. Every external crate in
//! `[workspace.dependencies]` must therefore have a row, unless it is listed in
//! [`INFRASTRUCTURE_ONLY`].
//!
//! A workspace member (`path = …`) is mako's own code and belongs to the
//! service pages, not to this table.

use std::path::Path;

/// Where the documented table lives.
const DOC: &str = "site/content/docs/architecture/_index.md";

/// External crates the architecture table does not have to name.
///
/// The table is about the **domain** mako rests on — the crates that carry
/// German energy-market, billing or identifier semantics, where a version bump
/// changes what the platform means rather than how it runs. Everything below is
/// plumbing: it is replaceable without touching a single business rule, and a
/// reader planning a `metering` upgrade is not helped by a row for `anyhow`.
///
/// A crate belongs here only if that is true of it. Adding a domain crate to
/// this list is how the table quietly stops being the list it claims to be.
const INFRASTRUCTURE_ONLY: &[&str] = &[
    // ── Error handling, data structures, text ──
    "anyhow",
    "thiserror",
    "miette",
    "dashmap",
    "strsim",
    "arrow",
    // ── Serialisation and schema ──
    "serde",
    "serde_json",
    "schemars",
    // ── Numbers, time and identifiers ──
    //
    // `rust_decimal` is money's representation, not its rules: what mako holds
    // itself to is the rounding mode, which `check-rounding` pins.
    "rust_decimal",
    "time",
    "time-tz",
    "uuid",
    // ── Transport, configuration and process plumbing ──
    "reqwest",
    "tokio-util",
    "tower-http",
    "rmcp",
    "krafka",
    "figment",
    "figment-file-provider-adapter",
    // ── Security ──
    "jsonwebtoken",
    "cedar-policy",
    // ── Observability ──
    "tracing-subscriber",
    "tracing-opentelemetry",
    "opentelemetry",
    "opentelemetry_sdk",
    "opentelemetry-otlp",
    "opentelemetry-semantic-conventions",
    // ── Test harness ──
    "testcontainers",
    "testcontainers-modules",
];

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

    let documented: std::collections::BTreeSet<&str> =
        claims.iter().map(|(n, _)| n.as_str()).collect();
    let root_manifest =
        std::fs::read_to_string(workspace_root.join("Cargo.toml")).unwrap_or_default();
    let external = external_workspace_dependencies(&root_manifest);
    if external.len() < 20 {
        eprintln!(
            "check-dep-versions: found only {} external `[workspace.dependencies]` entries — \
             has the manifest layout changed?",
            external.len()
        );
        return false;
    }
    for name in &external {
        if INFRASTRUCTURE_ONLY.contains(&name.as_str()) || documented.contains(name.as_str()) {
            continue;
        }
        problems.push(format!(
            "  {name}: pinned in [workspace.dependencies] with no row in the table, and not \
             listed as infrastructure"
        ));
    }

    if problems.is_empty() {
        println!(
            "check-dep-versions: {} documented dependency version(s) match the manifests, and \
             every one of the {} external workspace dependencies is documented or infrastructure",
            claims.len(),
            external.len()
        );
        return true;
    }
    eprintln!("check-dep-versions: the architecture page and the manifests disagree:");
    for p in &problems {
        eprintln!("{p}");
    }
    eprintln!(
        "\nUpdate the table in {DOC} — and its description, if the new version changed what \
         mako uses. A crate that carries no domain meaning belongs in `INFRASTRUCTURE_ONLY` \
         instead, in the category it fits."
    );
    false
}

/// Every `[workspace.dependencies]` entry naming a crate from outside this
/// workspace.
///
/// A `path = …` entry is a member of this repository: it has no crates.io
/// version to document and its story is on a service page.
#[must_use]
pub fn external_workspace_dependencies(manifest: &str) -> Vec<String> {
    let Some(start) = manifest.find("[workspace.dependencies]") else {
        return Vec::new();
    };
    let body = &manifest[start + "[workspace.dependencies]".len()..];
    let body = body.find("\n[").map_or(body, |end| &body[..end]);

    let mut out = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, spec)) = line.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            continue;
        }
        if spec.contains("path") {
            continue;
        }
        out.push(name.to_owned());
    }
    out
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
        // A requirement operator is part of the claim: the table states
        // `~0.17` for `edifact-rs` because the manifest does, and a reader that
        // insisted on a leading digit dropped that row from both directions of
        // this check.
        let is_requirement = version.chars().next().is_some_and(|c| {
            c.is_ascii_digit() || c == '~' || c == '^' || c == '=' || c == '>' || c == '<'
        });
        if is_requirement && version.chars().any(|c| c.is_ascii_digit()) {
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

    /// A caret or tilde requirement is still a version claim.
    ///
    /// `edifact-rs` is pinned `~0.17`, and a reader that required a leading
    /// digit dropped its row — so neither direction of this check saw the crate
    /// at all: the pin went unchecked *and* the reverse scan called it
    /// undocumented.
    #[test]
    fn reads_a_requirement_operator() {
        let doc = "| [`edifact-rs`](https://crates.io/crates/edifact-rs) | `~0.17` | syntax |";
        assert_eq!(
            documented_versions(doc),
            vec![("edifact-rs".to_owned(), "~0.17".to_owned())]
        );
        assert!(
            requirement("edifact-rs   = \"~0.17\"", "edifact-rs")
                .is_some_and(|r| r.starts_with("~0.17"))
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

    /// The reverse direction: a crate the manifest pins and the table forgets.
    ///
    /// One direction only catches a row that went stale; it is blind to a
    /// dependency that was never written down, which is the gap an auditor
    /// reading the table as an inventory falls into.
    #[test]
    fn external_dependencies_are_read_and_members_are_not() {
        let manifest = "[workspace.dependencies]\n\
                        # a comment\n\
                        metering = { version = \"0.22\" }\n\
                        anyhow = \"1\"\n\
                        mako-markt = { path = \"crates/mako-markt\" }\n\
                        \n\
                        [workspace.lints.clippy]\n\
                        pedantic = \"warn\"\n";
        assert_eq!(
            super::external_workspace_dependencies(manifest),
            vec!["metering".to_owned(), "anyhow".to_owned()]
        );
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
