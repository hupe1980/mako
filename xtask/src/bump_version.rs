/// Bump the workspace version in the root `Cargo.toml`.
///
/// Updates the following fields atomically:
///
/// 1. `[workspace.package].version = "X.Y.Z"`
/// 2. Every internal workspace crate in `[workspace.dependencies]` → `"X.Y"` (major.minor)
///
/// All workspace-member crates share a single `version.workspace = true` declaration,
/// so bumping `[workspace.package].version` propagates to every crate automatically.
/// The `[workspace.dependencies]` version entries (used for crates.io publishing) are
/// updated here so `cargo publish` resolves them correctly.
///
/// Usage:
/// ```text
/// cargo xtask bump-version 0.5.0
/// ```
pub fn run(workspace_root: &str, args: &[String]) -> bool {
    let new_version = match args.first() {
        Some(v) => v.trim(),
        None => {
            eprintln!(
                "error: bump-version requires a version argument, \
                 e.g. `cargo xtask bump-version 0.2.0`"
            );
            return false;
        }
    };

    // Validate: must be X.Y.Z
    let parts: Vec<&str> = new_version.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u64>().is_err()) {
        eprintln!("error: version must be X.Y.Z (e.g. 0.2.0), got `{new_version}`");
        return false;
    }
    let major_minor = format!("{}.{}", parts[0], parts[1]);

    let cargo_toml_path = format!("{workspace_root}/Cargo.toml");
    let src = match std::fs::read_to_string(&cargo_toml_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: could not read {cargo_toml_path}: {e}");
            return false;
        }
    };

    // Step 1: replace `version     = "…"` under [workspace.package].
    let updated = match replace_first_version_field(&src, new_version) {
        Some(s) => s,
        None => {
            eprintln!("error: could not find [workspace.package] version field");
            return false;
        }
    };

    // Step 2: replace version inside every internal workspace dep.
    // All workspace-member crates share the same X.Y.Z version; the deps use X.Y.
    //
    // The list is *derived* from `[workspace.dependencies]` rather than
    // hardcoded: a hardcoded list silently skips any crate added later, and the
    // resulting version skew only surfaces as a `failed to select a version`
    // build error on the next bump. Every dep whose inline table carries a
    // `path = "crates/…"` is a workspace member and must move together.
    let internal_deps = internal_workspace_deps(&updated);
    if internal_deps.is_empty() {
        eprintln!("error: no internal crates found in [workspace.dependencies]");
        return false;
    }
    let mut updated = updated;
    for dep in &internal_deps {
        match replace_dep_version(&updated, dep, &major_minor) {
            Some(s) => updated = s,
            None => {
                eprintln!("error: could not find {dep} dep version in [workspace.dependencies]");
                return false;
            }
        }
    }

    // Step 3: a member manifest that pins a sibling's version itself is invisible
    // to steps 1 and 2, and the stale pin only surfaces as `failed to select a
    // version` when that crate is published. Refuse the bump instead.
    let strays = member_pins(workspace_root);
    if !strays.is_empty() {
        eprintln!(
            "error: {} member manifest(s) pin an internal crate's version outside \
             [workspace.dependencies], so the bump cannot reach them:",
            strays.len()
        );
        for (path, line) in &strays {
            eprintln!("  {path}  {line}");
        }
        eprintln!(
            "\nDeclare the crate in [workspace.dependencies] and take it with \
             `{{ workspace = true }}`."
        );
        return false;
    }

    match std::fs::write(&cargo_toml_path, &updated) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: could not write {cargo_toml_path}: {e}");
            return false;
        }
    }

    println!("bumped workspace version -> {new_version}");
    println!("  [workspace.package] version = \"{new_version}\"");
    println!(
        "  [workspace.dependencies] {} internal crate(s) version = \"{major_minor}\"",
        internal_deps.len()
    );
    true
}

