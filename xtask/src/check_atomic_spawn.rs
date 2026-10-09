//! Guard: a correlation key is written in the batch of the events that justify it.
//!
//! A process is found by its business key — the MaLo, the MeLo, the `tx_id`,
//! the Belegnummer — through the correlation index, and nothing else. The
//! `makod` duplicate guard asks that index which process still occupies a key,
//! and every follow-up command and inbound answer resolves its process the same
//! way. So a process whose events and outbound message are durable while its key
//! is not is a process nobody can find: the answer is skipped as
//! `process_not_found`, and the ERP's re-issue of the command spawns a second
//! process and sends the counterparty a second message.
//!
//! `Process::run` takes the keys as `RunOptions::correlations` and writes them in
//! the same `AppendBatch` as the events — on a spawn and on a resume alike. A
//! separate `register_correlated` call is therefore never needed in a service,
//! and every one is refused over each service's shipped `src/` tree: it is a
//! second write that can fail on its own.
//!
//! Comments, string literals and `#[cfg(test)]` items are not scanned.

use std::path::Path;

use crate::check_workflow_purity::{blank_cfg_test, code_mask, line_of, walk_rs};

/// A finding: file, line, what is wrong.
type Finding = (String, usize, String);

/// Run the check over `services/*/src`. Returns `true` when clean.
pub fn run(workspace_root: &Path) -> bool {
    let mut files = Vec::new();
    let services = workspace_root.join("services");
    let Ok(entries) = std::fs::read_dir(&services) else {
        eprintln!("check-atomic-spawn: cannot read {}", services.display());
        return false;
    };
    for entry in entries.flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            walk_rs(&src, &mut files);
        }
    }
    files.sort();
    if files.is_empty() {
        eprintln!("check-atomic-spawn: no service sources found — refusing to report a clean tree");
        return false;
    }

    let mut findings: Vec<Finding> = Vec::new();
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            findings.push((path.display().to_string(), 0, "unreadable".to_owned()));
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        findings.extend(scan_source(&src, &rel));
    }

    if findings.is_empty() {
        println!("check-atomic-spawn: OK — {} service sources", files.len());
        return true;
    }
    eprintln!("check-atomic-spawn: {} finding(s)", findings.len());
    for (file, line, msg) in &findings {
        eprintln!("  {file}:{line}: {msg}");
    }
    eprintln!(
        "\nA correlation key lands in the same write as its events: pass it to \
         `Process::run` as `RunOptions::correlations` (in makod's command doors \
         `commands_api::spawn_process`). A key written afterwards can fail on its own, \
         and the duplicate guard then cannot see the running process."
    );
    false
}

/// Every `register_correlated` call in one file's shipped code.
fn scan_source(src: &str, rel: &str) -> Vec<Finding> {
    let mut mask = code_mask(src);
    blank_cfg_test(src, &mut mask);
    src.match_indices(".register_correlated(")
        .map(|(at, _)| at)
        .filter(|&at| mask[at])
        .map(|at| {
            (
                rel.to_owned(),
                line_of(src, at),
                "`register_correlated` writes a correlation key outside the batch of the \
                 events it belongs to"
                    .to_owned(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOOR: &str = "services/makod/src/orchestrator/commands_api/gpke.rs";

    #[test]
    fn a_registration_after_a_spawn_is_refused() {
        let src = r#"
async fn anmelden(state: &S) -> R {
    let process = Process::<W, Arc<SlateDbStore>>::new(Arc::clone(&state.store), t, wf);
    process.run(cmd, RunOptions::new().deadlines(&[d])).await?;
    if let Err(e) = state.store.as_process_registry()
        .register_correlated(state.tenant_id, malo, process.process_id(), id).await {}
    Ok(())
}
"#;
        let findings = scan_source(src, DOOR);
        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(findings[0].1, 6);
    }

    #[test]
    fn a_registration_on_a_resume_is_refused_too() {
        let src = r#"
async fn keyed(state: &S) {
    let process = Process::<W, S>::from_identity(store, identity);
    registry.register_correlated(t, key, id, i).await.ok();
}
"#;
        assert_eq!(scan_source(src, DOOR).len(), 1);
    }

    #[test]
    fn the_batched_key_and_prose_are_clean() {
        let src = r#"
/// The two-write shape was `Process::new(..)` then `.register_correlated(..)`.
async fn anmelden(state: &S) -> R {
    let msg = "never .register_correlated( after Process::new(";
    process.run(cmd, RunOptions::new().correlations(&keys)).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    fn fixture() { s.register_correlated(t, k, i, d); }
}
"#;
        assert!(scan_source(src, DOOR).is_empty());
    }
}
