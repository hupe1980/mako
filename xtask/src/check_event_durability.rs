//! Guard: an event and the work it reports commit together, every CloudEvent
//! publish has something durable behind it, and every envelope is built by
//! `mako_service::CloudEvent`.
//!
//! Three checks, each over the production text of `crates/*/src/**/*.rs` and
//! `services/*/src/**/*.rs` — `#[cfg(test)]` items and comment lines blanked
//! (`check_error_tuples::production_text`). `crates/mako-service/src` is the
//! one place allowed to do what the first two refuse: it is where the
//! envelope, the delivery primitive and the outbox live.
//!
//! **Envelope.** A `"specversion"` key — `"specversion":` — outside
//! `crates/mako-service`. A hand-built envelope drifts from the catalogue
//! (source, id, extensions) without anything noticing;
//! `CloudEvent::new(source(svc, tenant), TYPE, subject, data)` is the one
//! constructor.
//!
//! **Publish.** A call to `post_ce_with_retry(` or `post_ce_for_reply(`, or an
//! `application/cloudevents+json` request built by hand, outside
//! `crates/mako-service`. Calling the primitive is not the defect — every
//! outbox worker ends in it — calling it with nothing persisted behind it is.
//! So each call carries its decision in place: a `// durable: <why>` comment
//! within the six lines above it, naming what retries the event when the POST
//! fails (the outbox worker the call runs in, a row stamped only after
//! delivery that the next pass re-selects, a figure recomputed each sweep).
//! A publish with no such line fails.
//!
//! **Marker.** In a `services/*/src` function that enqueues — calls
//! `outbox::enqueue(`, `enqueue_to(`, `enqueue_command(`, or a function of the
//! same file that does — a marker-shaped write on a pool rather than on the
//! transaction: an `outbox::enqueue` handed a pool, an `idempotent_event(`
//! handed a pool, or a call named `mark_*`, `close_*`, `update_*`, `revoke*`,
//! `set_*`, `insert_*`, `upsert*`, `record_*` or `claim_*` whose first argument
//! is a pool. Such a write commits on its own; a failure before the outbox row
//! commits leaves a marker that says „done" with no event, and every later
//! pass skips the row it marked. Independently of any enqueue, an
//! `idempotent_event(` on a pool followed in the same function by another
//! write is refused: a deduplication marker committed ahead of its work turns
//! the sender's retry after a failure into a silent success.
//!
//! **Announcement after the write.** In a `services/*/src` function, a call
//! handed a pool to a *pure announcer* of the same crate — a function taking a
//! pool that writes nothing of its own and enqueues, directly or through
//! another such function — after a write-named call (`mark_`, `update_`,
//! `create_`, `report_`, `resolve_`, …) handed the pool. The write committed,
//! then the announcement on a transaction of its own: a failure between them
//! leaves the state changed and nothing announcing it.
//!
//! A write that may commit on its own carries `// committed apart: <why>`
//! within the three lines above it — a row created before the work starts, a
//! mark written after the event committed (the duplicate direction, absorbed
//! by consumer idempotency), a claim released on every path that does not
//! enqueue, a mark that must survive because the action it records is already
//! on the market.
//!
//! **Queue.** A `CREATE TABLE` in a `services/*/migrations/*.sql` that declares
//! both an attempt counter (`attempts` or `…_attempts`) and a
//! `…next_attempt_at` column: a hand-rolled retry queue beside
//! `mako_service::outbox`, with its own claim, backoff and dead-letter rule and
//! an operator view of its own. The one kept is `marktd`'s `event_delivery`:
//! it delivers one event to each subscriber of a set snapshotted at fan-out,
//! which a single-target outbox row cannot express ([`KEPT_QUEUES`]).
//!
//! The patterns are narrower than the rules. A pool is recognised by name
//! (`pool`, `self.pool`, `ctx.pool`, `state.pool`, `sink.pool`, `rt.pool`), a
//! repository method that hides its pool is invisible, and the function split
//! is textual. What the guard proves is that no write of these shapes sits on a
//! pool beside an enqueue unremarked, not that every marker is atomic.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The lowest number of `.rs` files a healthy walk reads.
const MIN_FILES: usize = 300;

