#![deny(unsafe_code)]
//! `netzbilanzd` — NNE/KA/MMM Billing daemon (NB role).
//!
//! Port: `:8680`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `billing` | see module docs |
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `pg` | see module docs |

pub mod billing;
pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod pg;
