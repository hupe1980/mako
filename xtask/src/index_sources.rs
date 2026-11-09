//! `cargo xtask index-sources` — the passage index agentd quotes rules from.
//!
//! A model states a rule only as a quote, and it quotes by **address** —
//! document, edition, chapter or paragraph — never by similarity: „A02" means
//! four different refusals in four Entscheidungsbäume, and five format releases
//! bind at once, so the nearest passage is the plausible wrong one. This task
//! cuts the operator's own `regulatories/` mirror into addressed passages and
//! writes them to [`INDEX`], which agentd's `quote_source` and `find_source`
//! read. The file lies inside the gitignored mirror: it is copyrighted text,
//! built on the operator's machine and never shipped.
//!
//! ## What is indexed
//!
//! - Every EDI@Energy document of `regulatories/bdew-mako/manifest.json` whose
//!   bytes still hash to the manifest's SHA-256, addressed by the document and
//!   edition its title names (`APERAK AHB` / `1.1`), one passage per chapter.
//!   A chapter is an entry of the document's own Inhaltsverzeichnis, found
//!   again in the body in the same order, so a footnote or table row that
//!   happens to start with a number is never taken for a heading.
//! - The consolidated statutes of [`STATUTES`] as gesetze-im-internet.de
//!   publishes them in XML, one passage per § and one per Absatz
//!   (`§ 41f`, `§ 41f Abs. 5`), the edition being the publication's build
//!   date. `--download` fetches them into `regulatories/gesetze/`.
//!
//! A document whose bytes differ from its manifest hash is left out and named:
//! a passage keyed to a hash its text did not come from would be a false
//! citation. A run that finds nothing to index writes nothing and fails, so an
//! empty index cannot stand in for a built one.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use sha2::{Digest as _, Sha256};

/// Where the index is written, under the workspace root.
pub const INDEX: &str = "regulatories/quellen-index.json";

/// A consolidated statute and where gesetze-im-internet.de publishes it.
struct Statute {
    /// The amtliche Abkürzung a citation names it by.
    doc: &'static str,
    /// The XML download.
    url: &'static str,
    /// The mirrored XML, under the workspace root.
    file: &'static str,
}

const STATUTES: &[Statute] = &[
    Statute {
        doc: "EnWG",
        url: "https://www.gesetze-im-internet.de/enwg_2005/xml.zip",
        file: "regulatories/gesetze/enwg_2005.xml",
    },
    Statute {
        doc: "MsbG",
        url: "https://www.gesetze-im-internet.de/messbg/xml.zip",
        file: "regulatories/gesetze/messbg.xml",
    },
];

#[derive(Serialize)]
struct Index {
    #[serde(rename = "_note")]
    note: &'static str,
    dokumente: Vec<Dokument>,
}

/// One document under one address.
#[derive(Serialize, Debug)]
pub struct Dokument {
    pub doc: String,
    pub edition: String,
    /// The file, relative to `regulatories/`.
    pub datei: String,
    pub sha256: String,
    pub stellen: Vec<Stelle>,
}

/// One addressed passage.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Stelle {
    /// `2.4.1`, `§ 41f`, `§ 41f Abs. 5`.
    pub at: String,
    /// The 1-based PDF page the passage starts on; none for a statute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seite: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub titel: Option<String>,
    pub text: String,
}

