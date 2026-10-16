//! Guard: `release/vex.json` carries one statement per advisory `deny.toml`
//! ignores, and no other.
//!
//! The reason beside an `ignore` entry is prose a person reads; a scanner
//! reading the release's SBOM needs the same judgement machine-readably, or it
//! re-raises every accepted advisory against every image. The VEX document
//! (OpenVEX v0.2.0) is that form, and it is published with each release.
//!
//! Both directions are refused. An ignore without a statement is an advisory
//! the release silences without saying why to anyone outside this tree; a
//! statement without an ignore claims a judgement about an advisory the policy
//! no longer accepts.
//!
//! Each statement must also be well-formed for its status (the OpenVEX specification:
//! `not_affected` needs a justification or an impact statement, `affected`
//! an action statement) and name at least one `pkg:cargo/` subcomponent.
//! When a local copy of the RustSec database is present
//! (`$CARGO_HOME/advisory-db`, which `cargo deny` maintains), the subcomponent
//! must be the crate the advisory is filed against — the check that catches
//! two IDs whose reasons have been swapped.
//!
//! A subcomponent that names a version must be a package one of the two
//! lockfiles holds — the workspace's `Cargo.lock` or `fuzz/Cargo.lock`, which
//! lies outside the workspace and is gated by its own `cargo deny` run under
//! the same `deny.toml`. A statement about a version neither lock carries is a
//! judgement about code the release does not contain, and it hides that the
//! version actually locked was never assessed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const VEX: &str = "release/vex.json";

/// Every lockfile the release's dependency graph is read from.
pub const LOCKS: &[&str] = &["Cargo.lock", "fuzz/Cargo.lock"];

const STATUSES: &[&str] = &["not_affected", "affected", "fixed", "under_investigation"];

const JUSTIFICATIONS: &[&str] = &[
    "component_not_present",
    "vulnerable_code_not_present",
    "vulnerable_code_not_in_execute_path",
    "vulnerable_code_cannot_be_controlled_by_adversary",
    "inline_mitigations_already_exist",
];

pub fn run(workspace_root: &Path) -> bool {
    let Ok(deny) = std::fs::read_to_string(workspace_root.join("deny.toml")) else {
        eprintln!("check-vex: cannot read deny.toml");
        return false;
    };
    let Ok(vex) = std::fs::read_to_string(workspace_root.join(VEX)) else {
        eprintln!("check-vex: cannot read {VEX}");
        return false;
    };
    let db = advisory_db();
    let mut findings = check(&deny, &vex, db.as_deref());
    let mut locks = Vec::new();
    for lock in LOCKS {
        match std::fs::read_to_string(workspace_root.join(lock)) {
            Ok(text) => locks.push((*lock, text)),
            Err(e) => findings.push(format!("{lock}: unreadable ({e})")),
        }
    }
    let locks: Vec<(&str, &str)> = locks.iter().map(|(n, t)| (*n, t.as_str())).collect();
    findings.extend(check_locked(&vex, &locks));
    let ignored = ignored_ids(&deny);
    if ignored.is_empty() {
        eprintln!("check-vex: deny.toml ignores no advisory — has the [advisories] table moved?");
        return false;
    }
    if !findings.is_empty() {
        eprintln!("check-vex: {VEX} and deny.toml disagree:");
        for f in &findings {
            eprintln!("  {f}");
        }
        return false;
    }
    let crate_check = match &db {
        Some(p) => format!("each crate matched against {}", p.display()),
        None => "crate names NOT matched: no local advisory-db (run `cargo deny fetch`)".into(),
    };
    println!(
        "check-vex: {} ignored advisory(ies), one VEX statement each, every versioned \
         subcomponent locked in {}; {crate_check}",
        ignored.len(),
        LOCKS.join(" or ")
    );
    true
}

fn advisory_db() -> Option<PathBuf> {
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))?;
    let db = home.join("advisory-db");
    db.join("crates").is_dir().then_some(db)
}

