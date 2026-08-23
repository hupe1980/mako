#![deny(unsafe_code)]
//! `accountingd` — Massenkontokorrent / Customer Account Ledger daemon (LF role).
//!
//! Port: `:9380`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `clients` | `vertragd` (who the customer is) and `outputd` (the document) |
//! | `config` | TOML + env configuration |
//! | `handlers` | the REST surface |
//! | `ledger` | the `doubleentry`-backed Kontokorrent and its GL chart |
//! | `mahnung` | a dunning case projected onto the page a customer receives |
//! | `pg` | PostgreSQL persistence for the customer/SEPA satellites |
//! | `sepa` | pain.001/.007/.008, camt and pain.002 |
//! | `sperr` | the §§ 41f/41g EnWG disconnection sequence |

pub mod clients;
pub mod config;
pub mod handlers;
pub mod ledger;
pub mod mahnung;
pub mod mcp_server;
pub mod pg;
pub mod sepa;
pub mod sperr;
