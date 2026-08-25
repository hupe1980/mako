//! Guard: every command a service dispatches to `makod` is one `makod` has.
//!
//! `makod` refuses an unknown command name with `422`, so a caller that spells
//! the wire name a second time can drift from the registry and only find out
//! when a real message arrives: the work is done, the dispatch fails, and the
//! Frist expires on a process that looked healthy.
//!
//! `mako_markt::commands` exists so the name is written once. Three rules keep
//! callers inside it:
//!
//! 1. **No bare literals** in `invoicd`'s route table.
//! 2. **Every constant resolves** to a name `makod` registers.
//! 3. **Every constant is in `DISPATCHED_BY_SERVICES`** — one that is missing
//!    is one `makod`'s own registry test never looks at.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Every `name: "…"` in `makod`'s command registry.
fn makod_commands(workspace_root: &Path) -> BTreeSet<String> {
    let path = workspace_root.join("services/makod/src/orchestrator/commands_api/registry.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    src.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("name: \"")?;
            let end = rest.find('"')?;
            Some(rest[..end].to_owned())
        })
        .collect()
}

/// `CONST_NAME` → wire name, from the shared catalogue.
fn command_constants(workspace_root: &Path) -> BTreeMap<String, String> {
    let path = workspace_root.join("crates/mako-markt/src/commands.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeMap::new();
    };
    let mut out = BTreeMap::new();
    for line in src.lines() {
        let Some(rest) = line.trim().strip_prefix("pub const ") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once(':') else {
            continue;
        };
        let Some(open) = tail.find('"') else { continue };
        let Some(close) = tail[open + 1..].find('"') else {
            continue;
        };
        out.insert(
            name.trim().to_owned(),
            tail[open + 1..open + 1 + close].to_owned(),
        );
    }
    out
}

/// The constant names listed in `DISPATCHED_BY_SERVICES`.
fn dispatched_list(workspace_root: &Path) -> BTreeSet<String> {
    let path = workspace_root.join("crates/mako-markt/src/commands.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return BTreeSet::new();
    };
    let Some(start) = src.find("pub const DISPATCHED_BY_SERVICES") else {
        return BTreeSet::new();
    };
    // `= &[` — not the `[` of the `&[&str]` type ahead of it.
    let Some(open) = src[start..].find("= &[") else {
        return BTreeSet::new();
    };
    let open = open + 3;
    let Some(close) = src[start + open..].find(']') else {
        return BTreeSet::new();
    };
    src[start + open + 1..start + open + close]
        .split(',')
        .map(|e| e.trim().to_owned())
        .filter(|e| !e.is_empty())
        .collect()
}

/// Every `accept:` / `reject:` value in `invoicd`'s route table, with its PID.
///
/// The second element is `Ok(const_name)` for the intended form and
/// `Err(literal)` for a bare string.
fn invoicd_answers(workspace_root: &Path) -> Vec<(u32, Result<String, String>)> {
    let path = workspace_root.join("services/invoicd/src/routing.rs");
    let Ok(src) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    // The route table only — the module docs and tests mention names too.
    let body = src
        .find("pub const ROUTES")
        .map_or(src.as_str(), |at| &src[at..]);
    let body = body.find("\n];").map_or(body, |end| &body[..end]);

    let mut out = Vec::new();
    let mut pid = 0_u32;
    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("pid: ")
            && let Some(value) = rest.strip_suffix(',')
            && let Ok(parsed) = value.parse::<u32>()
        {
            pid = parsed;
        }
        for key in ["accept: ", "reject: "] {
            let Some(rest) = line.strip_prefix(key) else {
                continue;
            };
            let value = rest.trim_end_matches(',').trim();
            if let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
                out.push((pid, Err(inner.to_owned())));
            } else if let Some(name) = value.strip_prefix("commands::") {
                out.push((pid, Ok(name.to_owned())));
            }
        }
    }
    out
}

/// Check every dispatched command against the catalogue and the registry.
///
/// Returns `true` when all three rules hold.
pub fn run(workspace_root: &Path) -> bool {
    let registry = makod_commands(workspace_root);
    let constants = command_constants(workspace_root);
    let dispatched = dispatched_list(workspace_root);
    let answers = invoicd_answers(workspace_root);

    if registry.len() < 50 || constants.len() < 30 || answers.len() < 10 {
        eprintln!(
            "check-answer-commands: scanned {} registry entries, {} constants and {} \
             answer commands — has the layout changed?",
            registry.len(),
            constants.len(),
            answers.len()
        );
        return false;
    }

    let mut ok = true;
    for (pid, answer) in &answers {
        match answer {
            Err(literal) => {
                eprintln!(
                    "check-answer-commands: invoicd answers PID {pid} with the bare literal \
                     {literal:?}.\n  Name the constant in `mako_markt::commands` instead: a \
                     literal is outside the mechanism that keeps the wire name and makod's \
                     registry linked."
                );
                ok = false;
            }
            Ok(const_name) => {
                let Some(wire) = constants.get(const_name) else {
                    eprintln!(
                        "check-answer-commands: invoicd answers PID {pid} with \
                         `commands::{const_name}`, which `mako_markt::commands` does not define."
                    );
                    ok = false;
                    continue;
                };
                if !registry.contains(wire) {
                    eprintln!(
                        "check-answer-commands: invoicd answers PID {pid} with {wire:?} \
                         (`{const_name}`), which makod does not register.\n  The check would \
                         run, the verdict would be persisted, and the dispatch would fail — so \
                         the answer never reaches the counterparty and the Antwortfrist expires \
                         on a process that looked healthy."
                    );
                    ok = false;
                }
                if !dispatched.contains(const_name) {
                    eprintln!(
                        "check-answer-commands: `{const_name}` is dispatched by invoicd but is \
                         missing from `DISPATCHED_BY_SERVICES`, so makod's own registry test \
                         never checks it."
                    );
                    ok = false;
                }
            }
        }
    }

    if ok {
        println!(
            "check-answer-commands: {} answer commands all name a catalogued constant that \
             makod registers",
            answers.len()
        );
    }
    ok
}
