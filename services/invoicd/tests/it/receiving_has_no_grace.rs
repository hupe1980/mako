//! Receiving an E-Rechnung has no grace.
//!
//! § 27 Abs. 38 UStG derogates from § 14 Abs. 1 and 2 UStG for the
//! Rechnungsaussteller: each of its graces (Nr. 1–3) lets the issuer send
//! something other than an E-Rechnung, and none is a ground for a recipient to
//! refuse one. So no receiving path of invoicd may consult the transition rule
//! or a turnover figure — an inbound invoice gated on either would be refused
//! for a reason the statute does not give. invoicd issues no invoice, so the
//! rule applies nowhere in it.

use std::path::{Path, PathBuf};

/// Identifiers whose presence means the transition rule or a turnover figure
/// is consulted, each matched as a whole word.
const FORBIDDEN: &[&str] = &[
    "e_rechnung",
    "sonstige_rechnung_zulaessig",
    "edi_zulaessig",
    "SonstigeRechnungFall",
    "EdiFall",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir").flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `word` occurs in `line` bounded by non-identifier characters, so
/// `pruefe_rechnung` does not read as `e_rechnung`.
fn has_word(line: &str, word: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    line.match_indices(word).any(|(i, _)| {
        let before = line[..i].chars().next_back();
        let after = line[i + word.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

#[test]
fn no_receiving_path_consults_the_transition_rule_or_a_turnover() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(!files.is_empty(), "invoicd/src has no Rust files");

    let mut findings = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source");
        for (n, line) in text.lines().enumerate() {
            let hit = FORBIDDEN.iter().any(|w| has_word(line, w))
                || line.to_lowercase().contains("gesamtumsatz");
            if hit {
                findings.push(format!("{}:{}: {}", file.display(), n + 1, line.trim()));
            }
        }
    }
    assert!(
        findings.is_empty(),
        "an invoicd receiving path consults § 27 Abs. 38 UStG or a Gesamtumsatz — the \
         transition rule relieves the issuer, never the recipient, so an inbound \
         E-Rechnung is not gated on it:\n{}",
        findings.join("\n")
    );
}

#[test]
fn the_word_match_does_not_read_pruefe_rechnung_as_the_rule() {
    assert!(!has_word("esa::pruefe_rechnung(", "e_rechnung"));
    assert!(has_word(
        "use mako_fristen::{heute, e_rechnung};",
        "e_rechnung"
    ));
}
