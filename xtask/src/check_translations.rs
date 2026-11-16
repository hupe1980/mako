//! Guard: every translated site page is pinned to the English source it was
//! translated from, and states none of the figures the English pages' guards
//! hold.
//!
//! The site is English; a curated subset is translated (`site/content/**/
//! *.<lang>.md` for each `[languages.<lang>]` of `site/config.toml`). A
//! translation is a copy, and a copy drifts silently: the English page changes,
//! the German one keeps describing what used to be. So every translated page
//! names its source in its front matter —
//!
//! ```toml
//! [extra]
//! translated_from = "content/docs/guide/einrichtung.md"   # relative to site/
//! source_sha256   = "<sha256 of that file when it was translated>"
//! ```
//!
//! — and this guard refuses
//!
//! - a translated page without both keys, whose source does not exist, or
//!   that has no default-language twin (`x.de.md` beside `x.md`), because Zola
//!   pairs the two by file name and the language switch and `hreflang` read
//!   that pairing;
//! - a stale hash: the source changed since the translation, so the page is
//!   re-translated and the hash updated with `--rehash <file>`;
//! - a count of a guarded noun — MCP tools, workflows, Prüfidentifikatoren,
//!   services, specialists, message types, roles. The makod tests and
//!   `check-tool-grants` hold those figures on the English pages; a translated
//!   copy would be a second statement no test reads, so it links to the English
//!   page instead.
//!
//! `cargo xtask check-translations --rehash <file>` writes the current hash of
//! the page's source into its front matter. Run it after re-translating, never
//! instead of it.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Nouns whose counts the English pages' guards hold. A number directly before
/// one — optionally with one word between, as in „183 typisierte MCP-Tools" —
/// is a figure a translation must not restate.
const GUARDED_NOUNS: &[&str] = &[
    "MCP-Tools",
    "MCP-Werkzeuge",
    "Tools",
    "Werkzeuge",
    "Workflows",
    "MaKo-Workflows",
    "Prüfidentifikatoren",
    "PIDs",
    "Dienste",
    "Services",
    "Spezialisten",
    "Nachrichtentypen",
    "EDIFACT-Typen",
    "Rollen",
    "Marktrollen",
];

/// German numerals a count can be written in, besides digits.
const NUMERALS: &[&str] = &[
    "zwei",
    "drei",
    "vier",
    "fünf",
    "sechs",
    "sieben",
    "acht",
    "neun",
    "zehn",
    "elf",
    "zwölf",
    "dreizehn",
    "vierzehn",
    "fünfzehn",
    "sechzehn",
    "siebzehn",
    "achtzehn",
    "neunzehn",
    "zwanzig",
    "dreißig",
    "vierzig",
    "fünfzig",
    "hundert",
];

pub fn run(workspace_root: &Path, args: &[String]) -> bool {
    let site = workspace_root.join("site");
    if let Some(pos) = args.iter().position(|a| a == "--rehash") {
        let Some(file) = args.get(pos + 1) else {
            eprintln!("check-translations: --rehash needs the translated page's path");
            return false;
        };
        return rehash(&site, Path::new(file));
    }
    let langs = match languages(&site) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("check-translations: {e}");
            return false;
        }
    };
    let mut pages = Vec::new();
    collect(&site.join("content"), &langs, &mut pages);
    pages.sort();
    let mut findings = Vec::new();
    for path in &pages {
        let rel = path
            .strip_prefix(&site)
            .unwrap_or(path)
            .display()
            .to_string();
        let Ok(text) = std::fs::read_to_string(path) else {
            findings.push(format!("{rel}: cannot read"));
            continue;
        };
        let twin = twin_path(path, &langs).is_some_and(|t| t.exists());
        findings.extend(check_page(&rel, &text, twin, |src| {
            std::fs::read(site.join(src)).ok()
        }));
    }
    if !findings.is_empty() {
        eprintln!("check-translations: {} finding(s):", findings.len());
        for f in &findings {
            eprintln!("  {f}");
        }
        return false;
    }
    println!(
        "check-translations: {} translated page(s), each current with its English source:",
        pages.len()
    );
    for p in &pages {
        println!("  {}", p.strip_prefix(&site).unwrap_or(p).display());
    }
    true
}

/// The non-default languages `site/config.toml` declares.
fn languages(site: &Path) -> Result<Vec<String>, String> {
    let src = std::fs::read_to_string(site.join("config.toml"))
        .map_err(|e| format!("cannot read site/config.toml: {e}"))?;
    let cfg: toml::Table =
        toml::from_str(&src).map_err(|e| format!("site/config.toml does not parse: {e}"))?;
    let langs: Vec<String> = cfg
        .get("languages")
        .and_then(|l| l.as_table())
        .map(|t| t.keys().cloned().collect())
        .unwrap_or_default();
    if langs.is_empty() {
        return Err("site/config.toml declares no [languages.<lang>]".to_owned());
    }
    Ok(langs)
}

