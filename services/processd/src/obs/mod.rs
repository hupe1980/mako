//! The process read model: every `de.mako.*` event projected into one
//! `process_projections` row per business process.
//!
//! It answers what the `makod` Prometheus metrics cannot: which MaLo is stuck
//! past its Antwortfrist, which counterparty has the highest rejection rate,
//! what the per-PID KPIs of a month are, and how the network arm treated its
//! own undertaking's Lieferanten against third parties (§ 7a Abs. 5 EnWG).
//!
//! ```text
//! makod ──(CloudEvents)──► marktd ──(fan-out)──► processd POST /webhook
//!                                                        │
//!                                              obs::project::project
//!                                                        │
//!                                               process_projections
//! ```
//!
//! `POST /webhook` is processd's one inbound door: it verifies the signature
//! once and hands each event to the projection before the decision modules
//! see it. The same function serves [`rebuild`](crate::obs::rebuild), which replays `marktd`'s
//! event log into an empty or damaged table.
//!
//! ## Routes
//!
//! | Method | Path | Cedar action | Description |
//! |---|---|---|---|
//! | GET  | `/obs/processes` | `read-process` | Query projections |
//! | GET  | `/obs/processes/{process_id}` | `read-process` | One projection |
//! | GET  | `/obs/kpis` | `read-kpi` | Per-PID KPIs for a month |
//! | GET  | `/obs/overdue` | `read-overdue` | Past their Antwortfrist |
//! | GET  | `/api/v1/audit/gleichbehandlung` | `read-kpi` | § 7a Abs. 5 EnWG parity evidence |
//! | POST | `/api/v1/obs/rebuild` | `rebuild-projection` | Replay `marktd`'s event log |
//! | GET  | `/obs/metrics` | none — restrict at the ingress | Business gauges, Prometheus text |
//!
//! ## Event mapping
//!
//! | `ce_type`                    | Resulting `ProcessState` |
//! |------------------------------|--------------------------|
//! | `de.mako.process.initiated`  | `Initiated`              |
//! | `de.mako.aperak.accepted`    | `Running`                |
//! | `de.mako.aperak.rejected`    | `Rejected` + ERC code    |
//! | `de.mako.aperak.timeout`     | `AperakTimeout`          |
//! | `de.mako.process.completed`  | `Completed`              |
//! | `de.mako.process.failed`     | `Failed`                 |
//!
//! ## Two deadline clocks
//!
//! Every process carries two, and the read model reports them as two numbers.
//! See [`domain`](crate::obs::domain) for why conflating them is the defect this module is shaped
//! to prevent. The business windows come from `mako_fristen::antwort`, never
//! from a literal beside a call site.
//!
//! ## Producers
//!
//! [`worker`](crate::obs::worker) emits `de.obs.deadline.approaching` and `de.obs.stp.parity.alert`
//! to `marktd`'s ingest when `[obs] outbound_url` is configured.

pub mod domain;
pub mod error;
pub(crate) mod mcp;
pub mod pg;
pub mod project;
pub mod rebuild;
pub mod repository;
pub mod rest;
pub mod worker;

pub use rest::router;

#[cfg(test)]
mod testing;
