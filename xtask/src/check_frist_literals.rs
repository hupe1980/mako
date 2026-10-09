//! Guard: a string that names a regulatory source carries no Frist literal.
//!
//! A Frist written as prose beside its source — „Antwort binnen 5 Werktagen
//! (BK6-24-174)" — reads as sourced, and that is exactly what makes it
//! dangerous: `check-citations` passes the Aktenzeichen, a reviewer sees the
//! citation and stops, and the number beside it is free to be wrong. The
//! windows live in `mako_fristen` (`antwort`, `vorlauf`, the `aperak_*`
//! functions) and in the domain crates' own `fristen` modules; a string that
//! states one renders it from there.
//!
//! ## What it refuses
//!
//! A Rust string literal that contains **both**
//!
//! - a source name: a BK-Aktenzeichen (`BK6-…`, `BK7-…`, `BK8-…`) or a
//!   document name — GPKE, GeLi Gas, WiM, MaBiS, AHB, AWH, KoV — and
//! - a Frist literal: a number of Werktage (`5 Werktage`, `3. WT`,
//!   `6 Werktagen`) or a clock time (`11:00`, `12 Uhr`). Midnight (`00:00`)
//!   is not one: it names the boundary of a day, a Zeitpunkt rather than a
//!   Frist.
//!
//! A document name is needed beside the Aktenzeichen because the worst
//! sentence names „GPKE Teil 2" and no Aktenzeichen at all.
//!
//! ## What it scans
//!
//! String literals — the text an API, an MCP tool or a log line returns —
//! in `crates/*/src` and `services/*/src`, up to the first `#[cfg(test)]` of a
//! file. Comments and doc comments are a separate surface and are not scanned:
//! they describe a Frist, they do not hand one to a caller.
//!
//! `crates/mako-fristen` is excluded: it is the catalogue, and its `source`
//! strings quote the Festlegung verbatim — the one place a Frist is written
//! beside its source by design. [`EXEMPT`] lists every other admitted literal,
//! each with its reason.

use std::path::Path;

use regex::Regex;

/// Crates whose literals are the catalogue itself.
const CATALOGUES: &[&str] = &[
    "crates/mako-fristen/",
    // The Meilenstein catalogue: each entry's `fundstelle` quotes its source.
    "crates/mako-nbw/src/meilenstein.rs",
];

/// A literal admitted although it names a source and a Frist.
struct Exempt {
    /// Workspace-relative path.
    path: &'static str,
    /// A fragment of the admitted literal.
    fragment: &'static str,
    /// Why it is right. Read by the tests; for the guard it is the entry's
    /// documentation.
    #[allow(
        dead_code,
        reason = "reached only in some feature or target combinations"
    )]
    reason: &'static str,
}

/// Admitted literals. The list may only shrink.
const EXEMPT: &[Exempt] = &[
    Exempt {
        path: "services/makod/src/api/mcp_server.rs",
        fragment: "A Prüfmitteilung has no Frist",
        reason: MABIS_TABELLE_2,
    },
    Exempt {
        path: "services/mabis-syncd/src/mcp_server.rs",
        fragment: "MaBiS submission health snapshot",
        reason: MABIS_TABELLE_2,
    },
    Exempt {
        path: "services/mabis-syncd/src/mcp_server.rs",
        fragment: "MaBiS Summenzeitreihe triage",
        reason: MABIS_TABELLE_2,
    },
    Exempt {
        path: "services/mabis-syncd/src/mcp_server.rs",
        fragment: "mabis-syncd MCP — MaBiS Summenzeitreihe submission state",
        reason: MABIS_TABELLE_2,
    },
    Exempt {
        path: "services/makod/src/orchestrator/deadline_dispatch.rs",
        fragment: "the Strom APERAK was not delivered within 45 minutes",
        reason: "the alert names the window `mako_fristen::aperak_strom_due_at` applied; the \
                 dispatcher reports the lapse of that very deadline",
    },
    Exempt {
        path: "services/netzbilanzd/src/mcp_server.rs",
        fragment: "on the 06:00 Gastag",
        reason: "06:00 is where the Gastag begins, a day boundary and not a Frist",
    },
    Exempt {
        path: "services/sperrd/src/events.rs",
        fragment: "6 WT nach dem frühestmöglichen Sperrtermin",
        reason: SPERR_AUSFUEHRUNG,
    },
    Exempt {
        path: "services/sperrd/src/mcp_server.rs",
        fragment: "6 Werktagen nach dem frühestmöglichen Sperrtermin",
        reason: SPERR_AUSFUEHRUNG,
    },
];

