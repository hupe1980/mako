//! Guard: a quotation attributed to a mirrored Festlegung occurs in it.
//!
//! A „…" quotation after „MaBiS Kap. 3.10" reads as the document's own words,
//! and a reader who does not hold the PDF cannot tell a verbatim sentence from a
//! paraphrase, a sentence from another chapter or one from another Festlegung.
//! `check-citations` checks the form of a citation; this checks the words.
//!
//! ## What counts as a quotation of a document
//!
//! A „…" quotation in a Rust comment or a Markdown paragraph whose preceding
//! text ends in a document's name and a locator — `GPKE Teil 2 Kap. 2.1.2:`,
//! `BilAReM Nr. 6.4.3`, `WiM Teil 1 S. 34` — or, in a crate that implements one
//! document ([`CRATE_DOCUMENTS`]), a bare `Kap. n`. A quotation naming anything
//! else (an EBD, an AHB, a statute) is not checked here.
//!
//! ## How the words are compared
//!
//! Both texts are reduced to lower-case words, so hyphenation, soft hyphens,
//! quote marks and line breaks do not count. `…` and `[…]` split a quotation
//! into fragments, and each fragment's words must occur in order in the
//! document, within [`SLACK`] document words per quoted word: `pdftotext`
//! interleaves a table's columns, so a cell's sentence arrives with its
//! neighbours' words between its own. A reordered sentence or a changed word
//! still fails. Each document is read in both of `pdftotext`'s orders, raw and
//! `-layout`, and a fragment passes in either.
//!
//! `regulatories/` is gitignored, so on a checkout without the mirror, or
//! without poppler's `pdftotext`, the guard reports SKIPPED and passes. Run
//! `cargo xtask sync-regulatories --download` first.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use regex::Regex;

/// A mirrored document, the name the tree cites it by, and where it lies.
struct Document {
    /// How a citation names it, as a regex fragment.
    name: &'static str,
    /// The PDF under the workspace root.
    path: &'static str,
}

/// The current editions. WiM Teil 1 is Anlage 2a to BK6-24-174
/// (`…_WiM_Teil1_Aenderung.pdf`), not the BK6-22-024 edition beside it.
const DOCUMENTS: &[Document] = &[
    Document {
        name: r"MaBiS",
        path: "regulatories/bentza/BK6-24-174_MaBiS_Lesefassung.pdf",
    },
    Document {
        name: r"BilAReM",
        path: "regulatories/bnetza-bk6-23-241-bilarem-festlegungstext.pdf",
    },
    Document {
        name: r"GPKE Teil 1",
        path: "regulatories/bentza/BK6-24-174_GPKE_Teil1_Lesefassung.pdf",
    },
    Document {
        name: r"GPKE Teil 2",
        path: "regulatories/bentza/BK6-24-174_GPKE_Teil2_Lesefassung.pdf",
    },
    Document {
        name: r"GPKE Teil 3",
        path: "regulatories/bentza/BK6-24-174_GPKE_Teil3_Lesefassung.pdf",
    },
    Document {
        name: r"GPKE Teil 4",
        path: "regulatories/bentza/Anlage1d_GPKE_Teil4.pdf",
    },
    Document {
        name: r"WiM (?:Strom )?Teil 1",
        path: "regulatories/bentza/BK6-24-174_WiM_Teil1_Aenderung.pdf",
    },
    Document {
        name: r"WiM (?:Strom )?Teil 2",
        path: "regulatories/bentza/BK6-24-174_WiM_Teil2_Lesefassung.pdf",
    },
];

/// A crate whose bare `Kap. n` names one document: its subject is that
/// document, so its comments cite chapters without repeating the name.
const CRATE_DOCUMENTS: &[(&str, &str)] = &[("crates/mako-mabis/", r"MaBiS")];

/// Document words allowed per quoted word between a fragment's first and last.
const SLACK: usize = 3;

/// How many following words a soft-hyphenated head is joined with.
const SOFT_HYPHEN_REACH: usize = 12;

/// How far before a quotation its citation may stand, in characters.
const LEAD: usize = 90;

/// The workspace roots scanned, and the extension read under each.
const SCANNED: &[(&str, &str)] = &[
    ("crates", "rs"),
    ("services", "rs"),
    ("crates", "md"),
    ("services", "md"),
    ("site/content", "md"),
];

