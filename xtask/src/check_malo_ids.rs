//! Guard: every identifier literal carries a valid check digit.
//!
//! A Marktlokations-ID is eleven digits, and the eleventh is a check digit
//! defined by the BDEW Anwendungshilfe ("Lok- und Waggon-Kennzeichnungsverfahren"):
//!
//! ```text
//! sum = Σ digits at odd positions (1,3,5,7,9)
//!     + Σ digits at even positions (2,4,6,8,10) × 2
//! check = (10 − sum mod 10) mod 10
//! ```
//!
//! The first digit is the Codevergabestelle and is never `0`.
//!
//! ## Why a guard and not a code review
//!
//! `metering::MaloId` and `meterstore::encode::parse_malo` validate this at the
//! parse, so a fixture, demo payload or documentation example carrying a
//! mistyped ID is no longer merely untidy — it is a value the storage layer
//! *refuses*. The failure surfaces at a store boundary, far from the file that
//! declares it, and usually in whichever environment first runs against a real
//! database.
//!
//! Checking literals where they are written turns that into a build failure
//! naming the file, the ID and the digit it should have carried.
//!
//! ## EIC codes, and their object type
//!
//! The same argument covers the 16-character EIC codes that name a Bilanzkreis
//! and a Bilanzierungsgebiet, and there are **two** ways to get one wrong:
//!
//! * the sixteenth character is a check character, which `rubo4e` validates;
//! * the third character is the ENTSO-E **object type**, and it is what
//!   separates the two identifiers — a Bilanzkreis is a **Party** (`X`), held
//!   by a Bilanzkreisverantwortlicher, while a Bilanzierungsgebiet is an
//!   **Area** (`Y`). Germany issues them on that basis (EIC functions *Balance
//!   Group* and *Metering Grid Area*, Energie Codes und Services).
//!
//! Both are sixteen characters and both can carry a valid check character, so
//! nothing but the type separates them — and MSCONS SG6 carries both as free
//! text under different `LOC` qualifiers. A series filed against the wrong one
//! is a misfiling the BIKO cannot tell from a correct submission. So a literal
//! on a line that *names* one identifier and carries the other is reported.
//!
//! Validation uses `rubo4e` itself rather than a second implementation, so the
//! guard can never disagree with the code that matters.
//!
//! ## What counts as a MaLo literal
//!
//! An eleven-digit run — not part of a longer digit sequence, so the digits
//! inside a 33-character MeLo are never examined — on a line that also mentions
//! a MaLo (`malo`, `marktlokation`, `location_id`), or on the line directly
//! below one that does. Deliberately narrow: the repository is full of other
//! eleven-digit numbers (timestamps, Zählernummern, BDEW Codenummern), and a
//! guard that flagged those would be turned off rather than fixed.

use std::path::{Path, PathBuf};

/// One flagged literal, with the sentence explaining it.
struct Finding {
    path: PathBuf,
    line: usize,
    message: String,
}

/// IDs that are *meant* to be invalid, with the reason.
///
/// A test that proves a malformed identifier is refused needs a malformed
/// identifier. Each entry is a deliberate negative, not an oversight. The
/// allowlist covers both families this guard checks — MaLo-IDs and EIC codes.
const DELIBERATE: &[(&str, &str)] = &[
    (
        "11XRWENET-----1X",
        "`agentd` verifies that a malformed EIC cannot become a trusted routing \
         authority field; the valid neighbouring fixture ends in `E`",
    ),
    (
        "51238696782",
        "the refusal fixture: `crates/mako-markt`, `crates/energy-api` and \
         `services/productd` assert that a wrong check digit is rejected. It fails \
         the BDEW Anwendungshilfe arithmetic, and also failed the Luhn variant a \
         dependency briefly used — so the assertion states the rule rather than \
         pinning one implementation's behaviour",
    ),
    (
        "41373559242",
        "`makotest` pins its MaLo binding to the BDEW §8.1 worked example \
         (`4137355924` → `41373559241`) and asserts the neighbouring digit is \
         refused. Without the negative, the test would pass against a binding \
         that accepted anything",
    ),
    (
        "51238297069",
        "`mako-emob` pins `VirtualMaloId`'s refusal to the **shape** rather than \
         the arithmetic: eleven digits are refused whether or not the check \
         digit holds, because a Netzbetreiber issuing the valid neighbour of a \
         minted id collides just as hard. The test asserts both spellings",
    ),
    (
        "10YDE-EON------2",
        "`makotest` pins its EIC binding to the ENTSO-E worked example \
         (`10YDE-EON------` → `…1`) and asserts the neighbouring check character \
         is refused",
    ),
];

