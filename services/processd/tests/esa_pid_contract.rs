//! Guards that the ESA module listens for the PIDs `makod` actually spawns from.
//!
//! Same failure class the LF module once had (see `pid_contract.rs`): a module
//! keyed on an *answer* PID never matches an event and silently never runs.
//! The ESA handshake makes that especially easy to get wrong, because the MSB
//! both receives and sends within one process — 17007 arrives, 19011 leaves,
//! and both are plausible `u32`s that appear in the same AHB chapter.
//!
//! The module only exists in a build carrying the MSB role; `role_separation.rs`
//! asserts its absence elsewhere.
#![cfg(feature = "role-msb-strom")]

use processd::esa_module::{ESA_ANSWERED_PIDS, EsaOrderPayload};

/// The MSB's **own** answers. An event never carries one as `makopid`.
const ANSWER_PIDS: &[u32] = &[15_003, 19_011, 19_012, 19_013, 19_014];

#[test]
fn no_esa_trigger_is_an_answer_pid() {
    for pid in ESA_ANSWERED_PIDS {
        assert!(
            !ANSWER_PIDS.contains(pid),
            "{pid} is an answer the MSB sends, not an order it receives — a module \
             keyed on it can never fire"
        );
    }
}

/// The four inbound steps of WiM Teil 2 Kap. 4, and no others.
#[test]
fn the_esa_module_answers_exactly_the_inbound_kapitel_4_pids() {
    let mut got = ESA_ANSWERED_PIDS.to_vec();
    got.sort_unstable();
    assert_eq!(
        got,
        vec![17_007, 17_008, 35_003, 39_002],
        "35003 Werteanfrage, 17007 Bestellung, 17008 Abbestellung, 39002 Stornierung"
    );
}

/// Every PID the module claims carries a published Antwortfrist, or the queue
/// entry it creates has no deadline for an operator to work to.
#[test]
fn every_answered_pid_carries_a_published_frist() {
    let received = time::macros::datetime!(2026-03-02 8:00 UTC);
    for pid in ESA_ANSWERED_PIDS {
        let w = mako_fristen::antwort::operator_window(*pid, received);
        assert!(
            w.is_regulatory,
            "PID {pid} has no published Antwortfrist — its queue entry would carry none"
        );
        assert!(
            w.source.contains("Teil 2"),
            "PID {pid} must draw its window from WiM Teil 2, got {:?}",
            w.source
        );
    }
}

/// The module reacts to the PIDs it claims, and to nothing else.
#[test]
fn parse_accepts_the_claimed_pids_and_rejects_the_answers() {
    let event = |pid: u32| {
        serde_json::json!({
            "subject": "0195f1a0-0000-7000-8000-000000000001",
            "makopid": pid,
            "time": "2026-03-02T08:00:00Z",
            "data": { "malo_id": "51238696012", "abonnement": "Z01" },
        })
    };
    for pid in ESA_ANSWERED_PIDS {
        assert!(
            EsaOrderPayload::parse(&event(*pid)).is_some(),
            "the module must react to {pid}"
        );
    }
    for pid in ANSWER_PIDS {
        assert!(
            EsaOrderPayload::parse(&event(*pid)).is_none(),
            "the module must not react to its own answer {pid}"
        );
    }
}
