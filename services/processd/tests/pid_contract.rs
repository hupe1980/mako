//! Guards that processd listens for the PIDs `makod` actually emits.
//!
//! processd reacts to `de.mako.process.initiated`, whose `makopid` is the PID of
//! the **inbound** EDIFACT message that spawned the process. Answer PIDs never
//! appear there: `makod`'s ingest dispatcher matches inbound PIDs against a
//! per-workflow spawn table and reports anything else as
//! `pid_not_in_spawn_table`.
//!
//! The LF module was keyed on **55008** — the *outbound* Bestätigung of the
//! NB-seitiges Lieferende, whose inbound trigger is **55007**. Nothing failed:
//! the module simply never matched an event, so the 24-hour LF answer
//! automation silently never ran. A test is the only thing that catches that,
//! because both numbers are plausible `u32`s and both appear in the AHB for the
//! same process.
//!
//! These assertions compare processd's trigger PIDs against the canonical
//! constants in `mako-gpke`, which are the same ones `makod`'s spawn table is
//! driven from.

// The LF module only exists in a build that carries an LF role; in an `nb-only`
// or `msb-only` binary there is nothing here to pin (see `role_separation.rs`,
// which asserts that absence).
#![cfg(any(feature = "role-lf-strom", feature = "role-lf-gas"))]

use processd::lf_module::{BEENDIGUNG_ZUORDNUNG, LF_ANTWORT_PROCESSES, NB_LIEFERENDE};

#[test]
fn the_lf_module_triggers_on_the_pids_makod_spawns_from() {
    assert_eq!(
        mako_gpke::LF_ABMELDUNG_PIDS,
        [NB_LIEFERENDE.trigger_pid],
        "the NB-seitiges Lieferende trigger must be the inbound PID of the \
         `gpke-lf-abmeldung` workflow, not one of its answers"
    );
    assert_eq!(
        mako_gpke::BEENDIGUNG_ZUORDNUNG_PIDS,
        [BEENDIGUNG_ZUORDNUNG.trigger_pid],
        "the Beendigung der Zuordnung trigger must be the inbound PID of the \
         `gpke-beendigung-zuordnung` workflow"
    );
}

#[test]
fn no_lf_trigger_is_an_answer_pid() {
    // The answers to the two processes. An event never carries these as
    // `makopid`, so a module keyed on one can never fire.
    const ANSWER_PIDS: &[u32] = &[55_008, 55_009, 55_011, 55_012];
    for process in LF_ANTWORT_PROCESSES {
        assert!(
            !ANSWER_PIDS.contains(&process.trigger_pid),
            "{} is keyed on {}, which is an answer PID — `makod` only emits \
             `process.initiated` for inbound message PIDs",
            process.name,
            process.trigger_pid
        );
    }
}

#[test]
fn every_lf_process_dispatches_a_registered_command() {
    for process in LF_ANTWORT_PROCESSES {
        for command in [process.bestaetigen, process.ablehnen] {
            assert!(
                mako_markt::commands::DISPATCHED_BY_SERVICES.contains(&command),
                "{} dispatches {command:?}, which is not in DISPATCHED_BY_SERVICES — \
                 `makod` rejects an unregistered command name with HTTP 422",
                process.name
            );
        }
    }
}

/// Each process carries the EBD that actually governs its answer.
///
/// `E_0624` is "Anfrage zur Beendigung der Zuordnung prüfen" — PID 55010,
/// answered 55011/55012. processd once attached it to the NB-seitiges
/// Lieferende (55007 → 55008/55009), which `E_0609` governs. Both numbers are
/// plausible and appear in the same AHB chapter, so only an assertion separates
/// them.
#[test]
fn each_process_carries_its_own_ebd() {
    assert_eq!(BEENDIGUNG_ZUORDNUNG.ebd, Some("E_0624"));
    assert_eq!(NB_LIEFERENDE.ebd, Some("E_0609"));
}

/// The descriptors' EBD and answer PIDs must agree with the shared GPKE table
/// that also drives the Fristen — one source, checked from both ends.
#[test]
fn the_descriptors_agree_with_the_shared_gpke_table() {
    for process in LF_ANTWORT_PROCESSES {
        let o = mako_gpke::antwort_obligation(process.trigger_pid).unwrap_or_else(|| {
            panic!(
                "{} (PID {}) has no entry in mako_gpke::ANTWORT_OBLIGATIONS, so nothing \
                 gives its queue entry a regulatory Frist",
                process.name, process.trigger_pid
            )
        });
        assert_eq!(
            o.ebd, process.ebd,
            "{} disagrees with the GPKE table about its EBD",
            process.name
        );
        assert!(
            o.answered_by == "LF" || o.answered_by == "LFA",
            "{} is answered by {} — not a process the LF module may claim",
            process.name,
            o.answered_by
        );
    }
}

#[test]
fn the_two_processes_do_not_share_a_trigger() {
    let mut seen = std::collections::BTreeSet::new();
    for process in LF_ANTWORT_PROCESSES {
        assert!(
            seen.insert(process.trigger_pid),
            "{} duplicates trigger PID {} — the first match would shadow the second",
            process.name,
            process.trigger_pid
        );
    }
}
