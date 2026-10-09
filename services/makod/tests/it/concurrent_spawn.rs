//! Two initiating messages for one business key must yield one process.
//!
//! # Why this is a test
//!
//! Spawning is a check-then-act: `lookup_correlated` finds no live process, so
//! one is created. Two initiating messages for the same MaLo arriving together
//! both pass the check and both spawn. Nothing fails at that moment — the damage
//! surfaces later, when every follow-up resolves the key to two processes and
//! fails with `AmbiguousProcess`, while the duplicate runs its own regulatory
//! Fristen to expiry and reports them as missed.
//!
//! AS4 inbox deduplication does not cover this. It suppresses identical
//! retransmits of one message; this needs two *distinct* messages landing on one
//! business key, which is ordinary market traffic.
//!
//! ## What this test does and does not prove
//!
//! It drives the real dispatcher under concurrency and asserts the invariant —
//! one business key, one process. It does **not** reliably reproduce the losing
//! interleaving: the window between `lookup_correlated` returning empty and the
//! spawn committing is narrow, and whether a second ingest lands inside it
//! depends on store latency. Removing the lock does not make this test fail.
//!
//! The mutual exclusion that closes the window is asserted deterministically in
//! `ingest_dispatcher::business_key_lock_tests`, where the overlap is forced
//! rather than raced for. This test is the end-to-end complement: it proves the
//! guarded path still spawns, resumes and reports correctly with eight ingests
//! in flight, which the unit test cannot show.

use std::sync::Arc;

use mako_engine::{event_store::EventStore as _, ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900001000001";

/// PID 55001 — GPKE Anmeldung NN, the initiating message of a supplier change.
const PID: u32 = 55001;
const WORKFLOW: &str = "gpke-supplier-change";

fn fixture() -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(PID).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(edi_energy::MessageType::Utilmd)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no UTILMD profile carries {PID}"));
    let af = profile.anwendungsfall(PID).expect("carried");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN_MP.to_owned(),
            },
        )
        .unwrap_or_else(|e| panic!("skeleton for {PID}: {e}"));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Dispatch the same initiating message twice concurrently through one
/// dispatcher and count how many processes were spawned.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_initiating_messages_spawn_one_process() {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let dispatcher = Arc::new(EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN_MP),
    ));

    let edi = fixture();

    // Released together, so all tasks reach the lookup at the same moment. One
    // pair rarely interleaves; the window is narrow and an in-memory store
    // closes it fast.
    const RACERS: usize = 8;
    let gate = Arc::new(tokio::sync::Barrier::new(RACERS));
    let mut handles = Vec::with_capacity(RACERS);
    for _ in 0..RACERS {
        let d = Arc::clone(&dispatcher);
        let gate = Arc::clone(&gate);
        let edi = edi.clone();
        handles.push(tokio::spawn(async move {
            let msg = edi_energy::parse(edi.as_bytes()).expect("fixture parses");
            gate.wait().await;
            d.dispatch(&msg, WORKFLOW, PID).await
        }));
    }

    let mut outcomes = Vec::with_capacity(RACERS);
    for h in handles {
        outcomes.push(h.await.expect("task joins"));
    }

    // Some may legitimately fail: a second *initiating* message for a business
    // key that is already occupied is a real conflict, and the workflow rejects
    // it (`InvalidState`). What must never happen is that two of them succeed by
    // creating separate processes — silent at the time, and surfacing later as
    // `AmbiguousProcess` on every follow-up.
    assert!(
        outcomes.iter().any(Result::is_ok),
        "at least one dispatch must succeed: {outcomes:?}",
    );
    for outcome in &outcomes {
        if let Ok(IngestOutcome::Skipped { reason, .. }) = outcome {
            panic!("no message may be silently skipped, got: {reason}");
        }
    }

    // The invariant: one business key, one process.
    let streams = store
        .list_streams(Some("process/"))
        .await
        .expect("list process streams");
    assert_eq!(
        streams.len(),
        1,
        "{RACERS} concurrent initiating messages for one business key produced {} \
         processes. Every later message for this key would resolve to all of them \
         and fail with AmbiguousProcess, and each duplicate would run its own \
         regulatory Fristen to expiry and report them as missed. Streams: {streams:?}",
        streams.len(),
    );
}
