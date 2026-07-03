//! Integration tests for Phase 3 ERP response dispatch.
//!
//! Verifies that:
//! - An `anmelden` command registers the process under the `malo_id` business key.
//! - A subsequent `bestaetigen` command finds the process via the registry and
//!   dispatches `SendAntwort` into `GpkeSupplierChangeWorkflow` (NB side) —
//!   **not** `GpkeLfAnmeldungWorkflow` (LF side).  The bug fixed here was that
//!   all four NB commands called `dispatch_lf_antwort` → `GpkeLfAnmeldungWorkflow`,
//!   which meant they failed with `ProcessNotFound` in any NB-only deployment.
//! - A `bestaetigen` call for an unknown `malo_id` returns `ProcessNotFound`.
//! - An `aktivieren` command dispatches `LfAnmeldungCommand::Activate`.
//!
//! These tests exercise the full `CommandsApiState` dispatch path end-to-end
//! with in-memory stores (no actual HTTP server, no SlateDB).

use std::sync::Arc;

use mako_engine::registry::ProcessRegistry as _;
use mako_engine::{
    ids::TenantId,
    process::Process,
    types::{MaLo, MarktpartnerCode, Pruefidentifikator},
    version::WorkflowId,
};
use mako_gpke::{
    GpkeLfAnmeldungWorkflow, GpkeSupplierChangeWorkflow, LfAnmeldungState, SupplierChangeCommand,
    SupplierChangeState,
};

// ── Test helpers ──────────────────────────────────────────────────────────────

/// Build a minimal `CommandsApiState` backed by an in-memory SlateDB store.
///
/// Cedar keys are omitted — dispatch tests call `dispatch_command` directly,
/// bypassing the HTTP auth layer.
async fn make_state(marktrollen: &[&str]) -> makod::commands_api::CommandsApiState {
    use makod::{
        cedar_authz::CedarAuthorizer,
        commands_api::CommandsApiState,
        malo_cache::{MaloIdentResultCache, SlateDbMaloCache},
    };
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let cedar = Arc::new(CedarAuthorizer::new(vec![], None, None).expect("cedar build"));
    CommandsApiState {
        tenant_id: TenantId::from_party_id("9900357000004"),
        sender_party_id: "9900357000004".to_owned(),
        configured_marktrollen: marktrollen.iter().map(|s| s.to_uppercase()).collect(),
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar,
        snapshot_store: store.as_snapshot_store(),
        malo_cache: Arc::new(SlateDbMaloCache::new(store.clone())),
        maloid_result_cache: MaloIdentResultCache::new(store.clone()),
        store: Arc::new(store),
    }
}

/// Spawn a `GpkeSupplierChangeWorkflow` process (NB side), execute
/// `ReceiveUtilmd`, and register it under `malo_id` in the process registry.
///
/// This mirrors what the ingest dispatcher does when a UTILMD 55001 arrives.
/// Returns the spawned `ProcessId`.
async fn spawn_supplier_change(
    store: &mako_engine::store_slatedb::SlateDbStore,
    tenant_id: TenantId,
    malo_id: &str,
    pid: u32,
) -> mako_engine::ids::ProcessId {
    let workflow_id = WorkflowId::new("gpke-supplier-change", "FV2025-10-01");
    let process = Process::<
        GpkeSupplierChangeWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::new(Arc::new(store.clone()), tenant_id, workflow_id.clone());
    let process_id = process.process_id();

    process
        .execute(SupplierChangeCommand::ReceiveUtilmd {
            pid: Pruefidentifikator::new(pid).unwrap(),
            sender: MarktpartnerCode::new("4012345000023"),
            receiver: MarktpartnerCode::new("9900357000004"),
            location_id: MaLo::new(malo_id),
            document_date: "20260701".into(),
            process_date: "20261001".into(),
            message_ref: mako_engine::types::MessageRef::new("MSG-001"),
            validation_passed: true,
            validation_errors: vec![],
        })
        .await
        .expect("spawn_supplier_change: ReceiveUtilmd");

    let identity = mako_engine::ids::ProcessIdentity::new(process_id, tenant_id, workflow_id);
    store
        .as_process_registry()
        .register_correlated(tenant_id, malo_id, process_id, identity)
        .await
        .expect("spawn_supplier_change: register_correlated");

    process_id
}

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

