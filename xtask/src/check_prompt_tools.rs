//! Guard: a procedure may not tell a model to call a tool it cannot reach.
//!
//! `check-tool-grants` validates the **grant list** — that every
//! `tool://server/name` a manifest declares exists and agrees with the server's
//! own `read_only_hint`. Nothing validated the other half: the *prompt*, where
//! the procedure actually names the calls the model should make.
//!
//! Eleven of mako's twenty-six prompted specialists instructed a model to call
//! something ungranted, twelve instructions in total. Five of the names were not
//! MCP tools **at all** — `get_malo_grid`, `get_open_items`, `get_bnetza_report`
//! and `get_process_status` are HTTP endpoints on their services, and
//! `check_anmeldung` is an internal STP check. Two were real tools the manifest
//! simply had not granted.
//!
//! The failure is quiet by construction. agentplane reports an unknown tool name
//! back to the model as a failed call rather than ending the run — deliberately,
//! so the model can correct itself and never gets the tool it nearly named. So a
//! procedure step naming a nonexistent tool does not crash: the model asks, is
//! refused, improvises, and burns turns. The step silently does not happen, and
//! nobody notices, because the answer used to be prose.
//!
//! ## What it looks for
//!
//! An **instruction to call** — `Call …`/`Use …` followed by a backticked
//! `snake_case` identifier — whose name is not in that manifest's `tools:` list.
//! A backticked name that is merely *mentioned* is left alone: prompts
//! legitimately explain consequences ("without a valid contract, processd's
//! `check_anmeldung` would fail check 5"), and that is documentation, not a
//! call.

use std::path::{Path, PathBuf};

/// Scan the specialist manifests for unreachable call instructions.
///
/// Returns `true` when every tool a procedure tells the model to call is
/// granted to that agent.
pub fn run(workspace_root: &Path) -> bool {
    let dir = workspace_root.join("services/agentd/agents");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!(
            "check-prompt-tools: no agent manifests at {}",
            dir.display()
        );
        return false;
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
        .collect();
    files.sort();

    let mut findings = Vec::new();
    let mut checked = 0usize;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        for name in unreachable_calls(&src) {
            findings.push((path.clone(), name));
        }
    }

    if findings.is_empty() {
        println!(
            "check-prompt-tools: every tool named as a call in {checked} specialist procedures \
             is granted to that specialist"
        );
        return true;
    }

    eprintln!(
        "check-prompt-tools: {} procedure step(s) tell a model to call a tool the manifest does \
         not grant:",
        findings.len()
    );
    for (path, name) in &findings {
        eprintln!(
            "  {}  `{name}`",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    eprintln!(
        "\nagentplane reports an unknown tool name back to the model as a failed call rather \n\
         than ending the run, so this does not crash — the step silently does not happen.\n\
         Either grant the tool, or name one the agent actually has."
    );
    false
}

/// Tool names a manifest's procedure tells the model to call but does not grant.
///
/// Split from the filesystem so the rule is testable against manifest text.
#[must_use]
pub fn unreachable_calls(src: &str) -> Vec<String> {
    let granted = granted_tools(src);
    let Some(prompt) = constraints(src) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (index, _) in prompt.match_indices('`') {
        // The identifier between this backtick and the next.
        let rest = &prompt[index + 1..];
        let Some(end) = rest.find('`') else { break };
        let name = &rest[..end];
        if !is_tool_name(name) || granted.iter().any(|g| g == name) {
            continue;
        }
        // Only an instruction to call it. The 40-character window is what
        // separates "Call obsd `get_process`" from a sentence that merely
        // mentions the name two clauses later.
        let window_start = index.saturating_sub(48);
        let window = &prompt[window_start..index];
        let instructs = window.rsplit(['\n', '.']).next().is_some_and(|clause| {
            let lower = clause.to_ascii_lowercase();
            lower.contains("call ") || lower.contains("use ")
        });
        if instructs && !out.iter().any(|n| n == name) {
            out.push(name.to_owned());
        }
    }
    out
}

/// The `tools:` grant list, by bare tool name.
fn granted_tools(src: &str) -> Vec<String> {
    src.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("- ref: tool://")?;
            rest.split('/').nth(1).map(str::to_owned)
        })
        .collect()
}

/// The `constraints: |` block — the procedure the model is given.
fn constraints(src: &str) -> Option<&str> {
    let start = src.find("constraints: |")? + "constraints: |".len();
    let rest = &src[start..];
    // The block ends at the next key indented two spaces under `spec:`.
    let end = rest
        .match_indices('\n')
        .find(|(i, _)| {
            let line = rest[i + 1..].lines().next().unwrap_or("");
            line.starts_with("  ") && !line.starts_with("   ") && line.trim_end().ends_with(':')
        })
        .map_or(rest.len(), |(i, _)| i);
    Some(&rest[..end])
}

/// Whether a backticked token looks like a tool name rather than a field.
///
/// Tool names in mako's MCP servers are verb-led `snake_case`. Restricting to
/// that prefix set is what keeps a field like `risk_band` or a column like
/// `deadline_at` out of the findings.
fn is_tool_name(token: &str) -> bool {
    const VERBS: &[&str] = &[
        "get_",
        "list_",
        "check_",
        "create_",
        "update_",
        "run_",
        "submit_",
        "trigger_",
        "compute_",
        "suggest_",
        "preview_",
        "resolve_",
        "import_",
        "post_",
        "put_",
        "delete_",
        "search_",
        "explain_",
        "validate_",
        "assign_",
        "reverse_",
        "calculate_",
    ];
    token.len() > 4
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && VERBS.iter().any(|v| token.starts_with(v))
}

#[cfg(test)]
mod tests {
    use super::unreachable_calls;

    const GRANTS: &str = "\n  tools:\n    - ref: tool://obsd/get_process\n      mutates: false\n";

    #[test]
    fn it_catches_a_call_to_an_ungranted_tool() {
        let src = format!(
            "spec:\n  identity:\n    constraints: |\n      1. Call makod `get_process_status` now.\n  capabilities:\n{GRANTS}"
        );
        assert_eq!(unreachable_calls(&src), vec!["get_process_status"]);
    }

    #[test]
    fn a_granted_tool_is_not_a_finding() {
        let src = format!(
            "spec:\n  identity:\n    constraints: |\n      1. Call obsd `get_process` now.\n  capabilities:\n{GRANTS}"
        );
        assert!(unreachable_calls(&src).is_empty());
    }

    /// A prompt explaining a consequence is documentation, not an instruction.
    ///
    /// This exact sentence is in `grid-anomaly-agent.yaml`, and flagging it
    /// would have meant "fixing" correct prose.
    #[test]
    fn merely_naming_a_tool_is_not_calling_it() {
        let src = format!(
            "spec:\n  identity:\n    constraints: |\n      5. Without a valid NB contract: NB STP processd `check_anmeldung` would fail check 5.\n  capabilities:\n{GRANTS}"
        );
        assert!(
            unreachable_calls(&src).is_empty(),
            "explaining what another service would do is not an instruction to call it"
        );
    }

    /// A backticked field is not a tool.
    #[test]
    fn fields_are_not_tools() {
        let src = format!(
            "spec:\n  identity:\n    constraints: |\n      2. Use the `risk_band` and `deadline_at` values.\n  capabilities:\n{GRANTS}"
        );
        assert!(unreachable_calls(&src).is_empty());
    }
}
