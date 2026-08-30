//! Full order-reference correlation for the LOC-less ORDCHG 39000 Stornierung.
//!
//! The LF cancels a pending Sperrauftrag with ORDCHG 39000, which carries **no
//! LOC** — so it cannot correlate by MaLo. The Sperrauftrag spawn (ORDERS 17115)
//! indexes the process under its Belegnummer (message ref) via
//! `spawn_or_resume_keyed`; the Stornierung echoes that reference in `RFF+ON`, and
//! the ingest arm resumes the process via `extract_order_ref_from_msg`.
//!
//! This exercises the whole chain: spawn-under-order-ref → LOC-less reply →
//! order-ref extraction → resume. With the process registered under the ref the
//! ORDCHG resolves to `Dispatched`.

use std::sync::Arc;

use mako_engine::registry::ProcessRegistry as _;
use mako_engine::{
    ids::{ProcessIdentity, TenantId},
    process::Process,
    store_slatedb::SlateDbStore,
    types::{MaLo, MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004";
const LF_MP: &str = "4012345000023";
const ORDER_REF: &str = "ORD-17115-REF"; // Belegnummer echoed by the Stornierung's RFF+ON

/// A minimal ORDCHG 39000 Stornierung carrying `RFF+ON:<order_ref>` (no LOC).
fn ordchg_39000(order_ref: &str) -> String {
    format!(
        "UNB+UNOC:3+4012345000023:14+9900357000004:14+230101:0000+1'\
UNH+STORNO-1+ORDCHG:D:20B:UN:1.1'\
BGM+Z51+00039000'\
DTM+137:202301010000?+00:303'\
RFF+ON:{order_ref}'\
NAD+MS+4012345000023::293'\
NAD+MR+9900357000004::293'\
UNS+D'\
UNT+8+1'\
UNZ+1+1'"
    )
}

async fn make_env() -> (SlateDbStore, TenantId, EdifactIngestDispatcher) {
    let store = SlateDbStore::open_in_memory()
        .await
        .expect("in-memory store");
    let tenant = TenantId::from_party_id(OWN_MP);
    let dispatcher = EdifactIngestDispatcher::new(
        Arc::new(store.clone()),
        store.as_snapshot_store(),
        100,
        tenant,
    );
    (store, tenant, dispatcher)
}

#[tokio::test]
async fn gpke_sperrung_ordchg_39000_resumes_by_order_ref() {
    use mako_gpke::{GpkeSperrungWorkflow, SperrungCommand};

    let (store, tenant, dispatcher) = make_env().await;

    // Spawn the NB-side Sperrung process (as the ORDERS 17115 arm does) and index
    // it under the Sperrauftrag's Belegnummer.
    let workflow_id = WorkflowId::new("gpke-sperrung", "FV2025-10-01");
    let process = Process::<GpkeSperrungWorkflow, Arc<SlateDbStore>>::new(
        Arc::new(store.clone()),
        tenant,
        workflow_id.clone(),
    );
    let process_id = process.process_id();
    process
        .execute(SperrungCommand::ReceiveSperrung {
            pid: Pruefidentifikator::new(17115).unwrap(),
            sender: MarktpartnerCode::new(LF_MP),
            receiver: MarktpartnerCode::new(OWN_MP),
            location_id: MaLo::new("51238696012"),
            document_date: "20230101".into(),
            message_ref: MessageRef::new(ORDER_REF),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await
        .expect("spawn ReceiveSperrung");
    let identity = ProcessIdentity::new(process_id, tenant, workflow_id);
    store
        .as_process_registry()
        .register_correlated(tenant, ORDER_REF, process_id, identity)
        .await
        .expect("register under order ref");

    // The LF's LOC-less ORDCHG 39000 must resume that process by RFF+ON.
    let msg = edi_energy::parse(ordchg_39000(ORDER_REF).as_bytes()).expect("parse ORDCHG 39000");
    let outcome = dispatcher
        .dispatch(&msg, "gpke-sperrung", 39000)
        .await
        .expect("dispatch 39000");
    match outcome {
        IngestOutcome::Dispatched { workflow_name, .. } => {
            assert_eq!(workflow_name, "gpke-sperrung");
        }
        other => panic!("expected Dispatched (Stornierung resumed by order ref), got {other:?}"),
    }
}

#[tokio::test]
async fn geli_gas_sperrung_nb_ordchg_39000_resumes_by_order_ref() {
    use mako_geli_gas::{GasSperrungNbCommand, GeliGasSperrungNbWorkflow};

    let (store, tenant, dispatcher) = make_env().await;

    let workflow_id = WorkflowId::new("geli-gas-sperrung-nb", "FV2025-10-01");
    let process = Process::<GeliGasSperrungNbWorkflow, Arc<SlateDbStore>>::new(
        Arc::new(store.clone()),
        tenant,
        workflow_id.clone(),
    );
    let process_id = process.process_id();
    process
        .execute(GasSperrungNbCommand::ReceiveSperrung {
            pid: Pruefidentifikator::new(17115).unwrap(),
            sender: MarktpartnerCode::new(LF_MP),
            receiver: MarktpartnerCode::new(OWN_MP),
            location_id: MaLo::new("51238696012"),
            document_date: "20230101".into(),
            message_ref: MessageRef::new(ORDER_REF),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await
        .expect("spawn ReceiveSperrung (Gas)");
    let identity = ProcessIdentity::new(process_id, tenant, workflow_id);
    store
        .as_process_registry()
        .register_correlated(tenant, ORDER_REF, process_id, identity)
        .await
        .expect("register under order ref");

    let msg = edi_energy::parse(ordchg_39000(ORDER_REF).as_bytes()).expect("parse ORDCHG 39000");
    let outcome = dispatcher
        .dispatch(&msg, "geli-gas-sperrung-nb", 39000)
        .await
        .expect("dispatch 39000");
    match outcome {
        IngestOutcome::Dispatched { workflow_name, .. } => {
            assert_eq!(workflow_name, "geli-gas-sperrung-nb");
        }
        other => {
            panic!("expected Dispatched (Gas Stornierung resumed by order ref), got {other:?}")
        }
    }
}
