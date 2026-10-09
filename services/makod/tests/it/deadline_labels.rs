//! Guard: a registered deadline reaches the workflow that owns it, and that
//! workflow decides it through one guard.
//!
//! Every fired deadline takes one path into a workflow:
//! `deadline_dispatch` → [`mako_engine::process::Process::run_timeout`]
//! → [`mako_engine::workflow::Workflow::on_deadline`]. A label outside the
//! workflow's [`mako_engine::workflow::Workflow::DEADLINE_LABELS`] never reaches
//! the hook, and the engine refuses to register such a deadline at all. What is
//! left to check is the part neither the compiler nor the engine sees:
//!
//! 1. **Every label constant is owned** — a `pub const …_LABEL: &str` in a
//!    `mako-*` crate is listed in some workflow's `DEADLINE_LABELS`, or is a
//!    delivery window ([`mako_fristen::is_delivery_window`]).
//! 2. **Every owned label is decided** — each name a workflow lists in
//!    `DEADLINE_LABELS` appears in that file's guard: the `on_deadline` body or
//!    the `window_open` function both it and the `TimeoutExpired` arm call. A
//!    label listed and never matched would be accepted by the engine and then
//!    answered `None` on every firing.
//! 3. **The `TimeoutExpired` arm asks the same guard** — a `handle` arm that
//!    checks only `is_terminal()` acts on every label in every waiting state,
//!    which is how a healthy, answered process came to be rejected.
//! 4. **No guard decides a delivery window** — the APERAK and CONTRL windows
//!    are alerts about a message, never steps of the process beside them.
//! 5. **No label is written twice**, and **no `on_deadline` is a catch-all**.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Delivery-window constants: discharged by the outbox worker on delivery,
/// alerted on by the dispatcher when they fire, owned by no workflow.
const DELIVERY_WINDOWS: &[&str] = &[
    "APERAK_STROM_WINDOW_LABEL",
    "APERAK_GAS_FOLGEPROZESS_LABEL",
    "APERAK_GAS_INITIALPROZESS_LABEL",
    "APERAK_WINDOW_LABEL_PREFIX",
    "CONTRL_FRIST_LABEL",
];

/// Helper functions that resolve a label at the call site.
///
/// A `Deadline::new` argument that is a call to one of these is accepted: the
/// function itself returns constants, and the constants are checked by rule 1.
/// Anything else must be a constant path.
const LABEL_RESOLVERS: &[&str] = &["device_change_window_label"];

/// The functions a workflow's one guard may live in. `on_deadline` and the
/// `TimeoutExpired` arm both call it.
const GUARD_FNS: &[&str] = &["window_open", "answer_owed", "nomres_window_open"];

fn workspace_root() -> PathBuf {
    // …/services/makod → …
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("makod lives two levels below the workspace root")
        .to_path_buf()
}

/// Every `.rs` file under `root/rel`, recursively, excluding `target`.
fn rust_sources(root: &Path, rel: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join(rel), &mut out);
    out.sort();
    out
}

/// Every `crates/*/src/**/*.rs` — production code only, no tests, examples or
/// benches.
fn crate_src_sources(root: &Path) -> Vec<PathBuf> {
    rust_sources(root, "crates")
        .into_iter()
        .filter(|p| p.components().any(|c| c.as_os_str() == "src"))
        .collect()
}

/// `src` without its inline `#[cfg(test)] mod tests` tail.
fn production(src: &str) -> &str {
    src.split_once("\n#[cfg(test)]")
        .map_or(src, |(before, _)| before)
}

/// The text of the brace-delimited block opening at the first `{` at or after
/// `from`.
fn block_from(src: &str, from: usize) -> &str {
    let Some(open) = src[from..].find('{').map(|i| from + i) else {
        return "";
    };
    let mut depth = 0usize;
    for (i, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[from..=open + i];
                }
            }
            _ => {}
        }
    }
    &src[from..]
}

/// The bodies of every `fn <name>` in `src`.
fn fn_bodies<'a>(src: &'a str, name: &str) -> Vec<&'a str> {
    let needle = format!("fn {name}");
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find(&needle) {
        let at = from + at;
        // `fn window_open<F: …>(` and `fn window_open(` both count; a longer
        // name sharing the prefix does not.
        let next = src[at + needle.len()..].chars().next();
        if matches!(next, Some('(' | '<')) {
            out.push(block_from(src, at));
        }
        from = at + needle.len();
    }
    out
}

