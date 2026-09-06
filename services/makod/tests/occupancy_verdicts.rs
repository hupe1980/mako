//! Guard: which ingest dispatch arms decide whether a process still occupies
//! its business key, and how each of them decides it.
//!
//! A dispatch arm that passes no occupancy verdict resumes any process the
//! correlation index still holds, settled or not — so whether an arm passes one
//! is the difference between a re-sent Anmeldung starting a new process and it
//! being folded into a finished one. The set is small, it grows one workflow at
//! a time as a domain crate publishes `OccupiesBusinessKey` or a terminal-state
//! predicate, and it is stated in prose in two places. Nothing held it: the
//! stated set had drifted to name three workflows that pass a verdict without
//! naming two others that do, and the count with it.
//!
//! Read off the dispatcher's own source rather than at runtime, because the
//! verdict is a function passed at the call site and leaves no trace on the
//! dispatched process.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How a dispatch arm arrives at its verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Verdict {
    /// The workflow's state implements `OccupiesBusinessKey`.
    Trait,
    /// The state publishes an inherent terminal-state predicate the arm negates.
    Closure,
    /// The arm passes `None` and keeps the presence-based behaviour.
    None,
}

/// Every workflow the dispatcher decides occupancy for, and how.
///
/// Keyed by the workflow type's last path segment, which is what the call site
/// names. `EmobLeg` stands for the `leg!` macro in `emob.rs`, expanded once per
/// Modell-2 leg — `emob-anmeldung`, `emob-zuordnungsende`, `emob-abmeldung`.
const EXPECTED: &[(&str, Verdict, usize)] = &[
    ("EmobLeg", Verdict::Closure, 3),
    ("GaBiGasNominationWorkflow", Verdict::Closure, 1),
    ("GeliGasSperrungNbWorkflow", Verdict::Closure, 1),
    ("GeliGasSupplierChangeWorkflow", Verdict::Trait, 1),
    ("GeliGasZuordnungsmeldungWorkflow", Verdict::Trait, 1),
    ("GpkeSperrungWorkflow", Verdict::Closure, 1),
    ("GpkeZuordnungsmeldungWorkflow", Verdict::Trait, 1),
    ("WimDeviceChangeWorkflow", Verdict::Trait, 1),
    ("WimErsteinbauWorkflow", Verdict::Trait, 1),
    ("WimGeraeteubernahmeWorkflow", Verdict::Trait, 1),
    ("WimInsrptWorkflow", Verdict::Closure, 1),
    ("WimStammdatenWorkflow", Verdict::Closure, 1),
    ("WimWeiterverpflichtungWorkflow", Verdict::Trait, 1),
    ("WimWertebestellungWorkflow", Verdict::None, 1),
];

fn dispatcher_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/orchestrator/ingest_dispatcher")
}

/// The argument list of every `spawn_or_resume_guarded` / `_keyed` call in the
/// family modules, with the workflow it names.
///
/// Brace-matched rather than line-based: the call spans a dozen lines and its
/// arguments carry parentheses of their own.
fn call_sites() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dispatcher_dir())
        .expect("dispatcher module directory")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "rs"))
        // `mod.rs` holds the generic helpers the family modules call, so its
        // own `spawn_or_resume` bodies are the definitions, not call sites.
        .filter(|p| p.file_name().is_some_and(|n| n != "mod.rs"))
        .collect();
    files.sort();
    for path in files {
        let src = std::fs::read_to_string(&path).expect("dispatcher source");
        for prefix in ["spawn_or_resume_guarded::<", "spawn_or_resume_keyed::<"] {
            let mut from = 0;
            while let Some(at) = src[from..].find(prefix) {
                let open = from + at + prefix.len();
                let close = open + src[open..].find('>').expect("type argument ends");
                let workflow = src[open..close]
                    .trim()
                    .rsplit("::")
                    .next()
                    .expect("type path")
                    .to_owned();
                let mut i = close + 2; // past `>(`
                let mut depth = 1usize;
                for ch in src[i..].chars() {
                    i += ch.len_utf8();
                    match ch {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                out.push((workflow, src[close + 2..i].to_owned()));
                from = close;
            }
        }
    }
    out
}

fn observed() -> BTreeMap<String, (Verdict, usize)> {
    let mut out: BTreeMap<String, (Verdict, usize)> = BTreeMap::new();
    for (workflow, args) in call_sites() {
        let verdict = if args.contains("occupies_business_key") {
            Verdict::Trait
        } else if args.contains("is_terminal") || args.contains("ist_terminal") {
            Verdict::Closure
        } else {
            Verdict::None
        };
        // The Modell-2 legs share one macro body; name it for what it covers.
        let name = if workflow == "$wf" {
            "EmobLeg".to_owned()
        } else {
            workflow
        };
        let entry = out.entry(name).or_insert((verdict, 0));
        assert_eq!(
            entry.0, verdict,
            "one workflow, two different occupancy verdicts"
        );
        entry.1 += 1;
    }
    // Each leg of the Modell-2 macro is a workflow name of its own.
    if let Some(emob) = out.get_mut("EmobLeg") {
        emob.1 = 3;
    }
    out
}

#[test]
fn every_dispatch_arm_decides_occupancy_the_way_the_docs_say() {
    let observed = observed();
    let expected: BTreeMap<String, (Verdict, usize)> = EXPECTED
        .iter()
        .map(|(w, v, n)| ((*w).to_owned(), (*v, *n)))
        .collect();
    assert_eq!(
        observed, expected,
        "the ingest dispatcher's occupancy verdicts moved.\n\
         Update EXPECTED, then the two places that state the set in prose: \
         `Occupancy`'s doc comment in \
         src/orchestrator/ingest_dispatcher/mod.rs and the \
         \"Resuming a settled process\" section of concepts/PROCESS_COVERAGE.md."
    );
}

#[test]
fn the_stated_counts_are_the_counts() {
    let observed = observed();
    let by = |v: Verdict| -> usize {
        observed
            .values()
            .filter(|(kind, _)| *kind == v)
            .map(|(_, n)| n)
            .sum()
    };
    let (trait_impls, closures) = (by(Verdict::Trait), by(Verdict::Closure));
    let total = trait_impls + closures;

    let claim = format!(
        "{total} of the 63 workflow names dispatched here supply a verdict at \
         all — {trait_impls} through that trait, {closures} through an inherent \
         terminal-state closure"
    );
    let module = std::fs::read_to_string(dispatcher_dir().join("mod.rs")).expect("mod.rs");
    let flat = module
        .lines()
        .map(|l| l.trim_start_matches("///").trim())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        flat.contains(&claim),
        "`Occupancy`'s doc comment must state `{claim}`"
    );

    // `concepts/` is not in git, so a checkout without it is normal and only
    // costs this half of the guard.
    let concept = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../concepts/PROCESS_COVERAGE.md");
    let Ok(text) = std::fs::read_to_string(&concept) else {
        eprintln!("skipping: {} is not present", concept.display());
        return;
    };
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for phrase in [
        format!("**{total}** of the **63** workflow names"),
        format!("{trait_impls} through an `OccupiesBusinessKey`"),
        format!("{closures} through an inherent terminal-state closure"),
    ] {
        assert!(
            flat.contains(&phrase),
            "PROCESS_COVERAGE.md must state `{phrase}`"
        );
    }
}
