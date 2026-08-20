#![deny(unsafe_code)]
//! `sperrd` — Sperr-/Entsperrauftrag execution queue (NB role).
//!
//! Port: `:8780`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | TOML configuration |
//! | `model` | the typed domain — ORDERS/IFTSTA codes, not loose strings |
//! | `ingest` | inbound ORDERS 17115/17117 → work order |
//! | `handlers` | HTTP routes (all authenticated **and** authorized) |
//! | `events` | `de.sperr.*` CloudEvents |
//! | `worker` | the IFTSTA 21039 retry queue |
//! | `pg` | persistence |

pub mod config;
pub mod events;
pub mod handlers;
pub mod ingest;
pub mod mcp_server;
pub mod model;
pub mod pg;
pub mod worker;
