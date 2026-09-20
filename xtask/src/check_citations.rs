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
//! **A repealed statute.** `MessZV` is the one carried here: the
//! Messzugangsverordnung was repealed by Art. 12 G. v. 29.08.2016 and folded
//! into the MsbG, so every `§ … MessZV` names a provision that has not existed
//! for a decade. This is the same unfalsifiability as a malformed Aktenzeichen
//! arriving by a different route — the form is impeccable and the document is
//! gone. The living anchors are named in the root `AGENTS.md`; the ones this
//! guard's own findings most often want are `§ 60 Abs. 1 MsbG`
//! (Ersatzwertbildung, Plausibilisierung), `§ 60 Abs. 6 MsbG` (Messwert-Audit,
//! Löschfrist), `§ 30 MsbG` (Preisobergrenzen) and `§ 147 AO` (Aufbewahrung).
//! Not StromNZV or GasNZV: both went ausser Kraft with the end of 31.12.2025,
//! so RLM-Spitzenleistung and MMM are now `§ 20 Abs. 3 EnWG` via BK6-24-174.
//!
//! ## What it deliberately does not do
//!
//! It checks the *form* of a citation, not whether the chapter it names says
//! what the sentence claims. That needs the document, and the documents are not
//! in the tree. The form is what can be checked cheaply and is wrong often.
//!
//! ## What it scans
//!
//! Every `.rs`, `.md`, `.sql`, `.cedar`, `.yaml` and `.yml` file under `crates`,
//! `services`, `xtask`, `site/content`, `concepts` and `.github`, plus the root
//! `AGENTS.md`. The agent-instruction files are in scope deliberately: a
//! citation an agent reads before writing code is the one most likely to be
//! copied into it — and those files are `services/agentd/agents/*.yaml`, so a
//! scan set of `.rs`/`.md`/`.sql` alone stated that intent without meeting it.
//! Cedar policies earn their place the same way: `services/*/policies/*.cedar`
//! comment each granted action with the rule that justifies it, which is a
//! regulatory claim sitting in the file that decides who may act on it.
//!
//! `.json` is deliberately **excluded**. The only JSON here carrying an
//! Aktenzeichen is `crates/edi-energy/profiles/**`, which is BDEW's own AHB and
//! MIG text extracted verbatim. A guard that rewrote a citation there would be
//! correcting the primary source against a table derived from it, and the
//! extraction-fidelity suite exists precisely to keep that text byte-faithful.

use std::path::{Path, PathBuf};

/// One offending site.
type Finding = (PathBuf, usize, String);

