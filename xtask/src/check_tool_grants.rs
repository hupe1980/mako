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

use std::collections::{BTreeMap, BTreeSet};
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
    if !inventory_matches_the_docs(workspace_root, &tools) {
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

/// The documented size of the agent surface, checked against the source.
///
/// `README.md`, the landing page and `concepts/MARKET_LANDSCAPE.md` each state
/// how many services expose an MCP server and how many tools they add up to.
/// Those are the numbers a reader uses to decide whether the platform is worth
/// looking at, and nothing regenerates them — the previous figure ("14 of 17")
/// was wrong by one service and had been for a while, which is what a count with
/// no guard does.
///
/// Checked here rather than in a command of its own because this file already
/// walks every `#[tool(...)]` in the workspace; a second walker would be a
/// second definition of "an MCP tool" waiting to disagree with this one.
/// How many MCP **prompts** the platform publishes, and across how many servers.
///
/// A context grant is not a tool grant, so `check-tool-grants` would not
/// naturally count these — but they are the other half of what a specialist is
/// given, they are stated in 26 manifest comments and in the README, and nothing
/// regenerated the figure. It read "fifty of them across thirteen servers" while
/// the real numbers were 57 and 15, in every one of those places at once.
fn prompt_count(workspace_root: &Path) -> (usize, usize) {
    let services = workspace_root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        return (0, 0);
    };
    let mut total = 0usize;
    let mut serving = 0usize;
    for entry in entries.flatten() {
        let mut here = 0usize;
        for tail in ["src/mcp_server.rs", "src/api/mcp_server.rs"] {
            if let Ok(src) = std::fs::read_to_string(entry.path().join(tail)) {
                here += src.matches("#[prompt(").count();
            }
        }
        if here > 0 {
            serving += 1;
            total += here;
        }
    }
    (total, serving)
}

/// Every `tool://` grant across the specialist manifests.
///
/// Counted from the files rather than passed in from the caller's own loop, so
/// the documented figure and the checked figure come from one reading of the
/// same directory.
fn grant_count(workspace_root: &Path) -> usize {
    let dir = workspace_root.join("services/agentd/agents");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .map(|src| parse_grants(&src).len())
        .sum()
}

fn inventory_matches_the_docs(workspace_root: &Path, tools: &BTreeMap<String, bool>) -> bool {
    let services: BTreeSet<&str> = tools.keys().filter_map(|id| id.split('/').next()).collect();
    let total = tools.len();
    let serving = services.len();
    let all_services = std::fs::read_dir(workspace_root.join("services"))
        .map(|e| e.flatten().filter(|e| e.path().is_dir()).count())
        .unwrap_or(0);
    let agents = specialist_count(workspace_root);
    let grants = grant_count(workspace_root);
    let (prompts, prompt_servers) = prompt_count(workspace_root);

    // One phrase per place, spelled as it is written there, so a mismatch names
    // the file and the sentence rather than a number to hunt for. The list is
    // long because the number genuinely appears in that many places, and that is
    // the argument for the check rather than against it.
    //
    // **Totals only.** `26 advisory specialists`, `14 specialists` with triage
    // rules and `21 specialists` are real numbers about parts of the set; they
    // move for different reasons and are not covered here.
    let claims: Vec<(&str, String)> = vec![
        // ── How many MCP tools the platform exposes ──
        (
            "README.md",
            format!(
                "{serving} of the {all_services} services expose an MCP server — **{total} tools**"
            ),
        ),
        (
            "site/templates/index.html",
            format!("agent plane over the {total} MCP tools the platform exposes"),
        ),
        (
            "concepts/MARKET_LANDSCAPE.md",
            format!(
                "over the {total} MCP tools the other services expose ({serving} of {all_services})"
            ),
        ),
        // ── How many tools the specialists are actually granted ──
        //
        // The number that says whether least privilege is real. It went from
        // 904 (every specialist holding the whole read surface of every server
        // it touched) to a set each procedure names, and a figure that drifts
        // back up unnoticed is how the old shape returns.
        (
            "services/agentd/README.md",
            format!("{grants} grants across {agents} manifests"),
        ),
        (
            "services/agentd/policy/agentd.cedar",
            format!("// {grants} granted tools one by one."),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("{grants} grants across {agents} manifests"),
        ),
        (
            "concepts/AGENTD.md",
            format!("{grants} grants across {agents} manifests"),
        ),
        // ── How many step-by-step prompts a specialist may be granted ──
        (
            "services/agentd/README.md",
            format!("publish {prompts} step-by-step prompts across {prompt_servers} servers"),
        ),
        (
            "services/agentd/agents/mako-agent.yaml",
            format!("publish {prompts} step-by-step prompts across {prompt_servers} servers"),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("publish {prompts} step-by-step prompts across {prompt_servers} servers"),
        ),
        // ── How many specialists agentd ships ──
        //
        // `agentd`'s own suite asserts this count too, which fails when a
        // manifest is added — but that only forces somebody to edit the assert.
        // Nothing made them edit the twelve sentences that also state it.
        (
            "README.md",
            format!("governed consumer: {agents} declarative specialists"),
        ),
        ("README.md", format!("agentd<br/>{agents} LLM specialists")),
        (
            "README.md",
            format!("**{agents} declarative specialist manifests**"),
        ),
        ("README.md", format!("agent plane — {agents} specialists")),
        (
            "site/templates/index.html",
            format!("exposes — {agents} declarative specialists"),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("operator guide: {agents} specialist manifests"),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("All {agents} specialists declare"),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("MANIFEST[\"{agents} manifests"),
        ),
        (
            "site/content/docs/services/agentd.md",
            format!("one of the {agents} manifests"),
        ),
        (
            "services/agentd/README.md",
            format!("| {agents} manifests in"),
        ),
        (
            "services/agentd/README.md",
            format!("All {agents} specialists declare"),
        ),
        (
            ".github/copilot-instructions.md",
            format!("**{agents} declarative manifests**"),
        ),
    ];

    let mut stale = Vec::new();
    for (file, expected) in &claims {
        let path = workspace_root.join(file);
        match std::fs::read_to_string(&path) {
            // `concepts/` is not in git, so a checkout without it is normal and
            // must not fail the build — only a file that *exists* and disagrees
            // is a finding.
            Err(_) => continue,
            Ok(src) if src.contains(expected.as_str()) => {}
            Ok(_) => stale.push(format!("{file}: expected to contain \"{expected}\"")),
        }
    }

    if stale.is_empty() {
        println!(
            "check-tool-grants: {total} MCP tools across {serving} of {all_services} services, \
             {agents} specialists holding {grants} grants, {prompts} MCP prompts across \
             {prompt_servers} servers, as documented"
        );
        return true;
    }
    eprintln!(
        "check-tool-grants: the documented size of the agent surface no longer matches the \
         source. {total} tools across {serving} of {all_services} services, and {agents} \
         specialists, are declared.\n  {}",
        stale.join("\n  ")
    );
    false
}

/// How many specialists this workspace ships, counted from the manifests.
///
/// The directory rather than the embedded `manifests![]` list, because an
/// `agentd` test already pins those two to each other — so counting either one
/// here answers the same question, and the directory needs no Rust to read.
/// Role-scoped builds compile fewer, but the manifests on disk are not
/// role-gated and the docs describe the default build.
fn specialist_count(workspace_root: &Path) -> usize {
    std::fs::read_dir(workspace_root.join("services/agentd/agents"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "yaml"))
                .count()
        })
        .unwrap_or(0)
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
