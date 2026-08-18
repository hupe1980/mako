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
//! | `models` | the settlement-model vocabulary |
//! | `sect52` | §52 Abs. 1 violation detection |
//! | `settle` | the one path that settles a plant for a month |
//! | `validate` | what a registration must state before it can be settled |
//! | `pg` | see module docs |

pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod models;
pub mod pg;
pub mod routes;
pub mod sect52;
pub mod settle;
pub mod validate;
