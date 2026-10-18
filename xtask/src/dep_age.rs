//! `cargo xtask dep-age` — how far each direct dependency's pin is behind
//! crates.io.
//!
//! A report, not a failure: a pin behind the registry is often a decision (an
//! upgrade waiting on a migration), and `just ci` must not need the network.
//! It holds the manifests up against the registry, so a reader planning an
//! upgrade sees which pins are behind and by how much.
//!
//! The resolved version of each direct dependency is read from `Cargo.lock`;
//! the newest stable release from `GET https://crates.io/api/v1/crates/<name>`
//! (`crate.max_stable_version`), or from a recorded response with
//! `--recorded <file>` (the shape of `xtask/fixtures/crates-io-*.json`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// How a resolved version stands against the newest release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// The newest stable release is the one resolved.
    Current,
    /// A newer release the requirement already admits — `cargo update` takes it.
    Compatible(String),
    /// A newer release the requirement does not admit — a manifest edit, and
    /// under Cargo's caret rules a breaking one.
    Behind(String),
}

fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        it.next()??,
        it.next().flatten().unwrap_or(0),
        it.next().flatten().unwrap_or(0),
    ))
}

/// Caret compatibility: same major, or for `0.x` the same minor (and for
/// `0.0.x` the same patch).
fn compatible(a: (u64, u64, u64), b: (u64, u64, u64)) -> bool {
    match (a.0, b.0) {
        (0, 0) if a.1 == 0 && b.1 == 0 => a.2 == b.2,
        (0, 0) => a.1 == b.1,
        (x, y) => x == y,
    }
}

#[must_use]
pub fn standing(resolved: &str, newest: &str) -> Standing {
    match (parse(resolved), parse(newest)) {
        (Some(r), Some(n)) if n > r && compatible(r, n) => Standing::Compatible(newest.to_owned()),
        (Some(r), Some(n)) if n > r => Standing::Behind(newest.to_owned()),
        _ => Standing::Current,
    }
}

/// Names of every registry dependency a manifest declares, in every
/// dependency table (workspace, normal, dev, build, target-specific).
fn direct_names(manifest: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let Ok(doc) = manifest.parse::<toml::Table>() else {
        return out;
    };
    let mut tables: Vec<&toml::Table> = Vec::new();
    let mut scopes: Vec<&toml::Table> = vec![&doc];
    if let Some(ws) = doc.get("workspace").and_then(toml::Value::as_table) {
        scopes.push(ws);
    }
    if let Some(targets) = doc.get("target").and_then(toml::Value::as_table) {
        scopes.extend(targets.values().filter_map(toml::Value::as_table));
    }
    for scope in scopes {
        for key in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(d) = scope.get(key).and_then(toml::Value::as_table) {
                tables.push(d);
            }
        }
    }
    for t in tables {
        for (name, spec) in t {
            let is_path = spec.as_table().is_some_and(|s| s.contains_key("path"));
            let is_ws = spec.as_table().is_some_and(|s| s.contains_key("workspace"));
            if is_path || is_ws {
                continue;
            }
            let package = spec
                .as_table()
                .and_then(|s| s.get("package"))
                .and_then(toml::Value::as_str)
                .unwrap_or(name);
            out.insert(package.to_owned());
        }
    }
    out
}

/// The highest version `Cargo.lock` resolves for each registry package.
fn locked(lock: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let Ok(doc) = lock.parse::<toml::Table>() else {
        return out;
    };
    for p in doc
        .get("package")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(name), Some(version)) = (
            p.get("name").and_then(toml::Value::as_str),
            p.get("version").and_then(toml::Value::as_str),
        ) else {
            continue;
        };
        if !p
            .get("source")
            .and_then(toml::Value::as_str)
            .is_some_and(|s| s.starts_with("registry+"))
        {
            continue;
        }
        let keep = out
            .get(name)
            .is_none_or(|have| parse(version) > parse(have));
        if keep {
            out.insert(name.to_owned(), version.to_owned());
        }
    }
    out
}

/// `(crate, resolved, standing)` for every direct dependency `newest` knows.
#[must_use]
pub fn report(
    manifests: &[String],
    lock: &str,
    newest: &BTreeMap<String, String>,
) -> Vec<(String, String, Standing)> {
    let names: BTreeSet<String> = manifests.iter().flat_map(|m| direct_names(m)).collect();
    let resolved = locked(lock);
    names
        .into_iter()
        .filter_map(|n| {
            let r = resolved.get(&n)?;
            let latest = newest.get(&n)?;
            Some((n.clone(), r.clone(), standing(r, latest)))
        })
        .collect()
}

fn recorded(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    v["crates"]
        .as_object()
        .ok_or_else(|| format!("{}: no `crates` object", path.display()))?
        .iter()
        .map(|(k, v)| {
            Ok((
                k.clone(),
                v.as_str().ok_or("a version is not a string")?.to_owned(),
            ))
        })
        .collect()
}

fn live(names: &BTreeSet<String>) -> BTreeMap<String, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("mako-xtask dep-age (https://github.com/hupe1980/mako)")
        .build()
        .expect("http client");
    let mut out = BTreeMap::new();
    for name in names {
        let url = format!("https://crates.io/api/v1/crates/{name}");
        match client
            .get(&url)
            .send()
            .and_then(reqwest::blocking::Response::json::<serde_json::Value>)
        {
            Ok(v) => {
                if let Some(max) = v["crate"]["max_stable_version"].as_str() {
                    out.insert(name.clone(), max.to_owned());
                }
            }
            Err(e) => eprintln!("dep-age: {name}: {e}"),
        }
        // crates.io's crawler policy: at most one request per second.
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    out
}

