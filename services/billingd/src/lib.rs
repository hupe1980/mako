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
//! | `clients` | productd / edmd / marktd / vertragd HTTP clients |
//! | `config` | TOML + env configuration |
//! | `einvoice` | EN 16931 semantic model — built, stored, and rendered to CII / UBL |
//! | `error` | `BillingError` — the one coded JSON error envelope every route answers with |
//! | `handlers` | Axum HTTP handlers |
//! | `pg` | PostgreSQL persistence |
//! | `risk` | Deterministic release scoring |
//! | `mcp_server` | MCP server (11 read-only tools, 6 prompts) |
//!
//! The billing calculation engine itself lives in the `energy_billing` crate
//! and is used directly via `energy_billing::Product::build_engine()`. Document
//! *rendering* — Typst templates, the ZUGFeRD carrier, the publish gate, and the
//! projection of the semantic model onto what a template may print — lives in
//! `outputd`. billingd sends it the **EN 16931 model** plus the CII payload and
//! pins the template hash it answers with. Projecting the view here instead
//! would be two implementations of one contract, with the publish gate proving
//! templates against one and production feeding them the other.

pub mod billing_runs;
pub mod clients;
pub mod config;
pub mod einvoice;
pub mod error;
pub mod handlers;
pub mod pg;
pub mod risk;

pub mod mcp_server;