/// Member manifests pinning an internal crate's version themselves.
///
/// Returns `(path, offending line)` pairs.
fn member_pins(workspace_root: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for group in ["crates", "services"] {
        let Ok(entries) = std::fs::read_dir(format!("{workspace_root}/{group}")) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest = entry.path().join("Cargo.toml");
            let Ok(src) = std::fs::read_to_string(&manifest) else {
                continue;
            };
            for line in pinned_path_deps(&src) {
                out.push((manifest.display().to_string(), line));
            }
        }
    }
    out.sort();
    out
}

/// Lines declaring a `path` dependency that also states a `version`.
///
/// Split from the filesystem so the rule is testable against exact text.
#[must_use]
pub fn pinned_path_deps(manifest: &str) -> Vec<String> {
    manifest
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            !trimmed.starts_with('#')
                && trimmed.contains("path")
                && trimmed.contains("version")
                && trimmed.contains('=')
        })
        .map(|l| l.trim().to_owned())
        .collect()
}

/// Replace the **first** line matching `version\s*=\s*"…"` (always the
/// `[workspace.package]` entry in our root `Cargo.toml`).
fn replace_first_version_field(src: &str, new_version: &str) -> Option<String> {
    let mut result = String::with_capacity(src.len());
    let mut replaced = false;
    for line in src.lines() {
        if !replaced {
            let trimmed = line.trim_start();
            if let Some(after_kw) = trimmed.strip_prefix("version") {
                let after_kw_trim = after_kw.trim_start();
                if let Some(after_eq) = after_kw_trim.strip_prefix('=') {
                    if after_eq.trim_start().starts_with('"') {
                        let leading = &line[..line.len() - trimmed.len()];
                        let gap_len = after_kw.len() - after_kw_trim.len();
                        let gap = " ".repeat(gap_len);
                        result.push_str(leading);
                        result.push_str("version");
                        result.push_str(&gap);
                        result.push_str("= \"");
                        result.push_str(new_version);
                        result.push('"');
                        result.push('\n');
                        replaced = true;
                        continue;
                    }
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if !src.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    replaced.then_some(result)
}

/// Collect every dependency in `[workspace.dependencies]` that points at a
/// path inside `crates/` — i.e. the workspace's own member crates.
///
/// Returns them in file order so the rewrite is deterministic.
fn internal_workspace_deps(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in src.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[workspace.dependencies]";
            continue;
        }
        if !in_section || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, rest)) = trimmed.split_once('=') else {
            continue;
        };
        // Only inline tables with a `crates/` path are internal members.
        if rest.trim_start().starts_with('{') && rest.contains("path = \"crates/") {
            out.push(name.trim().to_owned());
        }
    }
    out
}

/// Find the line starting with `<dep_name>` (optionally followed by alignment
/// spaces) then `= {` and replace the `version = "…"` value within it.
fn replace_dep_version(src: &str, dep_name: &str, new_version: &str) -> Option<String> {
    let mut result = String::with_capacity(src.len());
    let mut replaced = false;
    for line in src.lines() {
        if !replaced {
            let trimmed = line.trim_start();
            let after_name = trimmed
                .strip_prefix(dep_name)
                .map(str::trim_start)
                .unwrap_or("");
            if after_name.starts_with("= {") {
                if let Some(updated) = replace_version_in_inline_table(line, new_version) {
                    result.push_str(&updated);
                    result.push('\n');
                    replaced = true;
                    continue;
                }
            }
        }
        result.push_str(line);
        result.push('\n');
    }
    if !src.ends_with('\n') && result.ends_with('\n') {
        result.pop();
    }
    replaced.then_some(result)
}

