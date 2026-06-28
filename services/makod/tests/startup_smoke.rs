//! Startup smoke test — verifies that every workflow registered by every
//! `EngineModule` has a matching entry in the deadline-dispatch coverage table.
//!
//! This test verifies that every workflow registered by every
//! `EngineModule` also has a matching entry in the deadline-dispatch coverage table,
//! catching missing `DISPATCH_TABLE` entries before they reach production.
//!
//! It instantiates the full module stack with in-memory stores and calls
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

use makod::deadline_dispatch;

/// Every workflow declared by the four production modules must appear in
/// `deadline_dispatch::DISPATCH_TABLE`.  If a new module or workflow is added
/// without a matching dispatch arm, this test panics with an actionable message
/// before the bug can reach a production binary.
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
        .register(Box::new(MabisModule))
        .build();

    // Panics with an actionable message if any registered workflow is absent
    // from the dispatch table — that panic is the test-failure signal.
    deadline_dispatch::assert_dispatch_coverage(ctx.registered_workflows());
}
