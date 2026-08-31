//! Guard: a business date is a Europe/Berlin date.
//!
//! Every date the German energy market states — a Lieferbeginn, a
//! Rechnungsdatum, the day a Frist starts counting, the day a price slice takes
//! effect — is a calendar date in German local time. Two idioms answer it with
//! the *UTC* date instead, and both are wrong for one hour every night (two in
//! summer), silently and without a signal in the value:
//!
//! | Idiom | What it answers |
//! |---|---|
//! | `OffsetDateTime::now_utc().date()` | the UTC calendar date |
//! | SQL `current_date` | the *session* time zone's date |
//!
//! The replacements are [`mako_fristen::heute`] on the Rust side and the
//! `heute()` SQL function each schema defines. This check refuses the two
//! idioms so the next one is caught at `just ci` rather than at a month
//! boundary, where an invoice dated into the previous month or a Frist counted
//! from the wrong day is expensive and quiet.

use std::path::{Path, PathBuf};

/// A single offending site.
type Finding = (PathBuf, usize, String);

/// Scan the workspace for UTC-dated business dates.
///
/// Returns `true` when every business date is a Berlin date.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    for dir in ["services", "crates", "xtask"] {
        collect(&workspace_root.join(dir), &mut findings);
    }

    if findings.is_empty() {
        println!("check-business-dates: every business date is a Europe/Berlin date");
        return true;
    }

    eprintln!(
        "check-business-dates: {} site(s) read a business date in UTC:",
        findings.len()
    );
    for (path, line, text) in &findings {
        eprintln!("  {}:{line}  {}", path.display(), text.trim());
    }
    eprintln!(
        "\nRust: use `mako_fristen::heute()` (or `berlin_date(instant)` for a \
         stored instant).\n\
         SQL:  use the schema's `heute()` function, not `current_date`."
    );
    false
}

/// Every `.rs` and `.sql` file under `dir`.
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
            continue;
        }
        // Two files name the UTC clock because they are what defines or
        // enforces the rule: `mako-fristen`, where the conversion lives, and
        // this check itself, whose patterns and fixtures are the literals.
        if path.ends_with("mako-fristen/src/lib.rs")
            || path.ends_with("xtask/src/check_business_dates.rs")
        {
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str());
        if !matches!(ext, Some("rs" | "sql")) {
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
#[must_use]
pub fn offending_lines(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        // Prose describing the rule is not a violation of it.
        if trimmed.starts_with("//") || trimmed.starts_with("--") {
            continue;
        }
        let hit =
            line.contains("now_utc().date()") || line.to_ascii_uppercase().contains("CURRENT_DATE");
        if hit {
            out.push((line.to_owned(), i + 1));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::offending_lines;

    #[test]
    fn flags_both_idioms() {
        let src = "let today = OffsetDateTime::now_utc().date();\n\
                   \"SELECT 1 WHERE d <= CURRENT_DATE\"\n";
        assert_eq!(offending_lines(src).len(), 2);
    }

    #[test]
    fn accepts_the_berlin_forms() {
        let src = "let today = mako_fristen::heute();\n\
                   \"SELECT 1 WHERE d <= heute()\"\n";
        assert!(offending_lines(src).is_empty());
    }

    #[test]
    fn prose_naming_the_rule_is_not_the_rule() {
        let src = "/// `now_utc().date()` answers the UTC date, not the German one.\n\
                   -- `current_date` is the session time zone's date.\n";
        assert!(offending_lines(src).is_empty());
    }
}
