//! Guard: every entry of the workspace `clippy.toml` fires on a planted use.
//!
//! A `disallowed-methods` / `disallowed-types` entry whose path does not
//! resolve is ignored by clippy without a word — a re-export moved, a method
//! renamed upstream — and the ban then reads as enforced while enforcing
//! nothing. `xtask/clippy-fixtures` uses every banned item once, each use
//! marked `// BAN: <path>`, and this check runs clippy over it and requires:
//!
//! - every configured path has a marker in the fixture, and every marker names
//!   a configured path;
//! - clippy reports every marker's path on the marker's own line.

use std::collections::BTreeSet;
use std::path::Path;

const FIXTURE: &str = "xtask/clippy-fixtures";

/// The `path`s of `disallowed-methods` and `disallowed-types` in `clippy.toml`.
fn configured(clippy_toml: &str) -> Result<BTreeSet<String>, String> {
    let doc: toml::Table = clippy_toml
        .parse()
        .map_err(|e| format!("clippy.toml: {e}"))?;
    let mut out = BTreeSet::new();
    for key in ["disallowed-methods", "disallowed-types"] {
        for entry in doc
            .get(key)
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
        {
            let path = entry
                .get("path")
                .and_then(toml::Value::as_str)
                .or_else(|| entry.as_str())
                .ok_or_else(|| format!("clippy.toml: an entry of `{key}` names no path"))?;
            out.insert(path.to_owned());
        }
    }
    Ok(out)
}

/// `(line, path)` of every `// BAN: <path>` marker, 1-based lines.
fn markers(src: &str) -> Vec<(usize, String)> {
    src.lines()
        .enumerate()
        .filter_map(|(i, l)| Some((i + 1, l.split_once("// BAN: ")?.1.trim().to_owned())))
        .collect()
}

/// `(line, message)` of every disallowed-item diagnostic in clippy's JSON
/// output.
fn reported(json_lines: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for line in json_lines.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let msg = &v["message"];
        let code = msg["code"]["code"].as_str().unwrap_or_default();
        if !matches!(
            code,
            "clippy::disallowed_methods" | "clippy::disallowed_types"
        ) {
            continue;
        }
        let Some(span) = msg["spans"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["is_primary"] == true))
        else {
            continue;
        };
        let line_no = span["line_start"].as_u64().unwrap_or_default() as usize;
        out.push((
            line_no,
            msg["message"].as_str().unwrap_or_default().to_owned(),
        ));
    }
    out
}

/// What is wrong, given the three inputs.
fn problems(
    config: &BTreeSet<String>,
    marked: &[(usize, String)],
    diagnostics: &[(usize, String)],
) -> Vec<String> {
    let mut out = Vec::new();
    let marked_paths: BTreeSet<&str> = marked.iter().map(|(_, p)| p.as_str()).collect();
    for path in config {
        if !marked_paths.contains(path.as_str()) {
            out.push(format!(
                "`{path}` is banned but the fixture plants no use of it"
            ));
        }
    }
    for (line, path) in marked {
        if !config.contains(path) {
            out.push(format!(
                "fixture line {line} marks `{path}`, which clippy.toml does not ban"
            ));
            continue;
        }
        let hit = diagnostics
            .iter()
            .any(|(l, msg)| l == line && msg.contains(&format!("`{path}`")));
        if !hit {
            out.push(format!(
                "clippy did not report `{path}` on fixture line {line} — the configured path no \
                 longer resolves, so the ban enforces nothing"
            ));
        }
    }
    out
}

