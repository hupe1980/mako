//! Guard: a specialist's procedure and its tool grants are the **same set**.
//!
//! `check-tool-grants` validates the grant list — that every `tool://server/name`
//! a manifest declares exists and agrees with the server's own `read_only_hint`.
//! This validates the tie between that list and the *procedure*, in both
//! directions, because each fails differently and neither fails loudly.
//!
//! **A call the agent cannot make.** A procedure step naming an ungranted tool
//! does not crash: agentplane reports an unknown tool name back to the model as a
//! failed call rather than ending the run — deliberately, so the model can
//! correct itself — so the model asks, is refused, improvises, and the step
//! silently does not happen. Some names are not MCP tools at all:
//! `get_malo_grid`, `get_open_items`, `get_bnetza_report` and
//! `get_process_status` are HTTP endpoints on their services, and
//! `check_anmeldung` is an internal STP check.
//!
//! **A grant nobody instructs.** The quieter direction, and what keeps a
//! specialist from holding a server's whole read surface: unreviewed reach no
//! test can see, a model asked to choose between seventeen marktd tools when its
//! procedure needs two, and a § 6a EnWG data boundary drawn wider than anything
//! asked for. Dropping the grant is usually right; where it is not, the procedure
//! has to say when the model should reach for it.
//!
//! ## What counts as an instruction
//!
//! A **sentence** containing an instruction verb — call, use, query, fetch,
//! retrieve, invoke, consult, poll, check — outside its backticks, and no
//! negation. Both halves are load-bearing:
//!
//! * the verb is looked for outside the backticks, or `check_findings` (a field
//!   on a netzbilanzd response) supplies its own instruction — and "read" is not
//!   a verb here for the same reason: a prompt reads a field far more often than
//!   it reads a service;
//! * a negation excuses the sentence, because "Do not call `calculate_billing`:
//!   it mutates" documents a boundary rather than crossing it.
//!
//! Sentence scope rather than a fixed window before the backtick is what catches
//! ``Call accountingd `get_balance` … and `list_ledger` ``, where the ungranted
//! tool sits eleven words past where a window would stop looking.

use std::path::{Path, PathBuf};

