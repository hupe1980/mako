//! Guard: an outbox wake-up is raised by the database, never by the producer.
//!
//! mako runs two durable outboxes — `mako_service`'s `event_outbox`, drained by
//! the `OutboxWorker` in eight services, and marktd's `event_log`, drained by
//! the fan-out worker. Both persist inside the producer's business transaction
//! and both have a background worker that polls on an interval. The wake-up is
//! what turns that interval from a delivery latency into a fallback.
//!
//! **Where the wake-up is raised decides whether it can work at all.** A
//! producer-side, in-process hint (`Notify::notify_one`) taken between the
//! `enqueue` and the `COMMIT` is spent on a snapshot in which the row does not
//! exist: the worker wakes, drains nothing, sleeps, and the event is delivered
//! at the next poll tick instead. Nothing fails and nothing is logged — the only
//! symptom is that every event takes a poll interval, which reads as "the
//! outbox is slow" rather than as a bug. The same hint also cannot cross a
//! process boundary, so under more than one replica a write served by one
//! process never wakes the worker in another.
//!
//! A `NOTIFY` raised by an `AFTER INSERT` trigger has neither problem: Postgres
//! queues it until the raising transaction commits, and delivers it to every
//! session listening on the channel. Both properties come from the database, so
//! no call site can get them wrong — which is why this guard checks that the
//! mechanism is *in* the database rather than checking that call sites order
//! themselves correctly.
//!
//! ## The three rules
//!
//! 1. **The trigger exists.** Each outbox's DDL declares an `AFTER INSERT …
//!    FOR EACH STATEMENT` trigger calling `pg_notify` on its own channel.
//! 2. **The worker listens on that same channel.** The Rust `NOTIFY_CHANNEL`
//!    constant must equal the channel the DDL raises on, and something must
//!    `listen(NOTIFY_CHANNEL)`. A trigger nobody listens to, or a listener on a
//!    channel nothing raises, is the same silent poll-interval delay.
//! 3. **No producer-side wake-up primitive.** `notify_one` must not appear on
//!    an outbox path. It is the obvious thing to reach for, and it is the one
//!    shape that cannot work.

use std::path::Path;

/// One outbox: where its DDL lives, where its worker lives, and the table the
/// trigger hangs off.
struct Outbox {
    /// Human name for the failure message.
    name: &'static str,
    /// File holding the `CREATE TRIGGER` (a Rust `SCHEMA` const or a migration).
    ddl: &'static str,
    /// File holding `NOTIFY_CHANNEL` and the `listen` call.
    worker: &'static str,
    /// Table the trigger fires on.
    table: &'static str,
}

const OUTBOXES: &[Outbox] = &[
    Outbox {
        name: "mako_service::outbox",
        ddl: "crates/mako-service/src/outbox.rs",
        worker: "crates/mako-service/src/outbox.rs",
        table: "event_outbox",
    },
    Outbox {
        name: "marktd fan-out",
        ddl: "services/marktd/migrations/0001_initial.sql",
        worker: "services/marktd/src/fanout.rs",
        table: "event_log",
    },
];

/// Trees where a producer-side wake-up is forbidden.
const NO_IN_PROCESS_WAKE: &[&str] = &["crates/mako-service/src/outbox.rs", "services/marktd/src"];

/// Run the check, returning `true` when both outboxes wake from the database.
pub fn run(workspace_root: &Path) -> bool {
    let mut findings: Vec<String> = Vec::new();

    for ob in OUTBOXES {
        let Ok(ddl) = std::fs::read_to_string(workspace_root.join(ob.ddl)) else {
            findings.push(format!("{}: cannot read {}", ob.name, ob.ddl));
            continue;
        };
        let Ok(worker) = std::fs::read_to_string(workspace_root.join(ob.worker)) else {
            findings.push(format!("{}: cannot read {}", ob.name, ob.worker));
            continue;
        };

        // Rule 1 — the trigger, and the channel it raises on.
        let Some(channel) = pg_notify_channel(&ddl) else {
            findings.push(format!(
                "{}: {} declares no `pg_notify(…)` — the {} outbox has no commit-coupled \
                 wake-up, so every event waits out a poll interval",
                ob.name, ob.ddl, ob.table
            ));
            continue;
        };
        let trigger_ok = ddl.contains(&format!("AFTER INSERT ON {}", ob.table))
            && ddl.contains("FOR EACH STATEMENT");
        if !trigger_ok {
            findings.push(format!(
                "{}: {} raises pg_notify('{channel}') but declares no \
                 `AFTER INSERT ON {} … FOR EACH STATEMENT` trigger to raise it from",
                ob.name, ob.ddl, ob.table
            ));
        }

        // Rule 2 — the worker listens, on that channel.
        let declared = notify_channel_const(&worker);
        match declared {
            Some(c) if c == channel => {}
            Some(c) => findings.push(format!(
                "{}: {} listens on \"{c}\" but {} raises pg_notify('{channel}') — \
                 a trigger nobody hears",
                ob.name, ob.worker, ob.ddl
            )),
            None => findings.push(format!(
                "{}: {} declares no `NOTIFY_CHANNEL` constant",
                ob.name, ob.worker
            )),
        }
        if !worker.contains("listen(NOTIFY_CHANNEL)") {
            findings.push(format!(
                "{}: {} never calls `listen(NOTIFY_CHANNEL)` — the channel is declared \
                 and nothing subscribes to it",
                ob.name, ob.worker
            ));
        }
    }

    // Rule 3 — no producer-side wake-up primitive anywhere on an outbox path.
    let mut sources = Vec::new();
    for tree in NO_IN_PROCESS_WAKE {
        let path = workspace_root.join(tree);
        if path.is_dir() {
            collect_sources(&path, &mut sources);
        } else {
            sources.push(path);
        }
    }
    for path in &sources {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for (idx, line) in blank_comments(&src).lines().enumerate() {
            if line.contains("notify_one(") {
                findings.push(format!(
                    "{rel}:{}: `notify_one` on an outbox path — an in-process wake-up taken \
                     before the commit is spent on a snapshot without the row, and cannot \
                     reach another replica at all. The trigger raises it instead.",
                    idx + 1
                ));
            }
        }
    }

    if findings.is_empty() {
        println!(
            "check-outbox-notify: both durable outboxes wake from an AFTER INSERT trigger, \
             on the channel their worker listens to"
        );
        return true;
    }

    eprintln!("check-outbox-notify: {} problem(s):", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

/// The channel name in the first `pg_notify('<channel>'` of `src`.
fn pg_notify_channel(src: &str) -> Option<String> {
    let at = src.find("pg_notify('")? + "pg_notify('".len();
    let rest = &src[at..];
    let end = rest.find('\'')?;
    Some(rest[..end].to_owned())
}

/// The value of `NOTIFY_CHANNEL: &str = "<channel>"` in `src`.
fn notify_channel_const(src: &str) -> Option<String> {
    let at = src.find("NOTIFY_CHANNEL: &str = \"")? + "NOTIFY_CHANNEL: &str = \"".len();
    let rest = &src[at..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// Blank comment bodies, keeping newlines, so a doc comment naming the forbidden
/// call — including this guard's own explanation of it — is not a call.
fn blank_comments(src: &str) -> String {
    src.lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                String::new()
            } else {
                l.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `.rs` file under `dir`.
fn collect_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}