fn manifests(root: &Path) -> Vec<String> {
    let mut paths = vec![
        root.join("Cargo.toml"),
        root.join("xtask/Cargo.toml"),
        root.join("makotest/Cargo.toml"),
    ];
    for dir in ["crates", "services"] {
        for e in std::fs::read_dir(root.join(dir))
            .into_iter()
            .flatten()
            .flatten()
        {
            paths.push(e.path().join("Cargo.toml"));
        }
    }
    paths
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect()
}

/// Print the report. Always succeeds unless an input cannot be read.
pub fn run(root: &Path, args: &[String]) -> bool {
    let manifests = manifests(root);
    let Ok(lock) = std::fs::read_to_string(root.join("Cargo.lock")) else {
        eprintln!("dep-age: Cargo.lock is unreadable");
        return false;
    };
    let newest = match args.iter().position(|a| a == "--recorded") {
        Some(i) => match args.get(i + 1).map(|p| recorded(Path::new(p))) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                eprintln!("dep-age: {e}");
                return false;
            }
            None => {
                eprintln!("dep-age: --recorded needs a file");
                return false;
            }
        },
        None => {
            let names: BTreeSet<String> = manifests.iter().flat_map(|m| direct_names(m)).collect();
            live(&names)
        }
    };
    let rows = report(&manifests, &lock, &newest);
    let (mut behind, mut compat) = (0, 0);
    for (name, resolved, s) in &rows {
        match s {
            Standing::Behind(n) => {
                behind += 1;
                println!("behind      {name:<28} {resolved:>10} → {n}");
            }
            Standing::Compatible(n) => {
                compat += 1;
                println!("compatible  {name:<28} {resolved:>10} → {n}  (cargo update)");
            }
            Standing::Current => {}
        }
    }
    println!(
        "dep-age: {} direct dependencies checked — {behind} behind a breaking release, {compat} with a compatible update",
        rows.len()
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"
[workspace.dependencies]
sepa = "0.7"
metering = { version = "0.23" }
meterstore = "0.13"
serde = "1"
mine = { path = "crates/mine" }

[dependencies]
agentplane = "0.29"
sqlx = { version = "0.8", default-features = false }
slatedb = { version = "0.14.1", optional = true }
time = { workspace = true }
"#;

    const LOCK: &str = r#"
[[package]]
name = "sepa"
version = "0.7.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "metering"
version = "0.23.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "meterstore"
version = "0.13.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "agentplane"
version = "0.29.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "sqlx"
version = "0.8.6"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "slatedb"
version = "0.14.1"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "serde"
version = "1.0.219"
source = "registry+https://github.com/rust-lang/crates.io-index"
[[package]]
name = "mine"
version = "0.1.0"
"#;

    fn fixture() -> BTreeMap<String, String> {
        recorded(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/crates-io-2026-10-05.json"))
            .unwrap()
    }

    /// Fed the recorded 2026-10-05 response, the report names all six pins
    /// that were behind a breaking release that day.
    #[test]
    fn the_recorded_response_names_every_pin_behind() {
        let rows = report(&[MANIFEST.to_owned()], LOCK, &fixture());
        let behind: BTreeMap<&str, (&str, &str)> = rows
            .iter()
            .filter_map(|(n, r, s)| match s {
                Standing::Behind(latest) => Some((n.as_str(), (r.as_str(), latest.as_str()))),
                _ => None,
            })
            .collect();
        let want: BTreeMap<&str, (&str, &str)> = [
            ("agentplane", ("0.29.0", "0.45.0")),
            ("metering", ("0.23.0", "0.24.0")),
            ("meterstore", ("0.13.0", "0.14.0")),
            ("sepa", ("0.7.0", "0.8.0")),
            ("slatedb", ("0.14.1", "0.17.0")),
            ("sqlx", ("0.8.6", "0.9.0")),
        ]
        .into();
        assert_eq!(behind, want);
    }

    #[test]
    fn a_patch_inside_the_requirement_is_compatible_not_behind() {
        let rows = report(&[MANIFEST.to_owned()], LOCK, &fixture());
        let serde = rows.iter().find(|(n, ..)| n == "serde").unwrap();
        assert_eq!(serde.2, Standing::Compatible("1.0.228".into()));
    }

    #[test]
    fn a_path_dependency_is_not_reported() {
        let mut newest = fixture();
        newest.insert("mine".into(), "9.0.0".into());
        assert!(
            report(&[MANIFEST.to_owned()], LOCK, &newest)
                .iter()
                .all(|(n, ..)| n != "mine")
        );
    }

    #[test]
    fn caret_rules() {
        assert_eq!(
            standing("1.2.3", "1.9.0"),
            Standing::Compatible("1.9.0".into())
        );
        assert_eq!(standing("1.2.3", "2.0.0"), Standing::Behind("2.0.0".into()));
        assert_eq!(
            standing("0.3.1", "0.3.9"),
            Standing::Compatible("0.3.9".into())
        );
        assert_eq!(standing("0.3.1", "0.4.0"), Standing::Behind("0.4.0".into()));
        assert_eq!(standing("0.4.0", "0.4.0"), Standing::Current);
    }

    /// The real tree parses: the manifests name dependencies the lockfile
    /// resolves.
    #[test]
    fn the_real_tree_yields_rows() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let lock = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
        let rows = report(&manifests(root), &lock, &fixture());
        assert!(rows.iter().any(|(n, ..)| n == "sqlx"), "{rows:?}");
    }
}