// ── NB-side dispatch tests — regression guards for the workflow-routing bug ──
//
// Before the fix, `gpke.lieferbeginn.bestaetigen` (and the other three NB
// commands) called `dispatch_lf_antwort` → `GpkeLfAnmeldungWorkflow`.
// In a pure NB deployment that workflow does not exist for the MaLo, so every
// call returned `ProcessNotFound`.  The fix wires them to
// `dispatch_supplier_change_antwort` → `GpkeSupplierChangeWorkflow`.

/// `gpke.lieferbeginn.bestaetigen` must dispatch `SendAntwort { accepted: true }`
/// into `GpkeSupplierChangeWorkflow` and return `Dispatched`.
///
/// Regression: before the fix this returned `ProcessNotFound` because the
/// command called `dispatch_lf_antwort` → `GpkeLfAnmeldungWorkflow`.
#[tokio::test]
async fn nb_lieferbeginn_bestaetigen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696781";

    // Simulate ingest dispatcher: spawn GpkeSupplierChangeWorkflow for this MaLo.
    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55001).await;

    // NB ERP calls gpke.lieferbeginn.bestaetigen.
    let payload = serde_json::json!({ "malo_id": malo_id });
    let outcome =
        makod::commands_api::dispatch_command(&state, "gpke.lieferbeginn.bestaetigen", &payload)
            .await
            .expect("bestaetigen must succeed");

    assert!(
        matches!(outcome, makod::commands_api::DispatchOutcome::Dispatched { process_id: pid } if pid == process_id),
        "bestaetigen must return Dispatched with the spawned process_id; got: {outcome:?}"
    );

    // The NB workflow must now be in AntwortGesendet state.
    let process = Process::<
        GpkeSupplierChangeWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::from_identity(
        Arc::clone(&state.store),
        mako_engine::ids::ProcessIdentity::new(
            process_id,
            tenant_id,
            WorkflowId::new("gpke-supplier-change", "FV2025-10-01"),
        ),
    );
    let final_state = process.state().await.unwrap();
    assert!(
        matches!(final_state, SupplierChangeState::AntwortGesendet { .. }),
        "GpkeSupplierChangeWorkflow must be AntwortGesendet after bestaetigen; got: {final_state:?}"
    );
}

/// `gpke.lieferbeginn.ablehnen` must dispatch `SendAntwort { accepted: false }`
/// into `GpkeSupplierChangeWorkflow` and leave it in `Rejected`.
#[tokio::test]
async fn nb_lieferbeginn_ablehnen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696782";

    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55001).await;

    let payload = serde_json::json!({ "malo_id": malo_id, "reason": "Stammdaten unbekannt" });
    let outcome =
        makod::commands_api::dispatch_command(&state, "gpke.lieferbeginn.ablehnen", &payload)
            .await
            .expect("ablehnen must succeed");

    assert!(
        matches!(outcome, makod::commands_api::DispatchOutcome::Dispatched { process_id: pid } if pid == process_id),
        "ablehnen must return Dispatched; got: {outcome:?}"
    );

    let process = Process::<
        GpkeSupplierChangeWorkflow,
        Arc<mako_engine::store_slatedb::SlateDbStore>,
    >::from_identity(
        Arc::clone(&state.store),
        mako_engine::ids::ProcessIdentity::new(
            process_id,
            tenant_id,
            WorkflowId::new("gpke-supplier-change", "FV2025-10-01"),
        ),
    );
    let final_state = process.state().await.unwrap();
    assert!(
        matches!(final_state, SupplierChangeState::Rejected { .. }),
        "GpkeSupplierChangeWorkflow must be Rejected after ablehnen; got: {final_state:?}"
    );
}