/// The RustSec IDs in `deny.toml`'s `[advisories] ignore = [...]`, comment
/// lines skipped (the rationale comments name the IDs too).
pub fn ignored_ids(deny: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut in_advisories = false;
    let mut in_ignore = false;
    for line in deny.lines() {
        let t = line.trim();
        if t.starts_with('[') && !t.starts_with("[[") && t.ends_with(']') && !in_ignore {
            in_advisories = t == "[advisories]";
            continue;
        }
        if t.starts_with("[[") {
            in_advisories = false;
            continue;
        }
        if !in_advisories || t.starts_with('#') {
            continue;
        }
        if t.starts_with("ignore") && t.contains('[') {
            in_ignore = true;
        }
        if in_ignore {
            for id in rustsec_ids(t) {
                out.insert(id);
            }
            if t.starts_with(']') || (t.contains(']') && !t.contains('{')) {
                in_ignore = false;
            }
        }
    }
    out
}

fn rustsec_ids(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("RUSTSEC-") {
        let cand: String = rest[i..].chars().take(17).collect();
        let b = cand.as_bytes();
        if b.len() == 17
            && b[8..12].iter().all(u8::is_ascii_digit)
            && b[12] == b'-'
            && b[13..17].iter().all(u8::is_ascii_digit)
        {
            out.push(cand);
        }
        rest = &rest[i + 8..];
    }
    out
}

/// Every disagreement between the ignore list and the VEX document.
pub fn check(deny: &str, vex: &str, db: Option<&Path>) -> Vec<String> {
    let mut findings = Vec::new();
    let ignored = ignored_ids(deny);
    let doc: serde_json::Value = match serde_json::from_str(vex) {
        Ok(v) => v,
        Err(e) => return vec![format!("{VEX} is not JSON: {e}")],
    };
    if doc.get("@context").and_then(|v| v.as_str()) != Some("https://openvex.dev/ns/v0.2.0") {
        findings.push(format!("{VEX}: `@context` is not OpenVEX v0.2.0"));
    }
    let Some(statements) = doc.get("statements").and_then(|v| v.as_array()) else {
        findings.push(format!("{VEX}: no `statements` array"));
        return findings;
    };

    let mut stated: BTreeMap<String, usize> = BTreeMap::new();
    for (i, st) in statements.iter().enumerate() {
        let Some(id) = st.pointer("/vulnerability/name").and_then(|v| v.as_str()) else {
            findings.push(format!("statement {i}: no `vulnerability.name`"));
            continue;
        };
        *stated.entry(id.to_string()).or_default() += 1;
        let status = st.get("status").and_then(|v| v.as_str()).unwrap_or("");
        if !STATUSES.contains(&status) {
            findings.push(format!("{id}: status `{status}` is not an OpenVEX status"));
        }
        let text = |k: &str| {
            st.get(k)
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
        };
        match status {
            "not_affected" => match text("justification") {
                Some(j) if !JUSTIFICATIONS.contains(&j) => {
                    findings.push(format!("{id}: justification `{j}` is not an OpenVEX one"));
                }
                None if text("impact_statement").is_none() => findings.push(format!(
                    "{id}: `not_affected` needs a justification or an impact statement"
                )),
                _ => {}
            },
            "affected" if text("action_statement").is_none() => {
                findings.push(format!("{id}: `affected` needs an action statement"));
            }
            _ => {}
        }
        let crates: Vec<String> = st
            .get("products")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|p| p.get("subcomponents").and_then(|v| v.as_array()))
            .flatten()
            .filter_map(|c| c.get("@id").and_then(|v| v.as_str()))
            .filter_map(|purl| purl.strip_prefix("pkg:cargo/"))
            .map(|rest| rest.split('@').next().unwrap_or(rest).to_string())
            .collect();
        if crates.is_empty() {
            findings.push(format!("{id}: names no `pkg:cargo/` subcomponent"));
        } else if let Some(db) = db {
            match advisory_package(db, id) {
                Some(pkg) if crates.iter().all(|c| *c == pkg) => {}
                Some(pkg) => findings.push(format!(
                    "{id}: filed against `{pkg}`, but the statement names {crates:?}"
                )),
                None => findings.push(format!("{id}: not in the advisory database")),
            }
        }
    }

    for id in &ignored {
        if !stated.contains_key(id) {
            findings.push(format!(
                "{id}: ignored in deny.toml with no VEX statement — the release silences it \
                 without saying why to any scanner"
            ));
        }
    }
    for (id, n) in &stated {
        if !ignored.contains(id) {
            findings.push(format!(
                "{id}: has a VEX statement but deny.toml does not ignore it — the statement \
                 is a judgement the policy no longer makes"
            ));
        }
        if *n > 1 {
            findings.push(format!("{id}: {n} statements — one per advisory"));
        }
    }
    findings
}