/// Every allowlisted identifier must still *fail* its check digit or character.
///
/// The allowlist exists so a fixture that is invalid on purpose can stay in the
/// tree. That only works while it really is invalid — and "invalid" is a
/// property of an arithmetic that has already been corrected under us once. An
/// entry that quietly became valid would silently exempt a real identifier from
/// every check this guard makes, and the tests asserting rejection would be
/// asserting nothing.
fn deliberate_entries_that_are_actually_valid() -> Vec<Finding> {
    DELIBERATE
        .iter()
        .filter(|(id, _)| is_actually_valid(id))
        .map(|(id, reason)| Finding {
            path: PathBuf::from("xtask/src/check_malo_ids.rs"),
            line: 0,
            message: format!(
                "{id} is allowlisted as deliberately invalid but it validates, so \
                 every test relying on it to be refused now asserts nothing. \
                 Allowlist reason given: {reason}"
            ),
        })
        .collect()
}

/// `true` when an allowlisted literal is in fact a valid identifier.
fn is_actually_valid(id: &str) -> bool {
    match id.len() {
        11 => check_digit(&id[..10]).is_some_and(|c| id.ends_with(c)),
        16 => rubo4e::identifiers::EicCode::new(id).is_ok(),
        _ => false,
    }
}

/// The BDEW check digit for the first ten digits, or `None` if `d10` is not ten
/// ASCII digits.
fn check_digit(d10: &str) -> Option<char> {
    let b = d10.as_bytes();
    if b.len() != 10 || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let digit = |i: usize| u32::from(b[i] - b'0');
    let odd: u32 = (0..10).step_by(2).map(digit).sum();
    let even: u32 = (1..10).step_by(2).map(digit).sum();
    let check = (10 - (odd + even * 2) % 10) % 10;
    char::from_digit(check, 10)
}

/// Whether a line talks about a Marktlokation.
fn mentions_malo(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    l.contains("malo") || l.contains("marktlokation") || l.contains("location_id")
}

/// Scan the workspace. Returns `true` when every MaLo literal validates.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    // `makotest` is in the list because its bindings are the same `MaloId`: a
    // fixture there is exactly as wrong as one in a Rust test, and nothing but
    // this check says so.
    for dir in [
        "services",
        "crates",
        "demos",
        "xtask",
        "makotest",
        "site/content",
    ] {
        collect(&workspace_root.join(dir), &mut findings);
    }
    findings.extend(deliberate_entries_that_are_actually_valid());
    findings.sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));

    if findings.is_empty() {
        println!("check-malo-ids: every MaLo-ID and EIC literal carries a valid check digit");
        return true;
    }

    eprintln!(
        "check-malo-ids: {} identifier literal(s) fail their check digit:",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {}:{}  {}", f.path.display(), f.line, f.message);
    }
    eprintln!(
        "\nThe eleventh digit is a check digit (BDEW Anwendungshilfe): sum the odd\n\
         positions, add twice the even positions, and the check digit is the\n\
         difference to the next multiple of ten. `metering::MaloId` refuses a\n\
         mismatch at the parse, so a fixture carrying one is rejected by the\n\
         storage layer at run time.\n\
         If a literal is deliberately invalid, add it to DELIBERATE with the reason."
    );
    false
}

fn collect(dir: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, findings);
        } else if path.extension().is_some_and(|e| {
            matches!(
                e.to_str(),
                Some("rs" | "py" | "json" | "yaml" | "yml" | "md" | "sh" | "sql" | "toml" | "edi")
            )
        }) {
            scan(&path, findings);
        }
    }
}

