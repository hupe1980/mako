//! Guard: every command `processd` can put on its approval queue is one the
//! queue can dispatch.
//!
//! An approval-queue entry names the `makod` command its approve and reject
//! decisions dispatch, and `makod` refuses an answer command with `422` when
//! its body lacks a field the handler needs — the Antwortcode, the MeLo, the
//! `tx_id`. A command the queue cannot fill is an operator decision that fails
//! at dispatch: the entry is released, nothing reaches the market, and it
//! expires against the Frist it was queued under.
//!
//! `processd`'s `ANSWER_CONTRACTS` (`src/pg/approval.rs`) states, per command,
//! the fields a decision must carry, and the decide path refuses a decision
//! without them before anything is sent. This guard keeps the table complete:
//! every `mako_markt::commands::*` constant named in a `processd` source file
//! that builds an `ApprovalQueueEntry` must either have a contract or be listed
//! in [`NOT_QUEUED`] with the reason it never reaches the queue as an answer.
//! An entry in [`NOT_QUEUED`] that nothing names any more is refused too.
//!
//! Comments, strings and `#[cfg(test)]` items are not scanned.

use std::collections::BTreeSet;
use std::path::Path;

use crate::check_workflow_purity::{blank_cfg_test, code_mask, line_of, walk_rs};

/// Where the contracts live.
const CONTRACTS: &str = "services/processd/src/pg/approval.rs";

/// The tree scanned.
const PROCESSD_SRC: &str = "services/processd/src";

/// Constants the queue-building files name that are never queued as an
/// answer, with why.
const NOT_QUEUED: &[(&str, &str)] = &[
    (
        "GPKE_BEENDIGUNG_ZUORDNUNG_ANFRAGEN",
        "the NB's Anfrage to the LFA, sent before the Anmeldung is decided",
    ),
    (
        "GPKE_ZUORDNUNG_INFORMIEREN",
        "a Meldepflicht: queued only as a follow-up with its body frozen",
    ),
    (
        "GPKE_ZUORDNUNG_BEENDEN",
        "a Meldepflicht: queued only as a follow-up with its body frozen",
    ),
    (
        "GELI_ZUORDNUNG_INFORMIEREN",
        "a Meldepflicht: queued only as a follow-up with its body frozen",
    ),
    (
        "GELI_ZUORDNUNG_BEENDEN",
        "a Meldepflicht: queued only as a follow-up with its body frozen",
    ),
];

/// Run the check. Returns `true` when clean.
pub fn run(workspace_root: &Path) -> bool {
    let Ok(contracts_src) = std::fs::read_to_string(workspace_root.join(CONTRACTS)) else {
        eprintln!("check-approval-commands: cannot read {CONTRACTS}");
        return false;
    };
    let contracts = contract_commands(&contracts_src);
    if contracts.is_empty() {
        eprintln!(
            "check-approval-commands: {CONTRACTS} holds no ANSWER_CONTRACTS — the queue has no \
             statement of what a decision must carry"
        );
        return false;
    }

    let mut files = Vec::new();
    walk_rs(&workspace_root.join(PROCESSD_SRC), &mut files);
    files.sort();
    let mut findings: Vec<String> = Vec::new();
    let mut named: BTreeSet<String> = BTreeSet::new();
    let mut scanned = 0usize;
    for path in &files {
        let Ok(src) = std::fs::read_to_string(path) else {
            findings.push(format!("{}: unreadable", path.display()));
            continue;
        };
        let rel = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if rel == CONTRACTS {
            continue;
        }
        let Some(hits) = queue_commands(&src) else {
            continue;
        };
        scanned += 1;
        for (constant, line) in hits {
            named.insert(constant.clone());
            if !contracts.contains(&constant) && !NOT_QUEUED.iter().any(|(c, _)| *c == constant) {
                findings.push(format!(
                    "{rel}:{line}: `{constant}` is named beside the approval queue but has no \
                     ANSWER_CONTRACTS entry — a decision on it cannot be dispatched"
                ));
            }
        }
    }
    for (constant, _) in NOT_QUEUED {
        if !named.contains(*constant) {
            findings.push(format!(
                "NOT_QUEUED names `{constant}`, which no queue-building file names any more — \
                 remove the entry"
            ));
        }
    }

    if findings.is_empty() {
        println!(
            "check-approval-commands: OK — {} contracts, {scanned} queue-building files",
            contracts.len()
        );
        return true;
    }
    eprintln!("check-approval-commands: {} finding(s)", findings.len());
    for f in &findings {
        eprintln!("  {f}");
    }
    false
}

