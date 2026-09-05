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
//! Four things are refused:
//!
//! * `round_dp(` in **any** spelling — the method call, the UFCS
//!   `Decimal::round_dp(&x, 2)`, or a free function of that name;
//! * a bare `Decimal::round()`, which rounds to an integer the same way;
//! * a `RoundingStrategy` other than `MidpointAwayFromZero` named anywhere,
//!   including in the `use` that imports it;
//! * a `round_dp_with_strategy(` call whose **argument list** does not carry
//!   `MidpointAwayFromZero`. Reading the argument list rather than the line
//!   catches the strategy imported as a bare variant
//!   (`use rust_decimal::RoundingStrategy::MidpointNearestEven;` then
//!   `.round_dp_with_strategy(2, MidpointNearestEven)`), which names no
//!   `RoundingStrategy::` path at the call at all. The list is read across
//!   lines, because `rustfmt` wraps it.
//!
//! What is left is the explicit
//! `round_dp_with_strategy(dp, RoundingStrategy::MidpointAwayFromZero)` and the
//! `round_kfm` helpers the billing crates define over it — the guard, not a
//! shared crate, is what keeps those from drifting apart.
//!
//! `f64::round` is half away from zero already and is exempt — but only on two
//! **structural** grounds, never because a float appears somewhere on the line:
//! the receiver of the `.round()` is itself a float expression, or the result is
//! `as`-cast, which a `Decimal` cannot be. A line-wide float test exempts
//! `(netto * dec!(1.19)).round()`, whose only float-looking token is a `Decimal`
//! literal. `round_sf`, `trunc`, `floor` and `ceil` are not rounding *modes* —
//! they answer different questions and are left alone.

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
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with("//") {
            continue;
        }
        // The strategy named anywhere, including the `use` that imports it.
        let named_strategy = t.contains("RoundingStrategy::") && !t.contains(KAUFMAENNISCH);
        // `round_dp(` in every spelling. `round_dp_with_strategy(` does not
        // contain it — the underscore is where the paren would be — so the two
        // rules cannot overlap.
        let bare_round_dp = t.contains("round_dp(");
        let weak_strategy = strategy_call_without_kaufmaennisch(&lines, i);
        let decimal_round = decimal_round_sites(t);
        if named_strategy || bare_round_dp || weak_strategy || decimal_round {
            out.push(((*line).to_owned(), i + 1));
        }
    }
    out
}

/// Whether a `round_dp_with_strategy(` starting on line `i` names a strategy
/// other than kaufmännisch.
///
/// The argument list is read from the opening paren to its balanced close,
/// across lines: `rustfmt` puts `(\n    2,\n    RoundingStrategy::…,\n)` on
/// four lines, and a per-line reader would see an unterminated list on the
/// first and no call at all on the third.
fn strategy_call_without_kaufmaennisch(lines: &[&str], i: usize) -> bool {
    const CALL: &str = "round_dp_with_strategy(";
    let mut rest = lines[i];
    let mut consumed = 0usize;
    while let Some(at) = rest.find(CALL) {
        let start = consumed + at + CALL.len();
        // The tail of this line plus as many following lines as it takes to
        // balance the parenthesis.
        let mut text = lines[i][start..].to_owned();
        let mut j = i;
        while unbalanced(&text) && j + 1 < lines.len() {
            j += 1;
            text.push('\n');
            text.push_str(lines[j]);
        }
        if !balanced_argument(&text).contains(KAUFMAENNISCH) {
            return true;
        }
        consumed = start;
        rest = &lines[i][start..];
    }
    false
}

/// Whether `text` still has an unclosed argument list.
fn unbalanced(text: &str) -> bool {
    let mut depth = 1i32;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return false;
                }
            }
            _ => {}
        }
    }
    true
}

/// The text up to the paren that closes an argument list already opened.
fn balanced_argument(text: &str) -> &str {
    let mut depth = 1i32;
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[..i];
                }
            }
            _ => {}
        }
    }
    text
}

/// Whether the line carries a `.round()` that is `Decimal`'s rather than
/// `f64`'s.
///
/// Two structural proofs of a float, and nothing weaker. A float *somewhere* on
/// the line proves nothing: `(netto * dec!(1.19)).round()` carries `1.19` and
/// rounds a `Decimal`.
fn decimal_round_sites(line: &str) -> bool {
    const ROUND: &str = ".round()";
    let mut at = 0usize;
    while let Some(i) = line[at..].find(ROUND) {
        let pos = at + i;
        at = pos + ROUND.len();
        // `Decimal` has no `as` cast, so `.round() as u32` is `f64::round`.
        if line[at..].trim_start().starts_with("as ") {
            continue;
        }
        if !receiver_is_a_float(&line[..pos]) {
            return true;
        }
    }
    false
}

