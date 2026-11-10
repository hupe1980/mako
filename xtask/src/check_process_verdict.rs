//! Guard: a consumer of `de.mako.process.completed` reads the verdict.
//!
//! One CloudEvent type carries both answers a counterparty can give: a
//! Bestätigung and an Ablehnung of the same request (BK6-24-174 GPKE Teil 2:
//! 55002/55078 confirm an Anmeldung, 55003/55080 refuse it). The type says the
//! process ended; `data.accepted` / `data.outcome` say how, and the response
//! PID (`makopid`) names which answer arrived. A consumer that branches on the
//! type alone reads every refused Anmeldung as a supply that started.
//!
//! **What counts.** Two places a consumer decides:
//!
//! - **Rust**, in every `crates/*/src/**/*.rs` and `services/*/src/**/*.rs`
//!   with `#[cfg(test)]` items and comment lines blanked: a comparison
//!   (`==`, `!=`) with, or a `match` arm (`=>`) on,
//!   `mako_events::mako::PROCESS_COMPLETED` or the literal
//!   `"de.mako.process.completed"`. The enclosing function must consult the
//!   verdict or the PID — name `accepted`, `outcome` or a `…verdict…`
//!   identifier, or an identifier containing `pid`.
//! - **Agent manifests**, `services/agentd/agents/*.yaml`: a trigger line
//!   naming `de.mako.process.completed` must tell the model about
//!   `data.accepted` or `data.outcome` — the model reads the event's `data`,
//!   and the line is what tells it the verdict lives there.
//!
//! The pattern is narrower than the rule. A subscription list naming the type
//! is not a decision and is not scanned, and a function that reads a PID for
//! some other purpose passes: the check proves the verdict is within reach of
//! the decision, not that the decision uses it.

use std::path::{Path, PathBuf};

const CONST_PATH: &str = "mako_events::mako::PROCESS_COMPLETED";
const LITERAL: &str = "\"de.mako.process.completed\"";
const TYPE: &str = "de.mako.process.completed";

/// The lowest number of `.rs` files a healthy walk reads.
const MIN_FILES: usize = 300;

/// One-based lines in `src` where a decision on the completed type sits in a
/// function that consults neither the verdict nor the PID.
pub fn blind_decisions(src: &str) -> Vec<usize> {
    let text = crate::check_error_tuples::production_text(src);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !is_decision(line) {
            continue;
        }
        let (start, end) = enclosing_fn(&lines, i);
        if !lines[start..end].iter().any(|l| consults_verdict(l)) {
            out.push(i + 1);
        }
    }
    out
}

/// Whether `line` compares with, or matches on, the completed type.
fn is_decision(line: &str) -> bool {
    [CONST_PATH, LITERAL].iter().any(|needle| {
        line.match_indices(needle).any(|(at, _)| {
            let before = line[..at].trim_end();
            let after = line[at + needle.len()..].trim_start();
            before.ends_with("==")
                || before.ends_with("!=")
                || after.starts_with("=>")
                || after.starts_with("==")
                || after.starts_with("!=")
                || after.starts_with('|')
        })
    })
}

/// Whether a token on `line` reads the verdict or a PID.
fn consults_verdict(line: &str) -> bool {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|t| t == "accepted" || t == "outcome" || t.contains("verdict") || t.contains("pid"))
}

/// The half-open line range of the function around line `i`: from the nearest
/// `fn` header above to the next `fn` header below, or the file's end.
fn enclosing_fn(lines: &[&str], i: usize) -> (usize, usize) {
    let start = (0..=i).rev().find(|&j| is_fn_header(lines[j])).unwrap_or(0);
    let end = (i + 1..lines.len())
        .find(|&j| is_fn_header(lines[j]))
        .unwrap_or(lines.len());
    (start, end)
}

fn is_fn_header(line: &str) -> bool {
    let mut t = line.trim_start();
    for prefix in [
        "pub(crate) ",
        "pub(super) ",
        "pub ",
        "const ",
        "async ",
        "unsafe ",
    ] {
        t = t.strip_prefix(prefix).unwrap_or(t);
    }
    // Twice: `pub const async fn` and `pub async fn` both reduce to `fn`.
    for prefix in ["const ", "async "] {
        t = t.strip_prefix(prefix).unwrap_or(t);
    }
    t.starts_with("fn ")
}