/// The `(name, version)` of every `[[package]]` in a `Cargo.lock`.
pub fn locked_packages(lock: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut name: Option<String> = None;
    for line in lock.lines() {
        let t = line.trim();
        if t == "[[package]]" {
            name = None;
        } else if let Some(v) = t.strip_prefix("name = ") {
            name = Some(v.trim_matches('"').to_string());
        } else if let Some(v) = t.strip_prefix("version = ") {
            if let Some(n) = name.take() {
                out.insert((n, v.trim_matches('"').to_string()));
            }
        }
    }
    out
}

/// Every versioned `pkg:cargo/` subcomponent no lock in `locks` (name, text)
/// holds.
pub fn check_locked(vex: &str, locks: &[(&str, &str)]) -> Vec<String> {
    let Ok(doc) = serde_json::from_str::<serde_json::Value>(vex) else {
        return Vec::new(); // `check` reports the parse failure
    };
    let locked: BTreeSet<(String, String)> = locks
        .iter()
        .flat_map(|(_, text)| locked_packages(text))
        .collect();
    let names: Vec<&str> = locks.iter().map(|(n, _)| *n).collect();
    let mut findings = Vec::new();
    for st in doc
        .get("statements")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let id = st
            .pointer("/vulnerability/name")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        let purls = st
            .get("products")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|p| p.get("subcomponents").and_then(|v| v.as_array()))
            .flatten()
            .filter_map(|c| c.get("@id").and_then(|v| v.as_str()))
            .filter_map(|purl| purl.strip_prefix("pkg:cargo/"));
        for rest in purls {
            let Some((name, version)) = rest.split_once('@') else {
                continue;
            };
            if !locked.contains(&(name.to_string(), version.to_string())) {
                findings.push(format!(
                    "{id}: names {name}@{version}, which neither {} locks — the statement \
                     assesses a version the release does not contain",
                    names.join(" nor ")
                ));
            }
        }
    }
    findings
}

