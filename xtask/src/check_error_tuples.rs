//! Guard: a handler fails with an `ApiError`, not a hand-built
//! `(StatusCode, …)` tuple.
//!
//! `mako_service::ApiError` renders one problem shape — `{ "error", "detail",
//! … }` — and keeps an internal error's text out of the body: a 500 carries
//! `"internal server error"` and an `error_id` that the log line carries
//! beside the full error. A tuple such as
//!
//! ```rust,ignore
//! Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
//! ```
//!
//! does both things `ApiError` exists to prevent: a body shape the caller
//! cannot rely on, and the database's own message — table names, constraint
//! names, SQL fragments — handed to whoever called. Every status a handler
//! refuses with has a variant (`PreconditionFailed`, `PayloadTooLarge`,
//! `TooManyRequests`, `NotImplemented`, `Unavailable`, …), and a stable
//! machine `code` or extra keys ride on any of them via `with_code` /
//! `with_fields`, so no refusal needs its own shape.
//!
//! A report body whose status follows from its own content — a readiness
//! probe, an ingest report counting its rejections — answers through its own
//! `IntoResponse`, which picks the status from the report; that is not a
//! tuple built at a call site.
//!
//! Clippy cannot express the rule: `disallowed-types` matches a type path, an
//! anonymous tuple has none, and banning `StatusCode` would ban the success
//! codes and `ApiError::status` with it. So this is a text scan.
//!
//! **What counts.** In every `services/*/src/**/*.rs`, with each
//! `#[cfg(test)]` item and every comment line blanked: a `(`, optional
//! whitespace (line breaks included), `StatusCode::NAME` — optionally
//! path-qualified, as in `axum::http::StatusCode::NAME` — optional whitespace,
//! and a `,`, where `NAME` is not a 2xx code. A helper call
//! `reply(StatusCode::NOT_FOUND, …)` counts too: it is the same ad-hoc failure
//! shape behind a function. Only the `#[cfg(test)]` item itself is blanked, not
//! the rest of its file: a test module in the middle of a handler file does not
//! hide the handlers after it.
//!
//! The rule is absolute: any tuple fails, named by file and line.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The 2xx `StatusCode` constants — the success answers a tuple may build.
const SUCCESS: &[&str] = &[
    "OK",
    "CREATED",
    "ACCEPTED",
    "NON_AUTHORITATIVE_INFORMATION",
    "NO_CONTENT",
    "RESET_CONTENT",
    "PARTIAL_CONTENT",
    "MULTI_STATUS",
    "ALREADY_REPORTED",
    "IM_USED",
];

/// The lowest number of `.rs` files a healthy walk of `services/*/src` reads.
const MIN_FILES: usize = 100;

/// `src` with every `#[cfg(test)]` item and every comment line blanked,
/// newlines kept so an offset still maps to its line.
fn production_text(src: &str) -> String {
    let mut text = String::with_capacity(src.len());
    let mut skip_depth: Option<i64> = None;
    let mut opened = false;
    for line in src.lines() {
        let t = line.trim_start();
        if skip_depth.is_none() && t.starts_with("#[cfg(test)]") {
            skip_depth = Some(0);
            opened = false;
        }
        if let Some(depth) = skip_depth.as_mut() {
            for c in line.chars() {
                match c {
                    '{' => {
                        *depth += 1;
                        opened = true;
                    }
                    '}' => *depth -= 1,
                    _ => {}
                }
            }
            // The item ends where its braces balance, or at the `;` of a
            // brace-less item (`#[cfg(test)] use …;`).
            let ended = (opened && *depth <= 0) || (!opened && t.trim_end().ends_with(';'));
            if ended {
                skip_depth = None;
            }
            text.push('\n');
            continue;
        }
        if !t.starts_with("//") {
            text.push_str(line);
        }
        text.push('\n');
    }
    text
}

/// One-based line numbers of the error tuples in `src`.
pub fn tuple_lines(src: &str) -> Vec<usize> {
    let text = production_text(src);
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    for (open, _) in text.match_indices('(') {
        let mut i = open + 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < bytes.len()
            && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b':')
        {
            i += 1;
        }
        let token = &text[start..i];
        let Some(name) = status_name(token) else {
            continue;
        };
        if SUCCESS.contains(&name) {
            continue;
        }
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i) == Some(&b',') {
            lines.push(text[..open].matches('\n').count() + 1);
        }
    }
    lines
}