/// The guard text of one file: every `on_deadline` body plus every guard fn.
fn guard_text(src: &str) -> String {
    let mut text = String::new();
    for body in fn_bodies(src, "on_deadline") {
        text.push_str(body);
        text.push('\n');
    }
    for name in GUARD_FNS {
        for body in fn_bodies(src, name) {
            text.push_str(body);
            text.push('\n');
        }
    }
    text
}

/// The names listed in each `const DEADLINE_LABELS … = &[ … ];` of `src`.
fn declared_label_names(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(at) = src[from..].find("const DEADLINE_LABELS") {
        let at = from + at;
        let Some(eq) = src[at..].find('=').map(|i| at + i) else {
            break;
        };
        let Some(end) = src[eq..].find(';').map(|i| eq + i) else {
            break;
        };
        let list = &src[eq + 1..end];
        let list = list.trim().trim_start_matches('&').trim();
        let list = list.trim_start_matches('[').trim_end_matches(']');
        out.extend(
            list.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                // `module::NAME` and `LEG.window_label` are named by their
                // last segment, which is what a guard spells.
                .map(|s| s.rsplit(['.', ':']).next().unwrap_or(s).to_owned()),
        );
        from = end;
    }
    out
}

/// `pub const NAME: &str = "value";` in the `mako-*` crates, where `NAME`
/// names a deadline label.
fn label_constants(sources: &[PathBuf]) -> BTreeMap<String, Vec<(String, PathBuf)>> {
    let mut out: BTreeMap<String, Vec<(String, PathBuf)>> = BTreeMap::new();
    for path in sources {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in src.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("pub const ") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once(':') else {
                continue;
            };
            let name = name.trim();
            if !(name.ends_with("LABEL") || name.ends_with("WINDOW")) {
                continue;
            }
            if !tail.contains("&str") && !tail.contains("&'static str") {
                continue;
            }
            let Some(open) = tail.find('"') else { continue };
            let Some(close) = tail[open + 1..].find('"') else {
                continue;
            };
            let value = tail[open + 1..open + 1 + close].to_owned();
            out.entry(name.to_owned())
                .or_default()
                .push((value, path.clone()));
        }
    }
    out
}

/// The `TimeoutExpired` arms of `src` that do not ask the guard.
///
/// An arm is the text from `…TimeoutExpired { … } =>` to the next arm at the
/// same indentation; it passes when it calls one of [`GUARD_FNS`].
fn label_blind_timeout_arms(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let lines: Vec<&str> = src.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let is_arm = t.contains("TimeoutExpired {")
            && !t.starts_with("Some(")
            && !t.starts_with("//")
            && lines[i..lines.len().min(i + 4)]
                .iter()
                .any(|l| l.contains("} =>"));
        if !is_arm || t.starts_with("TimeoutExpired {") {
            continue;
        }
        let indent = line.len() - t.len();
        let arm: Vec<&str> = lines[i..]
            .iter()
            .enumerate()
            .take_while(|(k, l)| {
                *k == 0 || {
                    let lt = l.trim_start();
                    l.trim().is_empty() || l.len() - lt.len() > indent || lt.starts_with('}')
                }
            })
            .map(|(_, l)| *l)
            .take(40)
            .collect();
        let arm = arm.join("\n");
        let asks = GUARD_FNS
            .iter()
            .any(|g| arm.contains(&format!("{g}(")) || arm.contains(&format!("{g}::<")));
        if !asks {
            out.push(i + 1);
        }
    }
    out
}

/// Files that declare a `DEADLINE_LABELS` table, paired with their production
/// source.
fn workflow_files(root: &Path) -> Vec<(PathBuf, String)> {
    crate_src_sources(root)
        .into_iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(&p).ok()?;
            let src = production(&src).to_owned();
            src.contains("const DEADLINE_LABELS").then_some((p, src))
        })
        .collect()
}