/// Scan the specialist manifests for a procedure and a grant list that disagree.
///
/// Returns `true` when every tool a procedure tells the model to call is granted
/// to that agent, and every tool granted to it is named in the procedure.
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

    let mut unreachable = Vec::new();
    let mut uninstructed = Vec::new();
    let mut checked = 0usize;
    let mut grants = 0usize;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        checked += 1;
        grants += granted_tools(&src).len();
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for tool in unreachable_calls(&src) {
            unreachable.push(format!("{name}  `{tool}`"));
        }
        for tool in uninstructed_grants(&src) {
            uninstructed.push(format!("{name}  `{tool}`"));
        }
    }

    if unreachable.is_empty() && uninstructed.is_empty() {
        println!(
            "check-prompt-tools: {grants} grants across {checked} specialist procedures — every \
             tool named as a call is granted, and every grant is named"
        );
        return true;
    }

    if !unreachable.is_empty() {
        eprintln!(
            "check-prompt-tools: {} procedure step(s) tell a model to call a tool the manifest \
             does not grant:",
            unreachable.len()
        );
        for line in &unreachable {
            eprintln!("  {line}");
        }
        eprintln!(
            "\nagentplane reports an unknown tool name back to the model as a failed call rather\n\
             than ending the run, so this does not crash — the step silently does not happen.\n\
             Either grant the tool, or name one the agent actually has."
        );
    }
    if !uninstructed.is_empty() {
        eprintln!(
            "\ncheck-prompt-tools: {} grant(s) that no procedure step mentions:",
            uninstructed.len()
        );
        for line in &uninstructed {
            eprintln!("  {line}");
        }
        eprintln!(
            "\nA grant the procedure never names is reach nobody reviewed: it widens the § 6a\n\
             EnWG data boundary, and it puts one more tool in front of a model that was never\n\
             told when to reach for it. Drop the grant, or say in the procedure when it is used."
        );
    }
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

    let mut out: Vec<String> = Vec::new();
    for sentence in sentences(prompt) {
        if !instructs(&sentence) {
            continue;
        }
        for name in backticked(&sentence) {
            if is_tool_name(&name) && !granted.contains(&name) && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// Granted tools the procedure never mentions.
///
/// A plain mention is enough here — "Do not call `trigger_substitution`" is a
/// procedure that has considered the tool, which is what this direction is
/// asking about.
#[must_use]
pub fn uninstructed_grants(src: &str) -> Vec<String> {
    let Some(prompt) = constraints(src) else {
        return Vec::new();
    };
    let mentioned: Vec<String> = backticked(prompt);
    granted_tools(src)
        .into_iter()
        .filter(|tool| !mentioned.iter().any(|m| m == tool))
        .collect()
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

/// Every backticked token in a stretch of prompt.
fn backticked(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        rest = &rest[open + 1..];
        let Some(close) = rest.find('`') else { break };
        out.push(rest[..close].to_owned());
        rest = &rest[close + 1..];
    }
    out
}

/// Split a procedure into sentences.
///
/// A markdown line break ends a sentence as surely as a full stop does — a
/// numbered step is one instruction whether or not its author punctuated it —
/// and a sentence may still run across a wrapped line, so a bare newline is a
/// space and a blank line is a break. `. ` is the other break.
fn sentences(prompt: &str) -> Vec<String> {
    let mut out = Vec::new();
    for block in prompt.split("\n\n") {
        // A numbered or bulleted line starts a new instruction; anything else is
        // a continuation of the one above it.
        let mut current = String::new();
        for line in block.lines() {
            let trimmed = line.trim_start();
            let starts_item = trimmed.split_once(['.', ')']).is_some_and(|(head, _)| {
                !head.is_empty() && head.bytes().all(|b| b.is_ascii_digit())
            }) || trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with('#')
                || trimmed.starts_with('|');
            if starts_item && !current.trim().is_empty() {
                out.push(std::mem::take(&mut current));
            }
            current.push(' ');
            current.push_str(trimmed);
        }
        if !current.trim().is_empty() {
            out.push(current);
        }
    }
    // Then split each on `. `, which separates two instructions on one line.
    out.iter()
        .flat_map(|s| s.split(". ").map(str::to_owned))
        .collect()
}

/// Whether this sentence tells the model to make a call.
///
/// The verb is looked for **outside** the backticks: a tool named
/// `check_findings` would otherwise supply its own instruction verb.
fn instructs(sentence: &str) -> bool {
    let mut outside = String::with_capacity(sentence.len());
    let mut in_ticks = false;
    for c in sentence.chars() {
        if c == '`' {
            in_ticks = !in_ticks;
        } else if !in_ticks {
            outside.push(c.to_ascii_lowercase());
        }
    }

    /// Verbs that make a sentence an instruction to reach a tool.
    ///
    /// Deliberately without "read": a prompt reads a *field* far more often than
    /// it reads a service, and `netzbilanz-agent`'s "Read `check_findings`
    /// FIRST" — a field on the draft — would otherwise be an instruction to
    /// call a tool nobody has.
    const VERBS: &[&str] = &[
        "call ",
        "use ",
        "query ",
        "fetch ",
        "retrieve ",
        "invoke ",
        "consult ",
        "poll ",
        "check ",
    ];
    /// A sentence that says *not* to is documenting the boundary, not crossing it.
    const NEGATIONS: &[&str] = &[
        "do not ",
        "don't ",
        "never ",
        "cannot ",
        "can not ",
        "there is no ",
        "is not a tool",
        "not an mcp tool",
        "would fail",
        "rather than ",
        "not a grant",
    ];

    VERBS.iter().any(|v| outside.contains(v)) && !NEGATIONS.iter().any(|n| outside.contains(n))
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
        "find_",
        "lookup_",
        "approve_",
        "reject_",
        "summarize_",
    ];
    token.len() > 4
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && VERBS.iter().any(|v| token.starts_with(v))
}

#[cfg(test)]
mod tests {
    use super::{uninstructed_grants, unreachable_calls};

    const GRANTS: &str = "\n  tools:\n    - ref: tool://obsd/get_process\n      mutates: false\n";

    fn manifest(procedure: &str) -> String {
        format!("spec:\n  identity:\n    constraints: |\n{procedure}\n  capabilities:\n{GRANTS}")
    }

    #[test]
    fn it_catches_a_call_to_an_ungranted_tool() {
        let src = manifest("      1. Call makod `get_process_status` now, then `get_process`.");
        assert_eq!(unreachable_calls(&src), vec!["get_process_status"]);
    }

    #[test]
    fn a_granted_tool_is_not_a_finding() {
        let src = manifest("      1. Call obsd `get_process` now.");
        assert!(unreachable_calls(&src).is_empty());
    }

    /// **The shape that shipped.** A second tool joined onto one instruction, so
    /// the verb is nowhere near the backtick that matters.
    ///
    /// `check-prompt-tools` looked at a fixed window before each backtick, and
    /// `accountingd/list_ledger` — instructed by
    /// `invoice-reconciliation-agent`, granted to a different specialist — was
    /// eleven words past the end of it. The model asked, was refused, and the
    /// reconciliation ran without the ledger behind the balance.
    #[test]
    fn a_conjoined_second_tool_is_still_a_call() {
        let src = manifest(
            "      2. Call obsd `get_process` for the outstanding balance, and `list_ledger`\n\
             \x20        for the entries behind it.",
        );
        assert_eq!(unreachable_calls(&src), vec!["list_ledger"]);
    }

    /// "check X" is an instruction, whatever the author's verb of choice.
    #[test]
    fn check_is_an_instruction_verb() {
        let src = manifest("      5. For NB rejections: check marktd `get_malo_grid`.");
        assert_eq!(unreachable_calls(&src), vec!["get_malo_grid"]);
    }

    /// A prompt explaining a consequence is documentation, not an instruction.
    #[test]
    fn merely_naming_a_tool_is_not_calling_it() {
        let src = manifest(
            "      5. Without a valid NB contract: NB STP processd `check_anmeldung` would fail check 5.",
        );
        assert!(
            unreachable_calls(&src).is_empty(),
            "explaining what another service would do is not an instruction to call it"
        );
    }

    /// A manifest that documents a boundary names the tool it will not call.
    #[test]
    fn a_refusal_to_call_is_not_a_call() {
        let src = manifest(
            "      1. Prepare the parameters and report them. Do not call `calculate_billing`: \
             it mutates.",
        );
        assert!(unreachable_calls(&src).is_empty());
    }

    /// A backticked field is not a tool — even one whose name starts with a verb.
    #[test]
    fn fields_are_not_tools() {
        let src =
            manifest("      2. Read `check_findings` and `risk_band` and `deadline_at` first.");
        assert!(
            unreachable_calls(&src).is_empty(),
            "a response field must not supply its own instruction verb"
        );
    }

    /// **Direction 2.** A grant the procedure never mentions is unreviewed reach.
    #[test]
    fn a_grant_no_step_mentions_is_a_finding() {
        let src = manifest("      1. Extract `malo_id` from the payload.");
        assert_eq!(uninstructed_grants(&src), vec!["get_process"]);
    }

    /// Mentioning it is enough for direction 2 — including to rule it out.
    #[test]
    fn a_mentioned_grant_is_not_a_finding() {
        let src = manifest("      1. Call obsd `get_process` for the timeline.");
        assert!(uninstructed_grants(&src).is_empty());
    }
}
