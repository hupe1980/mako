#![deny(unsafe_code)]
//! `billingd` — Multi-product Energy Billing Engine (LF role).
//!
//! Port: `:9280`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `calculator` | see module docs |
//! | `clients` | see module docs |
//! | `config` | see module docs |
//! | `handlers` | see module docs |
//! | `pg` | see module docs |
//! | `xrechnung` | see module docs |

pub mod calculator;
pub mod clients;
pub mod config;
pub mod handlers;
pub mod pg;
pub mod xrechnung;

pub mod mcp_server;
