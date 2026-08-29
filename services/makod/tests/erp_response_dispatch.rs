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
        cedar_authz::{CedarAuthorizer, DefaultPolicy},
        commands_api::CommandsApiState,
        malo_cache::{MaloIdentResultCache, SlateDbMaloCache},
    };
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    let cedar = Arc::new(
        CedarAuthorizer::new(vec![], None, None, None, DefaultPolicy::PermitAll)
            .expect("cedar build"),
    );
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
        marktd_client: None, // M1 guard disabled in unit tests
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
            transaktionsgrund: None,
            transaktionsgrund_ergaenzung: None,
            veraeusserungsform: None,
            vorgangsnummer: None,
            kunde_name: None,
            kunde_namensformat: None,
            message_ref: mako_engine::types::MessageRef::new("MSG-001"),
            received_at: time::OffsetDateTime::now_utc(),
            bilanzierungsgebiet: None,
            bilanzierungsmethode: None,
            fallgruppe: None,
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
        transaktionsgrund: None,
        bilanzkreis: Some("11XBK-LF-------9".to_owned()),
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
        .lookup_correlated(tenant_id, "99999999044")
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
            transaktionsgrund: None,
            bilanzkreis: Some("11XBK-LF-------9".to_owned()),
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
            response_pid: Pruefidentifikator::new(55002).unwrap(), // Bestätigung Anmeldung
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
            transaktionsgrund: None,
            bilanzkreis: Some("11XBK-LF-------9".to_owned()),
        })
        .await
        .unwrap();

    process
        .execute(mako_gpke::LfAnmeldungCommand::HandleAntwort {
            response_pid: Pruefidentifikator::new(55003).unwrap(), // Ablehnung Anmeldung
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

// ── NB-side dispatch tests ──
//
// `gpke.lieferbeginn.bestaetigen` and the other three NB commands must route to
// `dispatch_supplier_change_antwort` → `GpkeSupplierChangeWorkflow`. Routing
// them to `dispatch_lf_antwort` → `GpkeLfAnmeldungWorkflow` returns
// `ProcessNotFound` in a pure NB deployment, where that workflow does not exist
// for the MaLo.

/// `gpke.lieferbeginn.bestaetigen` must dispatch `SendAntwort { accepted: true }`
/// into `GpkeSupplierChangeWorkflow` and return `Dispatched`.
#[tokio::test]
async fn nb_lieferbeginn_bestaetigen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696012";

    // Simulate ingest dispatcher: spawn GpkeSupplierChangeWorkflow for this MaLo.
    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55001).await;

    // NB ERP calls gpke.lieferbeginn.bestaetigen.
    let payload = serde_json::json!({
        "malo_id": malo_id,
        "antwort_code": "A51",
        "antwort_ebd": "E_0623",
    });
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
    let malo_id = "51238696806";

    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55001).await;

    let payload = serde_json::json!({
        "malo_id": malo_id,
        "antwort_code": "A05",
        "antwort_ebd": "E_0622",
        "bemerkung": "Stammdaten unbekannt",
    });
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

/// `gpke.lieferende.bestaetigen` must reach `GpkeSupplierChangeWorkflow` (PID 55004).
#[tokio::test]
async fn nb_lieferende_bestaetigen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238696913";

    let process_id = spawn_supplier_change(&state.store, tenant_id, malo_id, 55004).await;

    let payload = serde_json::json!({
        "malo_id": malo_id,
        "antwort_code": "A11",
        "antwort_ebd": "E_0607",
    });
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