fn scan(path: &Path, findings: &mut Vec<Finding>) {
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    // This file states invalid IDs on purpose — it is the guard.
    if path.file_name().is_some_and(|n| n == "check_malo_ids.rs") {
        return;
    }
    for (id, line_no) in offending(&src) {
        let expected = check_digit(&id[..10]).unwrap_or('?');
        let message = format!(
            "{id} — MaLo check digit should be {expected} ({}{expected})",
            &id[..10]
        );
        findings.push(Finding {
            path: path.to_path_buf(),
            line: line_no,
            message,
        });
    }
    for (code, line_no, want, got) in mistyped_eics(&src) {
        let (is_kind, want_kind) = if want == 'Y' {
            ("a Bilanzkreis (Party)", "a Bilanzierungsgebiet (Area, 'Y')")
        } else {
            ("a Bilanzierungsgebiet (Area)", "a Bilanzkreis (Party, 'X')")
        };
        let message = format!(
            "{code} is {is_kind} — EIC object type '{got}' — but the line names {want_kind}"
        );
        findings.push(Finding {
            path: path.to_path_buf(),
            line: line_no,
            message,
        });
    }
    for (code, line_no) in offending_eics(&src) {
        let expected = rubo4e::identifiers::EicCode::compute_check_char(&code[..15]).unwrap_or('?');
        let message = format!(
            "{code} — EIC check character should be {expected} ({}{expected})",
            &code[..15]
        );
        findings.push(Finding {
            path: path.to_path_buf(),
            line: line_no,
            message,
        });
    }
}

/// The failing MaLo literals in one source, as `(id, 1-based line)`.
///
/// Split from the filesystem so the rule is testable against exact text.
fn offending(src: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let context = mentions_malo(line) || i > 0 && mentions_malo(lines[i - 1]);
        if !context {
            continue;
        }
        for id in eleven_digit_runs(line) {
            if DELIBERATE.iter().any(|(d, _)| *d == id) {
                continue;
            }
            // The Vergabestelle is 1–9; a leading zero means this is some other
            // eleven-digit number that happens to sit on a MaLo line.
            if id.starts_with('0') {
                continue;
            }
            if check_digit(&id[..10]).is_some_and(|c| id.ends_with(c)) {
                continue;
            }
            out.push((id, i + 1));
        }
    }
    out
}

/// An EIC on a line that names the identifier it belongs to must carry the
/// matching ENTSO-E object type.
///
/// A Bilanzkreis is a **Party** (`X`) — held by a Bilanzkreisverantwortlicher —
/// and a Bilanzierungsgebiet is an **Area** (`Y`). Both are sixteen characters
/// with a valid check character, so nothing but the type character separates
/// them, and MSCONS SG6 carries both as free text under different `LOC`
/// qualifiers. A fixture that names one and carries the other is the confusion
/// the BIKO cannot detect, written down.
fn mistyped_eics(src: &str) -> Vec<(String, usize, char, char)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if line.trim_start().starts_with("//!") {
            continue;
        }
        // A test that proves the mismatch is refused has to state the
        // mismatch. `is_err()`/`expect_err` on the line is that statement, and
        // is the only exemption — a mismatch nobody asserts against is a bug.
        if line.contains("is_err()") || line.contains("expect_err") {
            continue;
        }
        let l = line.to_ascii_lowercase();
        // `bilanzierungsgebiet` contains `bilanz`, so test the longer word first.
        let want = if l.contains("bilanzierungsgebiet") {
            'Y'
        } else if l.contains("bilanzkreis") {
            'X'
        } else {
            continue;
        };
        for tok in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if tok.len() != 16 {
                continue;
            }
            let b = tok.as_bytes();
            if !(b[0].is_ascii_digit() && b[1].is_ascii_digit()) {
                continue;
            }
            let got = b[2] as char;
            if matches!(got, 'X' | 'Y' | 'Z' | 'W' | 'T' | 'V' | 'A') && got != want {
                out.push((tok.to_owned(), i + 1, want, got));
            }
        }
    }
    out
}

/// The failing EIC literals in one source, as `(code, 1-based line)`.
///
/// An EIC is sixteen characters: two digits, an ENTSO-E object-type letter, a
/// thirteen-character body and a check character. No context word is required —
/// the shape is distinctive enough that a false positive is unlikely, and a
/// sixteen-character token that *looks* like an EIC and is not valid is worth
/// looking at either way.
fn offending_eics(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        if line.trim_start().starts_with("//!") {
            continue; // this guard's own prose
        }
        for tok in line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
            if tok.len() != 16 {
                continue;
            }
            if DELIBERATE.iter().any(|(d, _)| *d == tok) {
                continue;
            }
            let b = tok.as_bytes();
            let shaped = b[0].is_ascii_digit()
                && b[1].is_ascii_digit()
                && matches!(b[2], b'X' | b'Y' | b'Z' | b'W' | b'T' | b'V' | b'A');
            if shaped && rubo4e::identifiers::EicCode::new(tok).is_err() {
                out.push((tok.to_owned(), i + 1));
            }
        }
    }
    out
}