const PUBLISH_CALLS: &[&str] = &["post_ce_with_retry(", "post_ce_for_reply("];
const HAND_BUILT_CE_HEADER: &str = "application/cloudevents+json";
const DURABLE_REASON: &str = "// durable:";
const COMMITTED_APART: &str = "// committed apart:";
const ENQUEUE_CALLS: &[&str] = &[
    "outbox::enqueue(",
    "outbox::enqueue_to(",
    "outbox::enqueue_command(",
    "enqueue_command(",
    "enqueue_to(",
];
const MARKER_PREFIXES: &[&str] = &[
    "mark_", "close_", "update_", "revoke", "set_", "insert_", "upsert", "record_", "claim_",
];
/// Call names that write: the marker shapes plus the verbs a repository
/// function that changes state starts with.
const WRITE_PREFIXES: &[&str] = &[
    "mark_", "close_", "update_", "revoke", "set_", "insert_", "upsert", "record_", "claim_",
    "report_", "create_", "resolve_", "delete_", "cancel_",
];
const POOL_NAMES: &[&str] = &[
    "pool",
    "self.pool",
    "ctx.pool",
    "state.pool",
    "sink.pool",
    "rt.pool",
];

/// Retry queues kept beside `mako_service::outbox`, as `(migration, table)`.
pub const KEPT_QUEUES: &[(&str, &str)] = &[(
    // One row per (event, subscriber), the subscriber set snapshotted when the
    // event fans out; a single-target outbox row cannot carry that.
    "services/marktd/migrations/0001_initial.sql",
    "event_delivery",
)];

/// `(line, table)` of each `CREATE TABLE` in `sql` that declares an attempt
/// counter and a next-attempt column together.
pub fn retry_queues(sql: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let lines: Vec<&str> = sql.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let upper = lines[i].trim_start().to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("CREATE TABLE") {
            let name: String = rest
                .trim_start()
                .trim_start_matches("IF NOT EXISTS")
                .trim_start()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.')
                .collect::<String>()
                .to_ascii_lowercase();
            let start = i;
            let mut counter = false;
            let mut next = false;
            while i < lines.len() {
                let col = lines[i]
                    .split_whitespace()
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase();
                counter |= col == "attempts" || col.ends_with("_attempts");
                next |= col.ends_with("next_attempt_at");
                if lines[i].trim_start().starts_with(");") {
                    break;
                }
                i += 1;
            }
            if counter && next {
                out.push((start + 1, name));
            }
        }
        i += 1;
    }
    out
}

/// Per migration: the retry queues not kept by name.
fn queue_findings(root: &Path) -> BTreeMap<String, Vec<Finding>> {
    let mut found = BTreeMap::new();
    let mut dirs: Vec<_> = std::fs::read_dir(root.join("services"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("migrations"))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for d in dirs {
        let mut files: Vec<_> = std::fs::read_dir(&d)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "sql"))
            .collect();
        files.sort();
        for f in files {
            let rel = f
                .strip_prefix(root)
                .unwrap_or(&f)
                .to_string_lossy()
                .replace('\\', "/");
            let sql = std::fs::read_to_string(&f).unwrap_or_default();
            let hits: Vec<Finding> = retry_queues(&sql)
                .into_iter()
                .filter(|(_, t)| !KEPT_QUEUES.contains(&(rel.as_str(), t.as_str())))
                .map(|(line, _)| Finding {
                    line,
                    what: "a hand-rolled retry queue (an attempt counter beside a next-attempt \
                           column) — owe the delivery as a `mako_service::outbox` row instead",
                })
                .collect();
            if !hits.is_empty() {
                found.insert(rel, hits);
            }
        }
    }
    found
}

/// One finding: file-relative line and what is wrong there.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    pub line: usize,
    pub what: &'static str,
}

/// Every finding in one file's source. `mako_service` exempts the envelope and
/// publish checks; `service` enables the marker check.
#[cfg(test)]
pub fn scan_source(src: &str, mako_service: bool, service: bool) -> Vec<Finding> {
    scan_source_with(src, mako_service, service, &BTreeSet::new())
}

