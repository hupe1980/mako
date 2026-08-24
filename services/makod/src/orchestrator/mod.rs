//! Orchestration layer — wire ↔ workflow mediation.
//!
//! Inbound: `ingest_dispatcher` routes parsed messages into workflow spawns or
//! resumes via the `adapters` registries. Outbound: `edifact_renderer` turns
//! outbox intents into wire bytes. `commands_api` drives workflows from ERP
//! commands, `deadline_dispatch` from expired Fristen, and the remaining
//! modules cover §20b Netzzugang and read-model projection.

pub mod adapters;
pub mod commands_api;
pub mod deadline_dispatch;
pub mod dvgw_ingest;
pub mod edifact_renderer;
pub mod ingest_dispatcher;
pub mod netzzugang;
pub mod projection_worker;
