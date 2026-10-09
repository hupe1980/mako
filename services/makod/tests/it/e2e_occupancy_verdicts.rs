//! On the ingest path a settled process does not block its business key, and a
//! live one does.
//!
//! The correlation index is append-only, so an entry proves a process once
//! existed and never that one still runs. Resuming on the entry alone feeds a
//! second initiating message into a finished process, whose initiating command
//! refuses anything outside its initial state: the message is dead-lettered and
//! no process can ever spawn for that key again. Each arm therefore asks the
//! verdict its workflow's crate publishes (`OccupiesBusinessKey`).

use std::sync::Arc;

use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN: &str = "9900001000001";

fn fixture(mt: edi_energy::MessageType, pid: u32) -> edi_energy::AnyMessage {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .profiles_for(mt)
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .expect("a profile carries the PID");
    let af = profile.anwendungsfall(pid).expect("carried");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: "4012345000023".to_owned(),
                receiver: OWN.to_owned(),
            },
        )
        .expect("skeleton");
    edi_energy::parse(&bytes).expect("the skeleton parses")
}

async fn dispatcher() -> EdifactIngestDispatcher {
    let store = SlateDbStore::open_in_memory().await.expect("store");
    EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        TenantId::from_party_id(OWN),
    )
}

/// Two messages of `pid` for one business key: the first settles its process
/// on receipt, so the second must open a new one.
async fn second_spawns(mt: edi_energy::MessageType, pid: u32, workflow: &str) {
    let d = dispatcher().await;
    let msg = fixture(mt, pid);
    let first = d.dispatch(&msg, workflow, pid).await.expect("first");
    let IngestOutcome::Spawned { process_id: a, .. } = first else {
        panic!("the first {pid} spawns: {first:?}");
    };
    let second = d.dispatch(&msg, workflow, pid).await;
    match second {
        Ok(IngestOutcome::Spawned { process_id: b, .. }) => assert_ne!(a, b),
        other => panic!(
            "a second {pid} for a settled {workflow} process must spawn a new one, got {other:?}"
        ),
    }
}

/// MaBiS Clearingliste: a received list is settled once validated.
#[tokio::test]
async fn a_settled_clearingliste_does_not_block_the_next_one() {
    second_spawns(
        edi_energy::MessageType::Utilmd,
        55067,
        "mabis-clearingliste",
    )
    .await;
}

/// WiM Rechnungsabwicklung: a recorded Bestellung (ORDERS 17005) is terminal.
#[tokio::test]
async fn a_recorded_rechnungsabwicklung_does_not_block_the_next_one() {
    second_spawns(
        edi_energy::MessageType::Orders,
        17005,
        "wim-rechnungsabwicklung",
    )
    .await;
}

/// GPKE EoG: a Zuordnung being answered still holds the MaLo, so a second one
/// is not a new process.
#[tokio::test]
async fn a_live_eog_zuordnung_still_blocks_the_malo() {
    let d = dispatcher().await;
    let msg = fixture(edi_energy::MessageType::Utilmd, 55013);
    let first = d.dispatch(&msg, "gpke-eog", 55013).await.expect("first");
    assert!(matches!(first, IngestOutcome::Spawned { .. }), "{first:?}");
    let second = d.dispatch(&msg, "gpke-eog", 55013).await;
    assert!(
        !matches!(second, Ok(IngestOutcome::Spawned { .. })),
        "a live EoG Zuordnung must not be duplicated: {second:?}"
    );
}