pub fn run(root: &Path) -> bool {
    let fixture = root.join(FIXTURE);
    let config = match std::fs::read_to_string(root.join("clippy.toml"))
        .map_err(|e| format!("clippy.toml: {e}"))
        .and_then(|s| configured(&s))
    {
        Ok(c) if !c.is_empty() => c,
        Ok(_) => {
            eprintln!("check-clippy-bans: clippy.toml bans nothing — the scan certifies nothing");
            return false;
        }
        Err(e) => {
            eprintln!("check-clippy-bans: {e}");
            return false;
        }
    };
    let src = std::fs::read_to_string(fixture.join("src/lib.rs")).unwrap_or_default();
    let marked = markers(&src);
    let output =
        std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "clippy",
                "--locked",
                "--message-format=json",
                "--manifest-path",
            ])
            .arg(fixture.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", root.join("target/clippy-fixtures"))
            .env_remove("RUSTFLAGS")
            .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => {
            eprintln!("check-clippy-bans: cannot run cargo clippy: {e}");
            return false;
        }
    };
    let diagnostics = reported(&String::from_utf8_lossy(&output.stdout));
    let found = problems(&config, &marked, &diagnostics);
    if found.is_empty() {
        println!(
            "check-clippy-bans: all {} clippy.toml bans fire on their planted use",
            config.len()
        );
        return true;
    }
    eprintln!("check-clippy-bans:");
    for p in &found {
        eprintln!("  {p}");
    }
    if diagnostics.is_empty() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"
disallowed-methods = [ { path = "a::f", reason = "r" }, "a::g" ]
disallowed-types = [ { path = "a::T" } ]
"#;

    fn diag(line: usize, path: &str) -> String {
        format!(
            r#"{{"reason":"compiler-message","message":{{"message":"use of a disallowed method `{path}`","code":{{"code":"clippy::disallowed_methods"}},"spans":[{{"is_primary":true,"line_start":{line}}}]}}}}"#
        )
    }

    #[test]
    fn the_config_lists_both_tables_and_bare_strings() {
        let c = configured(CONFIG).unwrap();
        assert_eq!(c.into_iter().collect::<Vec<_>>(), ["a::T", "a::f", "a::g"]);
    }

    #[test]
    fn every_ban_reported_on_its_line_passes() {
        let config = configured(CONFIG).unwrap();
        let src = "x // BAN: a::f\ny // BAN: a::g\nz // BAN: a::T\n";
        let json = [diag(1, "a::f"), diag(2, "a::g"), diag(3, "a::T")].join("\n");
        assert!(problems(&config, &markers(src), &reported(&json)).is_empty());
    }

    /// A ban clippy stays silent on — the unresolvable-path case — is refused.
    #[test]
    fn a_silent_ban_is_refused() {
        let config = configured(CONFIG).unwrap();
        let src = "x // BAN: a::f\ny // BAN: a::g\nz // BAN: a::T\n";
        let json = [diag(1, "a::f"), diag(3, "a::T")].join("\n");
        let p = problems(&config, &markers(src), &reported(&json));
        assert_eq!(p.len(), 1, "{p:?}");
        assert!(p[0].contains("a::g"), "{p:?}");
    }

    /// A reported diagnostic on another line does not count for the marker.
    #[test]
    fn a_report_on_the_wrong_line_does_not_count() {
        let config: BTreeSet<String> = ["a::f".to_owned()].into();
        let p = problems(
            &config,
            &markers("x // BAN: a::f\n"),
            &reported(&diag(7, "a::f")),
        );
        assert_eq!(p.len(), 1, "{p:?}");
    }

    #[test]
    fn a_ban_without_a_planted_use_is_refused() {
        let config = configured(CONFIG).unwrap();
        let p = problems(
            &config,
            &markers("x // BAN: a::f\n"),
            &reported(&diag(1, "a::f")),
        );
        assert_eq!(p.len(), 2, "{p:?}");
    }

    #[test]
    fn the_real_config_and_fixture_agree_on_what_is_banned() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let config =
            configured(&std::fs::read_to_string(root.join("clippy.toml")).unwrap()).unwrap();
        let src = std::fs::read_to_string(root.join(FIXTURE).join("src/lib.rs")).unwrap();
        let marked: BTreeSet<String> = markers(&src).into_iter().map(|(_, p)| p).collect();
        assert_eq!(config, marked);
    }
}