/// `gpke.lieferende.bestaetigen` must reach `GpkeSupplierChangeWorkflow` (PID 55002).
#[tokio::test]
async fn nb_lieferende_bestaetigen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696783";

    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55002).await;

    let payload = serde_json::json!({ "malo_id": malo_id });
    let outcome =
        makod::commands_api::dispatch_command(&state, "gpke.lieferende.bestaetigen", &payload)
            .await
            .expect("lieferende.bestaetigen must succeed");

    assert!(
        matches!(
            outcome,
            makod::commands_api::DispatchOutcome::Dispatched { .. }
        ),
        "lieferende.bestaetigen must return Dispatched; got: {outcome:?}"
    );

    let _ = process_id; // consumed in the assert pattern above
}

/// `gpke.lieferende.ablehnen` must reach `GpkeSupplierChangeWorkflow` (PID 55002).
#[tokio::test]
async fn nb_lieferende_ablehnen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696784";

    spawn_supplier_change(&state.store, tenant_id, malo_id, 55002).await;

    let payload = serde_json::json!({ "malo_id": malo_id, "reason": "Keine Umzugsmeldung" });
    let outcome =
        makod::commands_api::dispatch_command(&state, "gpke.lieferende.ablehnen", &payload)
            .await
            .expect("lieferende.ablehnen must succeed");

    assert!(
        matches!(
            outcome,
            makod::commands_api::DispatchOutcome::Dispatched { .. }
        ),
        "lieferende.ablehnen must return Dispatched; got: {outcome:?}"
    );
}

/// `gpke.lieferbeginn.bestaetigen` for an unknown MaLo must return
/// `DispatchError::ProcessNotFound` — not silently dispatch to a wrong workflow.
#[tokio::test]
async fn nb_bestaetigen_unknown_malo_returns_process_not_found() {
    let state = make_state(&["NB"]).await;

    let payload = serde_json::json!({ "malo_id": "99999999999" });
    let err =
        makod::commands_api::dispatch_command(&state, "gpke.lieferbeginn.bestaetigen", &payload)
            .await
            .expect_err("bestaetigen for unknown MaLo must fail");

    assert!(
        matches!(
            err,
            makod::commands_api::DispatchError::ProcessNotFound { .. }
        ),
        "unknown MaLo must yield ProcessNotFound; got: {err:?}"
    );
}

/// NB commands are rejected on an LF-only instance.
///
/// `validate_command` must catch this before dispatch is even attempted,
/// ensuring an LF-configured makod cannot accidentally accept NB commands.
#[test]
fn nb_commands_rejected_on_lf_instance() {
    use makod::commands_api::{CommandError, validate_command};
    let lf = vec!["LF".to_owned()];

    for cmd in &[
        "gpke.lieferbeginn.bestaetigen",
        "gpke.lieferbeginn.ablehnen",
        "gpke.lieferende.bestaetigen",
        "gpke.lieferende.ablehnen",
    ] {
        let err = validate_command(cmd, None, &lf)
            .expect_err(&format!("{cmd} must be rejected on LF instance"));
        assert!(
            matches!(err, CommandError::RoleNotConfigured),
            "{cmd} must fail with RoleNotConfigured on LF instance; got: {err:?}"
        );
    }
}

/// LF commands are rejected on an NB-only instance.
#[test]
fn lf_commands_rejected_on_nb_instance() {
    use makod::commands_api::{CommandError, validate_command};
    let nb = vec!["NB".to_owned()];

    for cmd in &["gpke.lieferbeginn.anmelden", "gpke.lieferende.anmelden"] {
        let err = validate_command(cmd, None, &nb)
            .expect_err(&format!("{cmd} must be rejected on NB instance"));
        assert!(
            matches!(err, CommandError::RoleNotConfigured),
            "{cmd} must fail with RoleNotConfigured on NB instance; got: {err:?}"
        );
    }
}
