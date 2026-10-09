//! Guard: the ingest arms that decide resume-vs-spawn on index presence alone
//! only become fewer.
//!
//! The correlation index is append-only: an entry proves a process once
//! existed, never that one still runs. An ingest arm that resumes on the entry
//! alone feeds a second initiating message into a settled process, whose
//! initiating command refuses anything outside its initial state — the message
//! is dead-lettered and no process can spawn on that key again. An arm that
//! passes the occupancy verdict its workflow's crate publishes
//! (`spawn_or_resume_guarded`, or `spawn_or_resume_keyed` with `Some(..)`)
//! spawns past a settled process and resumes a live one.
//!
//! This guard counts the arms that pass no verdict — `spawn_or_resume::<…>`
//! and `spawn_or_resume_keyed::<…>(…, None)` — in
//! `services/makod/src/orchestrator/ingest_dispatcher/` and holds the count at
//! [`PRESENCE_ARMS`]: a new arm without a verdict fails, and an arm given one
//! fails until the constant is lowered with it, so the figure never overstates
//! what remains.

use std::path::Path;

use crate::check_workflow_purity::{
    blank_cfg_test, code_mask, line_of, match_brace, next_code_byte, walk_rs,
};

/// Arms deciding on presence alone, today.
const PRESENCE_ARMS: usize = 28;

/// The tree scanned.
const DISPATCHER: &str = "services/makod/src/orchestrator/ingest_dispatcher";

/// Functions that *implement* spawn-or-resume rather than being an arm of it.
const IMPLEMENTATIONS: &[&str] = &["spawn_or_resume"];

/// Run the check. Returns `true` when the count is exactly [`PRESENCE_ARMS`].
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    walk_rs(&workspace_root.join(DISPATCHER), &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!(
            "check-occupancy: no sources under {DISPATCHER} — refusing to report a clean tree"
        );
        return false;
    }
    let mut arms: Vec<String> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("check-occupancy: cannot read {}", path.display());
            return false;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        for line in presence_arms(&src) {
            arms.push(format!("{rel}:{line}"));
        }
    }
    let count = arms.len();
    if count == PRESENCE_ARMS {
        println!("check-occupancy: OK — {count} ingest arms still decide on index presence alone");
        return true;
    }
    if count > PRESENCE_ARMS {
        eprintln!(
            "check-occupancy: {count} ingest arms decide resume-vs-spawn on index presence alone, \
             {} more than PRESENCE_ARMS = {PRESENCE_ARMS}. Pass the workflow crate's occupancy \
             verdict (spawn_or_resume_guarded) on a new arm.",
            count - PRESENCE_ARMS
        );
    } else {
        eprintln!(
            "check-occupancy: {count} ingest arms decide on presence alone — lower PRESENCE_ARMS \
             in xtask/src/check_occupancy.rs from {PRESENCE_ARMS} to {count}."
        );
    }
    for arm in &arms {
        eprintln!("  {arm}");
    }
    false
}

/// Lines of the arms in `src` that pass no occupancy verdict.
fn presence_arms(src: &str) -> Vec<usize> {
    let mut mask = code_mask(src);
    blank_cfg_test(src, &mut mask);
    let bytes = src.as_bytes();

    // Byte ranges of the implementing functions' bodies, which are not arms.
    let mut skip: Vec<std::ops::Range<usize>> = Vec::new();
    for name in IMPLEMENTATIONS {
        let header = format!("fn {name}<");
        for (at, _) in src.match_indices(&header) {
            if !mask[at] {
                continue;
            }
            if let Some(open) = next_code_byte(bytes, &mask, at, b'{')
                && let Some(close) = match_brace(bytes, &mask, open)
            {
                skip.push(open..close);
            }
        }
    }
    let inside = |at: usize| skip.iter().any(|r| r.contains(&at));

    let mut out = Vec::new();
    for (at, _) in src.match_indices(".spawn_or_resume::<") {
        if mask[at] && !inside(at) {
            out.push(line_of(src, at));
        }
    }
    for (at, _) in src.match_indices(".spawn_or_resume_keyed::<") {
        if !mask[at] || inside(at) {
            continue;
        }
        // The call's argument list: from its `(` to the matching `)`.
        let Some(open) = next_code_byte(bytes, &mask, at, b'(') else {
            continue;
        };
        let Some(close) = match_paren(bytes, &mask, open) else {
            continue;
        };
        let last = src[open + 1..close]
            .trim_end()
            .trim_end_matches(',')
            .rsplit(',')
            .next()
            .unwrap_or("")
            .trim();
        if last == "None" {
            out.push(line_of(src, at));
        }
    }
    out.sort_unstable();
    out
}

/// The `)` closing the `(` at `open`, counting only unmasked parentheses.
fn match_paren(bytes: &[u8], mask: &[bool], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for i in open..bytes.len() {
        if !mask[i] {
            continue;
        }
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arm_without_a_verdict_is_counted() {
        let src = r#"
impl D {
    async fn arms(&self) {
        self.spawn_or_resume::<A>(k, "a", cmd, &fv, &[]).await;
        self.spawn_or_resume_guarded::<B>(k, "b", cmd, &fv, &[], v).await;
        self.spawn_or_resume_keyed::<C>(k, "c", cmd, &fv, &[], &[k], None).await;
        self.spawn_or_resume_keyed::<E>(k, "e", cmd, &fv, &[], &[k], Some(|s| !s.t())).await;
    }
}
"#;
        assert_eq!(presence_arms(src).len(), 2);
    }

    #[test]
    fn the_implementation_and_prose_are_not_arms() {
        let src = r#"
/// Unlike `self.spawn_or_resume::<W>(..)`, this one …
async fn spawn_or_resume<W>(&self) {
    self.spawn_or_resume_keyed::<W>(key, name, cmd, fv, d, &[], None).await
}
"#;
        assert!(presence_arms(src).is_empty());
    }
}
