#![deny(unsafe_code)]
//! `accountingd` — Massenkontokorrent / Customer Account Ledger daemon (LF role).
//!
//! Port: `:9380`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `pg` | see module docs |
//! | `sepa` | see module docs |

pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod pg;
pub mod sepa;
