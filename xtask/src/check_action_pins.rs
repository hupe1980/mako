//! Guard: every third-party action and every base image is pinned by digest,
//! and no workflow grants more than `contents: read` by default.
//!
//! A tag is a pointer anyone with push rights upstream can move. A workflow
//! naming `actions/checkout@v4` runs whatever that tag names on the day of the
//! run, with the job's token in reach — the class of the tj-actions compromise.
//! A 40-hex commit SHA names one tree, so an upstream change reaches this
//! repository only as a diff here. The same holds for a Dockerfile `FROM`: the
//! tag is for the reader, the `@sha256:` digest is what is pulled.
//!
//! Three rules, each refused separately:
//!
//! - every `uses:` names a 40-hex commit SHA and carries the tag it stands for
//!   in a trailing comment (`# v4.4.0`) — the SHA is read, the comment is for
//!   the reviewer of a pin bump. Local actions (`./…`) are exempt; a
//!   `docker://` image needs an `@sha256:` digest.
//! - every `FROM` in a Dockerfile names an `@sha256:` digest, unless it names
//!   an earlier stage of the same file or `scratch`.
//! - every workflow declares a top-level `permissions:` that grants at most
//!   `contents: read`; a job that needs more says so on the job. A workflow
//!   without the key inherits the repository default, which may be write-all.

use std::collections::BTreeSet;
use std::path::Path;

/// The lowest number of `uses:` lines a healthy scan may find — below it the
/// parser has stopped matching and a clean result would be about nothing.
const MIN_USES: usize = 60;

/// The lowest number of `FROM` lines naming a registry image.
const MIN_EXTERNAL_FROM: usize = 2;

/// What the workflow-level `permissions:` may grant.
const DEFAULT_PERMISSIONS: &[(&str, &str)] = &[("contents", "read")];

pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();
    let mut uses = 0usize;
    let mut external_from = 0usize;

    let dir = workspace_root.join(".github/workflows");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!("check-action-pins: no workflows at {}", dir.display());
        return false;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            findings.push(format!("{}: unreadable", path.display()));
            continue;
        };
        let rel = rel(workspace_root, path);
        let (n, f) = check_workflow_uses(&rel, &src);
        uses += n;
        findings.extend(f);
        findings.extend(check_workflow_permissions(&rel, &src));
    }

    for path in dockerfiles(workspace_root) {
        let Ok(src) = std::fs::read_to_string(&path) else {
            findings.push(format!("{}: unreadable", path.display()));
            continue;
        };
        let (n, f) = check_dockerfile(&rel(workspace_root, &path), &src);
        external_from += n;
        findings.extend(f);
    }

    if uses < MIN_USES || external_from < MIN_EXTERNAL_FROM {
        eprintln!(
            "check-action-pins: found {uses} `uses:` line(s) and {external_from} registry \
             `FROM` line(s); at least {MIN_USES} and {MIN_EXTERNAL_FROM} are expected. The \
             parser has stopped matching — fix it before trusting a green run."
        );
        return false;
    }
    if findings.is_empty() {
        println!(
            "check-action-pins: {uses} `uses:` pinned by commit, {external_from} base image(s) \
             by digest, every workflow defaults to `contents: read`"
        );
        return true;
    }
    eprintln!("check-action-pins: {} finding(s):", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `Dockerfile` and `*.Dockerfile` / `Dockerfile.*` at the root and one level
/// below it — where this workspace keeps them.
fn dockerfiles(root: &Path) -> Vec<std::path::PathBuf> {
    let is_dockerfile = |name: &str| {
        name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile")
    };
    let mut out = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() && !name.starts_with('.') && name != "target" {
                dirs.push(p);
            }
        }
    }
    for d in dirs {
        if let Ok(entries) = std::fs::read_dir(&d) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_file() && is_dockerfile(&name) {
                    out.push(e.path());
                }
            }
        }
    }
    out.sort();
    out
}

