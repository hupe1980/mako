//! `marktd` library root — exposes all modules for both `main.rs` and tests.

pub mod config;
pub mod consent_lifecycle;
pub mod fanout;
pub mod handlers;
pub mod mcp_server;
pub mod metrics;
pub mod mmma_worker;
pub mod openapi;
pub mod outbox;
pub mod pg;
pub mod retention;