/// Every maximal run of exactly eleven ASCII digits in `line`.
fn eleven_digit_runs(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i - start == 11 {
            out.push(line[start..i].to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{check_digit, eleven_digit_runs, offending};

    /// The Anwendungshilfe's own worked example, digit for digit.
    #[test]
    fn the_published_example_computes_its_check_digit() {
        assert_eq!(check_digit("4137355924"), Some('1'));
    }

    /// The architecture page publishes this arithmetic for an integrator to
    /// implement from, and prose cannot be compiled. Both reference vectors
    /// Identifikatoren V1.2 §8 prints must survive there, computed rather than
    /// transcribed — a page describing Luhn (doubling from the left, with the
    /// "subtract 9" reduction) yields a different digit for most bases and is
    /// the mistake this pins.
    #[test]
    fn the_published_algorithm_matches_the_implementation() {
        let doc = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../site/content/docs/architecture/domain-model.md");
        let Ok(page) = std::fs::read_to_string(&doc) else {
            panic!("{} is missing", doc.display());
        };

        // §8.1 — computed here, so the page cannot state a digit the
        // implementation does not produce.
        let malo = format!(
            "4137355924{}",
            check_digit("4137355924").expect("ten digits")
        );
        assert!(
            page.contains(&malo),
            "domain-model.md must carry the §8.1 reference vector {malo}"
        );

        // §8.2 — the same arithmetic over an alphanumeric base, through the
        // validator the services use. The published base carries Codetyp `A`,
        // which is a Cluster-Ressource.
        assert!(
            rubo4e::identifiers::CrId::new("A1137355925").is_ok(),
            "A1137355925 is the §8.2 reference vector"
        );
        assert!(
            page.contains("A1137355925"),
            "domain-model.md must carry the §8.2 reference vector A1137355925"
        );

        // The fingerprint of the Luhn description, which is the wrong
        // arithmetic here and the one a reader is likely to reach for.
        assert!(
            !page.contains("alternately multiply each digit by"),
            "domain-model.md describes the Luhn variant; §8.1 doubles the even \
             positions and has no digit-sum reduction"
        );
    }

    /// mako's canonical fixture, and the value it had to become.
    #[test]
    fn the_fixture_malo_is_checked() {
        assert_eq!(check_digit("5123869678"), Some('1'));
        assert!(offending("let malo = \"51238696780\";").len() == 1);
        assert!(offending("let malo = \"51238696781\";").is_empty());
    }

    /// A MeLo is 33 characters; the digits inside it are not a MaLo.
    #[test]
    fn a_melo_is_not_scanned_for_malo_ids() {
        let line = r#"    "melo_id": "DE0001234567890123456789012345678","#;
        assert!(
            eleven_digit_runs(line).is_empty(),
            "{:?}",
            eleven_digit_runs(line)
        );
    }

    /// An eleven-digit number with no MaLo context is not examined.
    #[test]
    fn only_lines_about_a_malo_are_examined() {
        assert!(offending("let timestamp = 20251001001;").is_empty());
        // …but the line under a MaLo mention is, because fixtures wrap.
        assert_eq!(offending("\"malo_id\":\n  \"51238696780\"").len(), 1);
    }

    /// A leading zero means it is not a MaLo — the Vergabestelle is 1–9.
    #[test]
    fn a_leading_zero_is_not_a_malo() {
        assert!(offending("malo-ish column 00056266802").is_empty());
    }

    /// EIC codes are checked against `rubo4e`, the validator the services use.
    #[test]
    fn eic_check_characters_are_validated() {
        // A published Bilanzkreis code, and the same body with a wrong check
        // character.
        assert!(super::offending_eics(r#"let bk = "11XSUEDWESTSTRO8";"#).is_empty());
        assert_eq!(
            super::offending_eics(r#"let bk = "11XSUEDWESTSTRO7";"#).len(),
            1
        );
        // A real Bilanzierungsgebiet (Area) code passes too.
        assert!(super::offending_eics(r#""10YDE-EON------1""#).is_empty());
        // Sixteen characters that are not EIC-shaped are left alone.
        assert!(super::offending_eics("abcdefghijklmnop").is_empty());
    }
}
