#![deny(unsafe_code)]
//! `portald` — Customer Portal read-model gateway (LF role).
//!
//! Port: `:9480`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `clients` | see module docs |
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `mcp_server` | see module docs |

pub mod clients;
pub mod config;
pub mod handlers;
pub mod mcp_server;
