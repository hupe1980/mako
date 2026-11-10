//! A second Anmeldung on a MaLo whose first was answered reaches a process of
//! its own and is answered.
//!
//! `gpke-supplier-change` and `gpke-lf-abmeldung` key their processes on the
//! MaLo, and the correlation index is append-only. Resuming on presence feeds
//! the next Anmeldung — or the Lieferende 55004 that reuses the workflow name
//! on the same MaLo — into the answered process, whose `ReceiveUtilmd` refuses
//! anything but `New`, and the message dead-letters. The NB's answer is the
//! last step of the Vorgang (BK6-24-174 GPKE Teil 2 Kap. 2.1.2 Prozessschritt
//! 5/6, S. 21–22; Kap. 2.5.1.2 Nr. 2/3, S. 55), and the LF's answer to the
//! Ankündigung 55007 is the last step of its process (Kap. 2.5.2.2 Nr. 2,
//! S. 61): after it the MaLo is free.

use std::sync::Arc;

use mako_engine::ids::{ProcessId, TenantId};
use mako_engine::process::{Process, RunOptions};
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::store_slatedb::SlateDbStore;
use mako_gpke::{
    GpkeLfAbmeldungWorkflow, GpkeSupplierChangeWorkflow, LfAbmeldungCommand, LfAbmeldungState,
    SupplierChangeCommand, SupplierChangeState,
};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const NB_MP: &str = "9900357000004";
const LF_MP: &str = "9900000000003";
const MALO: &str = "51238696012";

/// The newest profile's skeleton of `pid` from `sender` to `receiver`, naming
/// [`MALO`].
fn message(pid: u32, sender: &str, receiver: &str) -> edi_energy::AnyMessage {
    use edi_energy::profile::SkeletonParties;
    let code = edi_energy::Pruefidentifikator::new(pid).expect("five digits");
    let profile = edi_energy::ReleaseRegistry::global()
        .all_profiles()
        .iter()
        .filter(|p| p.has_anwendungsfall(code))
        .max_by_key(|p| p.valid_from())
        .unwrap_or_else(|| panic!("no profile carries {pid}"));
    let af = profile.anwendungsfall(pid).expect("the column");
    let bytes = profile
        .skeleton_interchange(
            af,
            &SkeletonParties {
                sender: sender.to_owned(),
                receiver: receiver.to_owned(),
            },
        )
        .expect("skeleton");
    let wire = String::from_utf8_lossy(&bytes).into_owned();
    let prefix = "LOC+Z16+";
    let at = wire
        .find(prefix)
        .expect("the message names a Marktlokation")
        + prefix.len();
    let end = at
        + wire[at..]
            .find(['\'', ':', '+'])
            .expect("LOC is terminated");
    let wire = format!("{}{MALO}{}", &wire[..at], &wire[end..]);
    edi_energy::parse(wire.as_bytes()).unwrap_or_else(|e| panic!("{pid} parses: {e}"))
}

async fn dispatcher(own: &str) -> (Arc<SlateDbStore>, EdifactIngestDispatcher, TenantId) {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant = TenantId::from_party_id(own);
    let dispatcher =
        EdifactIngestDispatcher::new(Arc::clone(&store), store.as_snapshot_store(), 100, tenant);
    (store, dispatcher, tenant)
}

async fn ingest(
    dispatcher: &EdifactIngestDispatcher,
    workflow: &str,
    pid: u32,
    sender: &str,
    receiver: &str,
) -> IngestOutcome {
    dispatcher
        .dispatch(
            &message(pid, sender, receiver),
            workflow,
            pid,
            time::OffsetDateTime::now_utc(),
        )
        .await
        .expect("ingest must not fail")
}

fn spawned(outcome: IngestOutcome) -> ProcessId {
    match outcome {
        IngestOutcome::Spawned { process_id, .. } => process_id,
        other => panic!("expected a new process, got {other:?}"),
    }
}

async fn process<W: mako_engine::workflow::Workflow>(
    store: &Arc<SlateDbStore>,
    tenant: TenantId,
    id: ProcessId,
) -> Process<W, Arc<SlateDbStore>> {
    let identity = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO)
        .await
        .expect("lookup")
        .into_iter()
        .find(|i| i.process_id == id)
        .expect("the live process is indexed under the MaLo");
    Process::from_identity(Arc::clone(store), identity)
}