/// `NAME` of a `StatusCode::NAME` token, path-qualified or not.
fn status_name(token: &str) -> Option<&str> {
    let (path, name) = token.rsplit_once("::")?;
    let qualified = path == "StatusCode" || path.ends_with("::StatusCode");
    let constant = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && name.starts_with(|c: char| c.is_ascii_uppercase());
    (qualified && constant).then_some(name)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Per file (workspace-relative, `/`-separated): the tuple lines; and the
/// number of files read.
pub fn scan(root: &Path) -> (BTreeMap<String, Vec<usize>>, usize) {
    let mut files = Vec::new();
    let mut services: Vec<_> = std::fs::read_dir(root.join("services"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("src"))
        .filter(|p| p.is_dir())
        .collect();
    services.sort();
    for src in &services {
        walk(src, &mut files);
    }
    let mut found = BTreeMap::new();
    for f in &files {
        let lines = tuple_lines(&std::fs::read_to_string(f).unwrap_or_default());
        if !lines.is_empty() {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(f)
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(rel, lines);
        }
    }
    (found, files.len())
}

/// One finding per file holding a tuple, naming each line.
pub fn findings(found: &BTreeMap<String, Vec<usize>>) -> Vec<String> {
    found
        .iter()
        .map(|(file, lines)| {
            let at: Vec<String> = lines.iter().map(|l| format!("{file}:{l}")).collect();
            format!(
                "{file}: {} `(StatusCode::…, …)` error tuple(s) — return an `ApiError` (`?`, \
                 `ApiError::bad_request`/`unprocessable`/`conflict`/`not_found`/…, \
                 `.with_code`/`.with_fields` for machine-readable keys, `ApiError::internal`, \
                 `ApiError::upstream_*`) instead:\n    {}",
                lines.len(),
                at.join("\n    ")
            )
        })
        .collect()
}

/// `cargo xtask check-error-tuples`.
pub fn run(root: &Path) -> bool {
    let (found, read) = scan(root);
    if read < MIN_FILES {
        eprintln!(
            "check-error-tuples: read {read} files under services/*/src, at least {MIN_FILES} \
             expected — the walk has stopped matching"
        );
        return false;
    }
    let out = findings(&found);
    if out.is_empty() {
        println!("check-error-tuples: no error tuple in {read} files under services/*/src");
        return true;
    }
    for f in &out {
        eprintln!("check-error-tuples: {f}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planted_tuple_is_counted_on_its_line() {
        let src = "fn h() -> Response {\n    (StatusCode::BAD_REQUEST, \"x\").into_response()\n}\n";
        assert_eq!(tuple_lines(src), [2]);
    }

    #[test]
    fn a_multi_line_and_a_qualified_tuple_count() {
        let src = "let r = (\n    StatusCode::INTERNAL_SERVER_ERROR,\n    e.to_string(),\n);\n\
                   Err((axum::http::StatusCode::BAD_GATEWAY , body))\n";
        assert_eq!(tuple_lines(src), [1, 5]);
    }

    #[test]
    fn success_codes_comments_tests_and_non_tuples_do_not_count() {
        let src = "(StatusCode::OK, Json(x))\n(StatusCode::CREATED, x)\n\
                   // (StatusCode::NOT_FOUND, \"doc\")\n\
                   /// (StatusCode::NOT_FOUND, \"doc\")\n\
                   assert_eq!(resp.status(), StatusCode::NOT_FOUND);\n\
                   (StatusCode::from_u16(500), x)\n\
                   match s { StatusCode::NOT_FOUND => 1 }\n\
                   (MyStatusCode::NOT_FOUND, x)\n\
                   #[cfg(test)]\nmod tests { fn t() { (StatusCode::NOT_FOUND, \"x\"); } }\n";
        assert!(tuple_lines(src).is_empty(), "{:?}", tuple_lines(src));
    }

    /// A test module in the middle of a file blanks itself, not the handlers
    /// after it — the shape that hid `edmd`'s correction and bulk doors.
    #[test]
    fn a_test_module_hides_only_itself() {
        let src = "#[cfg(test)]\nmod t {\n    fn x() { (StatusCode::NOT_FOUND, \"x\"); }\n}\n\
                   fn h() -> Response {\n    (StatusCode::BAD_REQUEST, \"x\").into_response()\n}\n\
                   #[cfg(test)]\nuse foo::bar;\n(StatusCode::CONFLICT, \"y\")\n";
        assert_eq!(tuple_lines(src), [6, 10]);
    }

    #[test]
    fn any_tuple_fails_naming_its_line() {
        let found = BTreeMap::from([("services/a/src/h.rs".to_owned(), vec![3, 7])]);
        let out = findings(&found);
        assert_eq!(out.len(), 1);
        assert!(
            out[0].contains("services/a/src/h.rs:3") && out[0].contains("services/a/src/h.rs:7"),
            "{out:?}"
        );
        assert!(findings(&BTreeMap::new()).is_empty());
    }

    /// Today's tree holds no tuple, and a planted one fails the scan.
    #[test]
    fn the_real_tree_holds_no_tuple_and_a_planted_one_fails() {
        let root = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/.."));
        let (mut found, read) = scan(root);
        assert!(read >= MIN_FILES, "{read}");
        assert_eq!(findings(&found), Vec::<String>::new());
        found.insert(
            "services/planted/src/lib.rs".into(),
            tuple_lines("(StatusCode::BAD_REQUEST, \"x\")"),
        );
        assert_eq!(findings(&found).len(), 1);
    }
}
