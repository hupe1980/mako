//! Guard: a registered deadline must reach the workflow that owns it.
//!
//! [`mako_engine::workflow::Workflow::on_deadline`] is keyed on the deadline's
//! **label** and its default answers `None`, so a label the workflow does not
//! match produces a deadline that fires, does nothing, and leaves the process in
//! its waiting state — no error, no event, no alert, and the deadline store
//! shows it as fired. Nothing else ties the string being registered to the
//! string being matched; the sources are the only place both ends are visible.
//!
//! Three rules:
//!
//! 1. **Every label constant is handled** — a `pub const …_LABEL: &str` in a
//!    `mako-*` crate appears inside some `on_deadline` body, unless it is a
//!    [delivery window](DELIVERY_WINDOWS).
//! 2. **No label is written twice** — `Deadline::new` and
//!    `PendingDeadline::new` take the owning crate's constant, never a literal.
//! 3. **No `on_deadline` is a catch-all** — a body ignoring `deadline.label()`
//!    also consumes the APERAK and CONTRL delivery windows running beside the
//!    business Frist, so a late acknowledgement fails the business process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Labels that are **delivery windows**, not business Fristen.
///
/// A delivery window asks one question — *did this message go out in time?* —
/// and `OutboxWorker::discharge_delivery_window` retires it the moment the
/// message is delivered. One that fires is a regulatory alert raised by `obsd`,
/// not a state transition, so no `on_deadline` matches it and none should:
/// letting a workflow consume its own APERAK window would fail the business
/// process because a technical acknowledgement was late.
///
/// See `mako_fristen::discharges_delivery_window`.
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

/// The body of every `fn on_deadline` in `src`, paired with its file.
fn on_deadline_bodies(sources: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for path in sources {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        // Inline `#[cfg(test)] mod tests` blocks construct deadlines and call
        // `on_deadline` directly; only the production impl is under scrutiny,
        // and its brace depth is what the body scan below relies on.
        let src = src
            .split_once("\n#[cfg(test)]")
            .map_or(src.as_str(), |(before, _)| before);
        let mut from = 0;
        while let Some(start) = src[from..].find("fn on_deadline") {
            let start = from + start;
            // The impl block indents trait methods by four spaces, so the first
            // `\n    }` after the signature closes the method.
            let end = src[start..]
                .find("\n    }")
                .map_or(src.len(), |e| start + e + 6);
            out.push((path.clone(), src[start..end].to_owned()));
            from = end;
        }
    }
    out
}

/// `pub const NAME: &str = "value";` in the `mako-*` crates, where `NAME`
/// names a deadline label.
fn label_constants(sources: &[PathBuf]) -> BTreeMap<String, (String, PathBuf)> {
    let mut out = BTreeMap::new();
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
            out.insert(name.to_owned(), (value, path.clone()));
        }
    }
    out
}

/// Rule 1 — every label constant is matched by some `on_deadline`.
#[test]
fn every_deadline_label_is_handled_by_a_workflow() {
    let root = workspace_root();
    let crate_sources = crate_src_sources(&root);
    let constants = label_constants(&crate_sources);
    assert!(
        constants.len() > 30,
        "the scanner found only {} label constants — it stopped matching the sources",
        constants.len()
    );

    let handled: String = on_deadline_bodies(&crate_sources)
        .into_iter()
        .map(|(_, body)| body)
        .collect();

    let unhandled: Vec<_> = constants
        .iter()
        .filter(|(name, (value, _))| {
            !DELIVERY_WINDOWS.contains(&name.as_str())
                && !handled.contains(name.as_str())
                && !handled.contains(value.as_str())
        })
        .map(|(name, (value, path))| {
            format!(
                "{name} = {value:?} ({})",
                path.strip_prefix(&root).unwrap_or(path).display()
            )
        })
        .collect();

    assert!(
        unhandled.is_empty(),
        "these deadline labels are registered but no `on_deadline` matches them, so \
         the deadline fires into the engine's default `None` and the process never \
         times out.\n  {}\n\nEither match the label in the owning workflow's \
         `on_deadline`, or — if it is a delivery window the outbox worker \
         discharges — add it to `DELIVERY_WINDOWS` with the reason.",
        unhandled.join("\n  ")
    );
}

/// Rule 2 — a label is registered by its constant, never by a literal copy.
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
        let src = src
            .split_once("\n#[cfg(test)]")
            .map_or(src.as_str(), |(before, _)| before)
            .to_owned();
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

/// Rule 3 — no `on_deadline` accepts every label on the stream.
#[test]
fn no_on_deadline_is_a_catch_all() {
    let root = workspace_root();
    let bodies = on_deadline_bodies(&crate_src_sources(&root));
    assert!(
        bodies.len() > 10,
        "the scanner found only {} `on_deadline` bodies — it stopped matching the sources",
        bodies.len()
    );

    let offenders: Vec<_> = bodies
        .iter()
        .filter(|(_, body)| {
            // A body that decides must *read* the label, not merely copy it into
            // the command it returns. Matching on the state alone accepts the
            // APERAK/CONTRL delivery windows that run beside the business Frist.
            let echoed = body
                .replace("deadline.label().into()", "")
                .replace("deadline.label().to_owned()", "")
                .replace("deadline.label().to_string()", "");
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
         label, so they consume every deadline registered on the stream — including \
         the APERAK 45-minute and CONTRL 6-hour *delivery* windows that run beside \
         the business Frist. A late acknowledgement then fails the business \
         process:\n  {}",
        offenders.join("\n  ")
    );
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