/// Rule 1 — every label constant is owned by a workflow the dispatcher routes
/// to, or is a delivery window.
#[test]
fn every_deadline_label_is_owned_by_a_routed_workflow() {
    let root = workspace_root();
    let constants = label_constants(&crate_src_sources(&root));
    assert!(
        constants.len() > 30,
        "the scanner found only {} label constants — it stopped matching the sources",
        constants.len()
    );
    let owned: Vec<&str> = makod::orchestrator::deadline_dispatch::owned_labels()
        .into_iter()
        .flat_map(|(_, labels)| labels.iter().copied())
        .collect();

    let mut unowned = Vec::new();
    for (name, defs) in &constants {
        if DELIVERY_WINDOWS.contains(&name.as_str()) {
            continue;
        }
        for (value, path) in defs {
            if !owned.contains(&value.as_str()) {
                unowned.push(format!(
                    "{name} = {value:?} ({})",
                    path.strip_prefix(&root).unwrap_or(path).display()
                ));
            }
        }
    }

    assert!(
        unowned.is_empty(),
        "these deadline labels are owned by no workflow the deadline dispatcher \
         routes to. The engine refuses to register them, and nothing would decide \
         one that fired:\n  {}\n\nList the label in the owning workflow's \
         `DEADLINE_LABELS` and decide it in its guard — or, for a delivery window \
         the outbox worker discharges, add it to `DELIVERY_WINDOWS`.",
        unowned.join("\n  ")
    );
}

/// Rule 2 — every label a workflow owns is matched by that workflow's guard.
#[test]
fn every_owned_label_is_decided_by_the_guard() {
    let root = workspace_root();
    let files = workflow_files(&root);
    assert!(
        files.len() > 30,
        "the scanner found only {} workflow files with a DEADLINE_LABELS table",
        files.len()
    );
    let mut undecided = Vec::new();
    for (path, src) in &files {
        let guard = guard_text(src);
        for name in declared_label_names(src) {
            if !guard.contains(&name) {
                undecided.push(format!(
                    "{}: {name}",
                    path.strip_prefix(&root).unwrap_or(path).display()
                ));
            }
        }
    }
    assert!(
        undecided.is_empty(),
        "these labels are listed in DEADLINE_LABELS but no guard matches them, so \
         every firing is answered `None`:\n  {}",
        undecided.join("\n  ")
    );
}

/// Rule 3 — every `TimeoutExpired` arm asks the same guard `on_deadline` does.
#[test]
fn every_timeout_arm_asks_the_guard() {
    let root = workspace_root();
    let mut blind = Vec::new();
    for (path, src) in workflow_files(&root) {
        for line in label_blind_timeout_arms(&src) {
            blind.push(format!(
                "{}:{line}",
                path.strip_prefix(&root).unwrap_or(&path).display()
            ));
        }
    }
    assert!(
        blind.is_empty(),
        "these `TimeoutExpired` arms decide without the workflow's guard. An arm \
         that checks only `is_terminal()` acts on every label in every waiting \
         state — an answered process is then ended by its own discharged window:\n  {}",
        blind.join("\n  ")
    );
}

