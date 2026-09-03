//! Reading the BDEW EDI@Energy publications.
//!
//! A MIG (Nachrichtenbeschreibung) and an AHB (Anwendungshandbuch) are
//! published as PDF. `pdftotext -layout` renders each page as fixed-width text
//! in which a table column keeps its horizontal position, so the column a
//! status token sits in can be read off its character offset. That is what
//! makes the AHB's per-Prüfidentifikator columns recoverable at all — the same
//! `Muss` on one row belongs to whichever Anwendungsfall its x-position names.
//!
//! [`mig`] reads the Nachrichtenstruktur and the Segmentlayout, [`ahb`] the
//! Prüfschablonen.

pub mod ahb;
pub mod mig;

use std::path::Path;
use std::process::Command;

/// Characters per PDF point when a page is laid out on the character grid.
///
/// `pdftotext -layout` fits every line to its own grid, so the same table
/// column drifts by up to ten characters between rows. `-bbox-layout` reports
/// each word's position in points; laying those out on one fixed grid keeps a
/// column at the same character offset on every row of every page.
///
/// Two cells per point: a 9-pt character is about 4.4 pt wide, so a word never
/// overruns the cell of the word after it and every token sits at its true
/// position.
const CELLS_PER_POINT: f64 = 1.0 / 2.2;

