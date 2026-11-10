//! Guard: SQL built at runtime is asserted safe in one place, and each file
//! builds only as much of it as its listed reason covers.
//!
//! sqlx refuses a query string that is not `'static` unless it is wrapped in
//! `AssertSqlSafe` — a string assembled at runtime is how SQL injection gets
//! in. mako's runtime SQL is assembled only from its own fragments, every value
//! bound; `mako_service::sql::dynamic` is where that is asserted. So:
//!
//! 1. `AssertSqlSafe` appears only in `crates/mako-service/src/sql.rs`.
//! 2. Every file calling `sql::dynamic(` is listed in [`SITES`] with its count
//!    and why its statements are built at runtime. A new call is a deliberate
//!    act — the reviewer reads the statement it builds — and a removed one
//!    shrinks the list, so the count can neither grow silently nor go stale.

use std::collections::BTreeMap;
use std::path::Path;

/// A column-list or clause constant spliced into a fixed statement.
const SPLICE: &str =
    "a column-list or clause constant spliced into a fixed statement; every value is bound";
/// A `GRANT` naming the configured runtime role.
const GRANT: &str = "a GRANT naming the configured runtime role, quoted as an identifier";

/// Every file that builds SQL at runtime: `(path, calls, why)`.
const SITES: &[(&str, usize, &str)] = &[
    ("crates/mako-service/src/service.rs", 1, GRANT),
    ("services/accountingd/src/pg.rs", 6, SPLICE),
    ("services/einsd/src/pg_einspeiser.rs", 3, SPLICE),
    (
        "services/einsd/tests/it/settlement_integration.rs",
        1,
        SPLICE,
    ),
    ("services/marktd/src/mcp_server.rs", 2, SPLICE),
    ("services/marktd/src/pg/bilanzierung.rs", 2, SPLICE),
    ("services/marktd/src/pg/correlation.rs", 3, SPLICE),
    ("services/marktd/src/pg/msb_rahmenvertrag.rs", 3, SPLICE),
    ("services/marktd/src/pg/nb_contract.rs", 3, SPLICE),
    ("services/marktd/src/pg/netzzugang.rs", 3, SPLICE),
    ("services/marktd/src/pg/partner.rs", 2, SPLICE),
    ("services/marktd/src/pg/subscription.rs", 3, SPLICE),
    ("services/marktd/src/pg/versorgung.rs", 3, SPLICE),
    ("services/netzbilanzd/src/pg.rs", 4, SPLICE),
    ("services/outputd/src/delivery/store.rs", 2, SPLICE),
    ("services/processd/src/obs/mcp.rs", 1, SPLICE),
    ("services/processd/src/obs/pg.rs", 5, SPLICE),
    ("services/processd/src/obs/rest.rs", 2, SPLICE),
    ("services/processd/src/obs/worker.rs", 2, SPLICE),
    ("services/processd/src/pg/approval.rs", 3, SPLICE),
    ("services/processd/src/pg/neuanlage.rs", 3, SPLICE),
    ("services/processd/src/sperr/pg.rs", 3, SPLICE),
    ("services/vertragd/src/pg/aggregator.rs", 2, SPLICE),
    ("services/vertragd/src/pg/kunden.rs", 3, SPLICE),
    ("services/vertragd/src/pg/messstellenvertrag.rs", 1, SPLICE),
    ("services/vertragd/src/pg/produkte.rs", 4, SPLICE),
    ("services/vertragd/src/pg/sect14a.rs", 2, SPLICE),
    ("services/vertragd/src/pg/vertraege.rs", 5, SPLICE),
];

/// Scan the workspace. Returns `true` when both rules hold.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings = Vec::new();
    let mut found: BTreeMap<String, usize> = BTreeMap::new();
    for dir in ["crates", "services"] {
        for (path, text) in rust_files(&workspace_root.join(dir)) {
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .display()
                .to_string();
            if rel != "crates/mako-service/src/sql.rs" && text.contains("AssertSqlSafe") {
                findings.push(format!(
                    "{rel}: `AssertSqlSafe` outside `mako_service::sql` — call `sql::dynamic`"
                ));
            }
            let n = dynamic_calls(&text);
            if n > 0 {
                found.insert(rel, n);
            }
        }
    }
    findings.extend(compare(&found));
    if findings.is_empty() {
        println!(
            "check-dynamic-sql: {} call(s) across {} file(s), each listed with its reason",
            found.values().sum::<usize>(),
            found.len()
        );
        true
    } else {
        for f in &findings {
            eprintln!("check-dynamic-sql: {f}");
        }
        false
    }
}

/// `sql::dynamic(` calls in `text`, comments skipped.
fn dynamic_calls(text: &str) -> usize {
    text.lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| l.matches("sql::dynamic(").count())
        .sum()
}

/// Where the tree and [`SITES`] disagree.
fn compare(found: &BTreeMap<String, usize>) -> Vec<String> {
    let listed: BTreeMap<&str, usize> = SITES.iter().map(|(p, n, _)| (*p, *n)).collect();
    let mut out = Vec::new();
    for (path, n) in found {
        match listed.get(path.as_str()) {
            None => out.push(format!(
                "{path}: {n} `sql::dynamic` call(s) and no entry in SITES — list it with why it builds SQL at runtime"
            )),
            Some(&want) if want != *n => out.push(format!(
                "{path}: {n} `sql::dynamic` call(s), SITES lists {want} — read the change, then update the count"
            )),
            Some(_) => {}
        }
    }
    for path in listed.keys() {
        if !found.contains_key(*path) {
            out.push(format!(
                "{path}: listed in SITES and builds no SQL at runtime — remove it"
            ));
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unlisted_file_and_a_grown_count_are_refused() {
        let mut found = BTreeMap::new();
        found.insert("services/new/src/pg.rs".to_owned(), 1);
        found.insert(SITES[0].0.to_owned(), SITES[0].1 + 1);
        let findings = compare(&found);
        assert!(
            findings.iter().any(|f| f.contains("no entry in SITES")),
            "{findings:?}"
        );
        assert!(
            findings.iter().any(|f| f.contains("update the count")),
            "{findings:?}"
        );
    }

    #[test]
    fn a_comment_is_not_a_call() {
        assert_eq!(
            dynamic_calls("// sql::dynamic( in prose\nsqlx::query(sql::dynamic(s))\n"),
            1
        );
    }

    #[test]
    fn the_tree_matches_the_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(run(&root), "see the findings above");
    }
}
