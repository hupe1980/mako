//! Ingest coverage for the Sperrung Storno/response reply PIDs.
//!
//! These ORDRSP replies were registered in the PID router but had no ingest
//! dispatch arm, so they fell through to `Skipped { pid_not_in_dispatch_table }`
//! — silently dropping the counterparty's answer:
//!
//! - `gpke-sperrung-lf` / `geli-gas-sperrung-lf`: **19128/19129** (ORDRSP answering
//!   the LF's Stornierung, ORDCHG 39000).
//! - `geli-gas-sperrung-nb`: **19118/19119** (ORDRSP gMSB answer to the forwarded
//!   Anfrage Sperrung).
//!
//! Each now has an arm that resumes the process by MaLo (the ORDRSP carries it in
//! LOC, exactly like the sibling 19116/19117 arms that already shipped). With no
//! process registered the outcome is `Skipped { process_not_found }` — proving the
//! PID reaches the resume path (adapter built the command, MaLo looked up) rather
//! than being dropped at routing.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const MALO: &str = "51238696012";

/// A minimal ORDRSP for `pid` carrying the MaLo in a LOC segment, so
/// `extract_malo_from_msg` resolves a non-empty correlation key.
fn ordrsp(pid: u32) -> String {
    format!(
        "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+ORDRSP:D:10A:UN:1.4c'\
BGM+7+000{pid}'\
DTM+137:202301010000?+00:303'\
RFF+Z13:{pid}'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
LOC+172+{MALO}'\
UNT+8+1'\
UNZ+1+1'"
    )
}

async fn make_dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(OWN_MP);
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    )
}

/// Dispatch `pid` (as an ORDRSP) to `workflow` and assert it reaches the resume
/// path — i.e. it is NOT dropped with `pid_not_in_dispatch_table` /
/// `pid_not_in_resume_table`. With no registered process the expected outcome is
/// `Skipped { process_not_found }`.
async fn assert_reaches_resume(workflow: &str, pid: u32) {
    let dispatcher = make_dispatcher().await;
    let msg = edi_energy::parse(ordrsp(pid).as_bytes())
        .unwrap_or_else(|e| panic!("parse ORDRSP {pid} failed: {e:?}"));
    let outcome = dispatcher
        .dispatch(&msg, workflow, pid)
        .await
        .unwrap_or_else(|e| panic!("dispatch {pid} failed: {e:?}"));
    match outcome {
        IngestOutcome::Skipped { reason, .. } => assert_eq!(
            reason, "process_not_found",
            "{workflow} PID {pid} must reach the resume path (no open process), \
             not be dropped at routing"
        ),
        // Dispatched/Spawned would also mean "reached the arm"; only the
        // pid_not_in_* skips are the bug.
        other => panic!("unexpected outcome for {workflow} {pid}: {other:?}"),
    }
}

#[tokio::test]
async fn gpke_sperrung_lf_dispatches_storno_ordrsp_19128_19129() {
    assert_reaches_resume("gpke-sperrung-lf", 19128).await;
    assert_reaches_resume("gpke-sperrung-lf", 19129).await;
}

#[tokio::test]
async fn geli_gas_sperrung_lf_dispatches_storno_ordrsp_19128_19129() {
    assert_reaches_resume("geli-gas-sperrung-lf", 19128).await;
    assert_reaches_resume("geli-gas-sperrung-lf", 19129).await;
}

#[tokio::test]
async fn geli_gas_sperrung_nb_dispatches_msb_ordrsp_19118_19119() {
    assert_reaches_resume("geli-gas-sperrung-nb", 19118).await;
    assert_reaches_resume("geli-gas-sperrung-nb", 19119).await;
}
