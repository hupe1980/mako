//! Guard: every SQL literal a service runs parses against that service's own
//! schema.
//!
//! mako uses `sqlx::query` / `query_as`, which prepare at run time. Nothing in
//! the build or the test suite sees the SQL, so a statement Postgres will not
//! accept compiles, ships, and fails on the first request that reaches it. Two
//! did:
//!
//! - `marktd`'s `GET /melos/{id}/sharing-eligibility` scoped `melo` by
//!   `m.tenant`, and `melo` — a market-wide identity table — has no `tenant`
//!   column. Cedar admitted the caller, the handler ran, and the answer was a
//!   500 with a Postgres message in the body.
//! - `accountingd`'s `eeg_payout_orders` INSERT named nineteen columns against
//!   eighteen placeholders, so every automatic pain.001 credit transfer for an
//!   EEG feed-in payment failed at the database and became one
//!   `tracing::warn!` in a path whose caller returns `()`.
//!
//! This asks the only thing that knows. One throwaway `postgres:17-alpine`, one
//! database per service, that service's migrations applied, and every SQL
//! literal in its sources handed to `PREPARE`.
//!
//! **What it reports.** Only the errors that are a defect whatever the caller
//! binds — a column or table the schema does not have, and an argument list
//! that does not match its target. Postgres' own „could not determine data type
//! of parameter" is not one: `$1` in a bare comparison is genuinely ambiguous
//! until sqlx binds a Rust type to it, and marktd's `preisblatt` `->` queries
//! reach it legitimately.
//!
//! A statement assembled with `format!` is checked too. What it interpolates is
//! always a constant the crate holds — a column list, a terminal-state set — so
//! those are read out of the sources and substituted. A placeholder naming
//! anything else is a runtime value; that call site is counted as unresolved
//! and never guessed at, so the gap is a number rather than an assumption.
//!
//! **Where the server comes from.** `MAKO_CHECK_SQL_URL` names one that is
//! already running and the check connects to it; otherwise it starts a
//! throwaway container and removes it afterwards. Neither available is a
//! refusal, never a green — a gate that reports success from a skip is not a
//! gate. `just test-db` takes the container path locally; CI takes the URL,
//! because a hosted runner has `services: postgres` and `psql` but no daemon a
//! job can reliably `docker exec` into.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Postgres SQLSTATEs that mean the schema will not accept the statement.
const BROKEN: &[&str] = &[
    "42703", // undefined_column
    "42P01", // undefined_table
    "42601", // syntax_error — reached here as a column/expression count mismatch
    "42P10", // invalid_column_reference
];

/// The container this runs against.
const IMAGE: &str = "postgres:17-alpine";
const CONTAINER: &str = "mako-check-sql";

/// One SQL literal and where it is written.
struct Statement {
    file: String,
    line: usize,
    sql: String,
}

/// Check every service's SQL against its own schema.
///
/// Returns `true` when every statement prepares.
pub fn run(workspace_root: &Path) -> bool {
    let services = workspace_root.join("services");
    let mut names: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir(&services) else {
        eprintln!("check-sql: no services directory at {}", services.display());
        return false;
    };
    for entry in entries.flatten() {
        if migrations(&entry.path()).is_empty() {
            continue;
        }
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    if names.is_empty() {
        eprintln!("check-sql: no service carries a schema");
        return false;
    }

    let Some(guard) = Postgres::open() else {
        return false;
    };

    let mut failed = false;
    let mut totals = (0usize, 0usize);
    for name in &names {
        let dir = services.join(name);
        let (statements, interpolated) = collect(&dir, workspace_root);
        totals.0 += statements.len();
        totals.1 += interpolated;
        match guard.check(
            name,
            &migrations(&dir),
            runs_ensure_schema(&dir),
            &statements,
        ) {
            Ok(broken) if broken.is_empty() => {
                println!(
                    "  {name:<14} {:>4} statement(s) prepared, {interpolated} unresolved",
                    statements.len()
                );
            }
            Ok(broken) => {
                failed = true;
                println!(
                    "  {name:<14} {:>4} statement(s), {} REFUSED",
                    statements.len(),
                    broken.len()
                );
                for line in broken {
                    println!("      {line}");
                }
            }
            Err(e) => {
                failed = true;
                println!("  {name:<14} could not be checked: {e}");
            }
        }
    }

    if failed {
        eprintln!(
            "\ncheck-sql: a statement names something its schema does not have, or an \
             argument list that does not match its target.\n\
             `sqlx::query` prepares at run time, so this fails on the request, not the build."
        );
        return false;
    }
    println!(
        "check-sql: {} statement(s) across {} service(s) prepare against their own schema; \
         {} could not be resolved",
        totals.0,
        names.len(),
        totals.1
    );
    true
}

/// The migration files of one service, in the order they apply.
/// Whether this service runs `mako_service::outbox::SCHEMA` at boot.
///
/// Read off the source rather than listed here, so a service that starts or
/// stops emitting through the outbox is covered without a second edit.
fn runs_ensure_schema(dir: &Path) -> bool {
    let mut sources = Vec::new();
    collect_rs(&dir.join("src"), &mut sources);
    sources
        .iter()
        .any(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("outbox::ensure_schema")))
}

