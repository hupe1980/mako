//! `makod` library surface — exposed for integration tests.
//!
//! This module re-exports the two subsystems that integration tests need to
//! exercise the full render → wire → parse → adapt pipeline without depending
//! on the binary entry point.
//!
//! # For integration tests
//!
//! ```rust,ignore
//! use makod::adapters::gpke_registry;
//! use makod::edifact_renderer::render_to_wire_bytes;
//! use makod::deadline_dispatch;
//! ```
//!
//! All other modules (`main`, `config`, `commands_api`, …) are compiled only
//! as part of the binary target and are not accessible from the library.

pub mod adapters;
pub mod api_bridge;
pub mod as4_ingest;
pub mod as4_sender;
pub mod cedar_authz;
pub mod commands_api;
pub mod config;
pub mod contrl_ack;
pub mod deadline_dispatch;
pub mod edifact_api;
pub mod edifact_renderer;
pub mod erp_adapter;
pub mod health;
pub mod ingest_dispatcher;
pub mod malo_admin_api;
pub mod malo_cache;
pub mod malo_ident_sender;
pub mod mcp_server;
pub mod metrics_api;
pub mod migration_api;
pub mod oidc_verifier;
pub mod openapi;
pub mod partner_api;
pub mod party_registry;
pub mod projection_worker;
pub mod redispatch_xml_ingest;
// startup symbols (MakodCtx, WorkersConfig, spawn_workers, validate_adapter_coverage)
// are pub(crate) and called only from main.rs. The lib target sees them as dead code
// because main.rs is a separate compilation unit. Allow dead_code here; the binary
// target's own dead-code check (via the bin unit) correctly skips these.
#[allow(dead_code)]
pub mod startup;
pub mod verzeichnisdienst_worker;
pub mod webdienste;
pub mod worker_health;
