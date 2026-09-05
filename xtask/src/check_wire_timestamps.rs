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
//! ## Two ways a timestamp reaches a wire
//!
//! **Hand-built JSON.** A `json!` field whose value expression mentions
//! `time::OffsetDateTime` or `time::Date` and does **not** convert it —
//! `.format(`, `.to_string()`, `rfc3339`, or a helper whose name says so. The
//! heuristic is deliberately shallow: it reads one line, and a multi-line value
//! expression is allowed through on the assumption that anyone spreading a field
//! across lines is doing something to it. That is the trade that keeps it free
//! of false positives while still catching every instance of the bug that
//! shipped.
//!
//! **A derived `Serialize`.** `Json(record)` and `serde_json::to_value(record)`
//! serve whatever the derive produces, and the `time` crate's `serde` feature
//! writes an `OffsetDateTime` as `1970-01-01 00:00:00.0 +00:00:00` — a space
//! where ISO 8601 puts a `T`, and an offset carrying seconds. It parses in
//! `time` and almost nowhere else. `obsd` served exactly that on
//! `GET /obs/processes` while its own MCP tools served RFC 3339 for the same
//! `deadline_at`, because only the hand-built path was checked.
//!
//! So an `OffsetDateTime` (or `PrimitiveDateTime`) field of a struct deriving
//! `Serialize` must carry `#[serde(with = "time::serde::rfc3339")]`, or
//! `::option` for an `Option`. `time::Date` is exempt: it derives as
//! `"2026-06-01"`, which already is ISO 8601.

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
/// `.ok()`, `.unwrap_or(…)`, `.map(…)` and `.and_then(…)` are deliberately
/// **not** here: all four change the `Option` wrapper and leave the component
/// array exactly where it was. That distinction is the whole check — every field
/// that shipped the bug had one of those and nothing else. A `.map`/`.and_then`
/// counts only when the conversion is written *inside* it, which
/// [`maps_through_a_conversion`] decides.
const CONVERSIONS: &[&str] = &[".format(", ".to_string()", "rfc3339", "unix_timestamp"];

/// Names that say the callee turns a `time` value into text.
///
/// A `.and_then(fmt)` passes a helper rather than a closure, and the helper's
/// name is all there is to read.
const FORMATTING_NAMES: &[&str] = &[
    ".format(",
    ".to_string()",
    "rfc3339",
    "unix_timestamp",
    "fmt",
    "format",
];

/// Whether a `.map(…)` / `.and_then(…)` on `line` converts inside its argument.
///
/// `.map(|t| t.format(&Rfc3339).ok())` converts; `.map(Some)` and
/// `.and_then(|t| Some(t))` only move the value between `Option` wrappers and
/// ship the component array unchanged. The argument is read to its balanced
/// closing paren, so a conversion applied to some *other* sub-expression on the
/// same line does not clear the timestamp.
fn maps_through_a_conversion(line: &str) -> bool {
    for opener in [".map(", ".and_then("] {
        let mut rest = line;
        while let Some(at) = rest.find(opener) {
            let arg_start = at + opener.len();
            let arg = balanced_argument(&rest[arg_start..]);
            if FORMATTING_NAMES.iter().any(|n| arg.contains(n)) {
                return true;
            }
            rest = &rest[arg_start..];
        }
    }
    false
}

/// The text up to the paren that closes an argument list already opened.
fn balanced_argument(rest: &str) -> &str {
    let mut depth = 1usize;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &rest[..i];
                }
            }
            _ => {}
        }
    }
    rest
}