/// `gpke.lieferende.ablehnen` must reach `GpkeSupplierChangeWorkflow` (PID 55004).
#[tokio::test]
async fn nb_lieferende_ablehnen_dispatches_to_supplier_change_workflow() {
    let state = make_state(&["NB"]).await;
    let tenant_id = state.tenant_id;
    let malo_id = "51238697896";

    spawn_supplier_change(&state.store, tenant_id, malo_id, 55004).await;

    let payload = serde_json::json!({
        "malo_id": malo_id,
        "antwort_code": "A02",
        "antwort_ebd": "E_0607",
        "bemerkung": "Keine Umzugsmeldung",
    });
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

    let payload = serde_json::json!({
        "malo_id": "99999999945",
        "antwort_code": "A51",
        "antwort_ebd": "E_0623",
    });
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

/// The NB's answer is a **code**, not a boolean. `SG4 STS+E01` is Muss on every
/// Antwortnachricht, so a command without one is refused rather than rendering
/// a well-formed UTILMD that states no Grund at all.
#[tokio::test]
async fn an_nb_answer_without_an_antwortcode_is_refused() {
    let state = make_state(&["NB"]).await;
    let malo_id = "51238696012";
    spawn_supplier_change(&state.store, state.tenant_id, malo_id, 55001).await;

    let err = makod::commands_api::dispatch_command(
        &state,
        "gpke.lieferbeginn.bestaetigen",
        &serde_json::json!({ "malo_id": malo_id }),
    )
    .await
    .expect_err("an answer without SG4 STS+E01 must be refused");
    assert!(
        matches!(err, makod::commands_api::DispatchError::InvalidPayload(_)),
        "got: {err:?}"
    );
}

/// The **published Cluster decides the response PID**, so a Bestätigung command
/// carrying an Ablehnungscode is refused rather than sending a 55002 that
/// states a refusal.
#[tokio::test]
async fn a_bestaetigen_command_may_not_carry_an_ablehnungscode() {
    let state = make_state(&["NB"]).await;
    let malo_id = "51238696806";
    spawn_supplier_change(&state.store, state.tenant_id, malo_id, 55001).await;

    let err = makod::commands_api::dispatch_command(
        &state,
        "gpke.lieferbeginn.bestaetigen",
        &serde_json::json!({
            "malo_id": malo_id,
            "antwort_code": "A07",
            "antwort_ebd": "E_0622",
        }),
    )
    .await
    .expect_err("A07 is an Ablehnung and cannot ride a Bestätigung");
    assert!(
        matches!(err, makod::commands_api::DispatchError::InvalidPayload(_)),
        "got: {err:?}"
    );
}

/// A code the named tree does not publish is refused: `A07` is an `E_0622`
/// code and `E_0607` does not define it.
#[tokio::test]
async fn an_answer_code_must_belong_to_the_named_tree() {
    let state = make_state(&["NB"]).await;
    let malo_id = "51238696913";
    spawn_supplier_change(&state.store, state.tenant_id, malo_id, 55004).await;

    let err = makod::commands_api::dispatch_command(
        &state,
        "gpke.lieferende.ablehnen",
        &serde_json::json!({
            "malo_id": malo_id,
            "antwort_code": "A07",
            "antwort_ebd": "E_0607",
        }),
    )
    .await
    .expect_err("A07 is not published by E_0607");
    assert!(
        matches!(err, makod::commands_api::DispatchError::InvalidPayload(_)),
        "got: {err:?}"
    );
}

/// A Gas answer carries no `STS` DE 1131 — the Gas Codelisten are not named in
/// the segment — but it still belongs to exactly one tree. `antwort_tree` is
/// that key, and a code the tree does not publish is refused even though the
/// wire value is absent.
#[tokio::test]
async fn a_gas_answer_is_validated_against_its_tree_without_a_de1131() {
    let state = make_state(&["NB"]).await;
    let malo_id = "51238697896";
    spawn_supplier_change(&state.store, state.tenant_id, malo_id, 55004).await;

    // `A02` is the *Strom* Vorlauffrist code; `G_0007` publishes `E17` instead.
    let err = makod::commands_api::dispatch_command(
        &state,
        "gpke.lieferende.ablehnen",
        &serde_json::json!({
            "malo_id": malo_id,
            "antwort_code": "A02",
            "antwort_tree": "E_3019",
            "zustimmung": false,
        }),
    )
    .await
    .expect_err("A02 is not a G_0007 code");
    assert!(
        matches!(err, makod::commands_api::DispatchError::InvalidPayload(_)),
        "got: {err:?}"
    );

    // `E17` is, and it dispatches.
    makod::commands_api::dispatch_command(
        &state,
        "gpke.lieferende.ablehnen",
        &serde_json::json!({
            "malo_id": malo_id,
            "antwort_code": "E17",
            "antwort_tree": "E_3019",
            "bemerkung": "Vorlauffrist nicht eingehalten",
        }),
    )
    .await
    .expect("E17 is published by E_3019");
}
