#![deny(unsafe_code)]
//! `netzbilanzd` — the NB-role outbound billing daemon.
//!
//! Port: `:8680`
//!
//! Generates, checks and dispatches the invoices a Netzbetreiber owes its
//! counterparties: Netznutzungsentgelt and Konzessionsabgabe (NN-Rechnung,
//! PID 31002), the Mehr-/Mindermengensaldo (31005), the MSB-Rechnung (31009,
//! issued by the Messstellenbetreiber) and GeLi Gas abrechnungswürdige
//! Handlungen (31011); and it carries the Redispatch 2.0 cost sheets.
//!
//! ## Crate layout
//!
//! | Module | Purpose |
//! |---|---|
//! | [`request`] | the billing request model — one variant per settlement kind |
//! | [`billing`] | the seam between a request and the pure `grid-billing` engine |
//! | [`pg`] | tenant-scoped persistence, invoice numbering, lifecycle transitions |
//! | [`handlers`] | the billing REST surface |
//! | [`autorun`] | convenience endpoints that source their inputs from `edmd` / `marktd` |
//! | [`kostenblatt`] | Redispatch 2.0 Kostenblatt and §13a Vergütung |
//! | [`ausfallarbeit_api`] | the stateless BilAReM Kap.-3 compute surface |
//! | [`mcp_server`] | read-only MCP tooling |
//! | [`config`] | configuration |

pub mod ausfallarbeit_api;
pub mod autorun;
pub mod billing;
pub mod config;
pub mod handlers;
pub mod kostenblatt;
pub mod mcp_server;
pub mod pg;
pub mod request;