fn scan(path: &Path, findings: &mut Vec<(PathBuf, usize, String)>) {
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    for (line, i) in offending_lines(&src) {
        findings.push((path.to_path_buf(), i, line));
    }
    for (line, i) in unformatted_derive_fields(&src) {
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
        if CONVERSIONS.iter().any(|c| line.contains(c)) || maps_through_a_conversion(line) {
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

/// `OffsetDateTime`/`PrimitiveDateTime` fields of `Serialize` containers that
/// carry no RFC 3339 adapter, as `(line, 1-based number)`.
///
/// A container is a `struct` with named fields **or** an `enum`: an
/// event-sourced variant carries its fields the same way a struct does, and it
/// is serialised by the same derive into the same stored stream. `time::Date` is
/// not a finding: it derives as `"2026-06-01"`.
///
/// Shallow on purpose, in the same way [`offending_lines`] is. It tracks two
/// things — whether the container currently being read derives `Serialize`, and
/// the brace depth at which that container ends — and looks back over the
/// field's own attribute block for a `time::serde` adapter. A field whose
/// attributes span several lines is handled, because that is how
/// `#[serde(default = …, with = …)]` is usually formatted, and so is a
/// `#[derive(…)]` spread over several lines, which `rustfmt` produces as soon as
/// the trait list is long enough.
fn unformatted_derive_fields(src: &str) -> Vec<(String, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    // A `#[derive(…)]` being accumulated until its closing paren.
    let mut pending_derive: Option<String> = None;
    // A completed `Serialize` derive waiting for the item header it applies to.
    let mut serialize_pending = false;
    // Brace depth just outside the container being read, `None` when outside one.
    let mut container_base: Option<i32> = None;
    let mut depth = 0i32;

    for (i, raw) in lines.iter().enumerate() {
        let line = raw.trim_start();
        if line.starts_with("//") {
            continue;
        }
        let delta = line.matches('{').count() as i32 - line.matches('}').count() as i32;

        if let Some(buf) = pending_derive.as_mut() {
            buf.push_str(line);
            if parens_balanced(buf) {
                serialize_pending = buf.contains("Serialize");
                pending_derive = None;
            }
            continue;
        }
        if line.starts_with("#[derive") {
            if parens_balanced(line) {
                serialize_pending = line.contains("Serialize");
            } else {
                pending_derive = Some(line.to_owned());
            }
            continue;
        }
        if serialize_pending {
            if line.starts_with("#[") {
                continue;
            }
            serialize_pending = false;
            // A tuple or unit struct has no named fields to check.
            if (is_struct_header(line) || is_enum_header(line)) && !line.trim_end().ends_with(';') {
                container_base = Some(depth);
            }
            depth += delta;
            continue;
        }

        let Some(base) = container_base else {
            depth += delta;
            continue;
        };
        if depth > base
            && let Some(ty) = named_field_type(line)
            && (ty.contains("OffsetDateTime") || ty.contains("PrimitiveDateTime"))
        {
            // Any adapter naming RFC 3339 counts, including a local one — a
            // `Vec<OffsetDateTime>` has no `time::serde` module to point at.
            let attributes = attribute_block_before(&lines, i);
            if !(attributes.contains("rfc3339") || attributes.contains("serialize_with")) {
                out.push(((*raw).to_owned(), i + 1));
            }
        }
        depth += delta;
        if depth <= base {
            container_base = None;
        }
    }
    out
}

/// Whether every `(` opened in `text` is closed again.
fn parens_balanced(text: &str) -> bool {
    let mut depth = 0i32;
    for c in text.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
    }
    depth <= 0
}

/// `true` for the header line of a named-field struct.
fn is_struct_header(line: &str) -> bool {
    line.strip_prefix("pub ")
        .map(|rest| rest.trim_start_matches(|c| c != 's'))
        .unwrap_or(line)
        .starts_with("struct ")
        || line.starts_with("struct ")
        || (line.starts_with("pub") && line.contains("struct "))
}

/// `true` for the header line of an enum.
///
/// An enum's struct variants hold named fields, and the derive writes them into
/// the same JSON a struct's would.
fn is_enum_header(line: &str) -> bool {
    line.starts_with("enum ") || (line.starts_with("pub") && line.contains("enum "))
}

/// The attribute lines immediately preceding the field at `index`, joined.
///
/// Walks back until the previous field, a brace, or the header of the struct or
/// enum variant the field belongs to, so it cannot pick up a *neighbouring*
/// field's or variant's adapter.
fn attribute_block_before(lines: &[&str], index: usize) -> String {
    let mut block = String::new();
    for line in lines[..index].iter().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("///") || trimmed.starts_with("//") {
            continue;
        }
        if named_field_type(trimmed).is_some()
            || trimmed.starts_with('{')
            || trimmed.starts_with('}')
            || trimmed.ends_with('{')
            || is_struct_header(trimmed)
            || is_enum_header(trimmed)
        {
            break;
        }
        block.push_str(line);
        block.push('\n');
    }
    block
}

/// The type of a `name: Type,` field line, if the line is one.
///
/// An enum's struct variant is often written on one line
/// (`Sent { at: OffsetDateTime },`), so the field name is whatever follows the
/// last brace, paren or comma ahead of the colon, and the type is read back to
/// the brace that closes the variant.
fn named_field_type(line: &str) -> Option<&str> {
    let (name, ty) = line.split_once(": ")?;
    let name = name.rsplit(['{', '(', ',']).next().unwrap_or(name).trim();
    let name = name.strip_prefix("pub ").unwrap_or(name).trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    let ty = ty.trim().trim_end_matches([',', ' ', '}']).trim();
    (!ty.is_empty()).then_some(ty)
}

#[cfg(test)]
mod tests {
    use super::{offending_lines, unformatted_derive_fields};

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

    /// The `obsd` projection as it shipped: RFC 3339 through the MCP tools,
    /// `time`'s own format through the REST routes, because only the hand-built
    /// path was checked.
    #[test]
    fn a_derived_serialize_field_needs_an_rfc_3339_adapter() {
        let shipped = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessProjection {
    /// The business Antwortfrist.
    pub deadline_at: Option<OffsetDateTime>,
    pub started_at: OffsetDateTime,
    pub deadline_source: Option<String>,
}
"#;
        assert_eq!(unformatted_derive_fields(shipped).len(), 2);

        let fixed = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessProjection {
    /// The business Antwortfrist.
    #[serde(with = "time::serde::rfc3339::option")]
    pub deadline_at: Option<OffsetDateTime>,
    #[serde(default = "unix_epoch", with = "time::serde::rfc3339")]
    pub started_at: OffsetDateTime,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub valid_from: Option<OffsetDateTime>,
}
"#;
        assert!(
            unformatted_derive_fields(fixed).is_empty(),
            "{:?}",
            unformatted_derive_fields(fixed)
        );
    }

    /// The shape the event-sourced crates use: a `Serialize` enum whose struct
    /// variants carry the timestamps.
    ///
    /// Eleven fields across `mako-wim` and `mako-geli-gas` sat in exactly this
    /// shape, writing `time`'s own format into the stored event stream, while a
    /// guard that only entered `struct` headers reported the tree clean.
    #[test]
    fn an_enum_variants_fields_are_scanned() {
        let shipped = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    AngebotErhalten {
        message_ref: MessageRef,
        bindungsfrist: OffsetDateTime,
        #[serde(default)]
        fruehester_start: Option<OffsetDateTime>,
    },
    Abgelehnt {
        grund: String,
    },
    BeendetDurchMsb { beendigung_zum: OffsetDateTime },
}
"#;
        assert_eq!(unformatted_derive_fields(shipped).len(), 3);

        let fixed = r#"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    AngebotErhalten {
        message_ref: MessageRef,
        #[serde(with = "time::serde::rfc3339")]
        bindungsfrist: OffsetDateTime,
        #[serde(default, with = "time::serde::rfc3339::option")]
        fruehester_start: Option<OffsetDateTime>,
    },
    Abgelehnt {
        grund: String,
    },
}
"#;
        assert!(
            unformatted_derive_fields(fixed).is_empty(),
            "{:?}",
            unformatted_derive_fields(fixed)
        );
    }

    /// `rustfmt` breaks a long trait list across lines, and the derive still
    /// derives `Serialize`.
    #[test]
    fn a_multi_line_derive_still_names_serialize() {
        let src = r#"
#[derive(
    Debug,
    Clone,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Row {
    pub started_at: OffsetDateTime,
}
"#;
        assert_eq!(unformatted_derive_fields(src).len(), 1);
    }

    /// A struct that follows a `Serialize` enum is a separate item, and the
    /// enum's brace depth must not carry into it.
    #[test]
    fn a_container_ends_at_its_closing_brace() {
        let src = r#"
#[derive(Serialize)]
pub enum Event {
    Sent { at: OffsetDateTime },
}

#[derive(Debug)]
pub struct Query {
    pub since: OffsetDateTime,
}
"#;
        let hits = unformatted_derive_fields(src);
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert!(hits[0].0.contains("Sent {"));
    }

    /// `.map` and `.and_then` change the `Option` wrapper, exactly as `.ok()`
    /// does. Only what is written inside them converts.
    #[test]
    fn mapping_without_a_conversion_is_not_formatting() {
        let unconverted = r#"
                "at": r.try_get::<Option<time::OffsetDateTime>, _>("at").ok().and_then(|t| t),
                "seen": r.try_get::<time::OffsetDateTime, _>("seen").ok().map(Some),
"#;
        assert_eq!(offending_lines(unconverted).len(), 2);

        let converted = r#"
                "at": r.try_get::<time::OffsetDateTime, _>("at").ok().and_then(|t| t.format(&Rfc3339).ok()),
                "seen": r.try_get::<Option<time::Date>, _>("seen").ok().flatten().map(|d| d.to_string()),
"#;
        assert!(
            offending_lines(converted).is_empty(),
            "{:?}",
            offending_lines(converted)
        );
    }

    /// A `time::Date` already derives as `"2026-06-01"`, and a struct that does
    /// not derive `Serialize` never reaches a wire at all.
    #[test]
    fn dates_and_non_serialized_structs_are_not_findings() {
        let src = r#"
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub valid_from: time::Date,
    pub valid_to: Option<time::Date>,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub since: Option<OffsetDateTime>,
}
"#;
        assert!(
            unformatted_derive_fields(src).is_empty(),
            "{:?}",
            unformatted_derive_fields(src)
        );
    }
}