/// Every `uses:` line in one workflow: the count seen and the refusals.
pub fn check_workflow_uses(file: &str, src: &str) -> (usize, Vec<String>) {
    let mut seen = 0usize;
    let mut findings = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim_start();
        if t.starts_with('#') {
            continue;
        }
        let t = t.strip_prefix("- ").unwrap_or(t).trim_start();
        let Some(rest) = t.strip_prefix("uses:") else {
            continue;
        };
        seen += 1;
        let (value, comment) = match rest.split_once('#') {
            Some((v, c)) => (v.trim(), Some(c.trim())),
            None => (rest.trim(), None),
        };
        let value = value.trim_matches(|c| c == '"' || c == '\'');
        let at = format!("{file}:{}", i + 1);
        if value.starts_with("./") {
            continue;
        }
        if let Some(image) = value.strip_prefix("docker://") {
            if !has_digest(image) {
                findings.push(format!(
                    "{at}: `{value}` — a container action needs an `@sha256:` digest"
                ));
            }
            continue;
        }
        let Some((_, reference)) = value.rsplit_once('@') else {
            findings.push(format!("{at}: `{value}` names no ref at all"));
            continue;
        };
        if !is_commit_sha(reference) {
            findings.push(format!(
                "{at}: `{value}` is pinned by `{reference}`, which upstream can move — \
                 pin the 40-hex commit SHA and keep the tag in a comment"
            ));
            continue;
        }
        if comment.is_none_or(str::is_empty) {
            findings.push(format!(
                "{at}: `{value}` carries no `# <tag>` comment — a reviewer cannot tell \
                 which release the SHA stands for"
            ));
        }
    }
    (seen, findings)
}

