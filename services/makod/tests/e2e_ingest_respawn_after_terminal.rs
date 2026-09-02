//! A settled process must not block its business key on the ingest path.
//!
//! The ingest dispatcher decided resume-vs-spawn on the mere *presence* of a
//! correlation-index entry. That index is append-only — `register_correlated`
//! writes on spawn and nothing removes the entry when the process ends — so the
//! second Sperrauftrag for a MaLo was fed into the *finished* process. Every
//! initiating command rejects outside its initial state (`ReceiveSperrung`
//! requires `New`), so the dispatch failed with `invalid_state` and no process
//! could ever spawn for that MaLo again.
//!
//! This is the same defect the commands-API duplicate guard fixed (see
//! `e2e_anmeldung_after_rejection`), on the other side of the same index: the
//! matched process is now rehydrated and asked whether it still occupies the
//! key, and a finished one has its entry retired.

use std::sync::Arc;

use mako_engine::{
    ids::{ProcessId, TenantId},
    process::Process,
    registry::ProcessRegistry as _,
    store_slatedb::SlateDbStore,
};
use mako_gpke::{GpkeSperrungWorkflow, SperrungCommand};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const NB_MP_ID: &str = "9900357000004";
const LF_MP_ID: &str = "4012345000023";
const MALO: &str = "51238696012";

/// ORDERS 17115 Sperrauftrag (LF → NB) carrying the MaLo in LOC.
///
/// `control_ref` varies the interchange/message reference so the second
/// Sperrauftrag is a distinct document, as it would be on the wire.
fn orders_17115(control_ref: &str) -> String {
    format!(
        "UNB+UNOC:3+{LF_MP_ID}:14+{NB_MP_ID}:14+250115:0800+{control_ref}'\
UNH+{control_ref}+ORDERS:D:09B:UN:1.4b'\
BGM+Z55+00017115+9'\
DTM+137:202501150800?+00:303'\
RFF+Z13:{control_ref}'\
NAD+MS+{LF_MP_ID}::293'\
NAD+MR+{NB_MP_ID}::293'\
LOC+7+{MALO}::Z13'\
LIN+1'\
UNT+9+{control_ref}'\
UNZ+1+{control_ref}'"
    )
}

async fn ingest(dispatcher: &EdifactIngestDispatcher, control_ref: &str) -> IngestOutcome {
    let wire = orders_17115(control_ref);
    let msg = edi_energy::parse(wire.as_bytes()).expect("ORDERS 17115 fixture parses");
    dispatcher
        .dispatch(&msg, "gpke-sperrung", 17115)
        .await
        .expect("ingest must not fail")
}

/// Processes indexed under the MaLo for `gpke-sperrung`, in index order.
async fn indexed(store: &Arc<SlateDbStore>, tenant: TenantId) -> Vec<ProcessId> {
    store
        .as_process_registry()
        .lookup_correlated(tenant, MALO)
        .await
        .expect("correlation lookup")
        .into_iter()
        .filter(|id| id.workflow_id.name.as_ref() == "gpke-sperrung")
        .map(|id| id.process_id)
        .collect()
}

/// Replayed state of the single process indexed under the MaLo.
async fn state_of(
    store: &Arc<SlateDbStore>,
    tenant: TenantId,
    process_id: ProcessId,
) -> mako_gpke::SperrungState {
    let identity = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO)
        .await
        .expect("correlation lookup")
        .into_iter()
        .find(|id| id.process_id == process_id)
        .expect("process is in the correlation index");
    Process::<GpkeSperrungWorkflow, Arc<SlateDbStore>>::from_identity(Arc::clone(store), identity)
        .state()
        .await
        .expect("state replay")
}

