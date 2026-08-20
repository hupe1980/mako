//! `invoicd` — INVOIC plausibility check and settlement for the Lieferant role.
//!
//! # What it does
//!
//! Every INVOIC a market partner sends the LF arrives as a
//! `de.mako.process.initiated` CloudEvent from `marktd`. `invoicd` checks it
//! against the reference data the PID calls for, records the result, and
//! answers the counterparty — accept or dispute — through `makod`.
//!
//! ```text
//! marktd ──POST /webhook──► invoicd ──route_for(pid)──► check
//!                              │                          │
//!                        marktd price sheets ─────────────┘
//!                              │
//!                    persist receipt (§ 147 AO)
//!                              │
//!                     makod ◄──answer command
//!                              │
//!                       ERP webhook ◄── de.invoic.receipt.*
//! ```
//!
//! # Two invariants
//!
//! **Persist before dispatch.** A received INVOIC is a Buchungsbeleg (§ 147
//! Abs. 3 AO, § 14b UStG, 8-year retention). The receipt is written before the
//! answer is sent, and a failed write aborts the dispatch rather than answering
//! an invoice that is not in the audit trail.
//!
//! **Nothing is dropped.** An event that cannot become a receipt goes to
//! `invoic_dlq` with the reason, and `invoicd_dlq_open_total` counts them.
//!
//! # Modules
//!
//! | Module | Purpose |
//! |---|---|
//! | [`routing`] | Which PID is checked how and answered with what |
//! | [`handler`] | The inbound webhook pipeline |
//! | [`selbstausstellen`] | The self-issued Mehrmengen-Rechnung (PID 31006) |
//! | [`server`] | Router, operator API, [`mako_service::Daemon`] impl |
//! | [`pg`] | The receipt store |
//! | [`erp_outbox`] | Retry worker for ERP notifications |
//! | [`payment_overdue`] | `de.invoic.payment.overdue` worker |
//! | [`mcp_server`] | Read-only MCP surface |
//!
//! # Configuration
//!
//! Loaded via `mako_service::load_config("invoicd")`: `invoicd.toml` (path
//! overridable with `INVOICD_CONFIG`) as the base layer, with any key
//! overridable by an `INVOICD_`-prefixed environment variable (`__` separates
//! nested sections, e.g. `INVOICD_DATABASE__URL`). See [`config`].
//!
//! At startup `invoicd` calls `PUT /api/v1/subscriptions/invoicd` on `marktd`
//! to register for `de.mako.process.initiated`, filtered to the PIDs in
//! [`routing::ROUTES`]. The `PUT` is idempotent and safe on every restart.

#![deny(unsafe_code)]

pub mod config;
pub mod erp_outbox;
pub mod handler;
pub mod mcp_server;
pub mod payment_overdue;
pub mod pg;
pub mod routing;
pub mod selbstausstellen;
pub mod server;