/// Why a MaBiS clearing-window figure is admitted.
const MABIS_TABELLE_2: &str = "MaBiS has no answer Frist; the WT figures are clearing-window \
     positions of BK6-24-174 Anlage 3 Kap. 3.10 Tabelle 2, a date range anchored on the month \
     end, which `mako_mabis::fristen::Bilanzierungsmonat` resolves to dates and not to a number \
     of Werktage";

/// Why the Sperrung execution window is admitted.
const SPERR_AUSFUEHRUNG: &str = "the execution window is the Hinweis column of GPKE Teil 2 \
     § 3.5.1.2 Nr. 1 and has no catalogue entry; `mako_fristen::vorlauf::SPERRAUFTRAG_VORLAUF_WT` \
     is the 6 WT before the frühestmöglicher Sperrtermin, a different window";

/// One finding: path, line, literal excerpt.
type Finding = (String, usize, String);

/// Scan the workspace. Returns `true` when no literal is refused.
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    for tree in ["crates", "services"] {
        let Ok(entries) = std::fs::read_dir(workspace_root.join(tree)) else {
            continue;
        };
        for e in entries.flatten() {
            collect_rs(&e.path().join("src"), &mut files);
        }
    }
    files.sort();
    if files.is_empty() {
        eprintln!("check-frist-literals: no source file found — the layout has probably changed");
        return false;
    }
    let matcher = Matcher::new();
    let mut findings = Vec::new();
    for path in &files {
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if CATALOGUES.iter().any(|c| rel.starts_with(c)) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        findings.extend(matcher.findings(&rel, &src));
    }
    if findings.is_empty() {
        println!(
            "check-frist-literals: no string literal in {} file(s) states a Frist beside its source",
            files.len()
        );
        return true;
    }
    eprintln!(
        "check-frist-literals: {} literal(s) state a Frist beside a source — render it from \
         `mako_fristen` instead:",
        findings.len()
    );
    for (path, line, excerpt) in &findings {
        eprintln!("  {path}:{line}  {excerpt}");
    }
    false
}

fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

struct Matcher {
    source: Regex,
    frist: Regex,
}

impl Matcher {
    fn new() -> Self {
        Self {
            source: Regex::new(
                r"\bBK[678]-[0-9]{2}|\bGPKE\b|\bGeLi Gas\b|\bWiM\b|\bMaBiS\b|\bAHB\b|\bAWH\b|\bKoV\b",
            )
            .expect("source pattern"),
            frist: Regex::new(
                r"(?i)\b\d+\.? ?(werktag\w*|wt\b)|\b(0?[1-9]|1\d|2[0-3]):\d{2}\b|\b(0?[1-9]|1\d|2[0-3]) uhr\b|\b\d+ (working|business) days?\b",
            )
            .expect("frist pattern"),
        }
    }

    /// Whether one literal's text is refused.
    fn refuses(&self, text: &str) -> bool {
        self.source.is_match(text) && self.frist.is_match(text)
    }

    fn findings(&self, rel: &str, src: &str) -> Vec<Finding> {
        let production = src.find("#[cfg(test)]").map_or(src, |i| &src[..i]);
        string_literals(production)
            .into_iter()
            .filter(|(_, text)| self.refuses(text))
            .filter(|(_, text)| {
                !EXEMPT
                    .iter()
                    .any(|e| e.path == rel && text.contains(e.fragment))
            })
            .map(|(line, text)| {
                let excerpt: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let excerpt: String = excerpt.chars().take(140).collect();
                (rel.to_owned(), line, excerpt)
            })
            .collect()
    }
}

