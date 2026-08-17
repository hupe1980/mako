//! Guard: every agent tool grant matches a real MCP tool, and says the truth
//! about whether it mutates.
//!
//! An `agentd` manifest grants `tool://einsd/get_plant` with a `mutates` flag
//! and, when it mutates, `requires_approval: true`. Both facts are *restated*
//! from the server side, where the tool is declared with rmcp's
//! `annotations(read_only_hint = …)`. Two statements of one fact drift, and both
//! directions of drift are silent:
//!
//! * **A read declared mutating** stops for a human on every call. 91 grants
//!   were in this state — every `einsd` read, every `processd` read and
//!   `obsd/list_overdue_processes` — so those specialists could not look
//!   anything up without an approval that no worklist would ever show.
//! * **A mutation declared read-only** is dispatched unattended, which is the
//!   same mistake with the consequences reversed.
//! * **A grant naming no tool at all** fails at the first call, inside a run,
//!   as a message to the model rather than as a deployment error.
//! * **A mutating grant on a `tool-calling` agent** can never be dispatched at
//!   all: the arguments come from a model completion, agentplane labels every
//!   completion untrusted, and its taint gate refuses a mutating sink with
//!   untrusted arguments — *after* a human has approved the call. 108 grants
//!   across 27 manifests were in this state, each reading as an ability the
//!   agent did not have.
//!
//! The server's annotation is the authority: it sits beside the code that does
//! or does not write. This check makes the manifests agree with it.

use std::collections::BTreeMap;
use std::path::Path;

/// Tools that no server exposes *yet*, and why that is known rather than a typo.
///
/// Nothing else may be absent. An entry here is a promise that the gap is
/// tracked in `concepts/ROADMAP.md`, not a way to silence the check.
const PLANNED_TOOLS: &[(&str, &str)] = &[(
    "netzbilanzd/get_gas_imbalance",
    "GaBi Gas imbalance has no daemon yet — see ROADMAP 'GaBi Gas: eleven of twelve \
     de.gabi.* types still have no emitter'. `gabi-gas-agent` now fires on \
     `de.gabi.alocat.missing` (the KoV §6.4 window closing unsettled), but the imbalance \
     globs it also subscribes to stay dark until IMBNOT dispatch lands; the grant is kept \
     so the manifest still describes the intended reach.",
)];

/// Check every grant in every specialist manifest.
///
/// Returns `true` when all of them resolve and agree.
pub fn run(workspace_root: &Path) -> bool {
    let tools = mcp_tools(workspace_root);
    if tools.is_empty() {
        eprintln!("check-tool-grants: found no MCP tool declarations — has the layout changed?");
        return false;
    }

    let agents = workspace_root.join("services/agentd/agents");
    let Ok(entries) = std::fs::read_dir(&agents) else {
        eprintln!(
            "check-tool-grants: no manifests at {} — has the layout changed?",
            agents.display()
        );
        return false;
    };

    let mut missing: Vec<String> = Vec::new();
    let mut wrong: Vec<String> = Vec::new();
    let mut unapproved: Vec<String> = Vec::new();
    let mut unreachable: Vec<String> = Vec::new();
    let mut grants = 0usize;

    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    files.sort();

    for file in files {
        let Ok(src) = std::fs::read_to_string(&file) else {
            continue;
        };
        let name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        // `planned` is the only shape that can dispatch a mutating call: its
        // step arguments are `$input/…` references the runtime resolves, so
        // they keep the run input's labels. A tool-calling agent's arguments
        // come out of a model completion, which agentplane labels untrusted by
        // construction, and the taint gate refuses a mutating sink with
        // untrusted arguments — after the human approved it, which is worse
        // than refusing before.
        let planned = src.lines().any(|l| l.trim() == "kind: planned");

        for grant in parse_grants(&src) {
            grants += 1;
            let key = format!("{}/{}", grant.server, grant.tool);
            // agentplane's ToolId refuses `-` in a server component (hyphens
            // are reserved so the model-facing wire rendering stays injective),
            // so a hyphenated service directory like `mabis-syncd` is granted
            // as `mabis_syncd`. The inventory is keyed by directory name;
            // fall back to the hyphen spelling before calling a grant missing.
            let dir_key = format!("{}/{}", grant.server.replace('_', "-"), grant.tool);
            let Some(read_only) = tools.get(&key).or_else(|| tools.get(&dir_key)) else {
                if !PLANNED_TOOLS.iter().any(|(t, _)| *t == key) {
                    missing.push(format!("{name}: tool://{key} — no MCP server declares it"));
                }
                continue;
            };
            let mutating = !read_only;
            if grant.mutates != mutating {
                wrong.push(format!(
                    "{name}: tool://{key} declares mutates: {}, but the server declares it {}",
                    grant.mutates,
                    if *read_only { "read-only" } else { "mutating" }
                ));
            }
            if mutating && !grant.requires_approval {
                unapproved.push(format!(
                    "{name}: tool://{key} changes the world and asks nobody"
                ));
            }
            if mutating && !planned {
                unreachable.push(format!(
                    "{name}: tool://{key} is mutating, but this agent is `tool-calling` — the \
                     call can never be dispatched (model-written arguments are untrusted, and \
                     the taint gate refuses them). Move the agent to `execution.kind: planned` \
                     or drop the grant."
                ));
            }
        }
    }

    let failures = missing.len() + wrong.len() + unapproved.len() + unreachable.len();
    if failures == 0 {
        println!(
            "check-tool-grants: {grants} grants across the specialist manifests resolve and \
             agree with the servers' own read-only hints"
        );
        for (tool, why) in PLANNED_TOOLS {
            println!("  (planned, not yet served: tool://{tool} — {why})");
        }
        return true;
    }

    for line in missing
        .iter()
        .chain(&wrong)
        .chain(&unapproved)
        .chain(&unreachable)
    {
        eprintln!("  {line}");
    }
    eprintln!(
        "\ncheck-tool-grants: {failures} problem(s). The server's \
         `annotations(read_only_hint = …)` is the authority; fix the manifest to match, or fix \
         the annotation if the tool was mis-declared."
    );
    false
}

