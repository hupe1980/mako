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
//! *Tenorziffern* and its substance in *Anlagen*: GPKE Teil 4 is its Anlage 1d.
//! Those Anlagen number their own chapters, so the citable forms are
//! „GPKE Teil 4 Kap. 5" or „BK6-22-024 Anlage 1d". A bare `§` after the
//! Aktenzeichen names nothing.
//!
//! **An Anlage attached to the wrong Aktenzeichen.** An Anlage number belongs to
//! exactly one Beschluss, and the pairing is stated on each Anlage's own title
//! page. The two GPKE Aktenzeichen split by Teil rather than by age, and both
//! are current: Teil 1–3 are Anlagen 1a–1c to BK6-24-174, Teil 4 is Anlage 1d to
//! BK6-22-024. WiM Strom Teil 1/2 moved: BK6-22-024 carried them as Anlagen
//! 2a/2b and **BK6-24-174 Tenorziffer 2 now does**, so a `BK6-22-024 Anlage 2a`
//! cites the superseded edition while naming a number the current one uses.
//! That is the shape this guard exists for — the citation looks well-formed,
//! resolves to a real document, and is the wrong one.
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

/// A refused *pairing* of an Aktenzeichen with an Anlage number.
///
/// A literal is not enough here. The same claim is written „BK6-22-024
/// Anlage 2a", „**BNetzA BK6-22-024**, Anlage 2a" and „Anlage 2a zu
/// BK6-22-024", and enumerating the spellings is how a guard ends up narrower
/// than the rule it states. Matching the two halves near each other catches
/// every order and whatever punctuation sits between them.
///
/// „Near" is what makes it usable: a line may legitimately name both GPKE
/// Aktenzeichen — „BK6-24-174 (Teil 1–3), BK6-22-024 Anlage 1d" is exactly
/// right — so the two halves count as paired only when they sit within
/// [`PAIRING_SPAN`] characters of each other with no *other* Aktenzeichen
/// between them. A second `BK6-…` in the gap means the Anlage belongs to that
/// one, not to this one.
struct Pairing {
    /// The Aktenzeichen half.
    az: &'static str,
    /// The Anlage half.
    anlage: &'static str,
    /// What the pairing should be instead.
    reason: &'static str,
}

/// Anlage numbers whose owning Beschluss is not what the citation says.
///
/// The pairings come from the Anlagen's own title pages in the mirror:
/// `Anlage2a_WiM_Teil1_Lesefassung.pdf` reads „Anlage 2a zum Beschluss
/// BK6-22-024", `BK6-24-174_WiM_Teil1_Aenderung.pdf` reads „Anlage 2a zur
/// Festlegung BK6-24-174", and `Anlage1d_GPKE_Teil4.pdf` reads „Anlage 1d zur
/// Festlegung BK6-22-024". The mirror is not in the tree (it is gitignored), so
/// the table is literal — `cargo xtask sync-regulatories` is what refreshes the
/// documents behind it.
///
/// The WiM entries are the ones that matter: the Fristen tables are identical
/// across the two editions, but the ZP-Typ definitions are not, so a citation to
/// the superseded edition resolves to a real document and states the wrong rule.
const PAIRINGS: &[Pairing] = &[
    Pairing {
        az: "BK6-22-024",
        anlage: "Anlage 2a",
        reason: "Anlage 2a (WiM Strom Teil 1) is **BK6-24-174**'s since 06.06.2025 \
                 (Tenorziffer 2); BK6-22-024 carried the superseded edition. Cite it only \
                 where the sentence is deliberately about the text in force before that date",
    },
    Pairing {
        az: "BK6-22-024",
        anlage: "Anlage 2b",
        reason: "Anlage 2b (WiM Strom Teil 2) is **BK6-24-174**'s since 06.06.2025 \
                 (Tenorziffer 2); BK6-22-024 carried the superseded edition",
    },
    Pairing {
        az: "BK6-24-174",
        anlage: "Anlage 1d",
        reason: "Anlage 1d (GPKE Teil 4) belongs to **BK6-22-024** — the two GPKE \
                 Aktenzeichen split by Teil, not by age",
    },
];

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
    let mut scanned = 0usize;
    for dir in [
        "crates",
        "services",
        "xtask",
        "site/content",
        "concepts",
        ".github",
    ] {
        collect(
            &workspace_root.join(dir),
            workspace_root,
            &mut scanned,
            &mut findings,
        );
    }
    // The root `AGENTS.md` carries the domain rules every agent reads before
    // touching code, so its citations are the ones most likely to be copied.
    // It is a file rather than a directory, so the walk above misses it.
    check_file(
        &workspace_root.join("AGENTS.md"),
        workspace_root,
        &mut scanned,
        &mut findings,
    );

    // A guard that read no file finds no malformed citation, and reports the
    // clean line for it.
    if scanned == 0 {
        eprintln!(
            "check-citations: the scan read no file under any of the searched trees — \
             the layout has probably changed"
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-citations: every Festlegung cited in {scanned} file(s) is cited in a form \
             it publishes"
        );
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
fn check_file(path: &Path, root: &Path, scanned: &mut usize, findings: &mut Vec<Finding>) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if is_rule_site(rel) {
        return;
    }
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    *scanned += 1;
    for (line, i) in offending_lines(&src) {
        findings.push((path.to_path_buf(), i, line));
    }
}

