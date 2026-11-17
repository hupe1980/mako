#![deny(unsafe_code)]
#![expect(
    missing_docs,
    reason = "publish = false: a daemon's library exists for its binary and its tests, not as an API"
)]
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
//! **One exception, by design:** `GET /nachrechnen/{token}` ([`nachrechnen`])
//! serves the masked recomputation of one invoice without a customer session.
//! The verify token printed on the invoice is the credential, and billingd's
//! masked projection — no name, no address, the MaLo-ID masked — is all it
//! reaches.
//!
//! | Module | Purpose |
//! |---|---|
//! | [`auth`] | The single customer-authorization gate |
//! | [`clients`] | Thin upstream HTTP clients |
//! | [`config`] | `portald.toml` shape |
//! | [`handlers`] | Route handlers |
//! | [`nachrechnen`] | The public, token-addressed Rechnung-nachrechnen page |
//! | [`openapi`] | Every `/api/v1` route and the OpenAPI document describing them |
//! | [`server`] | Router assembly + [`mako_service::Daemon`] impl |

pub mod auth;
pub mod clients;
pub mod config;
pub mod handlers;
pub mod nachrechnen;
pub mod openapi;
pub mod server;
