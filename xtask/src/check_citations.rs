//! Guard: a Festlegung is cited in the form it actually publishes.
//!
//! A regulatory citation in a doc comment, an MCP tool description or a log line
//! is read as a statement about a published document. When the form is wrong the
//! claim is unfalsifiable rather than merely imprecise: nobody can look up
//! „BK6-22-024 § 4", so nobody discovers that the window it is attached to is
//! wrong too. An uncheckable citation shields whatever number stands beside it.
//!
//! ## What it refuses
//!
//! **`BK6-22-024 §…`.** The Beschluss has an operative part numbered in
//! *Tenorziffern* and its substance in *Anlagen*: GPKE Teil 4 is Anlage 1d, WiM
//! Strom Teil 1 and 2 are Anlagen 2a and 2b. Those Anlagen number their own
//! chapters, so the citable forms are „GPKE Teil 4 Kap. 5" or
//! „BK6-22-024 Anlage 1d". A bare `§` after the Aktenzeichen names nothing.
//!
//! **`BK6-24-174 GPKE Teil 4`.** The two GPKE Aktenzeichen split by Teil, not by
//! age, and both are current: Teil 1–3 are Anlagen 1a–1c to BK6-24-174, Teil 4
//! is Anlage 1d to BK6-22-024.
//!
//! ## What it deliberately does not do
//!
//! It checks the *form* of a citation, not whether the chapter it names says
//! what the sentence claims. That needs the document, and the documents are not
//! in the tree. The form is what can be checked cheaply and is wrong often.
//!
//! ## What it scans
//!
//! `crates`, `services`, `xtask`, `site/content`, `concepts`, `.github` and the
//! root `AGENTS.md`. The agent-instruction files are in scope deliberately: a
//! citation an agent reads before writing code is the one most likely to be
//! copied into it.

use std::path::{Path, PathBuf};

/// One offending site.
type Finding = (PathBuf, usize, String);

/// A refused citation form and why it is one.
struct Rule {
    /// The literal that may not appear.
    pattern: &'static str,
    /// What to write instead.
    reason: &'static str,
}

const RULES: &[Rule] = &[
    Rule {
        pattern: "BK6-22-024 §",
        reason: "BK6-22-024 has no numbered §§ — its operative part is numbered in \
                 Tenorziffern and its substance sits in Anlagen. Cite the Anlage and its \
                 chapter: „GPKE Teil 4 Kap. 5\" or „WiM Strom Teil 1 Kap. 2.3.2\"",
    },
    Rule {
        pattern: "BK6-22-024§",
        reason: "BK6-22-024 has no numbered §§ — cite the Anlage and its chapter",
    },
    Rule {
        pattern: "BK6-24-174 GPKE Teil 4",
        reason: "GPKE Teil 4 is Anlage 1d to **BK6-22-024**; BK6-24-174 carries Teil 1–3 \
                 as Anlagen 1a–1c",
    },
];

/// Files that may name a refused form because they define or enforce the rule.
const RULE_SITES: &[&str] = &[
    "xtask/src/check_citations.rs",
    "services/sperrd/tests/authorization_guard.rs",
    "services/sperrd/README.md",
];

/// Scan the workspace for malformed regulatory citations.
///
/// Returns `true` when every citation uses a form the document publishes.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    for dir in [
        "crates",
        "services",
        "xtask",
        "site/content",
        "concepts",
        ".github",
    ] {
        collect(&workspace_root.join(dir), workspace_root, &mut findings);
    }
    // The root `AGENTS.md` carries the domain rules every agent reads before
    // touching code, so its citations are the ones most likely to be copied.
    // It is a file rather than a directory, so the walk above misses it.
    check_file(
        &workspace_root.join("AGENTS.md"),
        workspace_root,
        &mut findings,
    );

    if findings.is_empty() {
        println!("check-citations: every Festlegung is cited in a form it publishes");
        return true;
    }

    eprintln!(
        "check-citations: {} malformed regulatory citation(s):",
        findings.len()
    );
    for (path, line, reason) in &findings {
        eprintln!("  {}:{line}  {reason}", path.display());
    }
    false
}

/// One file, when it is not under any scanned directory.
fn check_file(path: &Path, root: &Path, findings: &mut Vec<Finding>) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if is_rule_site(rel) {
        return;
    }
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    for (line, i) in offending_lines(&src) {
        findings.push((path.to_path_buf(), i, line));
    }
}

/// Every `.rs`, `.md` and `.sql` file under `dir`.
fn collect(dir: &Path, root: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, findings);
            continue;
        }
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "md" | "sql")
        ) {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if is_rule_site(rel) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, i) in offending_lines(&src) {
            findings.push((path.clone(), i, line));
        }
    }
}

/// Whether `rel` (workspace-relative) defines or enforces the rule.
fn is_rule_site(rel: &Path) -> bool {
    let rel = rel.to_string_lossy().replace('\\', "/");
    RULE_SITES.contains(&rel.as_str())
}

/// The offending lines of one source, as `(reason, 1-based line number)`.
///
/// Emphasis is stripped before matching: a citation written `**BK6-22-024** §4`
/// is the same claim, and a literal `contains` is blind to the asterisks.
#[must_use]
pub fn offending_lines(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let stripped = line.replace('*', "");
        for rule in RULES {
            if stripped.contains(rule.pattern) {
                out.push((format!("`{}` — {}", rule.pattern, rule.reason), i + 1));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::offending_lines;

    #[test]
    fn a_bare_paragraph_after_the_aktenzeichen_is_refused() {
        assert_eq!(
            offending_lines("/// APERAK Frist: 24h (BK6-22-024 §4).").len(),
            1
        );
        assert_eq!(offending_lines("// (BK6-22-024§5)").len(), 1);
    }

    #[test]
    fn emphasis_does_not_hide_the_citation() {
        assert_eq!(
            offending_lines("see **BK6-22-024** §4 for the window").len(),
            1,
            "a literal `contains` must not be defeated by Markdown emphasis"
        );
    }

    #[test]
    fn the_published_forms_pass() {
        assert!(offending_lines("/// GPKE Teil 4 Kap. 5 (BK6-22-024 Anlage 1d).").is_empty());
        assert!(offending_lines("/// GPKE Teil 2 § 3.5 (BK6-24-174 Anlage 1b).").is_empty());
        assert!(offending_lines("/// BK6-24-174 GPKE Teil 2 § 2.2.2").is_empty());
    }

    #[test]
    fn the_wrong_aktenzeichen_for_teil_4_is_refused() {
        assert_eq!(
            offending_lines("/// BK6-24-174 GPKE Teil 4 Kap. 5").len(),
            1,
            "Teil 4 is Anlage 1d to BK6-22-024"
        );
    }
}
