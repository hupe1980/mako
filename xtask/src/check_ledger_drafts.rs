//! Guard: money reaches accountingd's books only through a booking draft, and
//! a cent amount never passes through `f64`.
//!
//! ## Writes go through `booking`
//!
//! doubleentry's ledger owns its pool and joins no external transaction, so a
//! business row and its ledger entry cannot commit together. accountingd's
//! answer is the booking draft (`services/accountingd/src/booking.rs`): the
//! whole entry is staged in the business transaction and recorded after the
//! commit, idempotent by key, with a recovery worker for whatever a crash left
//! open. A call to the ledger's write methods anywhere else commits the entry
//! on its own — the half-applied write the draft exists to prevent, which works
//! in every test and loses a booking on the first crash between two commits.
//!
//! So in `services/accountingd/src`, `ledger.post(` and doubleentry's `.record(`,
//! `.record_batch(` and `.reversal(` appear only in `booking.rs` and
//! `ledger.rs`.
//!
//! ## No cent amount through `f64`
//!
//! `format!("{:.2}", ct as f64 / 100.0)` renders most amounts right and some
//! wrong: above 2^53 ct the value is no longer exact, and the `{:.2}` rounding
//! of a binary fraction is not kaufmännisch. Money is formatted from integers
//! (`format_ct_as_eur`). So nowhere in `crates/` or `services/` source does
//! `as f64 / 100` appear.

use std::path::Path;

/// The files allowed to call the ledger's write methods.
const WRITERS: &[&str] = &["booking.rs", "ledger.rs"];

/// The ledger write calls the draft path owns.
const WRITE_CALLS: &[&str] = &["ledger.post(", ".record(", ".record_batch(", ".reversal("];

/// The cent-through-float shape.
const CENT_FLOAT: &str = "as f64 / 100";

/// Scan the workspace. Returns `true` when both rules hold.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    let accountingd = workspace_root.join("services/accountingd/src");
    for (path, text) in rust_files(&accountingd) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if WRITERS.contains(&name) {
            continue;
        }
        findings.extend(ledger_writes(&text).into_iter().map(|(line, call)| {
            format!(
                "{}:{line}: `{call}` writes the ledger outside a booking draft — stage it with \
                 `booking::stage` in the business transaction and `booking::book` after the commit",
                rel(workspace_root, &path)
            )
        }));
    }
    for dir in ["crates", "services"] {
        for (path, text) in rust_files(&workspace_root.join(dir)) {
            if path.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            findings.extend(cent_floats(&text).into_iter().map(|line| {
                format!(
                    "{}:{line}: a cent amount formatted through `f64` — format the integer \
                     (`format_ct_as_eur`)",
                    rel(workspace_root, &path)
                )
            }));
        }
    }
    if findings.is_empty() {
        println!(
            "check-ledger-drafts: every ledger write is a booking draft; no cents through f64"
        );
        true
    } else {
        for f in &findings {
            eprintln!("check-ledger-drafts: {f}");
        }
        eprintln!("check-ledger-drafts: {} problem(s)", findings.len());
        false
    }
}

/// `(line, call)` for each ledger write call in `text`, comments skipped.
fn ledger_writes(text: &str) -> Vec<(usize, &'static str)> {
    code_lines(text)
        .flat_map(|(n, line)| {
            WRITE_CALLS
                .iter()
                .filter(move |call| line.contains(*call))
                .map(move |call| (n, *call))
        })
        .collect()
}

/// Line numbers where a cent amount is formatted through `f64`.
fn cent_floats(text: &str) -> Vec<usize> {
    code_lines(text)
        .filter(|(_, line)| line.contains(CENT_FLOAT))
        .map(|(n, _)| n)
        .collect()
}

/// `(1-based line, text)` for each line that is not a comment.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
        .filter(|(_, l)| !l.trim_start().starts_with("//"))
}

fn rust_files(dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .is_some_and(|n| n == "target" || n == "node_modules")
                {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push((path, text));
            }
        }
    }
    out
}

fn rel(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_handler_posting_to_the_ledger_is_refused() {
        let handler = "    let posted = ledger.post(&draft).await?;\n";
        assert_eq!(ledger_writes(handler), vec![(1, "ledger.post(")]);
    }

    #[test]
    fn an_axum_route_and_a_comment_are_not_ledger_writes() {
        let router = "    .route(\"/x\", post(handlers::post_x))\n    // ledger.post( stays out\n";
        assert!(ledger_writes(router).is_empty());
    }

    #[test]
    fn a_cent_float_is_refused_and_the_integer_form_is_not() {
        let bad = "    \"amount_eur\": format!(\"{:.2}\", ct as f64 / 100.0),\n";
        assert_eq!(cent_floats(bad), vec![1]);
        let good = "    \"amount_eur\": format_ct_as_eur(ct),\n";
        assert!(cent_floats(good).is_empty());
    }

    #[test]
    fn the_tree_holds_both_rules() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root), "see the findings above");
    }
}
