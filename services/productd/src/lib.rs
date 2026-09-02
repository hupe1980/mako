#![deny(unsafe_code)]
//! `productd` — Product and Tariff Catalog daemon (LF role).
//!
//! Port: `:9080`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `pg` | see module docs |
//! | `rounding` | kaufmännisches Runden — the one rounding mode |

pub mod behg;
pub mod bo4e_angebot;
pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod pg;
pub mod rounding;