fn collect(dir: &Path, langs: &[String], out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, langs, out);
        } else if twin_path(&path, langs).is_some() {
            out.push(path);
        }
    }
}

/// `x.de.md` → `x.md` when `de` is a declared language; `None` for any other file.
fn twin_path(path: &Path, langs: &[String]) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".md")?;
    let (base, lang) = stem.rsplit_once('.')?;
    langs
        .iter()
        .any(|l| l == lang)
        .then(|| path.with_file_name(format!("{base}.md")))
}

/// The findings for one translated page. `source` reads a file relative to
/// `site/`; `has_twin` says whether the default-language twin exists.
pub fn check_page(
    rel: &str,
    text: &str,
    has_twin: bool,
    source: impl Fn(&str) -> Option<Vec<u8>>,
) -> Vec<String> {
    let mut findings = Vec::new();
    if !has_twin {
        findings.push(format!(
            "{rel}: no default-language twin beside it — Zola pairs a translation with \
             the English page of the same name, and the language switch reads that pairing"
        ));
    }
    let (from, hash) = match pin(text) {
        Ok(p) => p,
        Err(e) => {
            findings.push(format!("{rel}: {e}"));
            (None, None)
        }
    };
    match (from, hash) {
        (Some(from), Some(hash)) => match source(&from) {
            None => findings.push(format!(
                "{rel}: translated_from = \"{from}\" does not exist (the path is relative to site/)"
            )),
            Some(bytes) => {
                let current = sha256_hex(&bytes);
                if current != hash {
                    findings.push(format!(
                        "{rel}: stale — {from} changed since it was translated. Re-translate \
                         the change, then run `cargo xtask check-translations --rehash site/{rel}`"
                    ));
                }
            }
        },
        (None, _) => findings.push(format!(
            "{rel}: no [extra] translated_from — name the English source (relative to site/)"
        )),
        (_, None) => findings.push(format!(
            "{rel}: no [extra] source_sha256 — run `cargo xtask check-translations --rehash \
             site/{rel}` once the translation is current"
        )),
    }
    for (line, figure) in guarded_figures(text) {
        findings.push(format!(
            "{rel}:{line}: states the figure „{figure}\" — the English page's guards hold \
             that number; link to the English page instead"
        ));
    }
    findings
}

type Pin = (Option<String>, Option<String>);