/// The NB's answer to the Anmeldung, an Ablehnung out of `E_0622`.
fn ablehnung() -> SupplierChangeCommand {
    SupplierChangeCommand::SendAntwort {
        antwort: mako_gpke::LfAntwort::ablehnung("A07", "E_0622"),
        obligations: vec![],
        lfa_lieferende: None,
    }
}

#[tokio::test]
async fn a_second_anmeldung_after_an_answered_one_spawns_and_is_answered() {
    let (store, dispatcher, tenant) = dispatcher(NB_MP).await;
    let wf = "gpke-supplier-change";

    let first = spawned(ingest(&dispatcher, wf, 55_001, LF_MP, NB_MP).await);
    process::<GpkeSupplierChangeWorkflow>(&store, tenant, first)
        .await
        .run(ablehnung(), RunOptions::new())
        .await
        .expect("the NB answers the first Anmeldung");

    let second = spawned(ingest(&dispatcher, wf, 55_001, LF_MP, NB_MP).await);
    assert_ne!(first, second);
    let p = process::<GpkeSupplierChangeWorkflow>(&store, tenant, second).await;
    p.run(ablehnung(), RunOptions::new())
        .await
        .expect("the second Anmeldung is answered");
    assert!(matches!(
        p.state().await.expect("state"),
        SupplierChangeState::Rejected { .. }
    ));
}

/// While the first Anmeldung is still owed its answer the MaLo is held: a
/// second one does not open a process beside it, so the NB's answer by MaLo
/// still resolves to exactly one.
#[tokio::test]
async fn an_unanswered_anmeldung_still_holds_the_malo() {
    let (store, dispatcher, tenant) = dispatcher(NB_MP).await;
    let wf = "gpke-supplier-change";
    let first = spawned(ingest(&dispatcher, wf, 55_001, LF_MP, NB_MP).await);
    let again = dispatcher
        .dispatch(
            &message(55_001, LF_MP, NB_MP),
            wf,
            55_001,
            time::OffsetDateTime::now_utc(),
        )
        .await;
    assert!(
        !matches!(again, Ok(IngestOutcome::Spawned { .. })),
        "an open Anmeldung must not be paralleled: {again:?}"
    );
    let live: Vec<ProcessId> = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO)
        .await
        .expect("lookup")
        .into_iter()
        .filter(|i| i.workflow_id.name.as_ref() == wf)
        .map(|i| i.process_id)
        .collect();
    assert_eq!(live, vec![first]);
}

/// The Lieferende 55004 that follows an answered Anmeldung on the same MaLo
/// runs in its own process rather than in the Anmeldung's.
#[tokio::test]
async fn a_later_abmeldung_is_told_apart_from_the_answered_anmeldung() {
    let (store, dispatcher, tenant) = dispatcher(NB_MP).await;
    let wf = "gpke-supplier-change";
    let anmeldung = spawned(ingest(&dispatcher, wf, 55_001, LF_MP, NB_MP).await);
    process::<GpkeSupplierChangeWorkflow>(&store, tenant, anmeldung)
        .await
        .run(ablehnung(), RunOptions::new())
        .await
        .expect("answered");
    let abmeldung = spawned(ingest(&dispatcher, wf, 55_004, LF_MP, NB_MP).await);
    assert_ne!(anmeldung, abmeldung);
}

/// The LF's side of the NB-initiated Lieferende: a second Ankündigung 55007 on
/// a MaLo whose first was answered opens its own process and is answered.
#[tokio::test]
async fn a_second_ankuendigung_after_an_answered_one_spawns_and_is_answered() {
    let (store, dispatcher, tenant) = dispatcher(LF_MP).await;
    let wf = "gpke-lf-abmeldung";
    let antwort = || LfAbmeldungCommand::SendAntwort {
        antwort: mako_gpke::LfAntwort::zustimmung("A10", "E_0609"),
    };

    let first = spawned(ingest(&dispatcher, wf, 55_007, NB_MP, LF_MP).await);
    process::<GpkeLfAbmeldungWorkflow>(&store, tenant, first)
        .await
        .run(antwort(), RunOptions::new())
        .await
        .expect("the LF answers the first Ankündigung");

    let second = spawned(ingest(&dispatcher, wf, 55_007, NB_MP, LF_MP).await);
    assert_ne!(first, second);
    let p = process::<GpkeLfAbmeldungWorkflow>(&store, tenant, second).await;
    p.run(antwort(), RunOptions::new())
        .await
        .expect("the second Ankündigung is answered");
    assert!(matches!(
        p.state().await.expect("state"),
        LfAbmeldungState::AntwortGesendet { .. }
    ));
}