/// One-based lines of a manifest trigger naming the completed type without
/// pointing at its verdict.
pub fn blind_triggers(yaml: &str) -> Vec<usize> {
    yaml.lines()
        .enumerate()
        .filter(|(_, l)| {
            l.trim_start().starts_with("- ")
                && l.contains(&format!("`{TYPE}`"))
                && !(l.contains("data.accepted") || l.contains("data.outcome"))
        })
        .map(|(i, _)| i + 1)
        .collect()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Run the guard over the workspace at `root`.
pub fn run(root: &Path) -> bool {
    let mut files = Vec::new();
    for top in ["crates", "services"] {
        let Ok(members) = std::fs::read_dir(root.join(top)) else {
            continue;
        };
        for m in members.flatten() {
            walk(&m.path().join("src"), &mut files);
        }
    }
    if files.len() < MIN_FILES {
        eprintln!(
            "check-process-verdict: read {} files under crates/*/src and services/*/src, at \
             least {MIN_FILES} expected — the walk has stopped matching",
            files.len()
        );
        return false;
    }
    let mut findings = Vec::new();
    for f in &files {
        let Ok(src) = std::fs::read_to_string(f) else {
            continue;
        };
        for line in blind_decisions(&src) {
            findings.push(format!(
                "{}:{line}: decides on `{TYPE}` in a function that reads neither \
                 `data.accepted`/`data.outcome` nor the response PID — an Ablehnung reads \
                 as a completion",
                f.strip_prefix(root).unwrap_or(f).display()
            ));
        }
    }
    let agents = root.join("services/agentd/agents");
    let mut manifests = 0;
    if let Ok(entries) = std::fs::read_dir(&agents) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "yaml") {
                continue;
            }
            manifests += 1;
            let Ok(yaml) = std::fs::read_to_string(&p) else {
                continue;
            };
            for line in blind_triggers(&yaml) {
                findings.push(format!(
                    "{}:{line}: an agent triggered by `{TYPE}` is not told that \
                     `data.accepted`/`data.outcome` carry the verdict",
                    p.strip_prefix(root).unwrap_or(&p).display()
                ));
            }
        }
    }
    if manifests == 0 {
        eprintln!(
            "check-process-verdict: no agent manifest under {}",
            agents.display()
        );
        return false;
    }
    if findings.is_empty() {
        println!(
            "check-process-verdict: every decision on `{TYPE}` reads the verdict ({} files, \
             {manifests} agent manifests)",
            files.len()
        );
        return true;
    }
    for f in &findings {
        eprintln!("check-process-verdict: {f}");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The read model's mapping as it stood: the type alone decides.
    #[test]
    fn a_mapping_on_the_type_alone_is_refused() {
        let src = "impl S {\n    pub fn from_ce_type(ce_type: &str) -> Option<Self> {\n        \
                   match ce_type {\n            mako_events::mako::APERAK_ACCEPTED => Some(Self::Running),\n            \
                   mako_events::mako::PROCESS_COMPLETED => Some(Self::Completed),\n            \
                   _ => None,\n        }\n    }\n}\n";
        assert_eq!(blind_decisions(src), vec![5]);
    }

    #[test]
    fn a_mapping_that_reads_the_verdict_passes() {
        let src = "pub fn from_event(ce_type: &str, data: &Value) -> Option<Self> {\n    \
                   match ce_type {\n        mako_events::mako::PROCESS_COMPLETED => Some(if verdict_is_negative(data) { R } else { C }),\n        \
                   _ => None,\n    }\n}\n";
        assert!(blind_decisions(src).is_empty());
    }

    /// marktd's shape: the response PID decides.
    #[test]
    fn a_decision_on_the_response_pid_passes() {
        let src = "pub async fn derive(ce_type: &str, pid: Option<u32>) {\n    \
                   let is_completed = ce_type == mako_events::mako::PROCESS_COMPLETED;\n    \
                   if is_completed && ABGELEHNT.contains(&pid) {}\n}\n";
        assert!(blind_decisions(src).is_empty());
        let literal = "fn h(t: &str, data: &Value) -> bool {\n    t == \"de.mako.process.completed\" \
                       && data[\"accepted\"] == true\n}\n";
        assert!(blind_decisions(literal).is_empty());
    }

    /// A subscription list names the type and decides nothing.
    #[test]
    fn a_subscription_list_is_not_a_decision() {
        let src = "fn types() -> Vec<&'static str> {\n    vec![\n        \
                   mako_events::mako::PROCESS_COMPLETED,\n        mako_events::mako::PROCESS_FAILED,\n    ]\n}\n";
        assert!(blind_decisions(src).is_empty());
    }

    /// The scan does not reach a test item, and does reach code after one.
    #[test]
    fn a_test_item_is_skipped_and_code_after_it_is_not() {
        let src = "#[cfg(test)]\nmod tests {\n    fn t(x: &str) -> bool { x == mako_events::mako::PROCESS_COMPLETED }\n}\n\
                   fn later(x: &str) -> bool {\n    x == \"de.mako.process.completed\"\n}\n";
        assert_eq!(blind_decisions(src), vec![6]);
    }

    #[test]
    fn an_agent_trigger_must_point_at_the_verdict() {
        let blind = "      ## TRIGGERED BY\n      - `de.mako.process.completed` — WiM MSB-change completed\n";
        assert_eq!(blind_triggers(blind), vec![2]);
        let told = "      - `de.mako.process.completed` — closed; `data.accepted: false` is an Ablehnung\n";
        assert!(blind_triggers(told).is_empty());
        let other = "      - `de.mako.process.failed` — failed\n";
        assert!(blind_triggers(other).is_empty());
    }
}
