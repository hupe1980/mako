//! Cross-cutting foundations — configuration, identity, authorization, and
//! health plumbing shared by the transport, orchestrator, and API layers.

pub mod cedar_authz;
pub mod config;
pub mod erp_adapter;
pub mod health;
pub mod malo_cache;
pub mod party_registry;
pub mod worker_health;