/// Every string literal in `src` as `(1-based line of its start, text)`.
///
/// Comments, char literals and lifetimes are skipped; a `\` line continuation
/// is folded so a literal split across lines is one text.
fn string_literals(src: &str) -> Vec<(usize, String)> {
    let b = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    while i < b.len() {
        match b[i] {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    if b[i] == b'\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            b'\'' => {
                // A char literal ('x', '\n', '\'') or a lifetime ('a).
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                    i += 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                } else {
                    // Possibly a multi-byte char literal; else a lifetime.
                    let rest = &src[i + 1..];
                    let mut chars = rest.char_indices();
                    match (chars.next(), chars.next()) {
                        (Some((_, c)), Some((j, '\''))) if c != '\'' => i += 1 + j + 1,
                        _ => i += 1,
                    }
                }
            }
            b'r' if is_raw_start(b, i) => {
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&b'#') {
                    hashes += 1;
                    j += 1;
                }
                j += 1; // the opening quote
                let start_line = line;
                let start = j;
                let closing: Vec<u8> = std::iter::once(b'"')
                    .chain(std::iter::repeat_n(b'#', hashes))
                    .collect();
                while j < b.len() && !b[j..].starts_with(&closing) {
                    if b[j] == b'\n' {
                        line += 1;
                    }
                    j += 1;
                }
                out.push((start_line, src[start..j.min(b.len())].to_owned()));
                i = j + closing.len();
            }
            b'"' => {
                let start_line = line;
                let mut text = String::new();
                let mut j = i + 1;
                while j < b.len() && b[j] != b'"' {
                    if b[j] == b'\\' {
                        match b.get(j + 1) {
                            Some(b'\n') => {
                                line += 1;
                                j += 2;
                                while j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                                    j += 1;
                                }
                                text.push(' ');
                                continue;
                            }
                            Some(b'n') => text.push('\n'),
                            Some(&c) => text.push(c as char),
                            None => {}
                        }
                        j += 2;
                        continue;
                    }
                    if b[j] == b'\n' {
                        line += 1;
                    }
                    let ch_len = utf8_len(b[j]);
                    text.push_str(&src[j..(j + ch_len).min(b.len())]);
                    j += ch_len;
                }
                out.push((start_line, text));
                i = j + 1;
            }
            c => i += utf8_len(c),
        }
    }
    out
}

fn is_raw_start(b: &[u8], i: usize) -> bool {
    let prev_ident = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
    if prev_ident {
        return false;
    }
    let mut j = i + 1;
    while b.get(j) == Some(&b'#') {
        j += 1;
    }
    b.get(j) == Some(&b'"')
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refused(src: &str) -> usize {
        Matcher::new().findings("crates/x/src/lib.rs", src).len()
    }

    #[test]
    fn a_frist_beside_an_aktenzeichen_is_refused() {
        assert_eq!(
            refused(r#"let s = "Antwort binnen 5 Werktagen (BK6-24-174)";"#),
            1
        );
    }

    #[test]
    fn a_clock_time_beside_a_document_name_is_refused() {
        assert_eq!(refused(r#"let s = "bis 11:00 Uhr (GPKE Teil 2)";"#), 1);
        assert_eq!(
            refused(r#"let s = "nächster Werktag 12 Uhr (APERAK AHB 1.1)";"#),
            1
        );
    }

    #[test]
    fn a_literal_split_across_lines_is_one_text() {
        let src = "let s = \"GNB has 10 Werktage to respond \\\n    (BK7-24-01-009).\";";
        assert_eq!(refused(src), 1);
    }

    #[test]
    fn a_rendered_frist_passes() {
        assert_eq!(
            refused(r#"let s = format!("Antwort bis {} ({})", due, "BK6-24-174");"#),
            0
        );
        assert_eq!(refused(r#"let s = "{n}. Werktag (GPKE Teil 2)";"#), 0);
    }

    #[test]
    fn comments_and_test_code_are_not_scanned() {
        assert_eq!(refused("// Antwort binnen 5 Werktagen (BK6-24-174)\n"), 0);
        assert_eq!(refused("/// 11:00 Uhr (GPKE Teil 2)\nfn f() {}"), 0);
        assert_eq!(
            refused("fn f() {}\n#[cfg(test)]\nmod t { const S: &str = \"5 WT (WiM)\"; }"),
            0
        );
    }

    #[test]
    fn a_raw_string_is_scanned() {
        assert_eq!(refused(r##"let s = r#"3 WT (WiM Teil 1)"#;"##), 1);
    }

    #[test]
    fn lifetimes_and_chars_do_not_derail_the_lexer() {
        let src = "fn f<'a>(x: &'a str) -> char { let c = '\"'; let d = 'é'; \"5 WT (WiM)\"; c }";
        assert_eq!(refused(src), 1);
    }

    #[test]
    fn an_exemption_names_its_reason() {
        for e in EXEMPT {
            assert!(!e.reason.is_empty() && !e.fragment.is_empty() && !e.path.is_empty());
        }
    }

    #[test]
    fn refuses_a_tree_it_found_nothing_in() {
        assert!(!run(Path::new("/nonexistent/mako/workspace/root")));
    }
}
