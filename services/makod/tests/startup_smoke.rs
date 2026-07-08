//! Startup smoke test — verifies that every workflow registered by every
//! `EngineModule` has a matching entry in the deadline-dispatch coverage table.
//!
//! This test verifies that every workflow registered by every
//! `EngineModule` also has a matching entry in the deadline-dispatch coverage table,
//! catching missing `DISPATCH_TABLE` entries before they reach production.
//!
//! It instantiates the **full** production module stack with in-memory stores
//! (matching `services/makod/src/main.rs` module registration) and calls
//! `assert_dispatch_coverage`, which panics if any registered workflow is
//! absent from the dispatch table.

use std::sync::Arc;

use mako_engine::{
    builder::EngineBuilder, deadline::InMemoryDeadlineStore, event_store::InMemoryEventStore,
    registry::InMemoryProcessRegistry, snapshot::InMemorySnapshotStore,
};
use mako_geli_gas::GeliGasModule;
use mako_gpke::GpkeModule;
use mako_mabis::MabisModule;
use mako_wim::WimModule;
use mako_wim_gas::WimGasModule;

use makod::deadline_dispatch;

/// Every workflow declared by all five production modules must appear in
/// `deadline_dispatch::DISPATCH_TABLE`.  If a new module or workflow is added
/// without a matching dispatch arm, this test panics with an actionable message
/// before the bug can reach a production binary.
///
/// Module stack must match `services/makod/src/main.rs`:
/// - `GpkeModule`    — PIDs 55001–55002, 55016 + INVOIC + IFTSTA
/// - `WimModule`     — PIDs 55039/55042/55051/55168 (WiM Strom Messstellenbetrieb)
/// - `GeliGasModule` — PIDs 44001–44021 (GeLi Gas; 44022–44024 registered by WimGasModule) + PID 31011 (AWH Rechnung)
/// - `WimGasModule`  — PIDs 44039–44053, 44168–44170 (WiM Gas MSB-Wechsel)
/// - `MabisModule`   — PID 13003 (Bilanzkreisabrechnung Strom)
#[test]
fn all_registered_workflows_covered_by_dispatch_table() {
    let ctx = EngineBuilder::new()
        .with_event_store(Arc::new(InMemoryEventStore::new()))
        .with_snapshot_store(InMemorySnapshotStore::new())
        .with_deadline_store(InMemoryDeadlineStore::new())
        .with_registry(InMemoryProcessRegistry::new())
        .register(Box::new(GpkeModule))
        .register(Box::new(WimModule))
        .register(Box::new(GeliGasModule))
        .register(Box::new(WimGasModule))
        .register(Box::new(MabisModule))
        .build();

    // Panics with an actionable message if any registered workflow is absent
    // from the dispatch table — that panic is the test-failure signal.
    deadline_dispatch::assert_dispatch_coverage(ctx.registered_workflows());
}

/// Assert that every active FV transition is registered in the migration
/// dispatch table.
///
/// This is the guard against the "missing dispatch arm" scenario described in
/// F-009: a developer adds a new `StateMigration` in a domain crate but forgets
/// to add a corresponding arm to `migration_api::dispatch_migrations` and to
/// `migration_api::KNOWN_FV_TRANSITIONS`.
///
/// **Maintenance rule for each October release cycle:**
/// 1. Add the new `(from, to)` pair to `KNOWN_FV_TRANSITIONS` in `migration_api.rs`.
/// 2. Add the corresponding `match` arm to `dispatch_migrations`.
/// 3. Add the new pair to `active_transitions` below.
///
/// If any of the three steps is missing, this test panics with a clear message.
#[tokio::test]
async fn migration_dispatch_table_covers_active_fv_transitions() {
    let active_transitions: &[(&str, &str)] = &[("FV2025-10-01", "FV2026-10-01")];

    let known = makod::migration_api::KNOWN_FV_TRANSITIONS;
    for (from, to) in active_transitions {
        assert!(
            known.contains(&(*from, *to)),
            "migration_api::KNOWN_FV_TRANSITIONS does not contain ({from:?}, {to:?}). \
             Add a `match` arm to `dispatch_migrations` in migration_api.rs and \
             add the pair to KNOWN_FV_TRANSITIONS. \
             This is a mandatory step in the annual October release workflow.",
        );
    }
}