/// The arm check catches the label-blind shape it exists for.
#[test]
fn the_arm_check_catches_a_label_blind_arm() {
    let planted = r#"
        match command {
            ThingCommand::TimeoutExpired { deadline_id, label } => {
                if state.is_terminal() {
                    return Ok(vec![].into());
                }
                Ok(vec![ThingEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    "#;
    assert_eq!(label_blind_timeout_arms(planted).len(), 1);

    let guarded = r#"
        match command {
            ThingCommand::TimeoutExpired { deadline_id, label } => {
                if !window_open(&label, lapsed) {
                    return Ok(vec![].into());
                }
                Ok(vec![ThingEvent::DeadlineExpired { deadline_id, label }].into())
            }
        }
    "#;
    assert!(label_blind_timeout_arms(guarded).is_empty());
}

/// The undecided-label check catches a label listed and never matched.
#[test]
fn the_decided_check_catches_an_unmatched_label() {
    let planted = r#"
        const DEADLINE_LABELS: &'static [&'static str] = &[A_LABEL, B_LABEL];
        fn on_deadline(deadline: &Deadline, state: &Self::State) -> Option<Self::Command> {
            window_open(deadline.label(), state).then(|| todo!())
        }
        fn window_open(label: &str, state: &S) -> bool {
            label == A_LABEL && matches!(state, S::Open)
        }
    "#;
    let guard = guard_text(planted);
    let undecided: Vec<String> = declared_label_names(planted)
        .into_iter()
        .filter(|n| !guard.contains(n))
        .collect();
    assert_eq!(undecided, vec!["B_LABEL".to_owned()]);
}

/// Rule 4 — no workflow guard decides a delivery window.
#[test]
fn no_guard_decides_a_delivery_window() {
    let root = workspace_root();
    let mut offenders = Vec::new();
    for (path, src) in workflow_files(&root) {
        let guard = guard_text(&src);
        for window in DELIVERY_WINDOWS {
            if guard.contains(window) {
                offenders.push(format!(
                    "{}: {window}",
                    path.strip_prefix(&root).unwrap_or(&path).display()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these workflow guards match a delivery window. The dispatcher alerts on a \
         lapsed delivery window before any workflow is consulted; a guard that \
         matched one would let a late acknowledgement end the business process:\n  {}",
        offenders.join("\n  ")
    );
}

/// The shared INVOIC workflow decides on the family's declared label.
///
/// The four billing families share one state machine, so their table is
/// `&[F::DEADLINE_LABEL]` — a generic path rule 1 resolves through the
/// dispatcher's `owned_labels`, and this test holds the guard to it.
#[test]
fn shared_invoic_guard_matches_the_family_label() {
    let root = workspace_root();
    let src = std::fs::read_to_string(root.join("crates/mako-invoic/src/lib.rs"))
        .expect("mako-invoic declares the shared INVOIC workflow");
    let guard = guard_text(production(&src));
    assert!(
        guard.contains("F::DEADLINE_LABEL"),
        "the shared INVOIC guard must decide on the family's declared label; \
         otherwise every billing family's settlement deadline is answered `None`"
    );
}

/// Rule 5a — a label is registered by its constant, never by a literal copy.
#[test]
fn no_deadline_is_registered_with_a_string_literal_label() {
    let root = workspace_root();
    let mut sources = rust_sources(&root, "services");
    sources.extend(rust_sources(&root, "crates"));

    let mut offenders = Vec::new();
    for path in &sources {
        // Tests, examples and benches construct deadlines from literals to
        // drive a workflow directly; the guard is about the production
        // registration sites, where a literal can drift from the constant the
        // workflow matches.
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some("tests" | "examples" | "benches")
            )
        }) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let src = production(&src).to_owned();
        for ctor in ["Deadline::new(", "PendingDeadline::new("] {
            let mut from = 0;
            while let Some(at) = src[from..].find(ctor) {
                let at = from + at;
                let args_start = at + ctor.len();
                let Some(args) = balanced_args(&src[args_start..]) else {
                    from = args_start;
                    continue;
                };
                let parts = split_top_level(args);
                // `Deadline::new(stream, process, tenant, workflow, label, due)`
                // `PendingDeadline::new(label, due)`
                let label = if ctor.starts_with("Pending") {
                    parts.first()
                } else {
                    parts.get(4)
                };
                if let Some(label) = label {
                    let label = label.trim();
                    let is_literal = label.starts_with('"');
                    let is_resolver = LABEL_RESOLVERS.iter().any(|f| label.starts_with(f));
                    if is_literal && !is_resolver {
                        let line = src[..at].matches('\n').count() + 1;
                        offenders.push(format!(
                            "{}:{line}  {label}",
                            path.strip_prefix(&root).unwrap_or(path).display()
                        ));
                    }
                }
                from = args_start + args.len();
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these deadlines are registered with a literal label instead of the owning \
         crate's constant. A literal drifts silently from the constant the workflow \
         matches, and the deadline then fires into nothing:\n  {}",
        offenders.join("\n  ")
    );
}

/// Rule 5b — a label constant's text appears nowhere else as a literal.
///
/// Rule 5a reads the argument a deadline constructor is given, so a label bound
/// to a local first — `let (workflow, label, hours) = match kind { … }` — walks
/// past it. This one is positional-free: any production statement that spells a
/// label constant's *text* has a second copy of it, and the two are then free
/// to drift. Doc comments are exempt; they register nothing.
#[test]
fn no_label_constant_is_spelled_a_second_time() {
    let root = workspace_root();
    let crate_sources = crate_src_sources(&root);
    let constants = label_constants(&crate_sources);
    // Where each label's own constant is declared — that file is the one place
    // the text belongs.
    let declared: BTreeMap<&str, &str> = constants
        .iter()
        .flat_map(|(name, defs)| {
            defs.iter()
                .map(move |(value, _)| (value.as_str(), name.as_str()))
        })
        .collect();

    let mut sources = rust_sources(&root, "services");
    sources.extend(crate_sources.iter().cloned());
    let mut offenders = Vec::new();
    for path in &sources {
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some("tests" | "examples" | "benches")
            )
        }) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let src = production(&src).to_owned();
        for (value, name) in &declared {
            let literal = format!("\"{value}\"");
            // The declaration itself is the one legitimate spelling.
            let declaration = format!("{name}: &str = {literal}");
            let declaration_static = format!("{name}: &'static str = {literal}");
            let mut from = 0;
            while let Some(at) = src[from..].find(&literal) {
                let at = from + at;
                from = at + literal.len();
                let line_start = src[..at].rfind('\n').map_or(0, |i| i + 1);
                let line_end = src[at..].find('\n').map_or(src.len(), |i| at + i);
                let line = &src[line_start..line_end];
                if line.contains(&declaration) || line.contains(&declaration_static) {
                    continue;
                }
                // A doc comment quoting the label is not a second
                // registration: no deadline is constructed from it.
                if line.trim_start().starts_with("//") {
                    continue;
                }
                offenders.push(format!(
                    "{}:{}  {literal} — say {name}",
                    path.strip_prefix(&root).unwrap_or(path).display(),
                    src[..at].matches('\n').count() + 1
                ));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these deadline labels are spelled as a literal beside the constant that \
         declares them. Two spellings drift, and a deadline whose label no guard \
         matches fires into nothing:\n  {}",
        offenders.join("\n  ")
    );
}

/// Rule 5c — no `on_deadline` accepts every label on the stream.
#[test]
fn no_on_deadline_is_a_catch_all() {
    let root = workspace_root();
    let mut bodies = Vec::new();
    // The engine's own default answers `None` to everything; it decides nothing.
    for path in crate_src_sources(&root)
        .into_iter()
        .filter(|p| !p.starts_with(root.join("crates/mako-engine")))
    {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for body in fn_bodies(production(&src), "on_deadline") {
            bodies.push((path.clone(), body.to_owned()));
        }
    }
    assert!(
        bodies.len() > 30,
        "the scanner found only {} `on_deadline` bodies — it stopped matching the sources",
        bodies.len()
    );

    let offenders: Vec<_> = bodies
        .iter()
        .filter(|(_, body)| {
            // A body that decides must *read* the label, not merely copy it into
            // the command it returns.
            let echoed = body
                .replace("label: deadline.label().into()", "")
                .replace("label: deadline.label().to_owned()", "")
                .replace("label: deadline.label().to_string()", "");
            !echoed.contains("deadline.label()")
        })
        .map(|(path, _)| {
            path.strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string()
        })
        .collect();

    assert!(
        offenders.is_empty(),
        "these `on_deadline` implementations do not discriminate on the deadline \
         label, so they would decide every deadline registered on the stream:\n  {}",
        offenders.join("\n  ")
    );
}

/// Rule 6 — every owned label is registered by some production code.
///
/// A label a workflow lists and decides but nothing ever registers is a window
/// that never opens: the guard reads as if the Frist were watched, and no
/// deadline is ever stored for it. A label counts as registered when it is
/// mentioned in shipped code anywhere but its own declaration, a `use`, a
/// `DEADLINE_LABELS` table or the guard that decides it — the constructor
/// argument, the `(label, due)` pair a door hands `spawn_process`, the table
/// field a constructor later reads.
#[test]
fn every_owned_label_is_registered_somewhere() {
    let root = workspace_root();
    let mut sources = crate_src_sources(&root);
    sources.extend(
        rust_sources(&root, "services")
            .into_iter()
            .filter(|p| p.components().any(|c| c.as_os_str() == "src")),
    );
    let files: Vec<(PathBuf, String)> = sources
        .into_iter()
        .filter_map(|p| {
            let src = std::fs::read_to_string(&p).ok()?;
            Some((p, without_test_items(&src)))
        })
        .collect();
    let owned: Vec<&str> = makod::orchestrator::deadline_dispatch::owned_labels()
        .into_iter()
        .flat_map(|(_, labels)| labels.iter().copied())
        .collect();
    let unregistered = unregistered_labels(&root, &files, &owned);
    assert!(
        unregistered.is_empty(),
        "these labels are owned and decided but no production code registers them, \
         so the window they name never opens:\n  {}\n\nRegister each from the Frist \
         `mako_fristen` publishes, or — where no Festlegung quantifies one — remove \
         it from the workflow's DEADLINE_LABELS.",
        unregistered.join("\n  ")
    );
}

/// The registration check catches a label that is only listed and decided.
#[test]
fn the_registration_check_catches_an_unregistered_label() {
    let root = PathBuf::from("/ws");
    let workflow = r#"
pub const ANTWORT_WINDOW_LABEL: &str = "thing-antwort";
pub const REGISTERED_LABEL: &str = "thing-registered";
impl Workflow for Thing {
    const DEADLINE_LABELS: &'static [&'static str] = &[ANTWORT_WINDOW_LABEL, REGISTERED_LABEL];
    fn on_deadline(deadline: &Deadline, state: &S) -> Option<C> {
        window_open(deadline.label(), state).then(|| todo!())
    }
}
fn window_open(label: &str, state: &S) -> bool {
    label == ANTWORT_WINDOW_LABEL || label == REGISTERED_LABEL
}
"#;
    // Another module's constant of the same name is registered — and must not
    // count for this one.
    let other = r#"
pub const ANTWORT_WINDOW_LABEL: &str = "other-antwort";
fn send() { PendingDeadline::new(ANTWORT_WINDOW_LABEL, due); }
"#;
    let door = r#"
use mako_thing::thing::REGISTERED_LABEL;
async fn door() { spawn_process(cmd, &[(REGISTERED_LABEL, due)]).await; }
"#;
    let files = vec![
        (
            root.join("crates/mako-thing/src/thing.rs"),
            workflow.to_owned(),
        ),
        (
            root.join("crates/mako-thing/src/other.rs"),
            other.to_owned(),
        ),
        (root.join("services/makod/src/door.rs"), door.to_owned()),
    ];
    let unregistered = unregistered_labels(
        &root,
        &files,
        &["thing-antwort", "thing-registered", "other-antwort"],
    );
    assert_eq!(
        unregistered,
        vec!["thing-antwort (crates/mako-thing/src/thing.rs)".to_owned()]
    );
}

/// `src` without every `#[cfg(test)]` item — not only a trailing test module.
///
/// A test-only helper in the middle of a file is common; cutting at the first
/// `#[cfg(test)]` would drop the production code after it.
fn without_test_items(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(at) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..at]);
        let item = &rest[at..];
        let semi = item.find(';');
        let brace = item.find('{');
        let end = match (semi, brace) {
            (Some(s), Some(b)) if s < b => s + 1,
            (_, Some(b)) => {
                let block = block_from(item, b);
                b + block.len()
            }
            (Some(s), None) => s + 1,
            (None, None) => item.len(),
        };
        rest = &item[end.min(item.len())..];
    }
    out.push_str(rest);
    out
}

/// One label constant: its crate (as a path segment), its module stem, its
/// name and its value.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LabelConst {
    krate: String,
    module: String,
    name: String,
    value: String,
    file: PathBuf,
}

