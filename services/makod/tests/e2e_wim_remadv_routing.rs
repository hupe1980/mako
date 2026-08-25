//! Conversation-ID routing for shared reply PIDs (REMADV 33001–33004 / COMDIS).
//!
//! REMADV PIDs are legitimately claimed by **both** Strom billing families
//! (`gpke-abrechnung` and `wim-invoic`), so the static PID router resolves them
//! by last-write-wins and `resolve_workflow`'s MP-ID→Sparte narrowing cannot tell
//! the two Strom families apart. `EdifactIngestDispatcher::dispatch` therefore runs
//! a correlation step (`correlation_route`) that re-routes the reply to the family
//! actually holding an open process for the referenced invoice (RFF+Z13).
//!
//! These tests prove:
//! 1. a REMADV for a WiM MSB-Rechnung (31009) resumes the `wim-invoic` process
//!    even when dispatched with the *wrong* statically-resolved `gpke-abrechnung`;
//! 2. an orphan REMADV (no correlated process) is still `Skipped` — the override
//!    never invents a route or mis-books.

use std::sync::Arc;

use edi_energy::AnyMessage;
use mako_engine::registry::ProcessRegistry as _;
use mako_engine::{
    ids::{ProcessIdentity, TenantId},
    process::Process,
    store_slatedb::SlateDbStore,
    types::{MarktpartnerCode, MessageRef, Pruefidentifikator},
    version::WorkflowId,
};
use mako_invoic::InvoicCommand;
use mako_wim::invoic::WimInvoicWorkflow;
use makod::ingest_dispatcher::{EdifactIngestDispatcher, IngestOutcome};

const OWN_MP: &str = "9900357000004"; // UNB recipient (our own party) in the fixture
const INVOICE_REF: &str = "REF001"; // must match the REMADV's RFF+Z13 back-reference

/// Minimal REMADV 33001 (Bestätigung) carrying `RFF+Z13:REF001` — the message-ref
/// of the original 31009 INVOIC the billing process was registered under.
const REMADV_33001: &str = "UNB+UNOC:3+4012345000023:14+9900357000004:14+250101:0000+1'\
UNH+1+REMADV:D:05A:UN:2.9f'\
BGM+239+00033001'\
DTM+137:20250101:102'\
RFF+Z13:REF001'\
NAD+MS+4012345000023::293'\
CTA+IC+:Abrechnungsstelle'\
COM+030 12345678:TE'\
CUX+2:EUR:9'\
DOC+Z41+INV-2025-001'\
MOA+9:100.00:EUR'\
UNS+D'\
MOA+9:100.00:EUR'\
AJT+Z01'\
UNT+14+1'\
UNZ+1+1'";

/// Build an in-memory dispatcher for the own-party tenant.
async fn make_dispatcher() -> (SlateDbStore, TenantId, EdifactIngestDispatcher) {
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

/// Spawn a `wim-invoic` process (MSB sent the 31009 INVOIC) and register it under
/// the invoice reference — exactly how the outbound `SendInvoic` command indexes it
/// for REMADV correlation.
async fn spawn_wim_invoic(store: &SlateDbStore, tenant: TenantId, invoice_ref: &str) {
    let workflow_id = WorkflowId::new("wim-invoic", "FV2025-10-01");
    let process = Process::<WimInvoicWorkflow, Arc<SlateDbStore>>::new(
        Arc::new(store.clone()),
        tenant,
        workflow_id.clone(),
    );
    let process_id = process.process_id();
    process
        .execute(InvoicCommand::SendInvoic {
            pid: Pruefidentifikator::new(31009).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"), // MSB invoicer
            recipient: MarktpartnerCode::new(OWN_MP),       // NB/LF/ESA payer
            document_date: "20250101".into(),
            invoice_ref: MessageRef::new(invoice_ref),
        })
        .await
        .expect("SendInvoic");
    let identity = ProcessIdentity::new(process_id, tenant, workflow_id);
    store
        .as_process_registry()
        .register_correlated(tenant, invoice_ref, process_id, identity)
        .await
        .expect("register_correlated");
}

#[tokio::test]
async fn remadv_routes_to_wim_invoic_despite_wrong_static_resolution() {
    let (store, tenant, dispatcher) = make_dispatcher().await;
    spawn_wim_invoic(&store, tenant, INVOICE_REF).await;

    let msg = edi_energy::parse(REMADV_33001.as_bytes()).expect("parse REMADV 33001");
    assert!(matches!(msg, AnyMessage::Remadv(_)), "fixture is a REMADV");

    // Dispatch with the WRONG statically-resolved family (last-write-wins could
    // have picked gpke-abrechnung for the Strom-shared REMADV PID).
    let outcome = dispatcher
        .dispatch(&msg, "gpke-abrechnung", 33001)
        .await
        .expect("dispatch");

    match outcome {
        IngestOutcome::Dispatched { workflow_name, .. } => assert_eq!(
            workflow_name, "wim-invoic",
            "correlation override must route the REMADV to the family owning the 31009 invoice"
        ),
        other => panic!("expected Dispatched to wim-invoic, got {other:?}"),
    }
}

#[tokio::test]
async fn orphan_remadv_is_skipped_not_misrouted() {
    let (_store, _tenant, dispatcher) = make_dispatcher().await;
    // No process registered under REF001 → no correlated owner.

    let msg = edi_energy::parse(REMADV_33001.as_bytes()).expect("parse REMADV 33001");
    let outcome = dispatcher
        .dispatch(&msg, "gpke-abrechnung", 33001)
        .await
        .expect("dispatch");

    // The static route stands (gpke-abrechnung); with no open GPKE process the
    // REMADV is Skipped — never spawned into the wrong family, never mis-booked.
    assert!(
        matches!(outcome, IngestOutcome::Skipped { .. }),
        "an orphan REMADV must be Skipped, got {outcome:?}"
    );
}