pub fn run(root: &Path) -> bool {
    if std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .is_err()
    {
        eprintln!(
            "check-quotations: SKIPPED — `pdftotext` (poppler) is not on PATH, so no \
             mirrored document can be read."
        );
        return true;
    }
    let rules = Rules::new();
    let mut files = Vec::new();
    for (dir, ext) in SCANNED {
        collect(&root.join(dir), ext, &mut files);
    }

    let mut indexes: HashMap<&str, Option<Vec<Index>>> = HashMap::new();
    let mut unmirrored: BTreeMap<&str, usize> = BTreeMap::new();
    let mut findings = Vec::new();
    let mut checked = 0usize;

    for file in &files {
        let Ok(source) = std::fs::read_to_string(file) else {
            continue;
        };
        let rel = file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .replace('\\', "/");
        let markdown = rel.ends_with(".md");
        for q in quotations(&source, markdown) {
            let Some(doc) = rules.attribute(&rel, &q.lead) else {
                continue;
            };
            let index = indexes
                .entry(doc.path)
                .or_insert_with(|| read_document(&root.join(doc.path)));
            let Some(index) = index else {
                *unmirrored.entry(doc.path).or_default() += 1;
                continue;
            };
            checked += 1;
            if !occurs(&q.text, index) {
                findings.push(format!(
                    "{rel}:{}: „{}\" is not in {}",
                    q.line,
                    q.text.split_whitespace().collect::<Vec<_>>().join(" "),
                    doc.path
                ));
            }
        }
    }

    for (path, n) in &unmirrored {
        eprintln!("check-quotations: SKIPPED {n} quotation(s) of {path} — not mirrored");
    }
    if findings.is_empty() {
        if checked == 0 {
            eprintln!(
                "check-quotations: SKIPPED — no quotation could be checked. Run `cargo xtask \
                 sync-regulatories --download` to mirror the documents."
            );
        } else {
            println!("check-quotations: {checked} quotation(s) occur in the documents they cite");
        }
        return true;
    }
    eprintln!(
        "check-quotations: {} quotation(s) do not occur in the document they cite. Quote \
         the sentence as the document prints it — marking omissions with „…\" — or drop \
         the quotation marks:",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

/// The citation patterns, compiled once.
struct Rules {
    named: Vec<(Regex, &'static Document)>,
    bare: Regex,
}

impl Rules {
    fn new() -> Self {
        const LOCATOR: &str = r"[^„\n]{0,6}(?:Kap\.|Kapitel|Nr\.|S\.)\s*[\d.]+[^„“]{0,40}$";
        let named = DOCUMENTS
            .iter()
            .map(|d| {
                let re =
                    Regex::new(&format!(r"\b{}\b{LOCATOR}", d.name)).expect("document pattern");
                (re, d)
            })
            .collect();
        // A bare chapter must stand right before the quotation (`Kap. 3.10: „`),
        // so a chapter named earlier in the sentence does not claim a title or
        // a term quoted after it.
        let bare = Regex::new(r"Kap\.\s*[\d.]+[^„“\w]{0,4}$").expect("bare locator");
        Self { named, bare }
    }

    /// The document a quotation is attributed to, read off the text before it.
    fn attribute(&self, file: &str, lead: &str) -> Option<&'static Document> {
        // A table cell is its own context: a citation in another cell does not
        // attribute this one.
        let lead = lead.rsplit('|').next().unwrap_or(lead);
        let named = self
            .named
            .iter()
            .filter_map(|(re, d)| re.find(lead).map(|m| (m.start(), *d)))
            .max_by_key(|(start, _)| *start)
            .map(|(_, d)| d);
        named.or_else(|| {
            let (_, name) = CRATE_DOCUMENTS
                .iter()
                .find(|(prefix, _)| file.starts_with(prefix))?;
            self.bare
                .is_match(lead)
                .then(|| DOCUMENTS.iter().find(|d| d.name == *name))
                .flatten()
        })
    }
}

/// One quotation and the text that leads up to it.
struct Quotation {
    line: usize,
    lead: String,
    text: String,
}

/// The „…" quotations of a source file, each with the text before it in the
/// same comment block (Rust) or paragraph (Markdown).
fn quotations(source: &str, markdown: bool) -> Vec<Quotation> {
    let quote = Regex::new(r#"„([^“”"]{12,}?)[“”"]"#).expect("quotation pattern");
    let mut out = Vec::new();
    for (start, block) in blocks(source, markdown) {
        for m in quote.captures_iter(&block) {
            let whole = m.get(0).expect("match");
            let before = &block[..whole.start()];
            let lead: String = before
                .chars()
                .rev()
                .take(LEAD)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(|c| if c == '\n' { ' ' } else { c })
                .collect();
            out.push(Quotation {
                line: start + before.matches('\n').count(),
                lead,
                text: m[1].to_owned(),
            });
        }
    }
    out
}

/// Comment blocks of a Rust file, or paragraphs of a Markdown file, each with
/// its first line number and its text stripped of comment markers.
fn blocks(source: &str, markdown: bool) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut current: Option<(usize, Vec<&str>)> = None;
    for (i, line) in source.lines().enumerate() {
        let body = if markdown {
            (!line.trim().is_empty()).then_some(line)
        } else {
            let t = line.trim_start();
            t.strip_prefix("//").map(|rest| {
                let rest = rest.strip_prefix(['/', '!']).unwrap_or(rest);
                let rest = rest.strip_prefix(' ').unwrap_or(rest);
                rest.strip_prefix('>')
                    .map_or(rest, |r| r.strip_prefix(' ').unwrap_or(r))
            })
        };
        match (body, current.as_mut()) {
            (Some(b), Some((_, lines))) => lines.push(b),
            (Some(b), None) => current = Some((i + 1, vec![b])),
            (None, _) => {
                if let Some((n, lines)) = current.take() {
                    out.push((n, lines.join("\n")));
                }
            }
        }
    }
    if let Some((n, lines)) = current {
        out.push((n, lines.join("\n")));
    }
    out
}

/// A document's words and where each occurs.
struct Index {
    positions: HashMap<String, Vec<usize>>,
}

impl Index {
    fn new(text: &str) -> Self {
        let tokens = tokens(text, true);
        let mut positions: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, t) in tokens.iter().enumerate() {
            match t.strip_suffix('\u{ad}') {
                // A soft hyphen ends the line inside a table cell, and the
                // word's tail arrives after the next column's text: index the
                // head joined with each of the words that follow it.
                Some(head) => {
                    for tail in tokens.iter().skip(i + 1).take(SOFT_HYPHEN_REACH) {
                        let tail = tail.trim_end_matches('\u{ad}');
                        positions
                            .entry(format!("{head}{tail}"))
                            .or_default()
                            .push(i);
                    }
                    positions.entry(head.to_owned()).or_default().push(i);
                }
                None => positions.entry(t.clone()).or_default().push(i),
            }
        }
        Self { positions }
    }
}

/// Both of `pdftotext`'s reading orders, or `None` when the PDF is absent.
fn read_document(pdf: &Path) -> Option<Vec<Index>> {
    if !pdf.is_file() {
        return None;
    }
    let read = |layout: bool| {
        let mut cmd = std::process::Command::new("pdftotext");
        if layout {
            cmd.arg("-layout");
        }
        let out = cmd.arg(pdf).arg("-").output().ok()?;
        out.status
            .success()
            .then(|| Index::new(&String::from_utf8_lossy(&out.stdout)))
    };
    let indexes: Vec<Index> = [false, true].into_iter().filter_map(read).collect();
    (!indexes.is_empty()).then_some(indexes)
}

/// Lower-case words, with end-of-line hyphenation joined.
fn words(text: &str) -> Vec<String> {
    tokens(text, false)
}

/// Lower-case words. A word broken by a soft hyphen keeps it as its last
/// character when `keep_soft` is set — the caller decides where its tail is —
/// and is joined to the next word otherwise.
fn tokens(text: &str, keep_soft: bool) -> Vec<String> {
    let mut s = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{ad}' && keep_soft {
            s.push_str("\u{ad} ");
            continue;
        }
        if c == '\u{ad}' || (c == '-' && chars.peek() == Some(&'\n')) {
            while chars.peek().is_some_and(|n| n.is_whitespace()) {
                chars.next();
            }
            continue;
        }
        s.push(c);
    }
    s.split(|c: char| !c.is_alphanumeric() && c != '\u{ad}')
        .filter(|w| !w.is_empty() && *w != "\u{ad}")
        .map(str::to_lowercase)
        .collect()
}