/// Within a single TOML inline-table line, replace the quoted value of
/// `version = "…"`.
fn replace_version_in_inline_table(line: &str, new_version: &str) -> Option<String> {
    let key = "version = \"";
    let start = line.find(key)?;
    let after_open = start + key.len();
    let close = line[after_open..].find('"')? + after_open;
    let mut out = String::with_capacity(line.len());
    out.push_str(&line[..after_open]);
    out.push_str(new_version);
    out.push_str(&line[close..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "
[workspace.package]
version     = \"0.9.0\"
authors     = [\"hupe1980\"]

[workspace.dependencies]
edi-energy       = { path = \"crates/edi-energy\", version = \"0.9\" }
mako-engine      = { path = \"crates/mako-engine\", version = \"0.9\" }
mako-markt       = { path = \"crates/mako-markt\", version = \"0.9\" }
grid-billing     = { path = \"crates/grid-billing\", version = \"0.9\" }
eeg-billing      = { path = \"crates/eeg-billing\", version = \"0.9\" }
mako-obs         = { path = \"crates/mako-obs\", version = \"0.9\" }
mako-service     = { path = \"crates/mako-service\", version = \"0.9\" }
invoic-checker   = { path = \"crates/invoic-checker\", version = \"0.9\" }
mako-pruefung    = { path = \"crates/mako-pruefung\", version = \"0.9\" }
energy-billing   = { path = \"crates/energy-billing\", version = \"0.9\" }
serde            = { version = \"1\", features = [\"derive\"] }
";

    #[test]
    fn bumps_package_version() {
        let out = replace_first_version_field(SAMPLE, "0.10.0").unwrap();
        assert!(out.contains("version     = \"0.10.0\""), "{out}");
        assert!(!out.contains("0.9.0"));
    }

    #[test]
    fn bumps_dep_version_aligned() {
        let out = replace_dep_version(SAMPLE, "mako-engine", "0.10").unwrap();
        assert!(out.contains("version = \"0.10\""), "{out}");
        assert!(out.contains("serde            = { version = \"1\""));
    }

    #[test]
    fn full_bump() {
        let v1 = replace_first_version_field(SAMPLE, "0.10.0").unwrap();
        let v2 = replace_dep_version(&v1, "mako-engine", "0.10").unwrap();
        let v3 = replace_dep_version(&v2, "grid-billing", "0.10").unwrap();
        let v4 = replace_dep_version(&v3, "energy-billing", "0.10").unwrap();
        assert!(v4.contains("version     = \"0.10.0\""), "{v4}");
        assert!(
            v4.contains("mako-engine      = { path = \"crates/mako-engine\", version = \"0.10\""),
            "{v4}"
        );
        assert!(
            v4.contains("grid-billing     = { path = \"crates/grid-billing\", version = \"0.10\""),
            "{v4}"
        );
        assert!(v4.contains("serde            = { version = \"1\""));
    }

    /// A member pinning a sibling's version itself is what step 3 refuses: the
    /// root bump cannot reach it, and the stale pin surfaces only at publish.
    #[test]
    fn a_member_pinning_a_sibling_version_is_reported() {
        let manifest = "[dependencies]\n\
             mako-engine  = { workspace = true }\n\
             mako-mabis   = { path = \"../mako-mabis\", version = \"0.18\" }\n\
             serde        = { version = \"1\" }\n";
        let hits = super::pinned_path_deps(manifest);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].starts_with("mako-mabis"));
    }

    /// A bare `path` (a service, never published) and a plain version are both
    /// fine — only the two together are unreachable by the bump.
    #[test]
    fn a_bare_path_or_a_plain_version_is_not_a_pin() {
        let manifest = "[dependencies]\n\
             mako-mabis   = { path = \"../../crates/mako-mabis\" }\n\
             serde        = { version = \"1\" }\n\
             # mako-old   = { path = \"../old\", version = \"0.1\" }\n";
        assert!(super::pinned_path_deps(manifest).is_empty());
    }

    #[test]
    fn rejects_invalid_version() {
        let parts: Vec<&str> = "not-a-version".split('.').collect();
        let valid = parts.len() == 3 && parts.iter().all(|p| p.parse::<u64>().is_ok());
        assert!(!valid);
    }
}
