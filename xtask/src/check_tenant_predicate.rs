//! Guard: a statement against a tenant-scoped table says which tenant.
//!
//! `tenant` is this platform's data-isolation key — a column on every row of
//! every multi-tenant table. Nothing in Postgres enforces that a query carries
//! it: a statement that omits the predicate is valid SQL, returns rows, and
//! looks exactly like one that is correctly scoped. The failure is silent in
//! the expensive direction, because the rows it reaches belong to somebody
//! else.
//!
//! The shape that got here was `UPDATE accounts … WHERE malo_id = $1 AND
//! lf_mp_id = $2` in `accountingd`. Neither column is a key — `accounts` is
//! keyed on `account_id` and carries only *non-unique* indexes over that pair —
//! so the statement matched a row in every tenant holding that Marktlokation,
//! and what it wrote was the IBAN, the Mandatsreferenz and the Abschlag. The
//! caller read the account tenant-scoped first, which is what made it look
//! right: that read proves the row exists in this tenant, not that it is the
//! only row the `UPDATE` reaches.
//!
//! ## The rule
//!
//! A statement touching a tenant-scoped table must either
//!
//! 1. name `tenant`, or
//! 2. filter on that table's **own primary key**.
//!
//! The second is not an exemption, it is the other way isolation can hold: a
//! `UUID` primary key is unique across the whole table and therefore across
//! every tenant in it, so `WHERE id = $1` already addresses exactly one row.
//! A *foreign* key is not that — `WHERE document_id = $1` on
//! `document_deliveries` selects by a column this table does not own, so the
//! predicate alone decides the rows and the tenant bound has to be written.
//! Distinguishing the two is the whole value of the check, and it is why the
//! rule needs no exemption list: an exemption records that somebody decided a
//! case was fine, and the reason rots where the decision cannot be re-derived.
//!
//! ## What is scanned
//!
//! Raw-string SQL literals (`r"…"`) under `services/*/src/**`, against the set
//! of tenant-scoped tables read out of **that service's own migrations**. The
//! schema is the authority, so the guard cannot disagree with it: a table that
//! gains a `tenant` column comes into scope automatically, and one that loses
//! it drops out.
//!
//! ## What it deliberately does not do
//!
//! It does not parse SQL. A textual scan cannot tell a correlated subquery's
//! `tenant` from the outer statement's, so a statement that names the column
//! anywhere passes. That is the honest limit of a cheap check, and it is still
//! the difference between „somebody thought about isolation here" and „nobody
//! did" — which is the state the defect above was in.

use std::collections::BTreeMap;
use std::path::Path;

/// The lowest number of tenant-scoped statements a healthy scan may find.
///
/// Measured, for the reason every floor here is: a floor set under what the
/// tree holds tolerates exactly the failure it exists to catch. A scan that
/// finds fewer has lost its grip on the layout, not found a cleaner codebase.
const MIN_STATEMENTS: usize = 250;

/// Tenant-scoped tables of one service, each mapped to its primary-key column.
///
/// A table with no single-column `PRIMARY KEY` maps to `None`: it has no key
/// that could stand in for the tenant bound, so only rule 1 is open to it.
fn tenant_tables(migrations: &Path) -> BTreeMap<String, Option<String>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(migrations) else {
        return out;
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (name, body) in create_tables(&src) {
            if !declares_column(&body, "tenant") {
                continue;
            }
            out.insert(name, primary_key(&body));
        }
    }
    out
}

/// `(table name, column body)` for every `CREATE TABLE` in `src`.
fn create_tables(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let lower = src.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find("create table") {
        let at = from + i;
        let rest = &src[at..];
        let Some(open) = rest.find('(') else { break };
        let head = &rest["create table".len()..open];
        let name = head
            .split_whitespace()
            .last()
            .unwrap_or_default()
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .trim_matches('"')
            .to_ascii_lowercase();
        // Balance to the matching close paren.
        let mut depth = 0usize;
        let mut end = None;
        for (j, ch) in rest[open..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + j);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => {
                if !name.is_empty() {
                    out.push((name, rest[open + 1..e].to_owned()));
                }
                from = at + e;
            }
            None => break,
        }
    }
    out
}

/// Whether `body` declares a column named `col` at the start of a line.
fn declares_column(body: &str, col: &str) -> bool {
    body.lines()
        .filter_map(|l| l.split_whitespace().next())
        .any(|first| first.eq_ignore_ascii_case(col))
}