/// The DDL behind `mako_service::outbox::SCHEMA`, read out of the source.
///
/// Textually, not by depending on `mako-service`: `xtask` would otherwise pull
/// in axum, sqlx and reqwest to read one string, and reading the literal that
/// ships is the stronger check anyway.
fn outbox_schema() -> Result<String, String> {
    const FILE: &str = "crates/mako-service/src/outbox.rs";
    const OPEN: &str = "pub const SCHEMA: &str = \"";
    let src = std::fs::read_to_string(FILE).map_err(|e| format!("{FILE}: {e}"))?;
    let at = src
        .find(OPEN)
        .ok_or_else(|| format!("{FILE}: no `pub const SCHEMA`"))?
        + OPEN.len();
    let rest = &src[at..];
    // The SQL carries no double quote, so the closing `";` is unambiguous.
    let end = rest
        .find("\";")
        .ok_or_else(|| format!("{FILE}: SCHEMA literal is unterminated"))?;
    // A leading `\` + newline is Rust's line continuation, not SQL.
    Ok(rest[..end].trim_start_matches('\\').trim_start().to_owned())
}

/// Every `.rs` file under `dir`.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn migrations(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for sub in ["migrations", "src/migrations"] {
        let Ok(entries) = std::fs::read_dir(dir.join(sub)) else {
            continue;
        };
        out.extend(
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "sql")),
        );
    }
    out.sort();
    out
}

/// Every SQL statement handed to a `sqlx::query*` call in the service's
/// sources, with the constants an assembled one interpolates resolved.
///
/// Returns the statements and how many call sites could not be resolved.
fn collect(dir: &Path, workspace_root: &Path) -> (Vec<Statement>, usize) {
    let mut files = Vec::new();
    walk(&dir.join("src"), &mut files);
    files.sort();

    // A `format!` in a query is always a fragment the crate holds as a `const`
    // — a column list, a terminal-state set. Reading those makes the assembled
    // statement checkable, which is most of what the service's own SQL is: the
    // `format!` sites are 12 % of the call sites and 100 % of `obsd`'s.
    //
    // Scoped to the file that writes the query, then to the crate — but only
    // for a name the crate defines once. Half a dozen `marktd` modules each
    // hold their own `SELECT_COLS`, and a flat map resolved every one of them
    // to whichever was read last, which is how a column list from
    // `subscriptions` ended up in a query against `partners`.
    let sources: Vec<(PathBuf, String)> = files
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok().map(|s| (f.clone(), s)))
        .collect();
    let mut everywhere: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (_, src) in &sources {
        for (name, value) in file_consts(src) {
            everywhere.entry(name).or_default().insert(value);
        }
    }
    let unambiguous: BTreeMap<String, String> = everywhere
        .into_iter()
        .filter(|(_, values)| values.len() == 1)
        .map(|(name, values)| {
            (
                name,
                values.into_iter().next().expect("one value by the filter"),
            )
        })
        .collect();

    let mut out = Vec::new();
    let mut unresolved = 0usize;
    for (file, src) in &sources {
        let rel = file
            .strip_prefix(workspace_root)
            .unwrap_or(file)
            .display()
            .to_string();
        let mut consts = unambiguous.clone();
        consts.extend(file_consts(src));
        for (at, fragment) in call_sites(src) {
            let mut consts = consts.clone();
            for (capture, name) in &fragment.bindings {
                if let Some(value) = consts.get(name).cloned() {
                    consts.insert(capture.clone(), value);
                }
            }
            let Some(sql) = resolve(&fragment.text, &consts) else {
                unresolved += 1;
                continue;
            };
            if !starts_a_statement(&sql) {
                continue;
            }
            out.push(Statement {
                file: rel.clone(),
                line: src[..at].matches('\n').count() + 1,
                sql,
            });
        }
    }
    (out, unresolved)
}

