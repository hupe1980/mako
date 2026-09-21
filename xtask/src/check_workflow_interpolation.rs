//! Guard: a workflow `run:` block reads untrusted values from the environment.
//!
//! GitHub substitutes `${{ … }}` into a `run:` script as **text, before bash
//! parses it**. A value holding a `'` therefore ends the quoted string it sits
//! in and the remainder of the blob is parsed as shell — which is a syntax
//! error on a good day and command substitution on a bad one, because the same
//! blob may hold `$(…)` or backticks.
//!
//! The release workflow did this with `steps.bake.outputs.metadata`. Turning on
//! `provenance: mode=max` and `sbom: true` grew that value from a small digest
//! map into megabytes of SLSA material, base64 source maps and build arguments
//! for seventeen targets, and the release died with
//! `syntax error near unexpected token '('` inside the blob. The same shape was
//! live twice more in the manifest job, on `steps.meta.outputs.json` — which
//! carries the OCI descriptions, free prose that already holds parentheses.
//!
//! The remedy is one line: name the value under `env:` and read `"$VAR"`. The
//! environment is not parsed, so no content can break out of it.
//!
//! ## What is allowed inline
//!
//! Values whose grammar excludes a quote: `matrix.*`, a handful of `github.*`
//! fields (repository, owner, sha, ref name, actor, workspace, run id/number,
//! event name), `env.*`, `runner.*`, `needs.*`, and a step output named
//! `version` or `digest`. Anything else — any step output carrying JSON, any
//! `github.event.*` field a user can write — must come through `env:`.
//!
//! `github.event.head_commit.message` is the textbook case and is not on the
//! list: a commit message is attacker-controlled on a fork PR.

use std::path::Path;

/// Expression prefixes whose value cannot contain a quote.
const INLINE_SAFE: &[&str] = &[
    "matrix.",
    "github.repository",
    "github.repository_owner",
    "github.sha",
    "github.ref_name",
    "github.actor",
    "github.workspace",
    "github.run_id",
    "github.run_number",
    "github.event_name",
    "env.",
    "runner.",
    "needs.",
    "secrets.",
];

/// Step outputs that are a bare identifier by construction.
const SAFE_OUTPUT_SUFFIX: &[&str] = &[".outputs.version", ".outputs.digest"];

/// The lowest number of `run:` blocks a healthy scan may find.
const MIN_RUN_BLOCKS: usize = 40;

/// Run the check, returning `true` when no risky value is interpolated inline.
pub fn run(workspace_root: &Path) -> bool {
    let dir = workspace_root.join(".github/workflows");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        eprintln!(
            "check-workflow-interpolation: no workflows at {}",
            dir.display()
        );
        return false;
    };
    let mut files: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yml" || e == "yaml"))
        .collect();
    files.sort();

    let mut blocks = 0usize;
    let mut findings: Vec<String> = Vec::new();

    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let mut in_run = false;
        let mut indent = 0usize;
        for (idx, line) in src.lines().enumerate() {
            let lead = line.len() - line.trim_start().len();
            if line.trim_start().starts_with("run: |") {
                in_run = true;
                indent = lead;
                blocks += 1;
                continue;
            }
            if in_run && !line.trim().is_empty() && lead <= indent {
                in_run = false;
            }
            if !in_run || line.trim_start().starts_with('#') {
                continue;
            }
            for expr in interpolations(line) {
                if INLINE_SAFE.iter().any(|p| expr.starts_with(p))
                    || SAFE_OUTPUT_SUFFIX.iter().any(|s| expr.contains(s))
                {
                    continue;
                }
                findings.push(format!("{rel}:{}  ${{{{ {expr} }}}}", idx + 1));
            }
        }
    }

    if blocks < MIN_RUN_BLOCKS {
        eprintln!(
            "check-workflow-interpolation: found only {blocks} `run:` block(s) in {} file(s) — \
             at least {MIN_RUN_BLOCKS} are expected. Fix the scanner before trusting a green run.",
            files.len()
        );
        return false;
    }

    if findings.is_empty() {
        println!(
            "check-workflow-interpolation: no untrusted value is interpolated into a `run:` \
             script ({blocks} block(s) scanned)"
        );
        return true;
    }

    eprintln!(
        "check-workflow-interpolation: {} value(s) interpolated into a `run:` script:",
        findings.len()
    );
    for f in &findings {
        eprintln!("  {f}");
    }
    eprintln!(
        "\nGitHub substitutes these as text before bash parses the line, so one `'` in the \
         value ends the quoted string and the rest is executed. Pass it under `env:` and read \
         \"$VAR\" — the environment is never parsed."
    );
    false
}

/// Every `${{ … }}` expression in one line, trimmed.
fn interpolations(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find("${{") {
        let after = &rest[start + 3..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim().to_owned());
        rest = &after[end + 2..];
    }
    out
}