/// `extra.translated_from` and `extra.source_sha256` of the TOML front matter.
fn pin(text: &str) -> Result<Pin, String> {
    let (fm, _) = front_matter(text).ok_or("no +++ front matter")?;
    let table: toml::Table =
        toml::from_str(fm).map_err(|e| format!("front matter does not parse: {e}"))?;
    let extra = table.get("extra").and_then(|e| e.as_table());
    let get = |k: &str| {
        extra
            .and_then(|e| e.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    };
    Ok((get("translated_from"), get("source_sha256")))
}

/// The front matter and the body after it.
fn front_matter(text: &str) -> Option<(&str, &str)> {
    let rest = text.strip_prefix("+++")?;
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let end = rest.find("\n+++")?;
    Some((&rest[..end], &rest[end + 4..]))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Every count of a guarded noun outside fenced code, with its 1-based line.
fn guarded_figures(text: &str) -> Vec<(usize, String)> {
    let nouns = GUARDED_NOUNS
        .iter()
        .map(|n| regex::escape(n))
        .collect::<Vec<_>>()
        .join("|");
    let numerals = NUMERALS.join("|");
    let re = regex::Regex::new(&format!(
        r"(?i)(?:^|[^\p{{L}}\d])((?:\d[\d.]*(?:\s*/\s*\d[\d.]*)?|(?:{numerals}))\s+(?:[\p{{L}}-]+\s+)?(?:{nouns}))(?:$|[^\p{{L}}])"
    ))
    .expect("the figure pattern compiles");
    let mut out = Vec::new();
    let mut fenced = false;
    for (i, line) in text.lines().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        for c in re.captures_iter(line) {
            out.push((i + 1, c[1].to_owned()));
        }
    }
    out
}

/// Write the current hash of the page's source into its front matter.
fn rehash(site: &Path, file: &Path) -> bool {
    let path = if file.is_absolute() {
        file.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(file)
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("check-translations: cannot read {}", path.display());
        return false;
    };
    let from = match pin(&text) {
        Ok((Some(from), _)) => from,
        Ok((None, _)) => {
            eprintln!(
                "check-translations: {} names no [extra] translated_from",
                path.display()
            );
            return false;
        }
        Err(e) => {
            eprintln!("check-translations: {}: {e}", path.display());
            return false;
        }
    };
    let Ok(bytes) = std::fs::read(site.join(&from)) else {
        eprintln!("check-translations: translated_from = \"{from}\" does not exist");
        return false;
    };
    let hash = sha256_hex(&bytes);
    let Some(updated) = set_hash(&text, &hash) else {
        eprintln!(
            "check-translations: {} has no `source_sha256 = \"…\"` line under [extra] to update",
            path.display()
        );
        return false;
    };
    if let Err(e) = std::fs::write(&path, updated) {
        eprintln!("check-translations: cannot write {}: {e}", path.display());
        return false;
    }
    println!(
        "check-translations: {} pinned to {from} at {hash}",
        path.display()
    );
    true
}

/// `text` with its front matter's `source_sha256` line set to `hash`.
fn set_hash(text: &str, hash: &str) -> Option<String> {
    let (fm, _) = front_matter(text)?;
    let start = text.find(fm)?;
    let mut out = String::with_capacity(text.len());
    let mut done = false;
    for line in fm.split_inclusive('\n') {
        if !done && line.trim_start().starts_with("source_sha256") {
            out.push_str(&format!("source_sha256 = \"{hash}\"\n"));
            done = true;
        } else {
            out.push_str(line);
        }
    }
    done.then(|| format!("{}{}{}", &text[..start], out, &text[start + fm.len()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &[u8] = b"English page\n";

    fn page(from: &str, hash: &str, body: &str) -> String {
        format!(
            "+++\ntitle = \"T\"\n\n[extra]\ntranslated_from = \"{from}\"\nsource_sha256 = \"{hash}\"\n+++\n{body}\n"
        )
    }

    fn src(path: &str) -> Option<Vec<u8>> {
        (path == "content/x.md").then(|| SOURCE.to_vec())
    }

    #[test]
    fn a_current_page_passes() {
        let text = page("content/x.md", &sha256_hex(SOURCE), "Hallo, du.");
        assert!(check_page("content/x.de.md", &text, true, src).is_empty());
    }

    #[test]
    fn a_page_without_its_source_is_refused() {
        let text = page("content/gone.md", &sha256_hex(SOURCE), "Hallo.");
        let f = check_page("content/x.de.md", &text, true, src);
        assert!(f.iter().any(|f| f.contains("does not exist")), "{f:?}");

        let bare = "+++\ntitle = \"T\"\n+++\nHallo.\n";
        let f = check_page("content/x.de.md", bare, true, src);
        assert!(
            f.iter().any(|f| f.contains("no [extra] translated_from")),
            "{f:?}"
        );
    }

    #[test]
    fn a_page_without_its_english_twin_is_refused() {
        let text = page("content/x.md", &sha256_hex(SOURCE), "Hallo.");
        let f = check_page("content/y.de.md", &text, false, src);
        assert!(
            f.iter().any(|f| f.contains("no default-language twin")),
            "{f:?}"
        );
    }

    #[test]
    fn a_stale_hash_is_refused_with_its_fix() {
        let text = page("content/x.md", &sha256_hex(b"older English\n"), "Hallo.");
        let f = check_page("content/x.de.md", &text, true, src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("stale"));
        assert!(f[0].contains("cargo xtask check-translations --rehash site/content/x.de.md"));
    }

    #[test]
    fn a_guarded_figure_is_refused() {
        for body in [
            "Die Agenten lesen 183 MCP-Tools.",
            "Es gibt 30 deklarative Spezialisten.",
            "77 Workflows über 498 Prüfidentifikatoren",
            "vierzehn Dienste",
            "482 / 482 Prüfidentifikatoren importiert",
        ] {
            let text = page("content/x.md", &sha256_hex(SOURCE), body);
            let f = check_page("content/x.de.md", &text, true, src);
            assert!(
                f.iter().any(|f| f.contains("states the figure")),
                "{body}: {f:?}"
            );
        }
    }

    #[test]
    fn other_numbers_and_code_pass() {
        let body = "Port 8080, 45 Minuten, PID 55001, zwei Prozesse, 14 von 17 Vorschlägen.\n\
                    ```\n183 MCP-Tools\n```";
        let text = page("content/x.md", &sha256_hex(SOURCE), body);
        assert!(check_page("content/x.de.md", &text, true, src).is_empty());
    }

    #[test]
    fn rehash_rewrites_only_the_hash_line() {
        let text = page("content/x.md", "old", "source_sha256 = \"body stays\"");
        let out = set_hash(&text, "new").expect("has a hash line");
        assert!(out.contains("source_sha256 = \"new\"\n"));
        assert!(out.contains("source_sha256 = \"body stays\""));
        assert_eq!(pin(&out).expect("parses").1.as_deref(), Some("new"));
    }

    #[test]
    fn twins_follow_the_declared_languages() {
        let langs = vec!["de".to_owned()];
        assert_eq!(
            twin_path(Path::new("a/b.de.md"), &langs),
            Some(PathBuf::from("a/b.md"))
        );
        assert_eq!(
            twin_path(Path::new("a/_index.de.md"), &langs),
            Some(PathBuf::from("a/_index.md"))
        );
        assert_eq!(twin_path(Path::new("a/b.md"), &langs), None);
        assert_eq!(twin_path(Path::new("a/b.fr.md"), &langs), None);
    }
}