/// One `- ref: tool://server/tool` block from a manifest.
struct Grant {
    server: String,
    tool: String,
    mutates: bool,
    requires_approval: bool,
}

fn parse_grants(src: &str) -> Vec<Grant> {
    let mut out: Vec<Grant> = Vec::new();
    for line in src.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("- ref: tool://") {
            let rest = rest.trim();
            if let Some((server, tool)) = rest.split_once('/') {
                out.push(Grant {
                    server: server.to_owned(),
                    tool: tool.to_owned(),
                    mutates: false,
                    requires_approval: false,
                });
            }
        } else if let Some(grant) = out.last_mut() {
            if let Some(v) = trimmed.strip_prefix("mutates:") {
                grant.mutates = v.trim() == "true";
            } else if let Some(v) = trimmed.strip_prefix("requires_approval:") {
                grant.requires_approval = v.trim() == "true";
            }
        }
    }
    out
}

/// `server/tool` → whether the server declares it read-only.
///
/// Read from the `#[tool(...)]` attributes themselves rather than from a
/// generated inventory: a checked-in inventory is a third statement of the same
/// fact, and it would go stale between regenerations.
fn mcp_tools(workspace_root: &Path) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    let services = workspace_root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        return out;
    };
    for entry in entries.flatten() {
        let service = entry.file_name().to_string_lossy().into_owned();
        // Most daemons keep the server at `src/mcp_server.rs`; makod nests it
        // under `src/api/`.
        for tail in ["src/mcp_server.rs", "src/api/mcp_server.rs"] {
            let path = entry.path().join(tail);
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (tool, read_only) in tools_in(&src) {
                out.insert(format!("{service}/{tool}"), read_only);
            }
        }
    }
    out
}

/// Every `#[tool(..)] async fn name` in one file, with its read-only hint.
///
/// The wire name is `name = "…"` when the attribute overrides it — `einsd`
/// declares `get_jahresmarktwert_tool` as `get_jahresmarktwert`, and a check
/// reading the function name would report a grant that is in fact correct.
fn tools_in(src: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut at = 0usize;
    while let Some(start) = src[at..].find("#[tool(") {
        let open = at + start + "#[tool(".len();
        // Balance parentheses to find the end of the attribute.
        let mut depth = 1usize;
        let mut i = open;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        let attr = &src[open..i.saturating_sub(1)];
        let after = &src[i..];
        at = i;

        let Some(fn_at) = after.find("fn ") else {
            continue;
        };
        // Only the declaration immediately following counts; anything further
        // away means this attribute is on something else.
        if after[..fn_at].contains('}') {
            continue;
        }
        let rest = &after[fn_at + 3..];
        let fn_name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if fn_name.is_empty() {
            continue;
        }

        let wire = attr
            .find("name")
            .and_then(|p| attr[p..].find('"').map(|q| p + q + 1))
            .and_then(|s| attr[s..].find('"').map(|e| attr[s..s + e].to_owned()))
            .filter(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'))
            .unwrap_or(fn_name);

        out.push((wire, attr.contains("read_only_hint = true")));
    }
    out
}
