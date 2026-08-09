//! Guard: axum 0.8 path parameters.
//!
//! axum 0.7 spelled a capture `/:id`; 0.8 spells it `/{id}` and **panics** at
//! `Router::route` on the old form:
//!
//! ```text
//! Path segments must not start with `:`. For capture groups, use `{capture}`.
//! ```
//!
//! That panic happens while the router is being assembled, which for a mako
//! daemon is startup — so the failure is a service that will not boot, found by
//! whoever deploys it rather than by the compiler. Five daemons carried it after
//! the axum 0.8 upgrade (78 routes in total), because nothing in the test suite
//! builds those routers.
//!
//! This check is the missing compiler. It scans every route literal in the
//! workspace and refuses the old spelling.

use std::path::Path;

/// Scan the workspace for axum 0.7-style route parameters.
///
/// Returns `true` when every route literal uses the 0.8 spelling.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();

    for dir in ["services", "crates"] {
        collect(&workspace_root.join(dir), &mut findings);
    }

    if findings.is_empty() {
        println!("check-routes: every route literal uses axum 0.8 `{{param}}` syntax");
        return true;
    }

    eprintln!(
        "check-routes: {} route literal(s) use axum 0.7 syntax and would panic at startup:",
        findings.len()
    );
    for (path, line, literal) in &findings {
        eprintln!("  {}:{line}  \"{literal}\"", path.display());
    }
    eprintln!("\nRewrite `/:name` as `/{{name}}` and `/*rest` as `/{{*rest}}`.");
    false
}

/// Every `.rs` file under `dir`, walked without a crate dependency.
fn collect(dir: &Path, findings: &mut Vec<(std::path::PathBuf, usize, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // `target/` under a crate would be generated code, and there is a
            // lot of it.
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect(&path, findings);
        } else if path.extension().is_some_and(|e| e == "rs") {
            scan_file(&path, findings);
        }
    }
}

fn scan_file(path: &Path, findings: &mut Vec<(std::path::PathBuf, usize, String)>) {
    let Ok(src) = std::fs::read_to_string(path) else {
        return;
    };
    for (index, line) in src.lines().enumerate() {
        // A route literal is a string starting with `/`. Restricting to that
        // shape keeps SQL casts (`::text`), URLs (`https://`) and format
        // strings out of the result.
        for literal in string_literals(line) {
            if !literal.starts_with('/') {
                continue;
            }
            if literal.contains("/:") || literal.contains("/*") {
                findings.push((path.to_path_buf(), index + 1, literal));
            }
        }
    }
}

/// The double-quoted literals on one line, unescaped only enough to end them.
fn string_literals(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut literal = String::new();
        let mut closed = false;
        while let Some((_, c)) = chars.next() {
            match c {
                '\\' => {
                    chars.next();
                }
                '"' => {
                    closed = true;
                    break;
                }
                other => literal.push(other),
            }
        }
        if closed {
            out.push(literal);
        }
    }
    out
}