/// `scan_source`, knowing the announcing helpers of the file's crate (see
/// [`announcers`]).
pub fn scan_source_with(
    src: &str,
    mako_service: bool,
    service: bool,
    announcers: &BTreeSet<String>,
) -> Vec<Finding> {
    let text = crate::check_error_tuples::production_text(src);
    let lines: Vec<&str> = text.lines().collect();
    let raw: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    if !mako_service {
        for (i, l) in lines.iter().enumerate() {
            if l.contains("\"specversion\":") || l.contains("\"specversion\" :") {
                out.push(Finding {
                    line: i + 1,
                    what: "a hand-built CloudEvent envelope (`\"specversion\"`) — build it with \
                           `mako_service::CloudEvent::new`",
                });
            }
            let publishes =
                PUBLISH_CALLS.iter().any(|c| l.contains(c)) || l.contains(HAND_BUILT_CE_HEADER);
            if publishes && !reason_above(&raw, i, 6, DURABLE_REASON) {
                out.push(Finding {
                    line: i + 1,
                    what: "a CloudEvent publish with no durable row behind it — enqueue it with \
                           `mako_service::outbox`, or state in a `// durable:` line above what \
                           retries it",
                });
            }
        }
    }
    if service {
        out.extend(marker_findings(&lines, &raw));
        out.extend(announce_after_findings(&lines, &raw, announcers));
    }
    out.sort();
    out.dedup();
    out
}

/// Whether a line within `window` lines above `i` (or `i` itself) of the raw
/// source carries `marker`.
fn reason_above(raw: &[&str], i: usize, window: usize, marker: &str) -> bool {
    let from = i.saturating_sub(window);
    raw.get(from..=i.min(raw.len().saturating_sub(1)))
        .is_some_and(|ls| ls.iter().any(|l| l.contains(marker)))
}

/// `(name, first line, end line exclusive)` of each function, by a textual
/// split at every `fn NAME` line.
fn functions(lines: &[&str]) -> Vec<(String, usize, usize)> {
    let mut starts = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(name) = fn_name(l) {
            starts.push((name, i));
        }
    }
    let mut out = Vec::new();
    for (k, (name, start)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(lines.len(), |(_, s)| *s);
        out.push((name.clone(), *start, end));
    }
    out
}

fn fn_name(line: &str) -> Option<String> {
    let t = line.trim_start();
    let mut rest = t;
    for kw in [
        "pub(crate) ",
        "pub(super) ",
        "pub ",
        "const ",
        "async ",
        "unsafe ",
    ] {
        rest = rest.strip_prefix(kw).unwrap_or(rest);
        rest = rest.strip_prefix(kw).unwrap_or(rest);
    }
    let rest = rest.strip_prefix("fn ")?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    (!name.is_empty()).then_some(name)
}

/// The first argument of the call opening at `open` (the index just past `(`)
/// in `body`, trimmed.
fn first_arg(body: &str, open: usize) -> &str {
    let rest = &body[open..];
    let mut depth = 0i32;
    for (k, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' if depth == 0 => return rest[..k].trim(),
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => return rest[..k].trim(),
            _ => {}
        }
    }
    rest.trim()
}

fn is_pool(arg: &str) -> bool {
    let a = arg.trim_start_matches('&').trim();
    let a = a.strip_suffix(".clone()").unwrap_or(a);
    let a = a.strip_suffix(".as_ref()").unwrap_or(a);
    POOL_NAMES.contains(&a)
}

/// The call names in `body` at which a marker-shaped write takes a pool, as
/// byte offsets. A call to a function of the same file that enqueues is not
/// one: it owns its transaction.
fn pool_writes(body: &str, own_tx: &BTreeSet<String>) -> Vec<(usize, &'static str)> {
    let mut out = Vec::new();
    for call in ["outbox::enqueue(", "idempotent_event("] {
        for (at, _) in body.match_indices(call) {
            if is_pool(first_arg(body, at + call.len())) {
                out.push((
                    at,
                    if call.starts_with("outbox") {
                        "an `outbox::enqueue` on a pool — it commits apart from the write it \
                         announces; enqueue on the business transaction"
                    } else {
                        "an idempotency marker on a pool beside an enqueue — commit it on the \
                         transaction that does the work"
                    },
                ));
            }
        }
    }
    let bytes = body.as_bytes();
    for (open, _) in body.match_indices('(') {
        let mut s = open;
        while s > 0 && (bytes[s - 1].is_ascii_alphanumeric() || bytes[s - 1] == b'_') {
            s -= 1;
        }
        let name = &body[s..open];
        if !MARKER_PREFIXES.iter().any(|p| name.starts_with(p)) || own_tx.contains(name) {
            continue;
        }
        if is_pool(first_arg(body, open + 1)) {
            out.push((
                s,
                "a marker-shaped write on a pool in a function that enqueues — run it on the \
                 transaction that enqueues, or state `// committed apart:` above it",
            ));
        }
    }
    out
}

