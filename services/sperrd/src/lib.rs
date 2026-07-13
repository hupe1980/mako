#![deny(unsafe_code)]
//! `sperrd` — Sperrung execution tracking daemon (NB role).
//!
//! Port: `:8780`
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
pub mod pg;
