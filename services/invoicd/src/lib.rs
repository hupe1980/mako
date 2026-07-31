//! `invoicd` — INVOIC plausibility-check daemon for the Lieferant (LF) role.
//!
//! ## Architecture
//!
//! ```text
//! marktd ──(POST /webhook)──► invoicd handler
//!                               │
//!                     parse MarktEvent JSON
//!                               │
//!                     ┌─────────▼──────────┐
//!                     │  ce_type routing    │
//!                     └─────────┬──────────┘
//!                               │
//!          ┌────────────────────┼───────────────────────────┐
//!          ▼                    ▼                           ▼
//!  "de.mako.process.   "de.mako.process.       all others
//!   initiated"         completed" + pid==27003  → 204 No Content
//!  + pid in INVOIC set → fetch Preisblatt from marktd
//!          │
//!  run InvoicCheckEngine::check()
//!          │
//!   ┌──────┴──────┐
//!   │             │
//!  Ok           Dispute
//!   │             │
//!  POST         POST
//!  /api/v1/     /api/v1/
//!  commands     commands
//!  (annehmen)   (ablehnen)
//! ```
//!
//! ## Configuration
//!
//! Loaded via `mako_service::load_config("invoicd")`: `invoicd.toml` (path
//! overridable with `INVOICD_CONFIG`) as the base layer, with any TOML key
//! overridable by an `INVOICD_`-prefixed environment variable (`__` separates
//! nested sections, e.g. `INVOICD_DATABASE__URL`). See [`config`] for the full
//! shape; the `[database]` block carries the pool tuning + `application_name`.
//!
//! ## Subscription registration
//!
//! At startup `invoicd` calls `PUT /api/v1/subscriptions/invoicd` on `marktd`
//! to ensure it receives `de.mako.process.initiated` events.  The idempotent
//! `PUT` is safe to call on every restart.

pub mod config;
pub mod erp_outbox;
pub mod handler;
pub mod mcp_server;
pub mod payment_overdue;
pub mod pg;
pub mod server;