/// `crates/mako-gpke/src/datenabruf.rs` → (`mako_gpke`, `datenabruf`); a
/// `lib.rs` or `mod.rs` names its directory.
fn crate_and_module(root: &Path, path: &Path) -> (String, String) {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    let krate = parts.get(1).cloned().unwrap_or_default().replace('-', "_");
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let module = if stem == "lib" || stem == "main" {
        krate.clone()
    } else if stem == "mod" {
        parts
            .get(parts.len().saturating_sub(2))
            .cloned()
            .unwrap_or_default()
    } else {
        stem
    };
    (krate, module)
}

/// `src` with comment lines, `use` statements, `pub const` label definitions,
/// `DEADLINE_LABELS` tables and the guard bodies blanked — what is left is
/// where a label can be registered.
fn registration_text(src: &str) -> String {
    let mut text = String::with_capacity(src.len());
    let mut in_use = false;
    for line in src.lines() {
        let t = line.trim_start();
        let skip = in_use
            || t.starts_with("//")
            || t.starts_with("use ")
            || t.starts_with("pub use ")
            || t.starts_with("pub(crate) use ")
            || (t.contains("const ") && t.contains("&str"))
            || (t.contains("const ") && t.contains("&'static str"))
            || t.contains("DEADLINE_LABELS");
        if t.starts_with("use ") || t.starts_with("pub use ") || t.starts_with("pub(crate) use ") {
            in_use = !t.ends_with(';');
        } else if in_use && t.ends_with(';') {
            in_use = false;
            text.push('\n');
            continue;
        }
        if !skip {
            text.push_str(line);
        }
        text.push('\n');
    }
    for name in std::iter::once("on_deadline").chain(GUARD_FNS.iter().copied()) {
        for body in fn_bodies(src, name) {
            text = text.replace(body, "");
        }
    }
    text
}