/// The crate an advisory is filed against, read from its file in the database.
fn advisory_package(db: &Path, id: &str) -> Option<String> {
    let crates = std::fs::read_dir(db.join("crates")).ok()?;
    for dir in crates.flatten() {
        let file = dir.path().join(format!("{id}.md"));
        if file.is_file() {
            return Some(dir.file_name().to_string_lossy().into_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const DENY: &str = r#"
[licenses]
allow = ["MIT"]

[advisories]
ignore = [
    # RUSTSEC-2099-0001: a comment naming another id, RUSTSEC-2099-9999.
    { id = "RUSTSEC-2099-0001", reason = "x" },
    { id = "RUSTSEC-2099-0002", reason = "y" },
]

[[bans.skip]]
name = "z"
reason = "RUSTSEC-2099-0003 in a bans reason is not an ignore"
"#;

    fn statement(id: &str, purl: &str) -> String {
        format!(
            r#"{{"vulnerability":{{"name":"{id}"}},"products":[{{"@id":"pkg:github/o/r","subcomponents":[{{"@id":"{purl}"}}]}}],"status":"not_affected","justification":"component_not_present"}}"#
        )
    }

    fn vex(statements: &[String]) -> String {
        format!(
            r#"{{"@context":"https://openvex.dev/ns/v0.2.0","statements":[{}]}}"#,
            statements.join(",")
        )
    }

    #[test]
    fn the_ignore_list_is_read_without_comments_or_other_tables() {
        let ids: Vec<_> = ignored_ids(DENY).into_iter().collect();
        assert_eq!(ids, ["RUSTSEC-2099-0001", "RUSTSEC-2099-0002"]);
    }

    #[test]
    fn equal_sets_pass() {
        let v = vex(&[
            statement("RUSTSEC-2099-0001", "pkg:cargo/a@1.0.0"),
            statement("RUSTSEC-2099-0002", "pkg:cargo/b@1.0.0"),
        ]);
        assert!(
            check(DENY, &v, None).is_empty(),
            "{:?}",
            check(DENY, &v, None)
        );
    }

    #[test]
    fn an_ignore_without_a_statement_is_refused() {
        let v = vex(&[statement("RUSTSEC-2099-0001", "pkg:cargo/a@1.0.0")]);
        let f = check(DENY, &v, None);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].starts_with("RUSTSEC-2099-0002: ignored"));
    }

    #[test]
    fn a_statement_without_an_ignore_is_refused() {
        let v = vex(&[
            statement("RUSTSEC-2099-0001", "pkg:cargo/a@1.0.0"),
            statement("RUSTSEC-2099-0002", "pkg:cargo/b@1.0.0"),
            statement("RUSTSEC-2099-0004", "pkg:cargo/c@1.0.0"),
        ]);
        let f = check(DENY, &v, None);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("does not ignore it"));
    }

    #[test]
    fn a_not_affected_without_a_reason_and_an_unknown_status_are_refused() {
        let bare = r#"{"vulnerability":{"name":"RUSTSEC-2099-0001"},"products":[{"@id":"p","subcomponents":[{"@id":"pkg:cargo/a"}]}],"status":"not_affected"}"#;
        let odd = statement("RUSTSEC-2099-0002", "pkg:cargo/b").replace("not_affected", "fine");
        let f = check(DENY, &vex(&[bare.to_string(), odd]), None);
        assert_eq!(f.len(), 2, "{f:?}");
    }

    #[test]
    fn a_statement_naming_the_wrong_crate_is_refused_against_the_database() {
        let dir = std::env::temp_dir().join(format!("check-vex-{}", std::process::id()));
        for (krate, id) in [("a", "RUSTSEC-2099-0001"), ("b", "RUSTSEC-2099-0002")] {
            std::fs::create_dir_all(dir.join("crates").join(krate)).unwrap();
            std::fs::write(dir.join("crates").join(krate).join(format!("{id}.md")), "").unwrap();
        }
        // Swapped: each statement names the other advisory's crate.
        let v = vex(&[
            statement("RUSTSEC-2099-0001", "pkg:cargo/b@1.0.0"),
            statement("RUSTSEC-2099-0002", "pkg:cargo/a@1.0.0"),
        ]);
        let f = check(DENY, &v, Some(&dir));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(f.len(), 2, "{f:?}");
        assert!(f[0].contains("filed against `a`"));
    }

    const LOCK: &str = "\
version = 4

[[package]]
name = \"a\"
version = \"1.0.0\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"

[[package]]
name = \"b\"
version = \"2.0.0\"
dependencies = [
 \"a\",
]
";

    #[test]
    fn the_lock_is_read_as_name_version_pairs() {
        let p: Vec<_> = locked_packages(LOCK).into_iter().collect();
        assert_eq!(
            p,
            [("a".into(), "1.0.0".into()), ("b".into(), "2.0.0".into())]
        );
    }

    #[test]
    fn a_subcomponent_locked_in_either_lock_passes() {
        let v = vex(&[
            statement("RUSTSEC-2099-0001", "pkg:cargo/a@1.0.0"),
            statement("RUSTSEC-2099-0002", "pkg:cargo/c@3.0.0"),
        ]);
        let fuzz = "[[package]]\nname = \"c\"\nversion = \"3.0.0\"\n";
        let f = check_locked(&v, &[("Cargo.lock", LOCK), ("fuzz/Cargo.lock", fuzz)]);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn a_subcomponent_version_no_lock_holds_is_refused() {
        let v = vex(&[
            statement("RUSTSEC-2099-0001", "pkg:cargo/a@0.9.0"),
            statement("RUSTSEC-2099-0002", "pkg:cargo/b"),
        ]);
        let f = check_locked(&v, &[("Cargo.lock", LOCK), ("fuzz/Cargo.lock", "")]);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("a@0.9.0"), "{f:?}");
        assert!(f[0].contains("fuzz/Cargo.lock"), "{f:?}");
    }
}