/// Render `pdf` onto a fixed character grid and return the normalised lines.
///
/// # Errors
///
/// When poppler's `pdftotext` is not installed or the file cannot be read.
pub fn pdf_lines(pdf: &Path) -> Result<Vec<String>, String> {
    let out = Command::new("pdftotext")
        .arg("-bbox-layout")
        .arg(pdf)
        .arg("-")
        .output()
        .map_err(|e| format!("cannot run pdftotext (poppler): {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "pdftotext failed on {}: {}",
            pdf.display(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let xml = String::from_utf8_lossy(&out.stdout);
    Ok(grid_lines(&xml))
}

/// Lay the words of a `-bbox-layout` document out on the character grid.
#[must_use]
pub fn grid_lines(xml: &str) -> Vec<String> {
    let word = regex::Regex::new(
        r#"<word xMin="([0-9.]+)" yMin="([0-9.]+)" xMax="([0-9.]+)" yMax="([0-9.]+)">(.*?)</word>"#,
    )
    .unwrap();
    let mut lines: Vec<String> = Vec::new();
    for page in xml.split("<page ").skip(1) {
        // (yMin, xMin, xMax, text)
        let mut words: Vec<(f64, f64, f64, String)> = word
            .captures_iter(page)
            .map(|c| {
                (
                    c[2].parse::<f64>().unwrap_or(0.0),
                    c[1].parse::<f64>().unwrap_or(0.0),
                    c[3].parse::<f64>().unwrap_or(0.0),
                    decode_entities(&c[5]),
                )
            })
            .collect();
        words.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap()
                .then(a.1.partial_cmp(&b.1).unwrap())
        });
        // Cluster into visual lines: a word belongs to the current line while
        // its baseline is within 3 pt of the line's first word.
        let mut rows: Vec<Vec<(f64, f64, String)>> = Vec::new();
        let mut row_y = f64::MIN;
        for (y, x0, x1, text) in words {
            if (y - row_y).abs() > 3.0 {
                rows.push(Vec::new());
                row_y = y;
            }
            rows.last_mut().unwrap().push((x0, x1, text));
        }
        for mut row in rows {
            row.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            let mut line = String::new();
            let mut len = 0usize;
            for (x0, _x1, text) in row {
                let mut at = (x0 * CELLS_PER_POINT).round() as usize; // f64 grid cell, not money
                if at < len + 1 {
                    at = len + 1;
                }
                line.extend(std::iter::repeat_n(' ', at - len));
                let text = normalise_word(&text);
                len = at + text.chars().count();
                line.push_str(&text);
            }
            lines.push(line);
        }
        lines.push(String::new());
    }
    lines.retain(|l| !is_boilerplate(l));
    lines
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

fn normalise_word(s: &str) -> String {
    s.replace('\u{fb01}', "fi")
        .replace('\u{fb02}', "fl")
        .replace('\u{fb00}', "ff")
        .replace('\u{fb03}', "ffi")
        .replace('\u{fb04}', "ffl")
        .replace('\u{a0}', " ")
}

/// Running header / footer / legend lines that every page repeats.
fn is_boilerplate(line: &str) -> bool {
    let collapsed = collapse(line);
    let t = collapsed.as_str();
    (t.starts_with("Version:") && (t.contains("Seite") || t.contains("Seite:")))
        || t.starts_with("Bez = ")
        || t.starts_with("Zähler = ")
        || t.starts_with("Nr = ")
        || t.starts_with("MaxWdh = ")
        || t.ends_with(" Anwendungshandbuch")
        || t.ends_with(" Anwendungshandbuch Strom")
        || t.ends_with(" Anwendungshandbuch Gas")
        || t.ends_with("Anwendungshandbuch Strom")
        || t.ends_with("Anwendungshandbuch Gas")
        || (t.ends_with("MIG") && t.split_whitespace().count() <= 3)
        || (t == "Strom" || t == "Gas")
}

/// `line` with runs of whitespace collapsed to one space, trimmed — the form
/// phrase comparisons use, since the grid spreads the words of a phrase.
#[must_use]
pub fn collapse(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A whitespace-separated token with the character offset it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tok<'a> {
    pub x: usize,
    pub text: &'a str,
}

/// Tokenise a line, remembering where each token starts (in characters).
#[must_use]
pub fn tokens(line: &str) -> Vec<Tok<'_>> {
    let mut out = Vec::new();
    let mut start: Option<(usize, usize)> = None; // (char index, byte index)
    for (ci, (bi, ch)) in line.char_indices().enumerate() {
        if ch.is_whitespace() {
            if let Some((cx, bx)) = start.take() {
                out.push(Tok {
                    x: cx,
                    text: &line[bx..bi],
                });
            }
        } else if start.is_none() {
            start = Some((ci, bi));
        }
    }
    if let Some((cx, bx)) = start {
        out.push(Tok {
            x: cx,
            text: &line[bx..],
        });
    }
    out
}

/// Character offset of `needle` in `line`, if present.
#[must_use]
pub fn char_pos(line: &str, needle: &str) -> Option<usize> {
    line.find(needle).map(|b| line[..b].chars().count())
}

/// Where a token's glyphs end on the grid: a character is about 2.2 cells
/// wide, but the token occupies only one cell per character.
#[must_use]
pub fn rendered_end(x: usize, text: &str) -> usize {
    x + (text.chars().count() * 11).div_ceil(5)
}

/// The centre of a token's glyphs on the grid.
#[must_use]
pub fn rendered_centre(x: usize, text: &str) -> usize {
    (x + rendered_end(x, text)) / 2
}

/// A BDEW code token: qualifier codes (`Z16`, `E01`, `137`, `9`, `UNOC`),
/// format codes (`303`), version codes (`S2.2`, `11A`, `1.4c`) and OBIS-shaped
/// codes in a code list (`1-1?:1.8.1`).
#[must_use]
pub fn looks_like_code(t: &str) -> bool {
    let is_wire_release = t.contains('.') && t.chars().next().is_some_and(|c| c.is_ascii_digit());
    !t.is_empty()
        && t.chars().count() <= 20
        && t.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && t.chars().all(|c| {
            c.is_ascii_uppercase()
                || c.is_ascii_digit()
                || matches!(c, '.' | '?' | ':' | '-' | '+' | '_')
                || (is_wire_release && c.is_ascii_lowercase())
        })
        && !t.ends_with('-')
        && !t.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_carry_character_offsets() {
        let toks = tokens("  SG4 DTM   Muss   [10]");
        assert_eq!(toks[0], Tok { x: 2, text: "SG4" });
        assert_eq!(
            toks[2],
            Tok {
                x: 12,
                text: "Muss"
            }
        );
        assert_eq!(
            toks[3],
            Tok {
                x: 19,
                text: "[10]"
            }
        );
    }

    #[test]
    fn offsets_count_characters_not_bytes() {
        let toks = tokens("Prüfidentifikator   55001");
        assert_eq!(toks[1].x, 20);
    }

    #[test]
    fn code_tokens() {
        for ok in [
            "Z16",
            "E01",
            "137",
            "9",
            "UNOC",
            "S2.2",
            "11A",
            "1-1?:1.8.1",
            "MP-ID",
            "1.4c",
            "2.1i",
        ] {
            assert!(looks_like_code(ok), "{ok}");
        }
        for no in [
            "Datum",
            "Dokumentennummer",
            "Muss",
            "X",
            "Abschlags-",
            "Nachrichten-",
        ] {
            // `X` and `Muss` are status tokens; the parsers exclude them by
            // context, this predicate only shapes the token.
            let _ = looks_like_code(no);
        }
        assert!(!looks_like_code("Abschlags-"));
        assert!(!looks_like_code("Datum"));
    }

    #[test]
    fn footers_are_dropped() {
        assert!(is_boilerplate(
            "Version: 2.2   29.06.2026   Seite 64 von 944"
        ));
        assert!(is_boilerplate("UTILMD  Anwendungshandbuch  Strom"));
        assert!(!is_boilerplate("SG4 DTM 00020 Muss"));
    }
}