#[tokio::test]
async fn a_settled_process_does_not_block_the_malo_on_the_ingest_path() {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant = TenantId::from_party_id(NB_MP_ID);
    let dispatcher =
        EdifactIngestDispatcher::new(Arc::clone(&store), store.as_snapshot_store(), 100, tenant);

    // ── First Sperrauftrag ────────────────────────────────────────────────────
    let IngestOutcome::Spawned {
        process_id: first, ..
    } = ingest(&dispatcher, "SPERR-001").await
    else {
        panic!("the first Sperrauftrag must spawn a process");
    };

    // Drive it to a terminal state, as the NB's field service does after
    // executing the disconnection.
    let identity = store
        .as_process_registry()
        .lookup_correlated(tenant, MALO)
        .await
        .expect("correlation lookup")
        .into_iter()
        .find(|id| id.process_id == first)
        .expect("the first Sperrauftrag is in the correlation index");
    let process = Process::<GpkeSperrungWorkflow, Arc<SlateDbStore>>::from_identity(
        Arc::clone(&store),
        identity,
    );
    if !process.state().await.expect("state replay").is_terminal() {
        process
            .execute(SperrungCommand::BestaetigueSperrung {
                durchgefuehrt: true,
                reason: None,
            })
            .await
            .expect("execution confirmation is a legal transition");
    }
    assert!(
        state_of(&store, tenant, first).await.is_terminal(),
        "the first process must be settled before the second Sperrauftrag arrives"
    );

    // ── Second Sperrauftrag for the same MaLo ─────────────────────────────────
    // It has to spawn a fresh process. Routed onto the settled one it would hit
    // `ReceiveSperrung`, which the aggregate rejects outside `New` — and the
    // MaLo could never be locked or unlocked again for the lifetime of the
    // store.
    let outcome = ingest(&dispatcher, "SPERR-002").await;
    let IngestOutcome::Spawned {
        process_id: second, ..
    } = outcome
    else {
        panic!("the second Sperrauftrag must spawn a fresh process, got {outcome:?}");
    };
    assert_ne!(
        first, second,
        "the second Sperrauftrag must not reuse the settled process"
    );

    // The prune is load-bearing: `resume_by_key` takes the first indexed match,
    // so the settled process must be gone before the replacement is registered
    // or every ORDRSP/ORDCHG answer would still land on the dead one.
    assert_eq!(
        indexed(&store, tenant).await,
        vec![second],
        "only the live Sperrung may remain indexed under the MaLo"
    );
}

/// The other half of the guard: a process that is still running keeps its key.
///
/// Two live Sperrung processes on one MaLo is the outcome the market cannot
/// untangle, so a repeat Sperrauftrag must still land on the existing process —
/// which decides for itself whether the command is legal — and the correlation
/// entry must survive.
///
/// The live process is seeded directly: the wire fixture does not satisfy the
/// full ORDERS 1.4b profile, so ingesting it lands in `Rejected` (terminal).
#[tokio::test]
async fn a_running_process_still_owns_the_malo() {
    use mako_engine::{
        ids::ProcessIdentity,
        types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
        version::WorkflowId,
    };

    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let tenant = TenantId::from_party_id(NB_MP_ID);
    let dispatcher =
        EdifactIngestDispatcher::new(Arc::clone(&store), store.as_snapshot_store(), 100, tenant);

    let live = Process::<GpkeSperrungWorkflow, Arc<SlateDbStore>>::new(
        Arc::clone(&store),
        tenant,
        WorkflowId::new("gpke-sperrung", "FV2025-10-01"),
    );
    live.execute(SperrungCommand::ReceiveSperrung {
        pid: Pruefidentifikator::new(17115).expect("valid PID"),
        sender: MarktpartnerCode::new(LF_MP_ID),
        receiver: MarktpartnerCode::new(NB_MP_ID),
        location_id: MaLo::new(MALO),
        document_date: "20250115".to_owned(),
        message_ref: MessageRef::new("SPERR-000"),
        validation_passed: true,
        validation_errors: Vec::new(),
    })
    .await
    .expect("the Sperrauftrag is a legal transition from New");
    let live_id = live.process_id();
    store
        .as_process_registry()
        .register_correlated(
            tenant,
            MALO,
            live_id,
            ProcessIdentity::new(
                live_id,
                tenant,
                WorkflowId::new("gpke-sperrung", "FV2025-10-01"),
            ),
        )
        .await
        .expect("register the live process under the MaLo");
    assert!(!state_of(&store, tenant, live_id).await.is_terminal());

    let wire = orders_17115("SPERR-002");
    let msg = edi_energy::parse(wire.as_bytes()).expect("fixture parses");
    let repeat = dispatcher.dispatch(&msg, "gpke-sperrung", 17115).await;
    assert!(
        repeat.is_err() || matches!(repeat, Ok(IngestOutcome::Dispatched { .. })),
        "a repeat Sperrauftrag must reach the running process, not spawn a second one: {repeat:?}"
    );
    assert_eq!(
        indexed(&store, tenant).await,
        vec![live_id],
        "the running process must keep its correlation entry"
    );
}
