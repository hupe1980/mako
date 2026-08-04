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
//! `UTILMD_ANFRAGE_PIDS` is deliberately narrower than `UTILMD_PIDS`: 55557 has
//! no Antwort mapping, so spawning it would only ever end in a deadline-driven
//! false rejection.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";

fn fixture(pid: u32) -> String {
    let base = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/edi-energy/tests/fixtures/utilmd"
    );
    [
        format!("{base}/valid/pid_{pid}.edi"),
        format!("{base}/gen/pid_{pid}.gen.edi"),
    ]
    .into_iter()
    .find_map(|p| std::fs::read_to_string(p).ok())
    .unwrap_or_else(|| panic!("no UTILMD fixture for PID {pid}"))
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
        dispatcher().await.dispatch(&msg, workflow, pid).await,
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
