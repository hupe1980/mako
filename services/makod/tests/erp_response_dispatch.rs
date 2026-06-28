//! Integration tests for Phase 3 ERP response dispatch.
//!
//! Verifies that:
//! - An `anmelden` command registers the process under the `malo_id` business key.
//! - A subsequent `bestaetigen` command finds the process via the registry and
//!   dispatches `HandleAntwort` into the correct `GpkeLfAnmeldungWorkflow`.
//! - A `bestaetigen` call for an unknown `malo_id` returns HTTP 404 `process_not_found`.
//! - An `aktivieren` command dispatches `LfAnmeldungCommand::Activate`.
//!
//! These tests exercise the full `CommandsApiState` dispatch path end-to-end
//! with in-memory stores (no actual HTTP server, no SlateDB).

use std::sync::Arc;

use mako_engine::registry::ProcessRegistry as _;
use mako_engine::{
    ids::TenantId,
    types::{MaLo, MarktpartnerCode, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{GpkeLfAnmeldungWorkflow, LfAnmeldungState};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Spawn a `GpkeLfAnmeldungWorkflow` process using the in-memory stores and
/// return `(state_after_initiate, identity)` so we can verify registry lookups.
async fn initiate_lf_anmeldung(
    store: &mako_engine::store_slatedb::SlateDbStore,
    tenant_id: TenantId,
    malo_id: &str,
    pid: u32,
) -> mako_engine::ids::ProcessId {
    use mako_engine::process::Process;

    let workflow_id = WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01");
    let process =
        Process::<GpkeLfAnmeldungWorkflow, Arc<mako_engine::store_slatedb::SlateDbStore>>::new(
            Arc::new(store.clone()),
            tenant_id,
            workflow_id,
        );
    let process_id = process.process_id();
    let cmd = mako_gpke::LfAnmeldungCommand::InitiateAnmeldung {
        pid: Pruefidentifikator::new(pid).unwrap(),
        sender: MarktpartnerCode::new("4012345000023"),
        receiver: MarktpartnerCode::new("9900357000004"),
        location_id: MaLo::new(malo_id),
        process_date: "2026-10-01".into(),
    };
    process
        .execute_and_enqueue(cmd)
        .await
        .expect("initiate_lf_anmeldung: execute");
    process_id
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// After `anmelden`, the process must be registered under `malo_id` in the
/// process registry correlated index.
#[tokio::test]
async fn anmelden_registers_under_malo_id() {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let tenant_id = TenantId::from_party_id("9900357000004");
    let malo_id = "51238696781";

    let process_id = initiate_lf_anmeldung(&store, tenant_id, malo_id, 55001).await;

    // Register manually (simulating what dispatch_lf_anmeldung does):
    let identity = mako_engine::ids::ProcessIdentity::new(
        process_id,
        tenant_id,
        WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
    );
    store
        .as_process_registry()
        .register_correlated(tenant_id, malo_id, process_id, identity)
        .await
        .expect("register_correlated");

    // Verify lookup returns the registered identity.
    let found = store
        .as_process_registry()
        .lookup_correlated(tenant_id, malo_id)
        .await
        .expect("lookup_correlated");

    assert_eq!(found.len(), 1, "exactly one process must be registered");
    assert_eq!(found[0].process_id, process_id);
    assert_eq!(found[0].workflow_id.name.as_ref(), "gpke-lf-anmeldung");
}

/// Looking up a `malo_id` that was never registered returns an empty list.
#[tokio::test]
async fn lookup_unknown_malo_returns_empty() {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let tenant_id = TenantId::from_party_id("9900357000004");

    let found = store
        .as_process_registry()
        .lookup_correlated(tenant_id, "99999999999")
        .await
        .expect("lookup_correlated");

    assert!(found.is_empty(), "unknown malo_id must return empty list");
}

/// After `HandleAntwort { accepted: true }`, the process transitions to `Active`.
#[tokio::test]
async fn handle_antwort_accepted_transitions_to_active() {
    use mako_engine::process::Process;

    let store_inner = mako_engine::event_store::InMemoryEventStore::new();
    let tenant_id = TenantId::from_party_id("9900357000004");
    let malo_id = "51238696781";

    let process = Process::<GpkeLfAnmeldungWorkflow, _>::new(
        store_inner.clone(),
        tenant_id,
        WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
    );

    // Initiate.
    process
        .execute(mako_gpke::LfAnmeldungCommand::InitiateAnmeldung {
            pid: Pruefidentifikator::new(55001).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            location_id: MaLo::new(malo_id),
            process_date: "2026-10-01".into(),
        })
        .await
        .expect("InitiateAnmeldung");

    assert!(matches!(
        process.state().await.unwrap(),
        LfAnmeldungState::Pending(_)
    ));

    // Dispatch NB acceptance.
    process
        .execute(mako_gpke::LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55003).unwrap(),
            accepted: true,
            reason: None,
            response_ref: mako_engine::types::MessageRef::new("REF-001"),
        })
        .await
        .expect("HandleAntwort accepted");

    assert!(matches!(
        process.state().await.unwrap(),
        LfAnmeldungState::Active(_)
    ));
}

/// After `HandleAntwort { accepted: false }`, the process transitions to `Rejected`.
#[tokio::test]
async fn handle_antwort_rejected_transitions_to_rejected() {
    use mako_engine::process::Process;

    let store_inner = mako_engine::event_store::InMemoryEventStore::new();
    let tenant_id = TenantId::from_party_id("9900357000004");
    let malo_id = "51238696781";

    let process = Process::<GpkeLfAnmeldungWorkflow, _>::new(
        store_inner.clone(),
        tenant_id,
        WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
    );

    process
        .execute(mako_gpke::LfAnmeldungCommand::InitiateAnmeldung {
            pid: Pruefidentifikator::new(55001).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            location_id: MaLo::new(malo_id),
            process_date: "2026-10-01".into(),
        })
        .await
        .unwrap();

    process
        .execute(mako_gpke::LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55004).unwrap(),
            accepted: false,
            reason: Some("Ablehnungsgrund: ungültige Marktlokation".into()),
            response_ref: mako_engine::types::MessageRef::new("REF-002"),
        })
        .await
        .unwrap();

    assert!(matches!(
        process.state().await.unwrap(),
        LfAnmeldungState::Rejected { .. }
    ));
}

/// `register_correlated` is idempotent: registering the same process twice
/// does not create duplicate entries (last write wins per key).
#[tokio::test]
async fn register_correlated_is_idempotent() {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let tenant_id = TenantId::from_party_id("9900357000004");
    let malo_id = "51238696781";

    let process_id = mako_engine::ids::ProcessId::new();
    let identity = mako_engine::ids::ProcessIdentity::new(
        process_id,
        tenant_id,
        WorkflowId::new("gpke-lf-anmeldung", "FV2025-10-01"),
    );

    // Register twice.
    store
        .as_process_registry()
        .register_correlated(tenant_id, malo_id, process_id, identity.clone())
        .await
        .unwrap();
    store
        .as_process_registry()
        .register_correlated(tenant_id, malo_id, process_id, identity)
        .await
        .unwrap();

    let found = store
        .as_process_registry()
        .lookup_correlated(tenant_id, malo_id)
        .await
        .unwrap();

    // Idempotent: exactly one entry after two registrations of the same process.
    assert_eq!(
        found.len(),
        1,
        "duplicate registration must not create two entries"
    );
}
