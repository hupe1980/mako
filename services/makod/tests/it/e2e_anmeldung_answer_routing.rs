//! Every answer PID on a supplier-change path must reach its dispatch arm.
//!
//! A PID the router registers but the ingest `match` has no branch for is
//! discarded as `Skipped { pid_not_in_* }` — nothing errors and no process
//! advances, so the Frist simply expires. These are the **success** paths of
//! their processes, where a silent drop is hardest to notice: the rejection
//! path keeps working.
//!
//! `e2e_dispatch_coverage_guard` covers the class; this pins the specific sets.
//! Both arms read the domain module's own PID constant
//! (`ANTWORT_PIDS_LF`, `UTILMD_ANFRAGE_PIDS`), so the dispatch table cannot
//! drift from the router registration.
//!
//! The NB arm spawns on `UTILMD_ANFRAGE_PIDS` (55001, 55004, 55077) — the
//! Anfragen `SendAntwort` can answer, because `response_pid_for` maps each of
//! them to its Bestätigung and Ablehnung.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";

/// The profile's own skeleton of the Anwendungsfall, addressed to `OWN_MP`.
fn fixture(pid: u32) -> String {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("a Prüfidentifikator");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no profile carries PID {pid}"));
    let af = profile.anwendungsfall(pid).expect("the column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN_MP.to_owned(),
            },
        )
        .expect("skeleton");
    String::from_utf8_lossy(&bytes).into_owned()
}

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

/// Dispatch `pid` to `workflow` and report whether it reached an arm.
async fn reaches_arm(pid: u32, workflow: &str) -> bool {
    let edi = fixture(pid);
    let msg = edi_energy::parse(edi.as_bytes()).expect("fixture parses");
    !matches!(
        dispatcher().await.dispatch(&msg, workflow, pid, time::OffsetDateTime::now_utc()).await,
        Ok(IngestOutcome::Skipped { reason, .. }) if reason.starts_with("pid_not_in_")
    )
}

#[tokio::test]
async fn geli_gas_bestaetigung_anmeldung_is_not_dropped() {
    assert!(
        reaches_arm(44002, "geli-gas-lf-anmeldung").await,
        "PID 44002 (Bestätigung Anmeldung NN) must reach the LFN dispatch arm — \
         it is the success path of the Gas supply start"
    );
}

/// The arm must cover the whole registered Anfrage set, not a copy of it.
#[tokio::test]
async fn every_geli_gas_answer_pid_reaches_the_lfn_arm() {
    for &pid in mako_geli_gas::lf_anmeldung::ANTWORT_PIDS_LF {
        assert!(
            reaches_arm(pid, "geli-gas-lf-anmeldung").await,
            "PID {pid} is in ANTWORT_PIDS_LF but never reaches a dispatch arm"
        );
    }
}

#[tokio::test]
async fn gpke_abmeldung_spawns_the_nb_process() {
    assert!(
        reaches_arm(55004, "gpke-supplier-change").await,
        "PID 55004 (Abmeldung) must reach the NB spawn arm"
    );
}

/// Every PID the workflow accepts must be dispatchable to it.
///
/// `UTILMD_ANFRAGE_PIDS` is the subset the workflow can answer. Any PID in it
/// that the dispatcher drops is a message mako advertises support for and then
/// discards.
#[tokio::test]
async fn every_supplier_change_anfrage_pid_reaches_the_spawn_arm() {
    for &pid in mako_gpke::UTILMD_ANFRAGE_PIDS {
        // Some have no shipped fixture yet; the arm is constant-driven, so they
        // are covered by construction and the coverage guard picks them up as
        // soon as a fixture lands.
        let base = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/edi-energy/tests/fixtures/utilmd"
        );
        let has_fixture = [
            format!("{base}/valid/pid_{pid}.edi"),
            format!("{base}/gen/pid_{pid}.gen.edi"),
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists());
        if !has_fixture {
            continue;
        }
        assert!(
            reaches_arm(pid, "gpke-supplier-change").await,
            "PID {pid} is in UTILMD_ANFRAGE_PIDS but never reaches a dispatch arm"
        );
    }
}

/// The Antwortfrist runs from the receipt instant the door recorded, not from
/// when the message reaches its dispatch arm: GPKE Teil 1 Kap. 7
/// „Fristenberechnung" (S. 27) counts from the ÜT, the „Tag des Empfangs der
/// Übertragungsdatei". A 55001 received on Tuesday and dispatched months later keeps Tuesday's Frist —
/// 11:00 Uhr des 1. WT nach dem ÜT — instead of one counted from the dispatch
/// day.
#[tokio::test]
async fn the_antwortfrist_runs_from_the_receipt_instant() {
    use mako_engine::deadline::DeadlineStore as _;
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN_MP),
    );
    let edi = fixture(55001);
    let msg = edi_energy::parse(edi.as_bytes()).expect("fixture parses");
    // Tue 2026-07-07 12:00 CEST.
    let received_at = time::macros::datetime!(2026-07-07 10:00 UTC);
    let outcome = dispatcher
        .dispatch(&msg, "gpke-supplier-change", 55001, received_at)
        .await
        .expect("dispatch");
    assert!(
        matches!(outcome, IngestOutcome::Spawned { .. }),
        "the Anmeldung spawns: {outcome:?}"
    );
    let due = store
        .as_deadline_store()
        .due_now(100)
        .await
        .expect("read deadlines");
    let frist = due
        .deadlines
        .iter()
        .find(|d| d.label() == mako_gpke::GPKE_PROCESS_RESPONSE_LABEL)
        .expect("the Antwortfrist is registered");
    let expected = mako_fristen::antwort::antwort_deadline(55001, received_at)
        .expect("55001 has a published Antwortfrist");
    assert_eq!(
        frist.due_at(),
        expected,
        "the Frist counts from the receipt instant"
    );
}
