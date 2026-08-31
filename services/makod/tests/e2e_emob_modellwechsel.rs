//! The three Modell-2 legs must actually run, not merely reach an arm.
//!
//! `e2e_dispatch_coverage_guard` proves each of the six Prüfidentifikatoren
//! resolves to a dispatch arm. That is a weaker statement than it sounds: an
//! adapter that errors and a process that spawns both count as „reached". This
//! file asserts the outcome — which PID **spawns** a process, which one only
//! **resumes** one, that a repeat is never dropped, and that the three legs
//! hold three processes on one Marktlokation.
//!
//! # Why the answer PIDs must not spawn
//!
//! Each leg registers both its PIDs to one workflow so the router can resolve
//! the answer. An answer arrives on a process *this side started*; spawning on
//! it would open a second process with nothing to answer while the real one's
//! Antwortfrist expires as a false timeout — and the counterparty would see
//! silence where mako's own event log records an answered process.
//!
//! Sources: AWH „Zum Modell 2" V1.3 Kap. 2.1.2 / 2.2.2; UTILMD AHB Strom 2.2
//! Kap. 11.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";

async fn dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN_MP),
    )
}

/// The shipped `edi-energy` fixture for `pid`.
fn fixture(pid: u32) -> Vec<u8> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/edi-energy/tests/fixtures/utilmd/valid/pid_",
    );
    std::fs::read(format!("{path}{pid}.edi"))
        .unwrap_or_else(|e| panic!("fixture for {pid} is missing: {e}"))
}

async fn dispatch_on(d: &EdifactIngestDispatcher, pid: u32, workflow: &str) -> IngestOutcome {
    let msg = edi_energy::parse(&fixture(pid))
        .unwrap_or_else(|e| panic!("fixture {pid} does not parse: {e}"));
    d.dispatch(&msg, workflow, pid)
        .await
        .unwrap_or_else(|e| panic!("dispatch of {pid} errored: {e}"))
}

/// Every leg's request PID opens a process.
#[tokio::test]
async fn each_request_pid_spawns_its_leg() {
    for (pid, workflow) in [
        (55_238, mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME),
        (55_240, mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME),
        (55_242, mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME),
    ] {
        let d = dispatcher().await;
        let outcome = dispatch_on(&d, pid, workflow).await;
        assert!(
            matches!(outcome, IngestOutcome::Spawned { .. }),
            "{pid} must open a {workflow} process, got {outcome:?}"
        );
    }
}

/// **An answer never opens a process.** It resumes the one this side started,
/// and finds nothing when there is none.
#[tokio::test]
async fn an_answer_pid_resumes_and_never_spawns() {
    for (pid, workflow) in [
        (55_239, mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME),
        (55_241, mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME),
        (55_243, mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME),
    ] {
        let d = dispatcher().await;
        let outcome = dispatch_on(&d, pid, workflow).await;
        assert!(
            !matches!(outcome, IngestOutcome::Spawned { .. }),
            "{pid} is an answer and must not spawn a {workflow} process, got {outcome:?}"
        );
    }
}

/// A repeated request is never dropped, and stays on its own leg.
///
/// Which of the two outcomes it takes depends on the day. The Modell-2
/// Prüfidentifikatoren exist only in the **FV2026-10-01** profile, and a
/// format applies six months after publication rather than on it (Allgemeine
/// Festlegungen 6.1d §2.5) — so before 01.10.2026 the AHB layer refuses every
/// 55238, the process reaches `Rejected`, and a resend legitimately opens a
/// fresh one. From 01.10.2026 the first process stays open and the resend
/// resumes it.
///
/// Both are correct; `Skipped` is not. A dropped duplicate is the failure this
/// pins: it would leave the counterparty's retry unanswered while mako's own
/// log shows nothing at all.
#[tokio::test]
async fn a_repeated_anmeldung_is_never_dropped() {
    let d = dispatcher().await;
    let workflow = mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME;

    let first = dispatch_on(&d, 55_238, workflow).await;
    let IngestOutcome::Spawned { process_id, .. } = first else {
        panic!("the first 55238 must spawn, got {first:?}");
    };

    match dispatch_on(&d, 55_238, workflow).await {
        // The Frist is still running: same process, no second stream.
        IngestOutcome::Dispatched {
            workflow_name,
            process_id: same,
        } => {
            assert_eq!(workflow_name, workflow);
            assert_eq!(same, process_id, "a live leg is resumed, not duplicated");
        }
        // The first was refused and is terminal: `Occupancy` lets a fresh
        // Anmeldung through rather than reopening a settled one.
        IngestOutcome::Spawned {
            workflow_name,
            process_id: fresh,
        } => {
            assert_eq!(workflow_name, workflow);
            assert_ne!(fresh, process_id, "a respawn is a new process");
        }
        other => panic!("a repeated 55238 must reach a process, got {other:?}"),
    }
}

/// **The three legs run side by side on one Marktlokation.**
///
/// The 55240 leg to the LF runs *inside* the Anmeldung's own 7-Werktage window
/// — that is the whole reason the Anmeldung's window is 7 and not 3. Folding
/// the legs into one workflow name would make the second message resume the
/// first's process, and the Anmeldung would be answered by the LF's `E_0511`.
#[tokio::test]
async fn the_legs_do_not_collide_on_one_marktlokation() {
    let d = dispatcher().await;

    let mut ids = Vec::new();
    for (pid, workflow) in [
        (55_238, mako_emob::EmobAnmeldungWorkflow::WORKFLOW_NAME),
        (55_240, mako_emob::EmobZuordnungsendeWorkflow::WORKFLOW_NAME),
        (55_242, mako_emob::EmobAbmeldungWorkflow::WORKFLOW_NAME),
    ] {
        match dispatch_on(&d, pid, workflow).await {
            IngestOutcome::Spawned { process_id, .. } => ids.push(process_id),
            other => panic!("{pid} must spawn its own process, got {other:?}"),
        }
    }
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 3, "each leg holds a process of its own");
}
