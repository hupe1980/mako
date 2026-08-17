//! Ingest coverage for the WiM Preisanfrage QUOTES Angebot reply PIDs.
//!
//! REQOTE 35001–35005 (Preisanfrage) opened the `wim-preisanfrage` process, but
//! the MSB's answering QUOTES 15001–15005 (Angebot) had no ingest arm and fell
//! through to `Skipped { pid_not_in_dispatch_table }` — the Angebot was dropped.
//!
//! QUOTES carries the MaLo in LOC (like REQOTE), so the Angebot now resumes the
//! process by MaLo. With no process registered the outcome is
//! `Skipped { process_not_found }` — proving the PID reaches the resume path
//! rather than being dropped at routing.

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const MALO: &str = "51238696012";

/// A minimal QUOTES Angebot for `pid` carrying the MaLo in a LOC segment.
fn quotes(pid: u32) -> String {
    format!(
        "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+1+QUOTES:D:10A:UN:1.3c'\
BGM+310+000{pid}'\
DTM+137:20230101:102'\
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

#[tokio::test]
async fn wim_preisanfrage_dispatches_quotes_angebot() {
    // 15003 is shared with the ESA Wertebestellung path; the other four are
    // Preisanfrage-only. All must reach the resume path when routed to
    // `wim-preisanfrage`.
    for pid in [15001u32, 15002, 15004, 15005] {
        let dispatcher = make_dispatcher().await;
        let msg = edi_energy::parse(quotes(pid).as_bytes())
            .unwrap_or_else(|e| panic!("parse QUOTES {pid} failed: {e:?}"));
        let outcome = dispatcher
            .dispatch(&msg, "wim-preisanfrage", pid)
            .await
            .unwrap_or_else(|e| panic!("dispatch {pid} failed: {e:?}"));
        match outcome {
            IngestOutcome::Skipped { reason, .. } => assert_eq!(
                reason, "process_not_found",
                "QUOTES {pid} must reach the resume path, not be dropped at routing"
            ),
            other => panic!("unexpected outcome for QUOTES {pid}: {other:?}"),
        }
    }
}
