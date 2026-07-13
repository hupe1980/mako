#![deny(unsafe_code)]
//! `vertragd` — Contract and Customer Management daemon (LF role).
//!
//! Port: `:9780`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | see module docs |
//! | `events` | see module docs |
//! | `handlers` | see module docs |
//! | `mcp_server` | see module docs |
//! | `pg` | see module docs |

pub mod config;
pub mod events;
pub mod handlers;
pub mod mcp_server;
pub mod pg;
