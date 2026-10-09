//! Guard: no BEHG price table carries a year no statute has priced.
//!
//! § 10 Abs. 2 BEHG fixes Festpreise for 2021–2025 and a 55–65 EUR/t corridor
//! for 2026. For 2027 the law in force names no figure: § 10 Abs. 3 Satz 2
//! Nr. 4/Nr. 5 BEHG tie the price to the „mengengewichteten Durchschnittspreis
//! der Versteigerungen … in dem jeweils vorletzten vorangegangenen Quartal",
//! which is not knowable in advance, and the 3. BEHG-ÄndG that would replace it
//! with a corridor (BT-Drs. 21/7869) is a Gesetzentwurf. A 2027 row in a price
//! table is therefore a guess, and on an invoice it reads exactly like a
//! statutory price.
//!
//! Refused, in every `.rs` file under `crates/` and `services/`:
//!
//! * a `(YYYY, …)` tuple with `YYYY >= FIRST_UNPRICED_YEAR` inside a `const` or
//!   `static` whose name contains `BEHG` or `NEHS`;
//! * in a file named `behg.rs`, a match arm `YYYY =>` (alone or in a `|`
//!   pattern) or a comparison `== YYYY` with such a year — the shape a
//!   year-keyed price function takes.
//!
//! [`FIRST_UNPRICED_YEAR`] moves only with an enacted Fassung of § 10 BEHG.

use std::path::{Path, PathBuf};

/// The first calendar year no enacted statute prices. Raising it is an edit
/// that cites the promulgated Fassung of § 10 BEHG.
const FIRST_UNPRICED_YEAR: u32 = 2027;

/// A single offending site: file, 1-based line, the line itself.
type Finding = (PathBuf, usize, String);

/// Scan the workspace.
///
/// Returns `true` when no BEHG price table carries an unpriced year.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    let mut scanned = 0usize;
    for dir in ["crates", "services"] {
        collect(
            &workspace_root.join(dir),
            workspace_root,
            &mut scanned,
            &mut findings,
        );
    }
    if scanned == 0 {
        eprintln!(
            "check-behg-prices: the scan read no source file under crates/ or services/ — \
             the layout has probably changed"
        );
        return false;
    }
    if findings.is_empty() {
        println!(
            "check-behg-prices: no BEHG price table in {scanned} source file(s) carries a year \
             from {FIRST_UNPRICED_YEAR} on"
        );
        return true;
    }
    for (path, line, text) in &findings {
        eprintln!("{}:{line}: {}", path.display(), text.trim());
    }
    eprintln!(
        "check-behg-prices: {} BEHG price row(s) for a year no enacted statute prices \
         (from {FIRST_UNPRICED_YEAR}). Supply such a price per period through billingd's \
         `[rates] behg_eur_per_t`, checked against `behg_regime`, never as a table row.",
        findings.len()
    );
    false
}

fn collect(dir: &Path, root: &Path, scanned: &mut usize, out: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, scanned, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            *scanned += 1;
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let behg_file = path.file_name().is_some_and(|n| n == "behg.rs");
            for (line, text) in scan(&src, behg_file) {
                out.push((rel.clone(), line, text));
            }
        }
    }
}

/// The offending lines of one source file, 1-based.
fn scan(src: &str, behg_file: bool) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut in_table = false;
    for (i, line) in src.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        if is_behg_table_head(code) {
            in_table = true;
        }
        let hit = (in_table && tuple_years(code).any(unpriced))
            || (behg_file && (arm_years(code).any(unpriced) || eq_years(code).any(unpriced)));
        if hit {
            out.push((i + 1, line.to_owned()));
        }
        if in_table && code.contains(';') {
            in_table = false;
        }
    }
    out
}

fn unpriced(year: u32) -> bool {
    year >= FIRST_UNPRICED_YEAR
}

/// `const BEHG_EUR_PER_T: …` / `static NEHS_…: …`.
fn is_behg_table_head(code: &str) -> bool {
    let t = code.trim_start();
    let t = t.strip_prefix("pub ").unwrap_or(t);
    let t = t.strip_prefix("pub(crate) ").unwrap_or(t);
    let Some(rest) = t
        .strip_prefix("const ")
        .or_else(|| t.strip_prefix("static "))
    else {
        return false;
    };
    let name = rest.split(':').next().unwrap_or("").to_ascii_uppercase();
    name.contains("BEHG") || name.contains("NEHS")
}

/// Four-digit years `20NN` read from the text following `(`.
fn tuple_years(code: &str) -> impl Iterator<Item = u32> + '_ {
    code.match_indices('(')
        .filter_map(|(i, _)| leading_year(code[i + 1..].trim_start()))
}

/// Years in a match-arm pattern: `2027 =>`, `2026 | 2027 =>`.
fn arm_years(code: &str) -> impl Iterator<Item = u32> + '_ {
    code.split_once("=>")
        .map(|(pat, _)| pat)
        .into_iter()
        .flat_map(|pat| pat.split('|'))
        .filter_map(|p| {
            let p = p.trim();
            (p.len() == 4).then(|| leading_year(p)).flatten()
        })
}

/// Years compared with `==`.
fn eq_years(code: &str) -> impl Iterator<Item = u32> + '_ {
    code.match_indices("==")
        .filter_map(|(i, _)| leading_year(code[i + 2..].trim_start()))
}

/// A year `20NN` at the start of `s`, not followed by another digit.
fn leading_year(s: &str) -> Option<u32> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.len() != 4 || !digits.starts_with("20") {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: &str = "
const BEHG_EUR_PER_T: &[(i32, u32)] = &[
    (2021, 25),
    (2025, 55),
    (2026, 60),
];
const ERDGAS_EMISSIONSFAKTOR_T_PER_GJ: &[(i32, Decimal)] = &[(2027, dec!(0.0558))];
";

    #[test]
    fn todays_table_is_clean() {
        assert!(scan(TODAY, false).is_empty());
    }

    /// The defect the guard exists for: a 2027 row nobody can source.
    #[test]
    fn a_2027_row_in_a_behg_table_is_refused() {
        let planted = TODAY.replace("    (2026, 60),", "    (2026, 60),\n    (2027, 60),");
        let hits = scan(&planted, false);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].1.contains("(2027, 60)"));
    }

    /// A table that is not a price table — the EBeV factor runs to 2030.
    #[test]
    fn a_non_behg_table_is_not_read() {
        assert!(scan("const FAKTOR: &[(i32, u32)] = &[(2030, 1)];", false).is_empty());
    }

    /// The year-keyed price function shape in a `behg.rs`.
    #[test]
    fn a_2027_arm_or_comparison_in_behg_rs_is_refused() {
        let arm = "        2022 | 2023 => 30,\n        2027 => 60,\n";
        assert_eq!(scan(arm, true).len(), 1);
        assert_eq!(scan("    (jahr == 2027).then(|| x)", true).len(), 1);
        assert!(scan("    (jahr == 2026).then(|| x)", true).is_empty());
        assert!(scan(arm, false).is_empty(), "only behg.rs is read for arms");
    }
}