/// The labels in `owned` that no production mention registers, as
/// `value (file)`.
fn unregistered_labels(root: &Path, files: &[(PathBuf, String)], owned: &[&str]) -> Vec<String> {
    // Every label constant, with where it lives.
    let mut consts: Vec<LabelConst> = Vec::new();
    for (path, src) in files {
        if !path.starts_with(root.join("crates")) {
            continue;
        }
        let (krate, module) = crate_and_module(root, path);
        for line in src.lines() {
            let t = line.trim();
            let Some(rest) = t.strip_prefix("pub const ") else {
                continue;
            };
            let Some((name, tail)) = rest.split_once(':') else {
                continue;
            };
            let Some(open) = tail.find('"') else { continue };
            let Some(close) = tail[open + 1..].find('"') else {
                continue;
            };
            consts.push(LabelConst {
                krate: krate.clone(),
                module: module.clone(),
                name: name.trim().to_owned(),
                value: tail[open + 1..open + 1 + close].to_owned(),
                file: path.clone(),
            });
        }
    }
    // `X as Y` anywhere: `Y` also spells `X`.
    let mut aliases: Vec<(String, String)> = Vec::new();
    for (_, src) in files {
        for c in &consts {
            let needle = format!("{} as ", c.name);
            for (at, _) in src.match_indices(&needle) {
                let alias: String = src[at + needle.len()..]
                    .chars()
                    .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                    .collect();
                if !alias.is_empty() && !aliases.contains(&(alias.clone(), c.name.clone())) {
                    aliases.push((alias, c.name.clone()));
                }
            }
        }
    }

    let mut registered: Vec<&LabelConst> = Vec::new();
    for (path, src) in files {
        let text = registration_text(src);
        let bytes = text.as_bytes();
        let (here_krate, _) = crate_and_module(root, path);
        for c in &consts {
            let spellings = std::iter::once(c.name.clone()).chain(
                aliases
                    .iter()
                    .filter(|(_, n)| *n == c.name)
                    .map(|(a, _)| a.clone()),
            );
            for spelling in spellings {
                for (at, _) in text.match_indices(spelling.as_str()) {
                    let before = at.checked_sub(1).map(|i| bytes[i]);
                    let after = bytes.get(at + spelling.len()).copied();
                    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
                    if before.is_some_and(is_ident) || after.is_some_and(is_ident) {
                        continue;
                    }
                    // The path segment before the name, if any.
                    let qualifier = text[..at].strip_suffix("::").map(|head| {
                        head.rsplit(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
                            .next()
                            .unwrap_or("")
                            .to_owned()
                    });
                    let refers = match qualifier.as_deref() {
                        Some(q) => q == c.module || q == c.krate,
                        // A bare spelling names the file's own constant where it
                        // defines one, and otherwise one its imports bring in.
                        None => {
                            let defines_own =
                                consts.iter().any(|o| o.file == *path && o.name == spelling);
                            if defines_own {
                                c.file == *path && c.name == spelling
                            } else {
                                c.file == *path
                                    || src.contains(&format!("{}::{}", c.module, c.name))
                                    || src.contains(&format!("{}::{{", c.module))
                                        && src.contains(&c.name)
                                    || (here_krate != c.krate
                                        && src.contains(&c.krate)
                                        && src.contains(&c.name))
                            }
                        }
                    };
                    if refers && !registered.contains(&c) {
                        registered.push(c);
                    }
                }
            }
        }
    }

    let mut out: Vec<String> = consts
        .iter()
        .filter(|c| owned.contains(&c.value.as_str()))
        .filter(|c| !registered.iter().any(|r| r.value == c.value))
        .map(|c| {
            format!(
                "{} ({})",
                c.value,
                c.file.strip_prefix(root).unwrap_or(&c.file).display()
            )
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The substring of `s` inside the parentheses that `s` opens into.
fn balanced_args(s: &str) -> Option<&str> {
    let mut depth = 1usize;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on commas that are not nested inside brackets.
fn split_top_level(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, ch) in args.char_indices() {
        match ch {
            '(' | '[' | '{' | '<' => depth += 1,
            ')' | ']' | '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&args[start..]);
    parts
}
