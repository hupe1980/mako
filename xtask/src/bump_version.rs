/// Bump the workspace version in the root `Cargo.toml`.
///
/// Updates exactly two fields atomically:
///
/// 1. `[workspace.package].version = "X.Y.Z"`
/// 2. `[workspace.dependencies].mako-engine` version → `"X.Y"` (major.minor,
///    so patch bumps of `mako-engine` don't require republishing dependents)
///
/// Usage:
/// ```text
/// cargo xtask bump-version 0.2.0
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

    // Step 2: replace version inside `mako-engine = { …, version = "X.Y" }`.
    let updated = match replace_dep_version(&updated, "mako-engine", &major_minor) {
        Some(s) => s,
        None => {
            eprintln!("error: could not find mako-engine dep version in [workspace.dependencies]");
            return false;
        }
    };

    match std::fs::write(&cargo_toml_path, &updated) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("error: could not write {cargo_toml_path}: {e}");
            return false;
        }
    }

    println!("bumped workspace version -> {new_version}");
    println!("  [workspace.package] version             = \"{new_version}\"");
    println!("  [workspace.dependencies] mako-engine version = \"{major_minor}\"");
    true
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

/// Find the line starting with `<dep_name> = {` and replace the `version =
/// "…"` value within it.
fn replace_dep_version(src: &str, dep_name: &str, new_version: &str) -> Option<String> {
    let needle = format!("{dep_name} = {{");
    let mut result = String::with_capacity(src.len());
    let mut replaced = false;
    for line in src.lines() {
        if !replaced && line.trim_start().starts_with(&needle) {
            if let Some(updated) = replace_version_in_inline_table(line, new_version) {
                result.push_str(&updated);
                result.push('\n');
                replaced = true;
                continue;
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
version     = \"0.1.0\"
authors     = [\"hupe1980\"]

[workspace.dependencies]
mako-engine = { path = \"crates/mako-engine\", version = \"0.1\" }
serde       = { version = \"1\", features = [\"derive\"] }
";

    #[test]
    fn bumps_package_version() {
        let out = replace_first_version_field(SAMPLE, "0.2.0").unwrap();
        assert!(out.contains("version     = \"0.2.0\""), "{out}");
        assert!(!out.contains("0.1.0"));
    }

    #[test]
    fn bumps_dep_version() {
        let out = replace_dep_version(SAMPLE, "mako-engine", "0.2").unwrap();
        assert!(out.contains("version = \"0.2\""), "{out}");
        assert!(out.contains("serde       = { version = \"1\""));
    }

    #[test]
    fn full_bump() {
        let v1 = replace_first_version_field(SAMPLE, "0.2.0").unwrap();
        let v2 = replace_dep_version(&v1, "mako-engine", "0.2").unwrap();
        assert!(v2.contains("version     = \"0.2.0\""), "{v2}");
        assert!(v2.contains("version = \"0.2\""), "{v2}");
        assert!(v2.contains("serde       = { version = \"1\""));
    }

    #[test]
    fn rejects_invalid_version() {
        let parts: Vec<&str> = "not-a-version".split('.').collect();
        let valid = parts.len() == 3 && parts.iter().all(|p| p.parse::<u64>().is_ok());
        assert!(!valid);
    }
}