fn marker_findings(lines: &[&str], raw: &[&str]) -> Vec<Finding> {
    let fns = functions(lines);
    let body_of = |start: usize, end: usize| lines[start..end].join("\n");
    // Functions of this file that enqueue, directly or through one another.
    let mut enqueuing: BTreeSet<String> = fns
        .iter()
        .filter(|(_, s, e)| {
            let b = body_of(*s, *e);
            ENQUEUE_CALLS.iter().any(|c| b.contains(c))
        })
        .map(|(n, _, _)| n.clone())
        .collect();
    loop {
        let before = enqueuing.len();
        for (name, s, e) in &fns {
            let b = body_of(*s, *e);
            let calls_one = enqueuing
                .iter()
                .any(|n| n != name && b.contains(&format!("{n}(")));
            if calls_one {
                enqueuing.insert(name.clone());
            }
        }
        if enqueuing.len() == before {
            break;
        }
    }
    let mut out = Vec::new();
    for (name, s, e) in &fns {
        let body = body_of(*s, *e);
        let line_of = |at: usize| s + body[..at].matches('\n').count();
        let writes = pool_writes(&body, &enqueuing);
        if enqueuing.contains(name) {
            for (at, what) in &writes {
                let line = line_of(*at);
                if !reason_above(raw, line, 3, COMMITTED_APART) {
                    out.push(Finding {
                        line: line + 1,
                        what,
                    });
                }
            }
        }
        // A deduplication marker on a pool followed by other work.
        for (at, _) in body.match_indices("idempotent_event(") {
            if !is_pool(first_arg(&body, at + "idempotent_event(".len())) {
                continue;
            }
            let after = &body[at + 1..];
            let more_work = after.contains(".begin()")
                || !pool_writes(after, &enqueuing).is_empty()
                || ENQUEUE_CALLS.iter().any(|c| after.contains(c));
            let line = line_of(at);
            if more_work && !reason_above(raw, line, 3, COMMITTED_APART) {
                out.push(Finding {
                    line: line + 1,
                    what: "a deduplication marker committed on a pool ahead of its work — a \
                           failure after it turns the sender's retry into a silent success; \
                           write it on the work's transaction, or after the work",
                });
            }
        }
    }
    out
}

/// The pure announcers of one crate's sources: functions that take a pool as
/// their first parameter, write nothing of their own, and enqueue — directly or
/// through another pure announcer. Each commits an announcement on a
/// transaction of its own, apart from any write its caller made.
pub fn announcers(sources: &[String]) -> BTreeSet<String> {
    let mut candidates: Vec<(String, String)> = Vec::new();
    for src in sources {
        let text = crate::check_error_tuples::production_text(src);
        let lines: Vec<&str> = text.lines().collect();
        for (name, s, e) in functions(&lines) {
            let body = lines[s..e].join("\n");
            let Some(open) = body.find('(') else { continue };
            let takes_pool = first_arg(&body, open + 1)
                .split_once(':')
                .is_some_and(|(n, t)| is_pool(n.trim()) && t.contains("PgPool"));
            // A write of its own — a query, or a call handed the transaction
            // other than the enqueue — makes it a writer that announces, not
            // an announcement on its own.
            let writes = body.contains("sqlx::query")
                || body
                    .lines()
                    .filter(|l| !ENQUEUE_CALLS.iter().any(|c| l.contains(c)))
                    .any(|l| l.contains("(&mut *tx") || l.contains("(&mut tx"));
            if takes_pool && !writes {
                candidates.push((name, body));
            }
        }
    }
    let mut out: BTreeSet<String> = candidates
        .iter()
        .filter(|(_, b)| ENQUEUE_CALLS.iter().any(|c| b.contains(c)))
        .map(|(n, _)| n.clone())
        .collect();
    loop {
        let before = out.len();
        for (name, body) in &candidates {
            if out
                .iter()
                .any(|n| n != name && body.contains(&format!("{n}(")))
            {
                out.insert(name.clone());
            }
        }
        if out.len() == before {
            return out;
        }
    }
}

