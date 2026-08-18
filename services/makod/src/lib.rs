//! `makod` library surface — exposed for integration tests.
//!
//! This crate re-exports the subsystems that integration tests need to
//! exercise the full render → wire → parse → adapt pipeline without depending
//! on the binary entry point.
//!
//! # Module layout
//!
//! The source tree is grouped along the service's natural seams:
//!
//! - [`transport`] — AS4 ingest/egress, acknowledgements, directory workers,
//!   BDEW API-Webdienste
//! - [`orchestrator`] — wire ↔ workflow mediation: ingest dispatch, adapter
//!   registries, command API, deadline dispatch, EDIFACT rendering
//! - [`api`] — operator-facing REST/OpenAPI/MCP surfaces
//! - [`core`] — configuration, identity, authorization, health plumbing
//!
//! Every module is additionally re-exported at the crate root under its
//! historical flat name (`makod::adapters`, `makod::commands_api`, …) so both
//! the integration-test suite and intra-crate `crate::<module>` paths keep
//! working unchanged.
//!
//! # Why one crate (and not a crate per layer)
//!
//! A crate split along these folders was considered and rejected: the shared
//! state types (`EdifactApiState`, the renderer/adapters pairing consumed by
//! both the AS4 sender and the commands API) would force a web of cross-crate
//! `pub` surface for what is genuinely one deployable unit. The folders give
//! the namespacing benefit; the single crate keeps `validate_adapter_coverage`
//! / `validate_dispatch_completeness` able to see every registry and dispatch
//! table at once.
//!
//! # For integration tests
//!
//! ```rust,ignore
//! use makod::adapters::gpke_registry;
//! use makod::edifact_renderer::render_to_wire_bytes;
//! use makod::deadline_dispatch;
//! ```

pub mod api;
pub mod core;
pub mod orchestrator;
pub mod transport;

// ── Flat-path re-exports ──────────────────────────────────────────────────────
//
// Keep every pre-folder module path (`makod::<module>` and `crate::<module>`)
// compiling. New code may use the grouped paths; existing code and the
// integration-test suite rely on these.

pub use crate::api::{
    edifact_api, invoic_api, malo_admin_api, mcp_server, metrics_api, migration_api, openapi,
    partner_api,
};
pub use crate::core::{
    cedar_authz, config, erp_adapter, health, malo_cache, party_registry, preflight, worker_health,
};
pub use crate::orchestrator::{
    adapters, commands_api, deadline_dispatch, edifact_renderer, ingest_dispatcher, netzzugang,
    projection_worker,
};
pub use crate::transport::{
    api_bridge, as4_ingest, as4_sender, contrl_ack, malo_ident_sender, redispatch_xml_ingest,
    verzeichnisdienst_worker, webdienste,
};

// startup symbols (MakodCtx, WorkersConfig, spawn_workers, validate_adapter_coverage)
// are pub(crate) and called only from main.rs. The lib target sees them as dead code
// because main.rs is a separate compilation unit. Allow dead_code here; the binary
// target's own dead-code check (via the bin unit) correctly skips these.
#[allow(dead_code)]
pub mod startup;
