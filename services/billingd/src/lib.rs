#![deny(unsafe_code)]
// The large serde_json::json!{} macro in clients.rs needs a higher recursion limit.
#![recursion_limit = "256"]
//! `billingd` — Multi-product Energy Billing Engine (LF role).
//!
//! Port: `:9280`
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | `clients` | tarifbd / edmd / marktd HTTP clients |
//! | `config` | TOML + env configuration |
//! | `handlers` | Axum HTTP handlers |
//! | `pg` | PostgreSQL persistence |
//! | `xrechnung` | XRechnung 3.0 / ZUGFeRD 2.3 CII XML generation |
//! | `mcp_server` | MCP server (12 tools, 6 prompts) |
//!
//! The billing calculation engine itself lives in the `energy_billing` crate
//! and is used directly via `energy_billing::Product::build_engine()`.

pub mod clients;
pub mod config;
pub mod handlers;
pub mod pg;
pub mod xrechnung;

pub mod mcp_server;