/// A call to an announcing helper handed a pool, after a write-named call
/// handed the pool in the same function: the write committed, then the
/// announcement on a transaction of its own — a failure between the two leaves the state
/// changed and nothing announcing it.
fn announce_after_findings(
    lines: &[&str],
    raw: &[&str],
    announcers: &BTreeSet<String>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    if announcers.is_empty() {
        return out;
    }
    for (_, s, e) in functions(lines) {
        let body = lines[s..e].join("\n");
        let bytes = body.as_bytes();
        let mut write_seen = false;
        for (open, _) in body.match_indices('(') {
            let mut st = open;
            while st > 0 && (bytes[st - 1].is_ascii_alphanumeric() || bytes[st - 1] == b'_') {
                st -= 1;
            }
            let name = &body[st..open];
            if name.is_empty() || !is_pool(first_arg(&body, open + 1)) {
                continue;
            }
            // The function's own signature is not a call.
            if body[..st].trim_end().ends_with("fn") {
                continue;
            }
            if announcers.contains(name) && write_seen {
                let line = s + body[..st].matches('\n').count();
                if !reason_above(raw, line, 3, COMMITTED_APART) {
                    out.push(Finding {
                        line: line + 1,
                        what: "an announcement enqueued on its own transaction after a write on \
                               the pool — enqueue it in the transaction that commits the write, \
                               or state `// committed apart:` above it",
                    });
                }
            }
            write_seen |=
                !announcers.contains(name) && WRITE_PREFIXES.iter().any(|p| name.starts_with(p));
        }
    }
    out
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

/// Per file (workspace-relative): its findings; and the number of files read.
pub fn scan(root: &Path) -> (BTreeMap<String, Vec<Finding>>, usize) {
    let mut files = Vec::new();
    for top in ["crates", "services"] {
        let mut dirs: Vec<_> = std::fs::read_dir(root.join(top))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("src"))
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        for d in &dirs {
            walk(d, &mut files);
        }
    }
    // Announcing helpers per crate (`services/<name>/` or `crates/<name>/`).
    let crate_of = |rel: &str| rel.splitn(3, '/').take(2).collect::<Vec<_>>().join("/");
    let mut per_crate: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut sources = Vec::new();
    for f in &files {
        let rel = f
            .strip_prefix(root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/");
        let src = std::fs::read_to_string(f).unwrap_or_default();
        per_crate
            .entry(crate_of(&rel))
            .or_default()
            .push(src.clone());
        sources.push((rel, src));
    }
    let helpers: BTreeMap<String, BTreeSet<String>> = per_crate
        .iter()
        .map(|(k, v)| (k.clone(), announcers(v)))
        .collect();
    let mut found = BTreeMap::new();
    for (rel, src) in &sources {
        let none = BTreeSet::new();
        let findings = scan_source_with(
            src,
            rel.starts_with("crates/mako-service/"),
            rel.starts_with("services/"),
            helpers.get(&crate_of(rel)).unwrap_or(&none),
        );
        let rel = rel.clone();
        if !findings.is_empty() {
            found.insert(rel, findings);
        }
    }
    (found, files.len())
}

/// `cargo xtask check-event-durability`.
pub fn run(root: &Path) -> bool {
    let (mut found, files) = scan(root);
    found.extend(queue_findings(root));
    if files < MIN_FILES {
        eprintln!(
            "check-event-durability: read only {files} files under crates/*/src and \
             services/*/src (expected at least {MIN_FILES}) — the walk is broken"
        );
        return false;
    }
    if found.is_empty() {
        println!(
            "check-event-durability: OK — {files} files, every envelope built by CloudEvent::new, \
             every publish durable, no unremarked marker on a pool beside an enqueue, no \
             retry queue beside the outbox"
        );
        return true;
    }
    for (file, findings) in &found {
        for f in findings {
            eprintln!("{file}:{}: {}", f.line, f.what);
        }
    }
    eprintln!(
        "check-event-durability: {} finding(s) in {} file(s)",
        found.values().map(Vec::len).sum::<usize>(),
        found.len()
    );
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str, mako_service: bool, service: bool) -> Vec<usize> {
        scan_source(src, mako_service, service)
            .into_iter()
            .map(|f| f.line)
            .collect()
    }

    /// The § 20b Netzzugang webhook as it hand-built its envelope.
    #[test]
    fn a_hand_built_envelope_is_refused_outside_mako_service() {
        let src = r#"
fn send() {
    let body = serde_json::json!({
        "specversion": "1.0",
        "type": T,
    });
}
"#;
        assert_eq!(lines(src, false, true), vec![4]);
        assert!(lines(src, true, false).is_empty());
        // A doc example and a test fixture are not production code.
        let doc = "/// \"specversion\": \"1.0\",\nfn f() {}\n";
        assert!(lines(doc, false, true).is_empty());
    }

    /// productd's acceptance POST and a bare primitive call are refused until
    /// they state what retries them.
    #[test]
    fn a_publish_needs_a_durable_reason() {
        let direct = r#"
async fn annehmen() {
    let builder = client
        .post(url)
        .header("Content-Type", "application/cloudevents+json");
}
"#;
        assert_eq!(lines(direct, false, true), vec![5]);
        let bare =
            "async fn f() {\n    mako_service::post_ce_with_retry(c, u, &ce, None).await;\n}\n";
        assert_eq!(lines(bare, false, true), vec![2]);
        let reasoned = "async fn f() {\n    // durable: the row is stamped only after delivery\n    \
                        mako_service::post_ce_with_retry(c, u, &ce, None).await;\n}\n";
        assert!(lines(reasoned, false, true).is_empty());
        assert!(
            lines(bare, true, false).is_empty(),
            "mako-service is the primitive"
        );
    }

    /// vertragd's `ablauf` worker: the contracts close on the pool, then each
    /// event is enqueued on a transaction of its own.
    #[test]
    fn a_marker_on_the_pool_before_an_enqueue_is_refused() {
        let src = r#"
pub async fn ablauf(pool: &PgPool) -> anyhow::Result<()> {
    let geschlossen = pg::close_due_supply(pool, &cfg.tenant).await?;
    for id in &geschlossen {
        let mut tx = pool.begin().await?;
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        tx.commit().await?;
    }
}
"#;
        assert_eq!(lines(src, false, true), vec![3]);
        let fixed = r#"
pub async fn ablauf(pool: &PgPool) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let geschlossen = pg::close_due_supply(&mut tx, &cfg.tenant).await?;
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
    tx.commit().await?;
}
"#;
        assert!(lines(fixed, false, true).is_empty());
        // Not in services/: not scanned for markers.
        assert!(lines(src, false, false).is_empty());
    }

    /// marktd's Widerruf: the consent is revoked by the repository, then the
    /// event goes out through a helper that enqueues on the pool.
    #[test]
    fn an_enqueue_on_a_pool_is_refused() {
        let src = r#"
async fn emit(pool: &sqlx::PgPool, evt: &MarktEvent) -> Result<(), sqlx::Error> {
    crate::outbox::enqueue(pool, &evt).await?;
    Ok(())
}
pub async fn revoke_einwilligung() {
    let revoked = repo.revoke(&tenant, id).await;
    emit(&pool, &evt).await;
}
"#;
        assert_eq!(lines(src, false, true), vec![3]);
    }

    /// netzbilanzd's run marked `submitted` on the pool in the function that
    /// announces its failure, ahead of any submission.
    #[test]
    fn a_status_marker_beside_an_announcing_helper_is_refused() {
        let src = r#"
pub async fn run_aggregation(&self) -> Result<Uuid> {
    pg::update_run_aggregated(&self.pool, run_id, malo_count).await?;
    self.mark_failed_and_emit(run_id).await;
}
async fn mark_failed_and_emit(&self, run_id: Uuid) {
    let mut tx = self.pool.begin().await?;
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
}
"#;
        assert_eq!(lines(src, false, true), vec![3]);
        let remarked = r#"
pub async fn run_aggregation(&self) -> Result<Uuid> {
    // committed apart: the aggregation figures; the status moves with the filing
    pg::update_run_aggregated(&self.pool, run_id, malo_count).await?;
    self.mark_failed_and_emit(run_id).await;
}
async fn mark_failed_and_emit(&self, run_id: Uuid) {
    mako_service::outbox::enqueue(&mut tx, &ce).await?;
}
"#;
        assert!(lines(remarked, false, true).is_empty());
    }

    /// processd's Sperrauftrag outcome: the order is made terminal on the pool,
    /// then its `ausgefuehrt`/`fehlgeschlagen` is enqueued by a helper on a
    /// transaction of its own.
    #[test]
    fn an_announcement_after_a_committed_write_is_refused() {
        let helper = r#"
pub async fn outcome(pool: &PgPool, tenant: &str, id: Uuid) {
    let ce = build(tenant, id);
    enqueue(pool, &ce, id).await;
}
async fn enqueue(pool: &PgPool, ce: &CloudEvent, id: Uuid) {
    let mut tx = pool.begin().await?;
    mako_service::outbox::enqueue(&mut *tx, ce).await?;
}
"#
        .to_owned();
        let handler = r#"
async fn finish(pool: &PgPool, id: Uuid) -> ApiResult<Response> {
    match report_outcome(pool, id, tenant, outcome).await? {
        Reported::Recorded { queued } => {
            events::outcome(pool, tenant, id).await;
        }
    }
}
"#;
        let set = announcers(&[helper, handler.to_owned()]);
        assert!(set.contains("outcome"));
        let found: Vec<usize> = scan_source_with(handler, false, true, &set)
            .into_iter()
            .map(|f| f.line)
            .collect();
        assert_eq!(found, vec![5]);
        // Enqueued inside the outcome's transaction: nothing announces apart.
        let fixed = r#"
async fn finish(pool: &PgPool, id: Uuid) -> ApiResult<Response> {
    match report_outcome(pool, id, tenant, outcome).await? {
        Reported::Recorded { queued } => {}
    }
}
"#;
        assert!(scan_source_with(fixed, false, true, &set).is_empty());
    }

    /// The four queues that each ran their own claim, backoff and dead-letter.
    #[test]
    fn a_retry_queue_beside_the_outbox_is_refused() {
        let sql = "
CREATE TABLE outbound_tasks (
    id            UUID PRIMARY KEY,
    attempts      INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE invoic_receipts (
    id                  UUID PRIMARY KEY,
    erp_attempts        SMALLINT NOT NULL DEFAULT 0,
    erp_next_attempt_at TIMESTAMPTZ
);
CREATE TABLE sperr_orders (
    iftsta_attempts        SMALLINT,
    iftsta_next_attempt_at TIMESTAMPTZ
);
CREATE TABLE IF NOT EXISTS document_deliveries (
    attempts        INT,
    next_attempt_at TIMESTAMPTZ
);
CREATE TABLE plain (
    attempt_count INT
);
";
        let tables: Vec<String> = retry_queues(sql).into_iter().map(|(_, t)| t).collect();
        assert_eq!(
            tables,
            [
                "outbound_tasks",
                "invoic_receipts",
                "sperr_orders",
                "document_deliveries"
            ]
        );
    }

    /// vertragd's `angebot.angenommen` marker committed ahead of the contract.
    #[test]
    fn a_dedup_marker_ahead_of_its_work_is_refused() {
        let src = r#"
pub async fn angebot(ctx: Ctx) -> ApiResult<()> {
    if !pg::idempotent_event(&ctx.pool, ce_id, T, &ce).await? {
        return Ok(());
    }
    let id = pg::insert_rahmenvertrag(&ctx.pool, kunde, &input).await?;
}
"#;
        assert_eq!(lines(src, false, true), vec![3]);
        let after_the_work = r#"
pub async fn angebot(ctx: Ctx) -> ApiResult<()> {
    if pg::event_received(&ctx.pool, ce_id).await? {
        return Ok(());
    }
    let id = pg::insert_rahmenvertrag(&ctx.pool, kunde, &input).await?;
    pg::idempotent_event(&ctx.pool, ce_id, T, &ce).await?;
}
"#;
        assert!(lines(after_the_work, false, true).is_empty());
        let on_the_tx = r#"
pub async fn cloud_event(ctx: Ctx) -> ApiResult<()> {
    let mut tx = ctx.pool.begin().await?;
    if !pg::idempotent_event(&mut *tx, ce_id, T, &ce).await? {
        return Ok(());
    }
    pg::update_komponente_status(&mut *tx, k.id).await?;
}
"#;
        assert!(lines(on_the_tx, false, true).is_empty());
    }
}
