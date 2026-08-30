//! Guard the crates.io publish order in `.github/workflows/release.yml`.
//!
//! `cargo publish` resolves a workspace dependency against the **registry**, not
//! the working tree, so a crate published before one it depends on fails with
//! „no matching package named `…` found". The release job publishes each crate
//! as its own step, so the order in that file is the whole contract — and
//! `check-publishable` deliberately sorts the list before compiling it, which
//! leaves the ordering unchecked.
//!
//! Two things are verified: every publishable workspace member appears in the
//! list, and no crate is published before a workspace dependency of its own.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

/// A workspace member's manifest, reduced to what the order depends on.
struct Member {
    /// `false` when the manifest sets `publish = false`.
    publishable: bool,
    /// Workspace members this one depends on *that survive publication*.
    ///
    /// A `path`-only dependency carries no version, so `cargo publish` strips
    /// it from the published manifest and the registry never has to hold it —
    /// which is why a `path`-only dev-dependency imposes no ordering. One that
    /// names a version, directly or through `workspace = true`, is retained and
    /// must already be on crates.io.
    deps: Vec<String>,
}

pub fn check_publish_order() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = env!("CARGO_MANIFEST_DIR")
        .rsplit_once('/')
        .map_or_else(|| ".".to_owned(), |(parent, _)| parent.to_owned());

    let members = read_members(&root)?;
    let order = read_publish_order(&root)?;
    let position: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, name)| (name.as_str(), i))
        .collect();

    let mut problems = String::new();

    for (name, member) in &members {
        if member.publishable && !position.contains_key(name.as_str()) {
            let _ = writeln!(
                problems,
                "  {name} is publishable but has no `cargo publish -p {name}` step"
            );
        }
    }

    for (index, name) in order.iter().enumerate() {
        let Some(member) = members.get(name) else {
            let _ = writeln!(
                problems,
                "  {name} is published but is not a workspace member"
            );
            continue;
        };
        for dep in &member.deps {
            match position.get(dep.as_str()) {
                Some(&at) if at > index => {
                    let _ = writeln!(
                        problems,
                        "  {name} (#{}) is published before its dependency {dep} (#{})",
                        index + 1,
                        at + 1
                    );
                }
                None if members.get(dep).is_some_and(|m| m.publishable) => {
                    let _ = writeln!(
                        problems,
                        "  {name} depends on {dep}, which is publishable but never published"
                    );
                }
                _ => {}
            }
        }
    }

    if !problems.is_empty() {
        return Err(format!(
            "release.yml publish order is not resolvable on crates.io:\n{problems}\n\
             `cargo publish` resolves workspace dependencies against the registry, \
             so each crate must follow every crate it depends on."
        )
        .into());
    }

    println!(
        "check-publish-order: {} crates publish in a dependency-respecting order ✓",
        order.len()
    );
    Ok(())
}

/// `true` when the manifest sets `publish = false`.
///
/// Matched on the parsed key rather than a literal, because the manifests align
/// their `=` on a column and the padding differs per crate.
fn declares_unpublished(text: &str) -> bool {
    text.lines().any(|line| {
        line.split_once('=')
            .is_some_and(|(key, value)| key.trim() == "publish" && value.trim() == "false")
    })
}

/// The `cargo publish -p <crate>` steps, in file order.
fn read_publish_order(root: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let workflow = std::fs::read_to_string(Path::new(root).join(".github/workflows/release.yml"))?;
    let mut order = Vec::new();
    for line in workflow.lines() {
        let Some(rest) = line.split_once("cargo publish -p ") else {
            continue;
        };
        let name: String = rest
            .1
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if !name.is_empty() && !order.contains(&name) {
            order.push(name);
        }
    }
    if order.is_empty() {
        return Err("no `cargo publish -p` steps found in release.yml".into());
    }
    Ok(order)
}

/// Every workspace member under `crates/`, with its intra-workspace deps.
fn read_members(root: &str) -> Result<HashMap<String, Member>, Box<dyn std::error::Error>> {
    let mut members = HashMap::new();
    let crates_dir = Path::new(root).join("crates");
    for entry in std::fs::read_dir(&crates_dir)? {
        let dir = entry?.path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&manifest)?;
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("unreadable crate directory")?
            .to_owned();
        members.insert(
            name,
            Member {
                publishable: !declares_unpublished(&text),
                deps: workspace_deps(&text, &crates_dir)?,
            },
        );
    }
    Ok(members)
}

/// Workspace members `text` depends on whose dependency survives publication.
///
/// Read off the `name = { … }` line shape the manifests use rather than parsed
/// as TOML: the check needs two facts, and a parser would be a dependency this
/// crate does not otherwise carry.
fn workspace_deps(
    text: &str,
    crates_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut deps = Vec::new();
    let mut in_deps = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_deps = trimmed.contains("dependencies");
            continue;
        }
        if !in_deps || trimmed.starts_with('#') {
            continue;
        }
        let Some((name, spec)) = trimmed.split_once('=') else {
            continue;
        };
        let name = name.trim();
        // A `path`-only entry is stripped on publish, so it constrains nothing.
        let versioned = spec.contains("workspace = true") || spec.contains("version");
        if versioned && !name.is_empty() && crates_dir.join(name).join("Cargo.toml").is_file() {
            let name = name.to_owned();
            if !deps.contains(&name) {
                deps.push(name);
            }
        }
    }
    Ok(deps)
}
