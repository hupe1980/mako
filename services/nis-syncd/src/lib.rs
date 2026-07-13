#![deny(unsafe_code)]
//! `nis-syncd` — NIS/GIS grid topology import adapter (NB role, stateless).
//!
//! Port: `:9680`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `mcp_server` | see module docs |
//! | `sync` | see module docs |

pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod sync;
