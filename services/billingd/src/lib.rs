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
//! | `billing_runs` | § 40b EnWG scheduled monthly runs |
//! | `clients` | tarifbd / edmd / marktd / vertragd HTTP clients |
//! | `config` | TOML + env configuration |
//! | `document` | The customer-facing document: template contract, Typst renderer, ZUGFeRD carrier, publish gate |
//! | `einvoice` | EN 16931 semantic model — built, stored, and rendered to CII / UBL |
//! | `handlers` | Axum HTTP handlers |
//! | `pg` | PostgreSQL persistence |
//! | `risk` | Deterministic release scoring |
//! | `template_store` | Content-addressed, append-only document templates |
//! | `mcp_server` | MCP server (12 tools, 6 prompts) |
//!
//! The billing calculation engine itself lives in the `energy_billing` crate
//! and is used directly via `energy_billing::Product::build_engine()`.

pub mod billing_runs;
pub mod clients;
pub mod config;
pub mod document;
pub mod einvoice;
pub mod handlers;
pub mod pg;
pub mod risk;
pub mod template_store;

pub mod mcp_server;
