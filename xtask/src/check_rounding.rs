//! Guard: money and quantity figures round kaufmännisch, never banker's.
//!
//! `rust_decimal::Decimal::round_dp` and `Decimal::round` round half **to
//! even**. German commercial practice, the EN 16931 / XRechnung validation
//! ecosystem and every BDEW settlement figure round half **away from zero**
//! (DIN 1333). The two modes agree everywhere except exact midpoints, so a bare
//! `round_dp` passes every test written against ordinary numbers and then
//! misstates a cent on the invoice where a price quoted in ct with three
//! decimals lands on a half-cent.
//!
//! Three things are refused: a bare `round_dp(`, a bare `Decimal::round()` —
//! which rounds to an integer the same way — and a `RoundingStrategy` other than
//! `MidpointAwayFromZero`. What is left is the explicit
//! `round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)` and the
//! `round_kfm` helpers the billing crates define over it — the guard, not a
//! shared crate, is what keeps those from drifting apart.
//!
//! `f64::round` is half away from zero already and is exempt: a line carrying a
//! float literal or an `f64`/`f32` cast is reading the float method, not
//! `Decimal`'s. `round_sf`, `trunc`, `floor` and `ceil` are not rounding
//! *modes* — they answer different questions and are left alone.

use std::path::{Path, PathBuf};

/// A single offending site: file, 1-based line, the line itself.
type Finding = (PathBuf, usize, String);

/// The check itself, which names the modes it refuses.
const RULE_SITES: &[&str] = &["xtask/src/check_rounding.rs"];

/// The one admissible rounding mode.
const KAUFMAENNISCH: &str = "MidpointAwayFromZero";

/// Scan the workspace for banker's rounding.
///
/// Returns `true` when every rounding is kaufmännisch.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    for dir in ["services", "crates", "xtask", "makotest"] {
        collect(&workspace_root.join(dir), workspace_root, &mut findings);
    }

    if findings.is_empty() {
        println!("check-rounding: every Decimal rounding is kaufmännisch (DIN 1333)");
        return true;
    }

    eprintln!(
        "check-rounding: {} site(s) round with a mode other than kaufmännisch:",
        findings.len()
    );
    for (path, line, text) in &findings {
        eprintln!("  {}:{line}  {}", path.display(), text.trim());
    }
    eprintln!(
        "\n`Decimal::round_dp` is banker's rounding (half to even). Round half away \n\
         from zero instead: `round_dp_with_strategy(dp, \
         RoundingStrategy::MidpointAwayFromZero)`, or the crate-local \
         `RoundMoney::round_kfm(dp)` where one is defined."
    );
    false
}

/// Every `.rs` file under `dir`, skipping build output.
fn collect(dir: &Path, root: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, root, findings);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if RULE_SITES.contains(&rel.to_string_lossy().replace('\\', "/").as_str()) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line, i) in offending_lines(&src) {
            findings.push((path.clone(), i, line));
        }
    }
}

/// The offending lines of one source file, as `(line, 1-based number)`.
///
/// Split from the filesystem so the rule is testable against exact text.
/// Comments describe the rule; they do not apply it.
#[must_use]
pub fn offending_lines(src: &str) -> Vec<(String, usize)> {
    src.lines()
        .enumerate()
        .filter(|(_, line)| {
            let t = line.trim_start();
            if t.starts_with("//") {
                return false;
            }
            if t.contains("RoundingStrategy::") && !t.contains(KAUFMAENNISCH) {
                return true;
            }
            if t.contains(".round_dp(") {
                return true;
            }
            // `f64::round` is already half away from zero. A float literal or a
            // float cast on the line is what tells the two methods apart.
            t.contains(".round()") && !reads_a_float(t)
        })
        .map(|(i, line)| (line.to_owned(), i + 1))
        .collect()
}

/// Whether the line is doing float arithmetic, and so `.round()` on it is
/// `f64::round` rather than `Decimal::round`.
fn reads_a_float(line: &str) -> bool {
    if line.contains("f64") || line.contains("f32") {
        return true;
    }
    // A float literal: a digit, a dot, a digit.
    let bytes = line.as_bytes();
    bytes.windows(3).any(|w| {
        w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit()
    })
}

#[cfg(test)]
mod tests {
    use super::offending_lines;

    #[test]
    fn a_bare_round_dp_is_refused() {
        let hits = offending_lines("let x = total.round_dp(2);\n");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].1, 1);
    }

    #[test]
    fn another_strategy_is_refused() {
        assert_eq!(
            offending_lines("v.round_dp_with_strategy(2, RoundingStrategy::MidpointNearestEven)\n")
                .len(),
            1
        );
    }

    #[test]
    fn a_bare_decimal_round_is_refused() {
        assert_eq!(offending_lines("let ct = eur_amount.round();\n").len(), 1);
    }

    /// `f64::round` is half away from zero already, so the float sites stand.
    #[test]
    fn float_rounding_is_left_alone() {
        for line in [
            "Some(((a - t) * 1000.0).round() / 10.0)\n",
            "let score = (closeness.clamp(0.0, 1.0) * 100.0).round() as u32;\n",
            "(self.check.total_tolerance * 1_000_000.0).round() as u32\n",
        ] {
            assert!(offending_lines(line).is_empty(), "{line}");
        }
    }

    #[test]
    fn the_admissible_forms_pass() {
        assert!(
            offending_lines(
                "v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)\n"
            )
            .is_empty()
        );
        assert!(offending_lines("let x = total.round_kfm(2);\n").is_empty());
    }

    #[test]
    fn prose_about_the_rule_is_not_a_violation() {
        assert!(offending_lines("// `round_dp` is banker's rounding\n").is_empty());
        assert!(offending_lines("//! RoundingStrategy::MidpointNearestEven\n").is_empty());
    }

    /// `round_sf` answers a different question (significant figures) and is not
    /// a mode choice, so the substring match must not catch it.
    #[test]
    fn other_decimal_helpers_are_untouched() {
        assert!(offending_lines("let x = v.round_sf(3);\nlet y = v.trunc();\n").is_empty());
    }
}
