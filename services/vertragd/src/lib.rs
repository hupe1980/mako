#![deny(unsafe_code)]
//! `vertragd` — retail contract and customer management for the LF role.
//!
//! Port: `:9780`
//!
//! `vertragd` owns the B2C and B2B contract lifecycle, the customer master data
//! every downstream document is addressed to, and the mapping from an OIDC
//! identity to the market locations it may see.
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`domain`] | The statutory rules — notice periods, price-change notice, term limits — as pure functions |
//! | [`config`] | Deployment configuration |
//! | [`events`] | `de.vertrag.*` CloudEvent construction and MaKo outcome parsing |
//! | [`outbound`] | Durable queue for every call owed to processd / edmd / productd / accountingd |
//! | [`workers`] | Daily lifecycle workers (Tarifwechsel, Verlängerung, Ablauf) |
//! | [`pg`] | PostgreSQL data access |
//! | [`handlers`] | HTTP surface |
//! | [`mcp_server`] | Read-only MCP tools and prompts |
//! | [`angebot_bo4e`] | Reading an accepted quotation out of its BO4E `Angebot` |
//! | [`dokumente`] | The § 41 Abs. 5 EnWG price-change notice, rendered and delivered through `outputd` |
//!
//! ## The two durability rails
//!
//! Nothing `vertragd` owes anyone is dispatched from a detached task. A
//! contract change and the obligations that follow from it commit together:
//!
//! - **customer-facing notices** go into the shared CloudEvent outbox
//!   (`mako_service::outbox`) and are delivered to the ERP with retry — and,
//!   where `outputd_url` is configured, are additionally rendered and sent as
//!   documents with per-channel delivery evidence ([`dokumente`]);
//! - **service-to-service calls** go into [`outbound`] and are performed by its
//!   worker with backoff and a dead-letter.
//!
//! A crash therefore costs a retry, never an obligation.

pub mod angebot_bo4e;
pub mod config;
pub mod dokumente;
pub mod domain;
pub mod events;
pub mod handlers;
pub mod mcp_server;
pub mod outbound;
pub mod pg;
pub mod workers;
