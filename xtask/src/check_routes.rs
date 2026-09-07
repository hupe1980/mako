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
//!
//! ## Untyped MCP tool parameters
//!
//! Same failure mode from a third direction. `rmcp` builds a tool's
//! `inputSchema` from its `Parameters<T>` type, and the MCP specification
//! requires that schema's root type to be `object`. `serde_json::Value`
//! schematises to the *empty* schema — no `type` — so a single tool declared
//! `Parameters<serde_json::Value>` panics the router build:
//!
//! ```text
//! Invalid input schema for `Parameters<serde_json::value::Value>`:
//! Schema is missing 'type' field.
//! ```
//!
//! `productd` and `accountingd` could not start at all. An argument-less tool
//! takes an empty struct; one that reads fields takes a struct naming them,
//! which is also the only way a model learns what to send.
//!
//! ## Two captures at one position
//!
//! axum matches a capture by **position**, not by name, so
//! `POST /api/v1/documents/{kind}` and `GET /api/v1/documents/{document_id}`
//! are the same place under two names — and registering both panics:
//!
//! ```text
//! Invalid route "/api/v1/documents/{document_id}": Insertion failed due to
//! conflict with previously registered route: /api/v1/documents/{kind}
//! ```
//!
//! Same failure mode, same blast radius: `outputd` could not start at all, and
//! nothing noticed until a demo ran it. Two literals in one crate that agree
//! segment for segment except for the *name* of a capture are therefore
//! reported here too. The fix is never to rename one — they are different
//! resources — but to give the odd one its own segment.

use std::path::Path;

/// Scan the workspace for axum 0.7-style route parameters.
///
/// Returns `true` when every route literal uses the 0.8 spelling.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();

    let mut conflicts = Vec::new();
    let mut untyped_tools = Vec::new();
    for dir in ["services", "crates"] {
        collect(&workspace_root.join(dir), &mut findings);
        collect_conflicts(&workspace_root.join(dir), &mut conflicts);
        collect_untyped_mcp_params(&workspace_root.join(dir), &mut untyped_tools);
    }

    if !untyped_tools.is_empty() {
        eprintln!(
            "check-routes: {} MCP tool(s) declare `Parameters<serde_json::Value>`, whose JSON \
             Schema has no root `type` — the router build panics and the service cannot start:",
            untyped_tools.len()
        );
        for (path, line) in &untyped_tools {
            eprintln!("  {}:{line}", path.display());
        }
        eprintln!(
            "\nUse an empty `#[derive(JsonSchema)] struct NoParams {{}}` for a tool that takes \
             no arguments, or a struct naming the fields it reads."
        );
    }

    if !conflicts.is_empty() {
        eprintln!(
            "check-routes: {} route pair(s) put two differently-named captures at one \
             position, which panics `Router::route` at startup:",
            conflicts.len()
        );
        for (crate_name, a, b) in &conflicts {
            eprintln!("  {crate_name}: \"{a}\"  vs  \"{b}\"");
        }
        eprintln!(
            "\naxum matches a capture by position, not by name. Give one of the two its own \
             path segment — they are different resources."
        );
    }

    if findings.is_empty() && conflicts.is_empty() && untyped_tools.is_empty() {
        println!(
            "check-routes: every route literal uses axum 0.8 `{{param}}` syntax, no two put \
             different captures at one position, and every MCP tool has a typed parameter"
        );
        return true;
    }
    if findings.is_empty() {
        return false;
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
/// MCP tools whose parameter type schematises without a root `type`.
fn collect_untyped_mcp_params(dir: &Path, out: &mut Vec<(std::path::PathBuf, usize)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            collect_untyped_mcp_params(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (i, line) in src.lines().enumerate() {
                // The guard states the shape it refuses, so it must not report
                // itself.
                if line.contains("Parameters<serde_json::Value>")
                    && !line.trim_start().starts_with("//")
                {
                    out.push((path.to_path_buf(), i + 1));
                }
            }
        }
    }
}

/// Route literals that would collide inside one crate's router.
///
/// Grouped per crate directly under the scanned root: a router is assembled
/// from one crate's sources, and two crates may legitimately serve the same
/// shape under different capture names.
fn collect_conflicts(root: &Path, out: &mut Vec<(String, String, String)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut literals = Vec::new();
        gather_route_literals(&path, &mut literals);
        literals.sort();
        literals.dedup();
        // A capture's *name* is what may differ; everything else must match.
        for (i, a) in literals.iter().enumerate() {
            for b in &literals[i + 1..] {
                if a != b && same_shape(a, b) {
                    out.push((name.clone(), a.clone(), b.clone()));
                }
            }
        }
    }
}

/// Whether two route literals put a capture at the same position with a
/// different name, and are otherwise identical.
fn same_shape(a: &str, b: &str) -> bool {
    let (sa, sb): (Vec<&str>, Vec<&str>) = (a.split('/').collect(), b.split('/').collect());
    if sa.len() != sb.len() {
        return false;
    }
    let mut differs = false;
    for (x, y) in sa.iter().zip(&sb) {
        let both_captures = x.starts_with('{') && y.starts_with('{');
        if x == y {
            continue;
        }
        if both_captures {
            differs = true;
            continue;
        }
        return false;
    }
    differs
}

/// Every `/`-prefixed string literal passed to `.route(` in a crate.
fn gather_route_literals(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            gather_route_literals(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // `.route(` may sit on its own line with the literal on the next,
            // so the marker is looked for in a two-line window.
            let lines: Vec<&str> = src.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let prev = if i > 0 { lines[i - 1] } else { "" };
                if !(line.contains(".route(") || prev.trim_end().ends_with(".route(")) {
                    continue;
                }
                for literal in string_literals(line) {
                    if literal.starts_with('/') {
                        out.push(literal);
                    }
                }
            }
        }
    }
}

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