/// Whether the expression `.round()` is called on is a float.
///
/// Reads back over the receiver — a balanced parenthesised group, or the
/// trailing path — rather than over the whole line, and discounts the digits
/// inside a `dec!(…)`, which are a `Decimal`'s and not a float literal's.
fn receiver_is_a_float(before: &str) -> bool {
    let receiver = strip_decimal_literals(&trailing_expression(before));
    if receiver.contains("f64") || receiver.contains("f32") {
        return true;
    }
    let bytes = receiver.as_bytes();
    bytes
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit())
}

/// The expression immediately to the left of a method call.
fn trailing_expression(before: &str) -> String {
    let chars: Vec<char> = before.trim_end().chars().collect();
    let is_path = |c: char| c.is_alphanumeric() || c == '_' || c == '.' || c == ':' || c == '!';
    let Some(&last) = chars.last() else {
        return String::new();
    };
    if last == ')' || last == ']' {
        let (open, close) = if last == ')' { ('(', ')') } else { ('[', ']') };
        let mut depth = 0i32;
        let mut i = chars.len();
        while i > 0 {
            i -= 1;
            if chars[i] == close {
                depth += 1;
            } else if chars[i] == open {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
        }
        // A call is its arguments *and* the path it is called on.
        let mut j = i;
        while j > 0 && is_path(chars[j - 1]) {
            j -= 1;
        }
        return chars[j..].iter().collect();
    }
    let mut j = chars.len();
    while j > 0 && is_path(chars[j - 1]) {
        j -= 1;
    }
    chars[j..].iter().collect()
}

/// `expr` with every `dec!(…)` removed.
fn strip_decimal_literals(expr: &str) -> String {
    let mut out = String::with_capacity(expr.len());
    let mut rest = expr;
    while let Some(at) = rest.find("dec!(") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "dec!(".len()..];
        let inner = balanced_argument(after);
        rest = &after[(inner.len() + 1).min(after.len())..];
    }
    out.push_str(rest);
    out
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

    /// A `Decimal` literal is not a float, and a line-wide float test says it
    /// is: `dec!(1.19)` carries a digit, a dot and a digit, and the VAT
    /// multiplication it sits in rounds a `Decimal` half to even.
    #[test]
    fn a_decimal_literal_does_not_exempt_the_round() {
        let hits = offending_lines("let brutto = (netto * dec!(1.19)).round();\n");
        assert_eq!(hits.len(), 1, "{hits:?}");
    }

    /// UFCS names the same method and rounds the same way.
    #[test]
    fn the_ufcs_spelling_is_refused() {
        assert_eq!(
            offending_lines("let x = Decimal::round_dp(&total, 2);\n").len(),
            1
        );
    }

    /// A strategy imported as a bare variant names no `RoundingStrategy::`
    /// path at the call site, so only the argument list gives it away — and
    /// `rustfmt` puts that list on four lines.
    #[test]
    fn a_variant_imported_strategy_is_refused() {
        let inline = "use rust_decimal::RoundingStrategy::MidpointNearestEven;\n\
                      let v = amount.round_dp_with_strategy(2, MidpointNearestEven);\n";
        assert_eq!(
            offending_lines(inline).len(),
            2,
            "{:?}",
            offending_lines(inline)
        );

        let wrapped = "let v = amount\n\
                       \x20   .round_dp_with_strategy(\n\
                       \x20       2,\n\
                       \x20       MidpointNearestEven,\n\
                       \x20   );\n";
        let hits = offending_lines(wrapped);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].0.contains("round_dp_with_strategy"));
    }

    /// The same call written kaufmännisch across four lines must pass, or the
    /// rule above would flag every correct site in `accountingd`.
    #[test]
    fn a_wrapped_kaufmaennisch_call_passes() {
        let src = "let v = amount\n\
                   \x20   .round_dp_with_strategy(\n\
                   \x20       2,\n\
                   \x20       RoundingStrategy::MidpointAwayFromZero,\n\
                   \x20   );\n";
        assert!(
            offending_lines(src).is_empty(),
            "{:?}",
            offending_lines(src)
        );
    }

    /// `Decimal` has no `as` cast, so a cast result is `f64::round` whatever
    /// the receiver is spelled from.
    #[test]
    fn an_as_cast_proves_the_float() {
        assert!(offending_lines("let at = (x0 * CELLS_PER_POINT).round() as usize;\n").is_empty());
    }

    /// `round_sf` answers a different question (significant figures) and is not
    /// a mode choice, so the substring match must not catch it.
    #[test]
    fn other_decimal_helpers_are_untouched() {
        assert!(offending_lines("let x = v.round_sf(3);\nlet y = v.trunc();\n").is_empty());
    }
}
