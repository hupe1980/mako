//! Ingest coverage for the IFTSTA status / Vollzugsmeldung reply PIDs.
//!
//! These IFTSTA replies were registered in the PID router but had no ingest arm,
//! so they fell through to `Skipped { pid_not_in_* }` — the status report was
//! dropped:
//!
//! - `gpke-supplier-change`: **21024–21028, 21033, 21035, 21045, 21047**
//!   (Vollzugs-/Statusmeldung → `ReceiveVollzugsmeldung`).
//! - `gpke-sperrung-lf`: **21039** (Auftragsstatus → `ReceiveIftsta`).
//! - `wim-device-change`: **21007, 21009–21018, 21029–21032** (status →
//!   `ReceiveIftsta`).
//!
//! The IFTSTA AHB profile carries the addressed location in a single **LOC**
//! segment (the minimal `.gen` fixtures omit it), so the reply resumes the
//! process by MaLo (GPKE) / MeLo (WiM device-change). With no process registered
//! the outcome is `Skipped { process_not_found }` — proving the PID reaches the
//! resume path (routed to the arm, adapter built the command, location extracted)
//! rather than being dropped at routing.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const LOCATION: &str = "51238696012";

/// A minimal IFTSTA for `pid` carrying the addressed location in a LOC segment.
fn iftsta(pid: u32) -> String {
    format!(
        "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+IFTSTA:D:18A:UN:2.1'\
BGM+Z03+000{pid}'\
DTM+137:20230101:102'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
LOC+172+{LOCATION}'\
STS+Z21+Z05'\
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

async fn assert_reaches_resume(workflow: &str, pid: u32) {
    let dispatcher = make_dispatcher().await;
    let msg = edi_energy::parse(iftsta(pid).as_bytes())
        .unwrap_or_else(|e| panic!("parse IFTSTA {pid} failed: {e:?}"));
    let outcome = dispatcher
        .dispatch(&msg, workflow, pid)
        .await
        .unwrap_or_else(|e| panic!("dispatch {pid} failed: {e:?}"));
    match outcome {
        IngestOutcome::Skipped { reason, .. } => assert_eq!(
            reason, "process_not_found",
            "{workflow} IFTSTA {pid} must reach the resume path, not be dropped at routing"
        ),
        other => panic!("unexpected outcome for {workflow} {pid}: {other:?}"),
    }
}

#[tokio::test]
async fn gpke_supplier_change_dispatches_vollzugsmeldung() {
    for pid in [21024u32, 21028, 21033] {
        assert_reaches_resume("gpke-supplier-change", pid).await;
    }
}

#[tokio::test]
async fn gpke_sperrung_lf_dispatches_iftsta_21039() {
    assert_reaches_resume("gpke-sperrung-lf", 21039).await;
}

#[tokio::test]
async fn wim_device_change_dispatches_iftsta_status() {
    for pid in [21009u32, 21015, 21032] {
        assert_reaches_resume("wim-device-change", pid).await;
    }
}