fn is_commit_sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn has_digest(image: &str) -> bool {
    image.split_once("@sha256:").is_some_and(|(_, d)| {
        d.len() == 64
            && d.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// Every `FROM` in one Dockerfile: the number naming a registry image, and the
/// refusals.
pub fn check_dockerfile(file: &str, src: &str) -> (usize, Vec<String>) {
    let mut stages: BTreeSet<String> = BTreeSet::new();
    let mut external = 0usize;
    let mut findings = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        let mut words = t.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("FROM")) {
            continue;
        }
        let rest: Vec<&str> = words.filter(|w| !w.starts_with("--")).collect();
        let Some(image) = rest.first() else {
            findings.push(format!("{file}:{}: `FROM` names no image", i + 1));
            continue;
        };
        // A stage is known only from the lines above, so `FROM x AS x` is not
        // a stage reference.
        let is_stage =
            image.eq_ignore_ascii_case("scratch") || stages.contains(&image.to_ascii_lowercase());
        if let [_, kw, name, ..] = rest.as_slice() {
            if kw.eq_ignore_ascii_case("AS") {
                stages.insert(name.to_ascii_lowercase());
            }
        }
        if is_stage {
            continue;
        }
        external += 1;
        if !has_digest(image) {
            findings.push(format!(
                "{file}:{}: `FROM {image}` is pinned by tag — a moved tag changes the image \
                 without a diff here; append `@sha256:<digest>`",
                i + 1
            ));
        }
    }
    (external, findings)
}

/// The top-level `permissions:` of one workflow, refused when absent or wider
/// than [`DEFAULT_PERMISSIONS`].
pub fn check_workflow_permissions(file: &str, src: &str) -> Vec<String> {
    let doc: serde_yaml_ng::Value = match serde_yaml_ng::from_str(src) {
        Ok(v) => v,
        Err(e) => return vec![format!("{file}: not YAML: {e}")],
    };
    let Some(perms) = doc.get("permissions") else {
        return vec![format!(
            "{file}: no top-level `permissions:` — every job inherits the repository \
             default, which may be write-all; declare `permissions: contents: read`"
        )];
    };
    let mut findings = Vec::new();
    match perms {
        serde_yaml_ng::Value::Mapping(m) => {
            for (k, v) in m {
                let k = k.as_str().unwrap_or_default();
                let v = v.as_str().unwrap_or_default();
                if v == "none" {
                    continue;
                }
                if !DEFAULT_PERMISSIONS.contains(&(k, v)) {
                    findings.push(format!(
                        "{file}: top-level `permissions:` grants `{k}: {v}` to every job — \
                         move it onto the job that needs it"
                    ));
                }
            }
        }
        serde_yaml_ng::Value::String(s) if s == "read-all" || s == "write-all" => {
            findings.push(format!(
                "{file}: top-level `permissions: {s}` — grant `contents: read` and let each \
                 job add what it needs"
            ));
        }
        _ => {}
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "11d5960a326750d5838078e36cf38b85af677262";

    #[test]
    fn a_tag_pinned_action_is_refused() {
        let (n, f) =
            check_workflow_uses("w.yml", "    steps:\n      - uses: actions/checkout@v4\n");
        assert_eq!(n, 1);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("upstream can move"));
    }

    #[test]
    fn a_branch_pinned_action_is_refused() {
        let src = "      - uses: pypa/gh-action-pypi-publish@release/v1\n";
        assert_eq!(check_workflow_uses("w.yml", src).1.len(), 1);
    }

    #[test]
    fn a_sha_without_its_tag_comment_is_refused() {
        let src = format!("      - uses: actions/checkout@{SHA}\n");
        let f = check_workflow_uses("w.yml", &src).1;
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("comment"));
    }

    #[test]
    fn a_sha_with_its_tag_comment_passes() {
        let src = format!(
            "      - uses: actions/checkout@{SHA} # v4.4.0\n        uses: \
             github/codeql-action/upload-sarif@{SHA} # v3\n      - uses: ./local\n"
        );
        let (n, f) = check_workflow_uses("w.yml", &src);
        assert_eq!(n, 3);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn a_short_sha_and_an_uppercase_sha_are_refused() {
        let src = "      - uses: a/b@11d5960 # v1\n      - uses: a/b@11D5960A326750D5838078E36CF38B85AF677262 # v1\n";
        assert_eq!(check_workflow_uses("w.yml", src).1.len(), 2);
    }

    #[test]
    fn a_commented_out_uses_is_not_a_step() {
        let src = "      # - uses: actions/checkout@v4\n";
        assert_eq!(check_workflow_uses("w.yml", src), (0, vec![]));
    }

    #[test]
    fn a_docker_action_needs_a_digest() {
        let src = "      - uses: docker://alpine:3\n";
        assert_eq!(check_workflow_uses("w.yml", src).1.len(), 1);
    }

    #[test]
    fn a_tag_only_from_is_refused_and_a_stage_reference_is_not() {
        let src = "\
FROM rust:1.94-bookworm AS chef
FROM chef AS builder
FROM --platform=$BUILDPLATFORM gcr.io/distroless/cc-debian12:nonroot AS runtime
FROM scratch
";
        let (n, f) = check_dockerfile("Dockerfile", src);
        assert_eq!(n, 2);
        assert_eq!(f.len(), 2, "{f:?}");
    }

    #[test]
    fn a_digest_pinned_from_passes() {
        let d = "9dac0a79194e45a7da0158a9c6da57b217585af0786db3845d1f0ec1a0dd182f";
        let src = format!(
            "FROM gcr.io/distroless/cc-debian12:nonroot@sha256:{d} AS runtime\nFROM runtime\n"
        );
        let (n, f) = check_dockerfile("Dockerfile", &src);
        assert_eq!(n, 1);
        assert!(f.is_empty(), "{f:?}");
    }

    #[test]
    fn workflow_permissions_wider_than_contents_read_are_refused() {
        let src = "on: push\npermissions:\n  contents: write\n  packages: write\njobs: {}\n";
        assert_eq!(check_workflow_permissions("w.yml", src).len(), 2);
        let src = "on: push\njobs: {}\n";
        assert_eq!(check_workflow_permissions("w.yml", src).len(), 1);
        let src = "on: push\npermissions: write-all\njobs: {}\n";
        assert_eq!(check_workflow_permissions("w.yml", src).len(), 1);
    }

    #[test]
    fn contents_read_at_the_top_passes() {
        let src = "on: push\npermissions:\n  contents: read\njobs:\n  a:\n    permissions:\n      id-token: write\n";
        assert!(check_workflow_permissions("w.yml", src).is_empty());
        let src = "on: push\npermissions: {}\njobs: {}\n";
        assert!(check_workflow_permissions("w.yml", src).is_empty());
    }
}