/// Build the index. `download` first fetches the statutes.
pub fn run(root: &Path, args: &[String]) -> bool {
    let download = args.iter().any(|a| a == "--download");
    if let Some(other) = args.iter().find(|a| a.as_str() != "--download") {
        eprintln!("index-sources: unknown argument `{other}` (only --download)");
        return false;
    }
    if download && !download_statutes(root) {
        return false;
    }

    let mut dokumente = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    bdew_documents(root, &mut dokumente, &mut skipped);
    for s in STATUTES {
        let path = root.join(s.file);
        let Ok(bytes) = std::fs::read(&path) else {
            skipped.push(format!(
                "{}: not mirrored at {} (run with --download)",
                s.doc, s.file
            ));
            continue;
        };
        let xml = String::from_utf8_lossy(&bytes);
        let Some((edition, stellen)) = statute_passages(&xml) else {
            skipped.push(format!(
                "{}: {} is not a gesetze-im-internet XML",
                s.doc, s.file
            ));
            continue;
        };
        dokumente.push(Dokument {
            doc: s.doc.to_owned(),
            edition,
            datei: s.file.trim_start_matches("regulatories/").to_owned(),
            sha256: hex(&Sha256::digest(&bytes)),
            stellen,
        });
    }

    for s in &skipped {
        eprintln!("index-sources: not indexed — {s}");
    }
    let passages: usize = dokumente.iter().map(|d| d.stellen.len()).sum();
    if dokumente.is_empty() || passages == 0 {
        eprintln!(
            "index-sources: nothing to index — the mirror is absent or unreadable. Run \
             `cargo xtask sync-regulatories --download` and `cargo xtask index-sources \
             --download`; no index was written"
        );
        return false;
    }
    let index = Index {
        note: "Generated by `cargo xtask index-sources` from the local regulatories/ mirror. \
               Copyrighted text: never commit or ship it.",
        dokumente,
    };
    let path = root.join(INDEX);
    let json = match serde_json::to_vec(&index) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("index-sources: serialise the index: {e}");
            return false;
        }
    };
    if let Err(e) = std::fs::write(&path, json) {
        eprintln!("index-sources: write {}: {e}", path.display());
        return false;
    }
    println!(
        "index-sources: {} document(s), {passages} passage(s) → {INDEX}; {} not indexed",
        index.dokumente.len(),
        skipped.len()
    );
    true
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn download_statutes(root: &Path) -> bool {
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .user_agent("mako-xtask/index-sources (+https://github.com/hupe1980/mako)")
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("index-sources: build the HTTP client: {e}");
            return false;
        }
    };
    for s in STATUTES {
        let body = match client
            .get(s.url)
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .and_then(reqwest::blocking::Response::bytes)
        {
            Ok(b) => b,
            Err(e) => {
                eprintln!("index-sources: fetch {}: {e}", s.url);
                return false;
            }
        };
        let Some(xml) = first_xml_in_zip(&body) else {
            eprintln!("index-sources: {} carries no XML file", s.url);
            return false;
        };
        let path = root.join(s.file);
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("index-sources: create {}: {e}", dir.display());
            return false;
        }
        if let Err(e) = std::fs::write(&path, xml) {
            eprintln!("index-sources: write {}: {e}", path.display());
            return false;
        }
        println!("index-sources: mirrored {} → {}", s.doc, s.file);
    }
    true
}

fn first_xml_in_zip(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).ok()?;
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).ok()?;
        if f.name().is_ok_and(|n| n.ends_with(".xml")) {
            let mut out = Vec::new();
            f.read_to_end(&mut out).ok()?;
            return Some(out);
        }
    }
    None
}

// ── EDI@Energy documents ─────────────────────────────────────────────────────