/// The `c::NAME` constants listed in `ANSWER_CONTRACTS`.
fn contract_commands(src: &str) -> BTreeSet<String> {
    let Some(start) = src.find("pub const ANSWER_CONTRACTS") else {
        return BTreeSet::new();
    };
    let end = src[start..].find("\n};").map_or(src.len(), |e| start + e);
    let mask = code_mask(src);
    constants_in(&src[start..end], "c::", &mask[start..end])
        .into_iter()
        .map(|(c, _)| c)
        .collect()
}

/// Every `mako_markt::commands::NAME` in a file that builds an
/// `ApprovalQueueEntry`, with its line; `None` for any other file.
fn queue_commands(src: &str) -> Option<Vec<(String, usize)>> {
    let mut mask = code_mask(src);
    blank_cfg_test(src, &mut mask);
    let builds = src
        .match_indices("ApprovalQueueEntry")
        .any(|(at, _)| mask[at]);
    if !builds {
        return None;
    }
    Some(
        constants_in(src, "mako_markt::commands::", &mask)
            .into_iter()
            .map(|(c, at)| (c, line_of(src, at)))
            .collect(),
    )
}

/// `prefix` followed by an upper-case constant name, in code, with its offset.
fn constants_in(src: &str, prefix: &str, mask: &[bool]) -> Vec<(String, usize)> {
    src.match_indices(prefix)
        .filter(|(at, _)| mask[*at])
        .filter_map(|(at, _)| {
            let name: String = src[at + prefix.len()..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
                .collect();
            (!name.is_empty()).then_some((name, at))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONTRACTS_FIXTURE: &str = r#"
pub const ANSWER_CONTRACTS: &[(&str, &[Pflichtfeld])] = {
    use mako_markt::commands as c;
    &[
        (c::GPKE_LIEFERBEGINN_BESTAETIGEN, MALO_CODE),
        (c::GPKE_LIEFERBEGINN_ABLEHNEN, MALO_CODE),
    ]
};
"#;

    #[test]
    fn the_contract_table_is_read() {
        let set = contract_commands(CONTRACTS_FIXTURE);
        assert!(set.contains("GPKE_LIEFERBEGINN_BESTAETIGEN"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn a_queued_command_without_a_contract_is_found() {
        let src = r#"
fn enqueue() {
    let e = ApprovalQueueEntry::pending(a, b, c, d, e, f)
        .with_commands(
            mako_markt::commands::WIM_WERTEBESTELLUNG_ANBIETEN,
            mako_markt::commands::GPKE_LIEFERBEGINN_ABLEHNEN,
            None,
        );
}
"#;
        let hits = queue_commands(src).expect("builds an entry");
        let contracts = contract_commands(CONTRACTS_FIXTURE);
        let missing: Vec<_> = hits
            .iter()
            .filter(|(c, _)| !contracts.contains(c))
            .map(|(c, _)| c.as_str())
            .collect();
        assert_eq!(missing, vec!["WIM_WERTEBESTELLUNG_ANBIETEN"]);
    }

    #[test]
    fn files_without_the_queue_and_test_code_are_not_scanned() {
        assert!(queue_commands("fn f() { mako_markt::commands::X; }").is_none());
        let src = r#"
// ApprovalQueueEntry in a comment only
fn f() { let _ = "ApprovalQueueEntry"; mako_markt::commands::X; }
"#;
        assert!(queue_commands(src).is_none());
        let with_test = r#"
fn real() { ApprovalQueueEntry::pending(); }
#[cfg(test)]
mod tests { fn t() { mako_markt::commands::ONLY_IN_TESTS; } }
"#;
        assert_eq!(queue_commands(with_test).expect("builds"), vec![]);
    }
}
