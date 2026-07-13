#![deny(unsafe_code)]
//! `einsd` — Einspeiser Registry + EEG/KWKG Settlement daemon.
//!
//! Port: `:9180`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `pg` | see module docs |

pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod pg;
