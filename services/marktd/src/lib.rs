//! `marktd` library root — exposes all modules for both `main.rs` and tests.

#![deny(unsafe_code)]
#![expect(
    missing_docs,
    reason = "publish = false: a daemon's library exists for its binary and its tests, not as an API"
)]

pub mod bundesland;
pub mod config;
pub mod consent_lifecycle;
pub mod daemon;
pub mod fanout;
pub mod handlers;
pub mod mcp_server;
pub mod metrics;
pub mod mmma_worker;
pub mod openapi;
pub mod outbox;
pub mod pg;
pub mod retention;
pub mod state;