/// The single-column primary key declared inline in `body`, if there is one.
fn primary_key(body: &str) -> Option<String> {
    for line in body.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("primary key") {
            continue;
        }
        let first = line.split_whitespace().next()?;
        // A table-level `PRIMARY KEY (a, b)` starts with the keyword itself and
        // names no column of its own here; it is composite and cannot stand in
        // for the tenant bound.
        if first.eq_ignore_ascii_case("primary") {
            return None;
        }
        return Some(first.trim_matches('"').to_ascii_lowercase());
    }
    None
}

/// Table names appearing after `FROM`/`JOIN`/`UPDATE`/`INTO` in `sql`.
fn touched_tables(sql: &str) -> Vec<String> {
    const KEYWORDS: [&str; 4] = ["from", "join", "update", "into"];
    let toks: Vec<&str> = sql.split(|c: char| c.is_whitespace()).collect();
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if KEYWORDS.iter().any(|k| t.eq_ignore_ascii_case(k))
            && let Some(next) = toks.get(i + 1)
        {
            let name: String = next
                .trim_matches(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .to_ascii_lowercase();
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// Whether `sql` filters on `column` (`column =` or `alias.column =`).
fn filters_on(sql: &str, column: &str) -> bool {
    let lower = sql.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(column) {
        let at = from + i;
        from = at + column.len();
        // Must be a whole identifier.
        let before_ok = at == 0 || {
            let c = lower.as_bytes()[at - 1];
            !(c.is_ascii_alphanumeric() || c == b'_')
        };
        let after = lower[from..].trim_start();
        if before_ok && after.starts_with('=') {
            return true;
        }
    }
    false
}

/// Every raw-string literal in `src` that looks like a SQL statement, with its
/// 1-based line number.
fn sql_literals(src: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] == b'r' && bytes[i + 1] == b'"' {
            // Not a suffix of an identifier (`for"` etc.).
            let ok = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if ok && let Some(close) = src[i + 2..].find('"') {
                let body = &src[i + 2..i + 2 + close];
                if body.len() >= 25 {
                    let upper = body.to_ascii_uppercase();
                    if ["SELECT", "UPDATE", "DELETE", "INSERT"]
                        .iter()
                        .any(|k| upper.contains(k))
                    {
                        out.push((body.to_owned(), src[..i].matches('\n').count() + 1));
                    }
                }
                i = i + 2 + close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Scan every service for a statement that does not say which tenant.
///
/// Returns `true` when every tenant-scoped statement carries the tenant or
/// addresses its table's own primary key.
#[must_use]
pub fn run(workspace_root: &Path) -> bool {
    scan(workspace_root, MIN_STATEMENTS)
}

/// [`run`], with the statement floor as a parameter so the tests can exercise
/// the rule on a fixture without tripping the production floor.
fn scan(workspace_root: &Path, floor: usize) -> bool {
    let services = workspace_root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        println!("check-tenant-predicate: FAIL — services/ is not readable");
        return false;
    };
    let mut dirs: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();

    let mut scanned = 0usize;
    let mut findings: Vec<String> = Vec::new();

    for dir in dirs {
        let tables = tenant_tables(&dir.join("migrations"));
        if tables.is_empty() {
            continue;
        }
        let src_dir = dir.join("src");
        let mut stack = vec![src_dir];
        let mut files = Vec::new();
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else {
                continue;
            };
            for e in rd.filter_map(Result::ok) {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
                    files.push(p);
                }
            }
        }
        files.sort();

        for path in files {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (sql, line) in sql_literals(&src) {
                let hit: Vec<String> = touched_tables(&sql)
                    .into_iter()
                    .filter(|t| tables.contains_key(t))
                    .collect();
                if hit.is_empty() {
                    continue;
                }
                scanned += 1;
                if filters_on(&sql, "tenant") || sql.to_ascii_lowercase().contains("tenant") {
                    continue;
                }
                if hit.iter().any(|t| {
                    tables
                        .get(t)
                        .and_then(Option::as_ref)
                        .is_some_and(|pk| filters_on(&sql, pk))
                }) {
                    continue;
                }
                let rel = path.strip_prefix(workspace_root).unwrap_or(&path);
                let one_line: String = sql.split_whitespace().collect::<Vec<_>>().join(" ");
                let shown: String = one_line.chars().take(100).collect();
                findings.push(format!("{}:{line}: {hit:?}\n      {shown}", rel.display()));
            }
        }
    }

    if scanned < floor {
        println!(
            "check-tenant-predicate: FAIL — found {scanned} tenant-scoped statement(s), at least \
             {floor} are expected.\n\
             The layout or the literal style moved and this guard has to be taught it; it has \
             not stopped being needed."
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-tenant-predicate: every one of {scanned} statement(s) against a tenant-scoped \
             table carries the tenant or addresses its table's own primary key"
        );
        return true;
    }

    println!(
        "check-tenant-predicate: FAIL — {} statement(s) against a tenant-scoped table say \
         nothing about which tenant:\n",
        findings.len()
    );
    for f in &findings {
        println!("  {f}");
    }
    println!(
        "\n`tenant` is the data-isolation key and Postgres does not supply it: a statement that \
         omits the predicate is valid SQL that reaches another operator's rows.\n\
         Add the tenant predicate, or — only where the filter is the table's **own** primary key, \
         which is already unique across every tenant — none is needed. A *foreign* key is not \
         that, and a caller that read the row tenant-scoped beforehand is not either."
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_reads_the_schema_for_tenant_scoped_tables_and_their_keys() {
        let body = "\n    account_id UUID PRIMARY KEY DEFAULT gen_random_uuid(),\n    malo_id TEXT NOT NULL,\n    tenant TEXT NOT NULL\n";
        assert!(declares_column(body, "tenant"));
        assert_eq!(primary_key(body).as_deref(), Some("account_id"));
    }

    /// A composite table-level key cannot stand in for the tenant bound.
    #[test]
    fn a_composite_primary_key_is_not_a_single_column_key() {
        let body =
            "\n    a TEXT NOT NULL,\n    tenant TEXT NOT NULL,\n    PRIMARY KEY (a, tenant)\n";
        assert_eq!(primary_key(body), None);
    }

    #[test]
    fn it_finds_the_touched_tables() {
        let sql = "UPDATE accounts SET x = 1 WHERE malo_id = $1";
        assert_eq!(touched_tables(sql), vec!["accounts".to_owned()]);
        let j = "SELECT * FROM a JOIN b ON a.id = b.id";
        assert_eq!(touched_tables(j), vec!["a".to_owned(), "b".to_owned()]);
    }

    /// `filters_on` matches a whole identifier, not a substring of one.
    #[test]
    fn a_column_name_is_not_matched_inside_a_longer_one() {
        assert!(filters_on("WHERE id = $1", "id"));
        assert!(filters_on("WHERE r.id = $2", "id"));
        assert!(!filters_on("WHERE document_id = $1", "id"));
        assert!(!filters_on("SELECT id FROM t", "id"));
    }

    /// The defect this guard exists for, and the two ways a statement is safe.
    ///
    /// A guard that cannot fail certifies nothing, so the first tree is the one
    /// that makes it fail.
    #[test]
    fn it_refuses_a_statement_that_names_no_tenant() {
        let dir = std::env::temp_dir().join("mako-check-tenant-predicate");
        let _ = std::fs::remove_dir_all(&dir);
        let svc = dir.join("services/a");
        std::fs::create_dir_all(svc.join("migrations")).unwrap();
        std::fs::create_dir_all(svc.join("src")).unwrap();
        std::fs::write(
            svc.join("migrations/0001.sql"),
            "CREATE TABLE accounts (\n    account_id UUID PRIMARY KEY,\n    malo_id TEXT NOT NULL,\n    tenant TEXT NOT NULL\n);\n",
        )
        .unwrap();

        // Filtering on non-key business columns: the defect.
        std::fs::write(
            svc.join("src/lib.rs"),
            "fn f() { q(r\"UPDATE accounts SET iban = $3 WHERE malo_id = $1 AND lf_mp_id = $2\"); }",
        )
        .unwrap();
        assert!(
            !scan(&dir, 1),
            "a statement naming no tenant must be refused"
        );

        // Rule 1 — the tenant predicate.
        std::fs::write(
            svc.join("src/lib.rs"),
            "fn f() { q(r\"UPDATE accounts SET iban = $3 WHERE malo_id = $1 AND tenant = $2\"); }",
        )
        .unwrap();
        assert!(scan(&dir, 1), "the tenant predicate is enough");

        // Rule 2 — the table's own primary key.
        std::fs::write(
            svc.join("src/lib.rs"),
            "fn f() { q(r\"UPDATE accounts SET iban = $2 WHERE account_id = $1\"); }",
        )
        .unwrap();
        assert!(scan(&dir, 1), "the table's own key is enough");

        // A *foreign* key is not.
        std::fs::write(
            svc.join("src/lib.rs"),
            "fn f() { q(r\"SELECT * FROM accounts WHERE kunden_nr = $1 ORDER BY malo_id\"); }",
        )
        .unwrap();
        assert!(
            !scan(&dir, 1),
            "a column the table does not key on is not an isolation bound"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The floor binds: an emptied layout is a moved layout, not a clean tree.
    #[test]
    fn an_empty_tree_is_refused() {
        let dir = std::env::temp_dir().join("mako-check-tenant-predicate-empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("services")).unwrap();
        assert!(!run(&dir), "a tree with no statement must be refused");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