/// A refused citation form and why it is one.
struct Rule {
    /// The literal that may not appear.
    pattern: &'static str,
    /// What to write instead.
    reason: &'static str,
    /// Files permitted to name this pattern because they state this rule.
    ///
    /// Deliberately narrower than [`RULE_SITES`], which exempts a file from
    /// *every* rule. A repealed statute has to stay nameable in the paragraph
    /// that records the repeal and nowhere else, and the paragraph recording
    /// this one lives in the root `AGENTS.md` — the file whose Festlegung
    /// citations are the most-copied in the tree and so the last one that
    /// should be exempted wholesale.
    sites: &'static [&'static str],
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
        sites: &[],
    },
    Rule {
        pattern: "BK6-22-024§",
        reason: "BK6-22-024 has no numbered §§ — cite the Anlage and its chapter",
        sites: &[],
    },
    Rule {
        pattern: "BK6-24-174 GPKE Teil 4",
        reason: "GPKE Teil 4 is Anlage 1d to **BK6-22-024**; BK6-24-174 carries Teil 1–3 \
                 as Anlagen 1a–1c",
        sites: &[],
    },
    Rule {
        pattern: "MessZV",
        reason: "the MessZV was repealed by Art. 12 G. v. 29.08.2016 and folded into the \
                 MsbG, so a „§ … MessZV\" cites a provision that no longer exists. The \
                 living anchors are in the root AGENTS.md: Ersatzwertbildung and \
                 Plausibilisierung are § 60 Abs. 1 MsbG, the Messwert-Audit and its \
                 Löschfrist § 60 Abs. 6 MsbG, Preisobergrenzen § 30 MsbG, and record \
                 retention § 147 AO / GoBD",
        sites: &["AGENTS.md"],
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
    let key = rel.to_string_lossy().replace('\\', "/");
    for (line, i) in offending_lines_in(&key, &src) {
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
            Some("rs" | "md" | "sql" | "cedar" | "toml" | "yaml" | "yml")
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
        let key = rel.to_string_lossy().replace('\\', "/");
        for (line, i) in offending_lines_in(&key, &src) {
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
#[cfg(test)]
pub fn offending_lines(src: &str) -> Vec<(String, usize)> {
    offending_lines_in("", src)
}

/// Every refused citation in `src`, for a file at workspace-relative `rel`.
///
/// The path is what lets a rule exempt the file that states it. `rel` is
/// compared with `/` separators; an empty `rel` matches no site, so a caller
/// with no path in hand gets every rule.
#[must_use]
pub fn offending_lines_in(rel: &str, src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let stripped = line.replace('*', "");
        for rule in RULES {
            if !rule.sites.is_empty() && rule.sites.contains(&rel) {
                continue;
            }
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

/// Every Aktenzeichen on `line`, as byte ranges, in order.
///
/// The token runs to the first character that cannot be part of one. `/` is in
/// the set because BK8 numbers carry it (`BK8-22/010-A`); without it that
/// Aktenzeichen truncates to `BK8-22` and stops matching itself.
fn aktenzeichen_spans(line: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for prefix in ["BK6-", "BK7-", "BK8-"] {
        for (i, _) in line.match_indices(prefix) {
            let end = line[i..]
                .char_indices()
                .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '/'))
                .map_or(line.len(), |(o, _)| i + o);
            spans.push((i, end));
        }
    }
    spans.sort_unstable();
    spans
}

/// Whether `anlage` on this line is claimed by `az`.
///
/// An Anlage number belongs to whichever Aktenzeichen it sits beside, so the
/// question is which one is **nearest** — on either side, since both „BK6-24-174
/// Anlage 1b" and „Anlage 2a zu BK6-22-024" are ordinary spellings. Only when
/// the nearest is `az`, and within [`PAIRING_SPAN`], is this the refused pair.
///
/// Nearest has to mean nearest in both directions, not merely „no other
/// Aktenzeichen in the gap". A sentence that names the wrong Aktenzeichen in
/// order to correct it — „GPKE Teil 4 is Anlage 1d to BK6-22-024", after an
/// earlier clause ended in BK6-24-174 — puts the owning Aktenzeichen *after*
/// the Anlage, where a gap-only test cannot see it, and refuses the passage
/// that states the rule correctly.
fn pairs_on(line: &str, az: &str, anlage: &str) -> bool {
    let spans = aktenzeichen_spans(line);
    for (b, _) in line.match_indices(anlage) {
        let b_end = b + anlage.len();
        let mut nearest: Option<(usize, bool)> = None;
        for &(start, end) in &spans {
            // Distance from the Anlage to this Aktenzeichen, on whichever side
            // it lies; zero when the two overlap.
            let gap = if end <= b {
                b - end
            } else {
                start.saturating_sub(b_end)
            };
            if nearest.is_none_or(|(best, _)| gap < best) {
                nearest = Some((gap, &line[start..end] == az));
            }
        }
        if let Some((gap, true)) = nearest {
            if gap <= PAIRING_SPAN {
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
    use super::{offending_lines, offending_lines_in};

    /// A repealed statute is refused wherever it is cited.
    ///
    /// The Messzugangsverordnung went away in 2016. The sweep that removed its
    /// citations left one behind in a `.cedar` policy, which is both halves of
    /// why this test exists: the rule had no guard, and the scan had no reach
    /// into the file holding the survivor.
    #[test]
    fn a_repealed_statute_is_refused() {
        assert_eq!(
            offending_lines("/// plausibility against the Preisblatt (§ 22 MessZV).").len(),
            1
        );
        assert_eq!(offending_lines("// see MessZV § 19").len(), 1);
    }

    /// The paragraph recording the repeal may name it; nothing else may.
    ///
    /// The exemption is per-rule rather than per-file: `AGENTS.md` carries more
    /// Festlegung citations than any other file in the tree, so exempting it
    /// from every rule to let it state this one would give up the check exactly
    /// where it pays most.
    #[test]
    fn the_paragraph_stating_the_repeal_may_name_it() {
        let row = "| Any \"§… MessZV\" citation | The MessZV was repealed by Art. 12 |";
        assert!(offending_lines_in("AGENTS.md", row).is_empty());
        assert_eq!(
            offending_lines_in("services/marktd/policies/marktd.cedar", row).len(),
            1,
            "only the file that states the rule is exempt from it"
        );
    }

    /// The exemption is scoped to its own rule, not to the file.
    #[test]
    fn the_exempt_file_is_still_checked_for_every_other_rule() {
        assert_eq!(
            offending_lines_in("AGENTS.md", "/// BK6-24-174 GPKE Teil 4 Kap. 5").len(),
            1,
            "AGENTS.md is exempt from the MessZV rule alone"
        );
    }

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

    /// A sentence that corrects the pairing names both Aktenzeichen, and the
    /// owning one comes last.
    ///
    /// „GPKE Teil 4 is Anlage 1d to BK6-22-024" is the passage that states the
    /// rule; the clause before it ends in BK6-24-174. The Anlage binds to the
    /// Aktenzeichen it sits beside, which is the one after it, so this must
    /// pass — a guard that refuses the text explaining the rule teaches the
    /// reader to delete the explanation.
    ///
    /// Proximity is the whole rule, and it is a rule about writing as much as
    /// about citing: a sentence that leaves the *wrong* Aktenzeichen nearer to
    /// the Anlage than the right one is ambiguous to a reader too, and is
    /// refused. Naming the owner adjacently — „Anlage 1d zu BK6-22-024" —
    /// resolves it for both.
    #[test]
    fn the_owning_aktenzeichen_may_follow_the_anlage() {
        for line in [
            "**BK6-24-174**. GPKE **Teil 4** is Anlage 1d to **BK6-22-024**, which is the one",
            "//! Models WiM Strom Teil 1 (Anlage 2a zu BK6-24-174) Kap. 3.1",
        ] {
            assert!(
                offending_lines(line).is_empty(),
                "the Anlage binds to the nearer Aktenzeichen, whichever side it is on: {line}"
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