/// Every fragment of the quotation occurs in one of the document's readings.
fn occurs(quotation: &str, readings: &[Index]) -> bool {
    let fragments = Regex::new(r"…|\.\.\.|\[[^\]]*\]").expect("ellipsis pattern");
    fragments
        .split(quotation)
        .map(words)
        .filter(|w| !w.is_empty())
        .all(|fragment| readings.iter().any(|r| in_order(&fragment, r)))
}

/// The words occur in order, the last within [`SLACK`] document words per
/// quoted word of the first.
fn in_order(fragment: &[String], index: &Index) -> bool {
    let Some(starts) = index.positions.get(&fragment[0]) else {
        return false;
    };
    let reach = fragment.len() * SLACK;
    starts.iter().any(|&start| {
        let mut at = start;
        fragment[1..].iter().all(|w| {
            let Some(list) = index.positions.get(w) else {
                return false;
            };
            let k = list.partition_point(|&p| p <= at);
            match list.get(k) {
                Some(&p) if p - start <= reach => {
                    at = p;
                    true
                }
                _ => false,
            }
        })
    })
}

fn collect(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                collect(&path, ext, out);
            }
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MaBiS Kap. 3.10, as `pdftotext` reads its opening and Tabelle 2's last row.
    const MABIS_3_10: &str = "3.10. Übersicht der Fristen und Stichtage zur Bilanzkreisabrechnung\n\
        Die nachstehende Tabelle gibt einen Überblick über die wesentlichen Fristen der MaBiS. Sie\n\
        beziehen sich auf den Zeitpunkt des Eingangs einer Meldung (beinhaltet Summenzeitreihen\n\
        und Prüfmitteilungen) beim BIKO.\n\
        Abrechnungsstichtag    42. WT        Ende des 8. Monats    Die abrechnungsrelevanten Versi\u{ad}\n\
                               (Datenstand 30. WT) (Datenstand Ende   onen der Summenzeitreihen er\u{ad}\n\
                                                   7. Monat)          halten nun den Datenstatus „ab\u{ad}\n\
                                                                      gerechnete Daten“.";

    fn mabis() -> Vec<Index> {
        vec![Index::new(MABIS_3_10)]
    }

    #[test]
    fn a_sentence_from_another_festlegung_is_refused() {
        // BilAReM Kap. 6.4.3's sentence, once attributed to MaBiS Kap. 3.10.
        assert!(!occurs(
            "Danach dürfen die Prozesse … nicht erneut gestartet werden.",
            &mabis()
        ));
    }

    #[test]
    fn a_table_cell_passes_across_its_neighbours_columns() {
        assert!(occurs(
            "Die abrechnungsrelevanten Versionen der Summenzeitreihen erhalten nun den \
             Datenstatus „abgerechnete Daten",
            &mabis()
        ));
    }

    #[test]
    fn an_omission_needs_its_ellipsis() {
        let r = mabis();
        assert!(!occurs(
            "Sie beziehen sich auf den Zeitpunkt des Eingangs einer Meldung beim BIKO und der Prüfmitteilungen",
            &r
        ));
        assert!(occurs(
            "Sie beziehen sich auf den Zeitpunkt des Eingangs einer Meldung … beim BIKO",
            &r
        ));
    }

    #[test]
    fn a_reordered_sentence_is_refused() {
        assert!(!occurs(
            "beim BIKO beziehen sich auf den Zeitpunkt",
            &mabis()
        ));
    }

    #[test]
    fn the_citation_before_a_quotation_names_its_document() {
        let rules = Rules::new();
        let doc = |file: &str, lead: &str| rules.attribute(file, lead).map(|d| d.name);
        assert_eq!(
            doc("crates/x/src/a.rs", "see GPKE Teil 2 Kap. 2.1.2 Nr. 1: "),
            Some("GPKE Teil 2")
        );
        assert_eq!(
            doc("crates/x/src/a.rs", "WiM Strom Teil 1 Kap. 2.1.1: "),
            Some(r"WiM (?:Strom )?Teil 1")
        );
        // A bare chapter is MaBiS only inside the crate that implements it.
        assert_eq!(
            doc(
                "crates/mako-mabis/src/a.rs",
                "The window closed. Kap. 3.10: "
            ),
            Some("MaBiS")
        );
        assert_eq!(
            doc(
                "crates/mako-gpke/src/a.rs",
                "The window closed. Kap. 3.10: "
            ),
            None
        );
        // The nearer citation wins.
        assert_eq!(
            doc("crates/x/src/a.rs", "MaBiS Kap. 3 and GPKE Teil 1 Kap. 7: "),
            Some("GPKE Teil 1")
        );
        assert_eq!(
            doc(
                "crates/mako-mabis/src/a.rs",
                "Kap. 17.1 and 17.3 continue as the "
            ),
            None,
            "a chapter earlier in the sentence does not claim the quotation"
        );
        assert_eq!(
            doc(
                "crates/mako-mabis/src/a.rs",
                "with Kap. 3.8.3:  | EBD name | Occasion | |---|---| | "
            ),
            None,
            "a citation in another table cell does not attribute this one"
        );
        // An EBD or AHB quotation is not attributed.
        assert_eq!(doc("crates/x/src/a.rs", "E_0622 Prüfschritt 30: "), None);
    }

    #[test]
    fn comment_markers_and_blockquotes_are_not_words() {
        let src =
            "fn x() {}\n/// BilAReM Kap. 6.4.3:\n/// > „Danach dürfen die\n/// > Prozesse\"\n";
        let q = quotations(src, false);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].line, 3);
        assert_eq!(words(&q[0].text), ["danach", "dürfen", "die", "prozesse"]);
    }
}