fn bdew_documents(root: &Path, out: &mut Vec<Dokument>, skipped: &mut Vec<String>) {
    let dir = root.join("regulatories/bdew-mako");
    let Ok(raw) = std::fs::read_to_string(dir.join("manifest.json")) else {
        skipped.push("EDI@Energy documents: regulatories/bdew-mako/manifest.json absent".into());
        return;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&raw) else {
        skipped.push("EDI@Energy documents: the manifest is not JSON".into());
        return;
    };
    if std::process::Command::new("pdftotext")
        .arg("-v")
        .output()
        .is_err()
    {
        skipped.push(
            "EDI@Energy documents: `pdftotext` (poppler) is not on PATH, so no PDF can be read"
                .into(),
        );
        return;
    }
    let mut files: Vec<Pending> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (name, entry) in manifest["files"].as_object().into_iter().flatten() {
        if !name.ends_with(".pdf") {
            continue;
        }
        let title = entry["title"].as_str().unwrap_or_default();
        let Some((doc, edition)) = address_of_title(title) else {
            skipped.push(format!(
                "{name}: no document and edition in the title „{title}“"
            ));
            continue;
        };
        if !seen.insert((doc.clone(), edition.clone())) {
            skipped.push(format!(
                "{name}: {doc} {edition} is already indexed from another file"
            ));
            continue;
        }
        let path = dir.join(name);
        if !path.exists() {
            skipped.push(format!("{name}: not mirrored"));
            continue;
        }
        let expected = entry["sha256"].as_str().unwrap_or_default().to_owned();
        files.push(Pending {
            name: name.clone(),
            path,
            doc,
            edition,
            expected,
        });
    }

    // pdftotext dominates; a handful of threads keeps a full mirror to minutes.
    let results: Vec<Result<Dokument, String>> = std::thread::scope(|scope| {
        let chunks: Vec<_> = files.chunks(files.len().div_ceil(8).max(1)).collect();
        let handles: Vec<_> = chunks
            .into_iter()
            .map(|chunk| scope.spawn(move || chunk.iter().map(read_pdf).collect::<Vec<_>>()))
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for r in results {
        match r {
            Ok(d) => out.push(d),
            Err(e) => skipped.push(e),
        }
    }
}

/// A mirrored PDF waiting to be read.
struct Pending {
    name: String,
    path: PathBuf,
    doc: String,
    edition: String,
    /// The manifest's SHA-256.
    expected: String,
}

fn read_pdf(p: &Pending) -> Result<Dokument, String> {
    let Pending {
        name,
        path,
        doc,
        edition,
        expected,
    } = p;
    let bytes = std::fs::read(path).map_err(|e| format!("{name}: {e}"))?;
    let actual = hex(&Sha256::digest(&bytes));
    if actual != *expected {
        return Err(format!(
            "{name}: its bytes no longer hash to the manifest's SHA-256 — run \
             `cargo xtask sync-regulatories`"
        ));
    }
    let out = std::process::Command::new("pdftotext")
        .args(["-enc", "UTF-8"])
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| format!("{name}: pdftotext: {e}"))?;
    if !out.status.success() {
        return Err(format!("{name}: pdftotext failed"));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let pages: Vec<&str> = text.split('\u{c}').collect();
    let stellen = chapter_passages(&pages);
    if stellen.is_empty() {
        return Err(format!(
            "{name}: no Inhaltsverzeichnis chapter found in the body"
        ));
    }
    Ok(Dokument {
        doc: doc.clone(),
        edition: edition.clone(),
        datei: format!("bdew-mako/{name}"),
        sha256: actual,
        stellen,
    })
}

/// The document and edition a BDEW catalogue title names.
///
/// `APERAK AHB 1.1` → (`APERAK AHB`, `1.1`); `UTILMD MIG Gas G1.2` →
/// (`UTILMD MIG Gas`, `G1.2`); a consolidated reading edition keeps its
/// Stand, so it is an address of its own and never the edition it corrects:
/// `APERAK AHB 1.0 - konsolidierte Lesefassung … Stand: 30.09.2025` →
/// (`APERAK AHB`, `1.0 Lesefassung 30.09.2025`). An Anwendungshilfe names its
/// version in parentheses.
#[must_use]
pub fn address_of_title(title: &str) -> Option<(String, String)> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let stand = Regex::new(r"Stand:? (\d{2}\.\d{2}\.\d{4})").expect("stand");
    let stand_of = |t: &str| stand.captures(t).map(|c| c[1].to_owned());
    let awh = Regex::new(r"^(.+?) \(Version ([^)]+)\)").expect("awh");
    if let Some(c) = awh.captures(&title) {
        let edition = match stand_of(&title) {
            Some(s) => format!("{} {s}", &c[2]),
            None => c[2].to_owned(),
        };
        return Some((c[1].to_owned(), edition));
    }
    let versioned = Regex::new(r"^(.+?) ([A-Z]?\d+\.\d+[a-z]?)(?: - (.*))?$").expect("versioned");
    let c = versioned.captures(&title)?;
    let mut edition = c[2].to_owned();
    if let Some(rest) = c.get(3) {
        if rest.as_str().contains("Lesefassung") {
            edition.push_str(" Lesefassung");
        }
        if let Some(s) = stand_of(rest.as_str()) {
            edition.push(' ');
            edition.push_str(&s);
        }
    }
    Some((c[1].to_owned(), edition))
}

/// One passage per chapter of the document's Inhaltsverzeichnis, found again
/// in the body in the same order.
#[must_use]
pub fn chapter_passages(pages: &[&str]) -> Vec<Stelle> {
    let leader = Regex::new(r"\.{5,}\s*\d+\s*$").expect("leader");
    let number = Regex::new(r"^(\d{1,2}(?:\.\d{1,3}){0,5})(?:\s+(.*))?$").expect("number");

    let lines: Vec<(usize, &str)> = pages
        .iter()
        .enumerate()
        .flat_map(|(p, text)| text.lines().map(move |l| (p, l.trim())))
        .collect();
    let toc_pages: std::collections::BTreeSet<usize> = pages
        .iter()
        .enumerate()
        .filter(|(_, t)| t.lines().filter(|l| leader.is_match(l)).count() >= 3)
        .map(|(p, _)| p)
        .collect();
    let Some(&last_toc) = toc_pages.iter().next_back() else {
        return Vec::new();
    };

    // The Inhaltsverzeichnis: each number with its title, which sits on the
    // number's own line or the next non-empty one.
    let mut entries: Vec<(String, String)> = Vec::new();
    for (i, (p, line)) in lines.iter().enumerate() {
        if !toc_pages.contains(p) {
            continue;
        }
        let Some(c) = number.captures(line) else {
            continue;
        };
        let num = c[1].to_owned();
        let title = c
            .get(2)
            .map(|t| t.as_str().to_owned())
            .filter(|t| !t.is_empty())
            .or_else(|| {
                lines[i + 1..]
                    .iter()
                    .map(|(_, l)| *l)
                    .find(|l| !l.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let title = leader
            .replace(&title, "")
            .trim_end_matches('.')
            .trim()
            .to_owned();
        if !entries.iter().any(|(n, _)| *n == num) {
            entries.push((num, title));
        }
    }

    // The body, searched in Inhaltsverzeichnis order.
    let body_start = lines
        .iter()
        .position(|(p, _)| *p > last_toc)
        .unwrap_or(lines.len());
    let mut cursor = body_start;
    let mut anchors: Vec<(usize, &str, &str)> = Vec::new();
    for (num, title) in &entries {
        let keyword = title.split_whitespace().next().unwrap_or_default();
        // A heading's title follows its number on the same line, or — where
        // the number stands alone — opens the next non-empty line.
        let found = (cursor..lines.len()).find(|&i| {
            let l = lines[i].1;
            let title_line = if l == num.as_str() {
                lines[i + 1..]
                    .iter()
                    .map(|(_, l)| *l)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default()
            } else if let Some(rest) = l
                .strip_prefix(num.as_str())
                .filter(|rest| rest.starts_with(char::is_whitespace))
            {
                rest.trim_start()
            } else {
                return false;
            };
            title_line.starts_with(keyword)
        });
        if let Some(i) = found {
            anchors.push((i, num, title));
            cursor = i + 1;
        }
    }

    anchors
        .iter()
        .enumerate()
        .map(|(k, (start, num, title))| {
            let end = anchors.get(k + 1).map_or(lines.len(), |(e, _, _)| *e);
            let mut text = String::new();
            let mut blank = false;
            for (_, l) in &lines[*start..end] {
                if l.is_empty() {
                    blank = true;
                    continue;
                }
                if !text.is_empty() {
                    text.push(if blank { '\n' } else { ' ' });
                }
                blank = false;
                text.push_str(l);
            }
            Stelle {
                at: (*num).to_owned(),
                seite: u32::try_from(lines[*start].0 + 1).ok(),
                titel: Some((*title).to_owned()).filter(|t| !t.is_empty()),
                text,
            }
        })
        .collect()
}

// ── Statutes ─────────────────────────────────────────────────────────────────

/// The edition and the passages of a gesetze-im-internet.de XML: one per §
/// and one per Absatz.
#[must_use]
pub fn statute_passages(xml: &str) -> Option<(String, Vec<Stelle>)> {
    let build = Regex::new(r#"<dokumente[^>]*builddate="(\d{4})(\d{2})(\d{2})"#).expect("build");
    let c = build.captures(xml)?;
    let edition = format!("Stand {}-{}-{}", &c[1], &c[2], &c[3]);

    let norm = Regex::new(r"(?s)<norm\b.*?</norm>").expect("norm");
    let enbez = Regex::new(r"<enbez>(.*?)</enbez>").expect("enbez");
    let titel = Regex::new(r"(?s)<titel[^>]*>(.*?)</titel>").expect("titel");
    let content =
        Regex::new(r"(?s)<textdaten>\s*<text[^>]*>\s*<Content>(.*?)</Content>").expect("content");
    let para = Regex::new(r"(?s)<P>(.*?)</P>").expect("para");
    let absatz = Regex::new(r"^\((\d+[a-z]?)\)").expect("absatz");

    let mut out = Vec::new();
    for n in norm.find_iter(xml) {
        let n = n.as_str();
        let Some(bez) = enbez.captures(n).map(|c| plain(&c[1])) else {
            continue;
        };
        let Some(num) = bez.strip_prefix("§ ") else {
            continue;
        };
        let Some(body) = content.captures(n) else {
            continue;
        };
        let at = format!("§ {}", num.trim());
        let title = titel.captures(n).map(|c| plain(&c[1]));
        let paras: Vec<String> = para.captures_iter(&body[1]).map(|c| plain(&c[1])).collect();
        if paras.is_empty() {
            continue;
        }
        for p in &paras {
            if let Some(a) = absatz.captures(p) {
                out.push(Stelle {
                    at: format!("{at} Abs. {}", &a[1]),
                    seite: None,
                    titel: title.clone(),
                    text: p.clone(),
                });
            }
        }
        out.push(Stelle {
            at,
            seite: None,
            titel: title,
            text: paras.join("\n"),
        });
    }
    Some((edition, out))
}

/// Markup to text: tags become spaces, entities their characters, runs of
/// whitespace one space.
fn plain(markup: &str) -> String {
    let tags = Regex::new(r"<[^>]+>").expect("tags");
    let text = tags.replace_all(markup, " ");
    let entity = Regex::new(r"&(#x[0-9a-fA-F]+|#\d+|amp|lt|gt|quot|apos);").expect("entity");
    let text = entity.replace_all(&text, |c: &regex::Captures<'_>| {
        let e = &c[1];
        match e {
            "amp" => "&".to_owned(),
            "lt" => "<".to_owned(),
            "gt" => ">".to_owned(),
            "quot" => "\"".to_owned(),
            "apos" => "'".to_owned(),
            _ => e
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| e.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_default(),
        }
    });
    let mut s = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // A tag between a word and its punctuation leaves a space before it.
    for p in [" .", " ,", " ;", " :"] {
        s = s.replace(p, &p[1..]);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aperak_ahb_1_0_and_1_1_are_separate_addresses() {
        assert_eq!(
            address_of_title("APERAK AHB 1.1"),
            Some(("APERAK AHB".into(), "1.1".into()))
        );
        assert_eq!(
            address_of_title("APERAK AHB 1.0"),
            Some(("APERAK AHB".into(), "1.0".into()))
        );
        assert_eq!(
            address_of_title(
                "APERAK AHB 1.0 - konsolidierte Lesefassung mit Fehlerkorrekturen Stand: 30.09.2025"
            ),
            Some(("APERAK AHB".into(), "1.0 Lesefassung 30.09.2025".into())),
            "a reading edition is never the edition it corrects"
        );
        assert_eq!(
            address_of_title("UTILMD MIG Gas G1.2"),
            Some(("UTILMD MIG Gas".into(), "G1.2".into()))
        );
        assert_eq!(
            address_of_title(
                "BDEW-Anwendungshilfe - Darstellung von Lokationsbündeln  (Version 1.0) Stand: 18.08.2026"
            ),
            Some((
                "BDEW-Anwendungshilfe - Darstellung von Lokationsbündeln".into(),
                "1.0 18.08.2026".into()
            ))
        );
    }

    const TOC: &str = "Inhaltsverzeichnis\n1\n\nGrundlegende Regelungen ........ 5\n\
        1.1 Abgrenzung ............ 5\n2\n\nEinsatz der APERAK ........ 9\n\
        2.4.1\n\nFristen Strom ........ 23\n";

    #[test]
    fn a_chapter_is_a_toc_entry_found_again_in_the_body() {
        let body1 = "1\n\nGrundlegende Regelungen\n1.1 Abgrenzung\nDer Text von 1.1.\n\
            2 Eine Fußnote, die mit einer Zahl beginnt.";
        let body2 = "2\n\nEinsatz der APERAK\nEinleitung.\n2.4.1\n\nFristen Strom\n\
            Der Empfänger teilt binnen 45 Minuten mit.";
        let stellen = chapter_passages(&[TOC, body1, body2]);
        let at: Vec<(&str, Option<u32>)> =
            stellen.iter().map(|s| (s.at.as_str(), s.seite)).collect();
        assert_eq!(
            at,
            vec![
                ("1", Some(2)),
                ("1.1", Some(2)),
                ("2", Some(3)),
                ("2.4.1", Some(3))
            ],
            "the footnote „2 Eine Fußnote“ is no heading: its line lacks the TOC title"
        );
        let s = stellen.iter().find(|s| s.at == "2.4.1").expect("2.4.1");
        assert!(s.text.contains("45 Minuten"), "{s:?}");
        assert_eq!(s.titel.as_deref(), Some("Fristen Strom"));
        let s = stellen.iter().find(|s| s.at == "1.1").expect("1.1");
        assert!(
            !s.text.contains("Einsatz"),
            "a passage ends at the next chapter: {s:?}"
        );
    }

    #[test]
    fn a_document_without_an_inhaltsverzeichnis_has_no_chapters() {
        assert!(chapter_passages(&["1 Erste Zeile\n2 Zweite"]).is_empty());
    }

    #[test]
    fn a_statute_is_addressed_by_paragraph_and_absatz() {
        let xml = r#"<?xml version="1.0"?><dokumente builddate="20261022215507" doknr="X">
            <norm><metadaten><enbez>Inhaltsübersicht</enbez></metadaten></norm>
            <norm><metadaten><jurabk>EnWG</jurabk><enbez>§ 41f</enbez><titel format="XML">Versorgungsunterbrechungen</titel></metadaten>
            <textdaten><text format="XML"><Content><P>(1) Bei der Nichterfüllung &amp; Mahnung <DL><DT>1.</DT><DD><LA>erstens</LA></DD></DL></P>
            <P>(5) Der Beginn der Unterbrechung der Energieversorgung ist dem Haushaltskunden acht Werktage im Voraus durch briefliche Mitteilung anzukündigen.</P></Content></text></textdaten></norm>
            </dokumente>"#;
        let (edition, stellen) = statute_passages(xml).expect("a statute");
        assert_eq!(edition, "Stand 2026-10-22");
        let at: Vec<&str> = stellen.iter().map(|s| s.at.as_str()).collect();
        assert_eq!(at, vec!["§ 41f Abs. 1", "§ 41f Abs. 5", "§ 41f"]);
        assert_eq!(
            stellen[1].text,
            "(5) Der Beginn der Unterbrechung der Energieversorgung ist dem Haushaltskunden \
             acht Werktage im Voraus durch briefliche Mitteilung anzukündigen."
        );
        assert_eq!(
            stellen[0].text,
            "(1) Bei der Nichterfüllung & Mahnung 1. erstens"
        );
    }
}