/// Every `const NAME: &str = "…";` in one source file.
fn file_consts(src: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("const ") {
        let start = from + at + "const ".len();
        from = start;
        let Some(colon) = src[start..].find(':') else {
            break;
        };
        let name = src[start..start + colon].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            continue;
        }
        let Some(eq) = src[start..].find('=') else {
            continue;
        };
        let mut i = start + eq + 1;
        while src[i..].starts_with(char::is_whitespace) {
            i += 1;
        }
        if let Some(value) = read_literal(&src[i..]) {
            out.insert(name.to_owned(), value);
        }
    }
    out
}

/// Substitute the `{NAME}` captures of an assembled statement.
///
/// `None` when a placeholder names something this cannot resolve — a runtime
/// value rather than a constant. Those are counted, never guessed at.
fn resolve(sql: &str, consts: &BTreeMap<String, String>) -> Option<String> {
    if !sql.contains('{') {
        return Some(sql.to_owned());
    }
    let mut out = String::with_capacity(sql.len());
    let mut rest = sql;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}')? + open;
        let name = &rest[open + 1..close];
        out.push_str(consts.get(name)?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

const CALLS: &[&str] = &[
    "sqlx::query(",
    "sqlx::query_as(",
    "sqlx::query_scalar(",
    "sqlx::query_as::",
    "sqlx::query_scalar::",
];

fn starts_a_statement(sql: &str) -> bool {
    let word = sql
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    matches!(
        word.as_str(),
        "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "WITH" | "VALUES"
    )
}

/// One statement as written, with the `name = expr` arguments that bind its
/// captures.
struct Fragment {
    text: String,
    /// Capture name → the last path segment of what it is bound to.
    bindings: BTreeMap<String, String>,
}

/// The `name = path::CONST` arguments following a `format!` literal.
///
/// Only the last segment is kept: the constants are looked up by their own
/// name, so `crate::pg::projection::TERMINAL_STATE_SQL` and a local
/// `TERMINAL_STATE_SQL` resolve alike.
fn bindings(after_literal: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    // Everything to the end of the `format!` call, which is where the argument
    // list is. A literal cannot contain an unbalanced `)`, so counting from the
    // start of the arguments is enough.
    for line in after_literal.lines().take(24) {
        let line = line.trim().trim_end_matches(',');
        let Some((name, value)) = line.split_once(" = ") else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        let value = value.trim().trim_end_matches([',', ')']);
        if let Some(last) = value.rsplit("::").next()
            && last.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            && !last.is_empty()
        {
            out.insert(name.to_owned(), last.to_owned());
        }
    }
    out
}

/// The statement text of each `sqlx::query…(` call, whether it is written as a
/// literal or assembled with `format!`.
fn call_sites(src: &str) -> Vec<(usize, Fragment)> {
    let mut out = Vec::new();
    for call in CALLS {
        let mut from = 0;
        while let Some(at) = src[from..].find(call) {
            let mut i = from + at + call.len();
            if call.ends_with("::")
                && let Some(open) = src[i..].find('(')
            {
                i += open + 1;
            }
            while src[i..].starts_with(char::is_whitespace) {
                i += 1;
            }
            // `&format!(<literal>, …)` — the literal is the statement, its
            // `{NAME}` captures resolved from the crate's constants. A capture
            // can also be bound by a trailing `name = path::CONST` argument,
            // which is the same constant reached by its path.
            if src[i..].starts_with("&format!") {
                let mut j = i + "&format!".len();
                while src[j..].starts_with(char::is_whitespace) {
                    j += 1;
                }
                j += 1; // the opening delimiter
                while src[j..].starts_with(char::is_whitespace) {
                    j += 1;
                }
                if let Some(body) = read_literal(&src[j..]) {
                    out.push((
                        j,
                        Fragment {
                            text: body,
                            bindings: bindings(&src[j..]),
                        },
                    ));
                }
            } else if let Some(body) = read_literal(&src[i..]) {
                out.push((
                    i,
                    Fragment {
                        text: body,
                        bindings: BTreeMap::new(),
                    },
                ));
            }
            from = from + at + call.len();
        }
    }
    out.sort_by_key(|(at, _)| *at);
    out
}

/// A Rust string literal at the start of `s`: `r"…"`, `r#"…"#`, or `"…"`.
fn read_literal(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'r') {
        let hashes = s[1..].bytes().take_while(|b| *b == b'#').count();
        if bytes.get(1 + hashes) != Some(&b'"') {
            return None;
        }
        let open = 1 + hashes + 1;
        let close = format!("\"{}", "#".repeat(hashes));
        let end = s[open..].find(&close)? + open;
        return Some(s[open..end].to_owned());
    }
    if bytes.first() == Some(&b'"') {
        let mut i = 1;
        let mut body = String::new();
        while i < s.len() {
            match s.as_bytes()[i] {
                b'\\' => {
                    match s.as_bytes().get(i + 1) {
                        Some(b'n') => body.push('\n'),
                        Some(b'"') => body.push('"'),
                        Some(b'\\') => body.push('\\'),
                        Some(b'\n') => {}
                        _ => return None,
                    }
                    i += 2;
                }
                b'"' => return Some(body),
                b => {
                    body.push(b as char);
                    i += 1;
                }
            }
        }
    }
    None
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether a `psql` client is on `PATH`.
fn psql_available() -> bool {
    Command::new("psql")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn docker_available() -> bool {
    Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Environment variable naming an already-running server.
///
/// Set it and `check-sql` connects with `psql "$URL"` instead of starting a
/// container of its own. That is what lets CI run this gate: a GitHub runner
/// has no Docker daemon a job can `docker exec` into reliably, but
/// `services: postgres` gives it a server on a port and `psql` on `PATH`.
const URL_ENV: &str = "MAKO_CHECK_SQL_URL";

/// Where the statements are prepared.
///
/// `Container` owns the server and removes it on drop. `Url` borrows one this
/// process did not start, so dropping it does nothing — deleting someone
/// else's database is not this guard's business.
enum Postgres {
    /// A throwaway container, removed when this value is dropped.
    Container,
    /// A server reachable at this URL, started and owned elsewhere.
    Url(String),
}

impl Postgres {
    /// The server to prepare against, or `None` when neither is available.
    fn open() -> Option<Self> {
        if let Ok(url) = std::env::var(URL_ENV)
            && !url.trim().is_empty()
        {
            if !psql_available() {
                eprintln!(
                    "check-sql: {URL_ENV} is set but `psql` is not on PATH — install the \
                     PostgreSQL client, or unset {URL_ENV} to use a container."
                );
                return None;
            }
            println!("check-sql: preparing against the server named by {URL_ENV}");
            return Some(Self::Url(url));
        }
        if !docker_available() {
            eprintln!(
                "check-sql: needs a Docker daemon to start {IMAGE}, or {URL_ENV} pointing \
                 at a running server, and has not run.\n\
                 This is a skip, not a pass — start Docker and run it again."
            );
            return None;
        }
        Self::start()
    }

    /// The command that feeds a script to the server on stdin.
    ///
    /// Streams merged through `sh -c` in both cases: `\echo` writes the marker
    /// to stdout and an error goes to stderr, so reading them apart loses the
    /// interleaving that says which statement failed.
    fn psql(&self) -> Command {
        match self {
            Self::Container => {
                let mut c = Command::new("docker");
                c.args([
                    "exec",
                    "-i",
                    CONTAINER,
                    "sh",
                    "-c",
                    "psql -U postgres -q -f - 2>&1",
                ]);
                c
            }
            Self::Url(url) => {
                let mut c = Command::new("sh");
                // The URL is an operator-supplied connection string; single-quote
                // it so a `&` or `?` in a password cannot reach the shell.
                c.args([
                    "-c",
                    &format!("psql '{}' -q -f - 2>&1", url.replace('\'', r"'\''")),
                ]);
                c
            }
        }
    }

    fn start() -> Option<Self> {
        let _ = Command::new("docker")
            .args(["rm", "-f", CONTAINER])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let started = Command::new("docker")
            .args([
                "run",
                "-d",
                "--name",
                CONTAINER,
                "-e",
                "POSTGRES_PASSWORD=check-sql",
                IMAGE,
            ])
            .stdout(std::process::Stdio::null())
            .status();
        if !started.is_ok_and(|s| s.success()) {
            eprintln!("check-sql: could not start {IMAGE}");
            return None;
        }
        let guard = Self::Container;
        for _ in 0..60 {
            let ready = Command::new("docker")
                .args(["exec", CONTAINER, "pg_isready", "-U", "postgres"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            if ready.is_ok_and(|s| s.success()) {
                return Some(guard);
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        eprintln!("check-sql: {IMAGE} did not become ready");
        None
    }

    /// Apply one service's schema to a fresh database and prepare every
    /// statement, returning the ones the server refuses.
    ///
    /// `outbox` applies `mako_service::outbox::SCHEMA` as well. That DDL is a
    /// Rust constant run by `ensure_schema` during the daemon's `migrate()`, not
    /// a file under `migrations/`, so without it the `event_outbox` table is
    /// absent here and every statement against it prepares against a schema the
    /// running service does not have — and the DDL itself first reaches a server
    /// at startup, where a mistake is eight daemons that will not boot.
    fn check(
        &self,
        service: &str,
        migrations: &[PathBuf],
        outbox: bool,
        statements: &[Statement],
    ) -> Result<Vec<String>, String> {
        let mut script = String::from("\\set ON_ERROR_STOP off\n\\set VERBOSITY verbose\n");
        script.push_str("DROP SCHEMA IF EXISTS public CASCADE;\nCREATE SCHEMA public;\n");
        if outbox {
            script.push_str(&outbox_schema()?);
            script.push('\n');
        }
        for path in migrations {
            let sql = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            script.push_str(&sql);
            script.push('\n');
        }
        // A marker line before each statement, so a `psql` error can be traced
        // back to the source line that wrote it.
        for (i, s) in statements.iter().enumerate() {
            script.push_str(&format!("\\echo @@{i}\nPREPARE p{i} AS {};\n", s.sql));
            script.push_str(&format!("DEALLOCATE p{i};\n"));
        }

        let out = self
            .psql()
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                use std::io::Write as _;
                child
                    .stdin
                    .take()
                    .expect("piped stdin")
                    .write_all(script.as_bytes())?;
                child.wait_with_output()
            })
            .map_err(|e| format!("{service}: psql: {e}"))?;

        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        let mut broken = Vec::new();
        let mut at: Option<usize> = None;
        let mut seen: BTreeMap<usize, ()> = BTreeMap::new();
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("@@") {
                at = rest.trim().parse().ok();
                continue;
            }
            let Some(rest) = line.split_once("ERROR:  ").map(|(_, r)| r) else {
                continue;
            };
            let Some((code, message)) = rest.split_once(": ") else {
                continue;
            };
            if !BROKEN.contains(&code) {
                continue;
            }
            let Some(i) = at.filter(|i| *i < statements.len()) else {
                continue;
            };
            if seen.insert(i, ()).is_some() {
                continue;
            }
            let s = &statements[i];
            broken.push(format!(
                "{}:{} — {message} ({code})\n        {}",
                s.file,
                s.line,
                s.sql.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
        Ok(broken)
    }
}

impl Drop for Postgres {
    fn drop(&mut self) {
        // Only a container this process started. A `Url` server belongs to
        // whoever started it.
        if matches!(self, Self::Container) {
            let _ = Command::new("docker")
                .args(["rm", "-f", CONTAINER])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}
