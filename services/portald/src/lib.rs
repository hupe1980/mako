#![deny(unsafe_code)]
//! `portald` — Customer Portal read-model gateway (LF role), port `:9480`.
//!
//! Aggregates Lastgang (`edmd`), invoices (`billingd`), the document inbox
//! (`outputd`), account ledger (`accountingd`), supply status (`marktd`) and
//! EEG settlement (`einsd`) into one customer-facing REST API, and proxies the
//! § 41 EnWG self-service writes to `vertragd` and `accountingd`.
//!
//! # Records and documents are two lists
//!
//! `/invoices` is what `billingd` *calculated* — drafts the risk gate is
//! holding included. `/dokumente` is what the customer was *sent*, byte for
//! byte, with the delivery evidence beside it. An inbox shows the second;
//! opening a document there records the read receipt a § 41f EnWG dispute asks
//! about.
//!
//! # Stateless by construction
//!
//! No database, no cache, no session store: every response is assembled from
//! the authoritative services on the request path, so a portal reply can never
//! be staler than they are and replicas need no coordination.
//!
//! # One authorization gate
//!
//! `portald` holds no customer↔MaLo map and verifies no tokens itself.
//! [`auth::authorize`] forwards the customer's bearer token to `vertragd`,
//! which owns the OIDC verifier and the customer record, and relays the
//! verdict. Handlers receive the resulting [`auth::PortalAuthCtx`] by value, so
//! a route cannot serve customer data without having asked.
//!
//! | Module | Purpose |
//! |---|---|
//! | [`auth`] | The single customer-authorization gate |
//! | [`clients`] | Thin upstream HTTP clients |
//! | [`config`] | `portald.toml` shape |
//! | [`handlers`] | Route handlers |
//! | [`mcp_server`] | Operator-facing read-only MCP surface |
//! | [`server`] | Router assembly + [`mako_service::Daemon`] impl |

pub mod auth;
pub mod clients;
pub mod config;
pub mod handlers;
pub mod mcp_server;
pub mod server;
