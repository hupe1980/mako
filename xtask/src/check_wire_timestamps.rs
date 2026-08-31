//! Guard: no raw `time` value reaches a JSON wire.
//!
//! `time::OffsetDateTime` and `time::Date` derive `Serialize`, and what they
//! derive is their **component array**:
//!
//! ```text
//! [2027, 15, 8, 0, 0, 0, 0, 0, 0]   // year, ordinal day, h, m, s, ns, ±h, ±m, ±s
//! [2027, 15]                        // year, ordinal day
//! ```
//!
//! That is `time`'s internal layout. It is documented nowhere a consumer looks,
//! it round-trips only through `time` itself, and dropping one into a
//! `serde_json::json!` ships it silently — the code compiles, the response is
//! valid JSON, and the field looks populated.
//!
//! The MCP surface is where it costs most: a deadline specialist's whole job is
//! deciding whether a Frist has passed, and an `obsd` `deadline_at` served as an
//! undocumented integer array is something it can only do arithmetic on by
//! guessing.
//!
//! Nothing in the type system objects, and no test notices, because a component
//! array is a perfectly good JSON value. This check is the missing compiler.
//!
//! ## What it looks for
//!
//! A `json!` field whose value expression mentions `time::OffsetDateTime` or
//! `time::Date` and does **not** convert it — `.format(`, `.to_string()`,
//! `rfc3339`, or a helper whose name says so. The heuristic is deliberately
//! shallow: it reads one line, and a multi-line value expression is allowed
//! through on the assumption that anyone spreading a field across lines is
//! doing something to it. That is the trade that keeps it free of false
//! positives while still catching every instance of the bug that shipped.

use std::path::{Path, PathBuf};

/// Scan the workspace for raw `time` values in JSON output.
///
/// Returns `true` when every timestamp on a wire is formatted.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    for dir in ["services", "crates"] {
        collect(&workspace_root.join(dir), &mut findings);
    }

    if findings.is_empty() {
        println!(
            "check-wire-timestamps: every `time` value on a JSON wire is formatted, \
             not serialised as a component array"
        );
        return true;
    }

    eprintln!(
        "check-wire-timestamps: {} JSON field(s) would serialise a `time` value as its \
         component array:",
        findings.len()
    );
    for (path, line, text) in &findings {
        eprintln!("  {}:{line}  {}", path.display(), text.trim());
    }
    eprintln!(
        "\n`time::OffsetDateTime` derives `Serialize` as [y, ordinal, h, m, s, ns, ±h, ±m, ±s]\n\
         and `time::Date` as [y, ordinal]. Neither is readable by a consumer.\n\
         Format it: `.format(&Rfc3339).ok()` for an instant, `.to_string()` for a date."
    );
    false
}

/// Every `.rs` file under `dir`.
fn collect(dir: &Path, findings: &mut Vec<(PathBuf, usize, String)>) {
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
        } else if path.extension().is_some_and(|e| e == "rs") {
            scan(&path, findings);
        }
    }
}

/// Calls that turn a `time` value into something a consumer can read.
///
/// `.ok()` and `.unwrap_or(…)` are deliberately **not** here: they change the
/// `Option` wrapper and leave the component array exactly where it was. That
/// distinction is the whole check — every field that shipped the bug had one of
/// those and nothing else.
const CONVERSIONS: &[&str] = &[
    ".format(",
    ".to_string()",
    ".map(",
    ".and_then(",
    "rfc3339",
    "unix_timestamp",
];

fn scan(path: &Path, findings: &mut Vec<(PathBuf, usize, String)>) {
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    for (line, i) in offending_lines(&src) {
        findings.push((path.to_path_buf(), i, line));
    }
}

/// The offending lines of one source file, as `(line, 1-based number)`.
///
/// Split from the filesystem so the rule itself is testable against the exact
/// text that shipped.
fn offending_lines(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let trimmed = line.trim_start();
        // A doc comment describing the bug is not the bug.
        if trimmed.starts_with("//") {
            continue;
        }
        // Only a JSON field: `"name": <expr>`. Anything else is ordinary Rust,
        // where a `time` value is exactly what is wanted.
        if !(trimmed.starts_with('"') && trimmed.contains("\": ")) {
            continue;
        }
        if !(line.contains("time::OffsetDateTime") || line.contains("time::Date")) {
            continue;
        }
        if CONVERSIONS.iter().any(|c| line.contains(c)) {
            continue;
        }
        // A value expression continued on the next line is doing something.
        if !line.trim_end().ends_with(',') {
            continue;
        }
        out.push((line.to_owned(), i + 1));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::offending_lines;

    /// The exact lines that shipped the bug, and the exact lines that fixed it.
    ///
    /// A guard with no test proving it catches the original is a guard nobody
    /// can trust, so these are copied verbatim from the diff that fixed them.
    #[test]
    fn it_catches_what_shipped_and_clears_the_fix() {
        let shipped = r#"
                "deadline_at": r.try_get::<Option<time::OffsetDateTime>, _>("deadline_at").unwrap_or(None),
                "started_at": r.try_get::<time::OffsetDateTime, _>("started_at").ok(),
                "datum": r.try_get::<Option<time::Date>, _>("mastr_datum").unwrap_or(None),
                "planned_date":  r.try_get::<Option<time::Date>, _>("planned_date").unwrap_or(None),
"#;
        assert_eq!(
            offending_lines(shipped).len(),
            4,
            "every field that served a component array must be caught"
        );

        let fixed = r#"
                "deadline_at": rfc3339_opt(r.try_get::<Option<time::OffsetDateTime>, _>("deadline_at").unwrap_or(None)),
                "datum": r.try_get::<Option<time::Date>, _>("mastr_datum")
                    .unwrap_or(None)
                    .map(|d| d.to_string()),
                "computed_at": row.try_get::<time::OffsetDateTime, _>("computed_at").ok()
                    .and_then(|t| t.format(&Rfc3339).ok()),
                "received_at":            r.try_get::<time::OffsetDateTime, _>("received_at").ok().and_then(fmt),
"#;
        assert!(
            offending_lines(fixed).is_empty(),
            "a formatted timestamp is not a finding: {:?}",
            offending_lines(fixed)
        );
    }

    /// `.ok()` and `.unwrap_or` are wrapper changes, not conversions.
    ///
    /// If either counted, every field that shipped the bug would pass.
    #[test]
    fn unwrapping_is_not_formatting() {
        let line = r#"                "at": r.try_get::<time::OffsetDateTime, _>("at").ok(),"#;
        assert_eq!(offending_lines(line).len(), 1);
    }

    /// Ordinary Rust holding a `time` value is not a wire.
    #[test]
    fn only_json_fields_are_examined() {
        let ordinary = r#"
        let now: time::OffsetDateTime = time::OffsetDateTime::now_utc();
        let d = r.try_get::<time::Date, _>("d").ok();
"#;
        assert!(offending_lines(ordinary).is_empty());
    }
}