/// Every `.rs`, `.md` and `.sql` file under `dir`, counting what it read into
/// `scanned`.
fn collect(dir: &Path, root: &Path, scanned: &mut usize, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, scanned, findings);
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
        *scanned += 1;
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
        for p in PAIRINGS {
            if pairs_on(&stripped, p.az, p.anlage) {
                out.push((format!("`{}` + `{}` — {}", p.az, p.anlage, p.reason), i + 1));
            }
        }
    }
    out
}

/// How far apart the two halves of a pairing may sit and still be one citation.
///
/// Wide enough for „Anlage 2a zum Beschluss BK6-22-024" and a Markdown cell
/// boundary, narrow enough that two unrelated citations on one line do not
/// bind to each other.
const PAIRING_SPAN: usize = 60;

/// Whether `az` and `anlage` name each other on this line.
///
/// True when some occurrence of each sits within [`PAIRING_SPAN`] of the other
/// with no further Aktenzeichen in the gap, in either order.
fn pairs_on(line: &str, az: &str, anlage: &str) -> bool {
    for (a, _) in line.match_indices(az) {
        for (b, _) in line.match_indices(anlage) {
            let (lo, hi) = if a < b {
                (a + az.len(), b)
            } else {
                (b + anlage.len(), a)
            };
            if hi < lo || hi - lo > PAIRING_SPAN {
                continue;
            }
            // Another Aktenzeichen in the gap means the Anlage attaches to it.
            if !line[lo..hi].contains("BK6-") && !line[lo..hi].contains("BK7-") {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {

    /// A scan that reaches no file has checked nothing, and must say so rather
    /// than report the clean line.
    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!super::run(std::path::Path::new(
            "/nonexistent/mako/workspace/root"
        )));
    }
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
        assert_eq!(
            offending_lines("/// see BK6-24-174 Anlage 1d Kap. 3").len(),
            1,
            "Anlage 1d is BK6-22-024's"
        );
    }

    /// The WiM Anlagen moved to BK6-24-174. Both spellings of the pairing are
    /// refused, because both appeared in the tree.
    #[test]
    fn the_superseded_wim_aktenzeichen_is_refused() {
        for line in [
            "//! - **BNetzA BK6-22-024**, Anlage 2a — WiM Strom Teil 1",
            "/// BK6-22-024 Anlage 2b Kap. 1.2",
            "//! Models WiM Strom Teil 1 (Anlage 2a zu BK6-22-024) Kap. 3.1",
            "/// WiM Strom Teil 2 (Anlage 2b zu BK6-22-024), Kapitel 4",
        ] {
            assert!(
                !offending_lines(line).is_empty(),
                "the superseded WiM pairing must be refused: {line}"
            );
        }
    }

    /// A line may legitimately name both GPKE Aktenzeichen. The Anlage binds to
    /// the nearer one, and an Aktenzeichen in the gap breaks the pairing.
    #[test]
    fn a_line_naming_both_aktenzeichen_binds_each_anlage_to_its_own() {
        for line in [
            "| Festlegung | BK6-24-174 (Teil 1–3), BK6-22-024 Anlage 1d (Teil 4) |",
            "/// GPKE Teil 2 (BK6-24-174 Anlage 1b) and Teil 4 (BK6-22-024 Anlage 1d);",
            "//! BK6-24-174 Anlagen 2a/2b; BK6-22-024 Anlage 1d",
        ] {
            assert!(
                offending_lines(line).is_empty(),
                "the Anlage binds to the Aktenzeichen beside it: {line}"
            );
        }
    }

    /// BK6-22-024 is still the Aktenzeichen for LFW24 and GPKE Teil 4, so the
    /// guard must not refuse it wholesale.
    #[test]
    fn the_aktenzeichen_is_not_refused_wholesale() {
        for line in [
            "//! - **BK6-22-024** — LFW24 (§ 20a EnWG)",
            "/// GPKE Teil 4 Kap. 5 (BK6-22-024 Anlage 1d).",
            "//! | **BK6-22-024** | LFW24 (§ 20a EnWG); GPKE **Teil 4** = Anlage 1d |",
            "//! - **BK6-24-174 Anlage 2a**, WiM Strom Teil 1 Kap. 3.5",
        ] {
            assert!(
                offending_lines(line).is_empty(),
                "a live BK6-22-024 citation must pass: {line}"
            );
        }
    }
}
