#![deny(unsafe_code)]
//! `obsd` — Business-Process Observability daemon.
//!
//! ## Architecture
//!
//! `obsd` is an L3 application service that subscribes to **all** `de.mako.*`
//! events from `marktd` and projects them into a `ProcessProjection` read-model
//! stored in PostgreSQL.  It **never** connects to `makod` directly.
//!
//! ```text
//! makod ──(CloudEvents)──► marktd ──(webhook fan-out, all events)──► obsd POST /webhook
//!                                                                         │
//!                                                               project ce_type → state
//!                                                                         │
//!                                                            ProcessProjectionRepository::upsert()
//!                                                                         │
//!                                                                    PostgreSQL
//! ```
//!
//! ## Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | POST   | `/webhook` | Inbound `MarktEvent` from `marktd` (all event types) |
//! | GET    | `/obs/processes` | Query process projections |
//! | GET    | `/obs/processes/{process_id}` | Get single process projection |
//! | GET    | `/obs/kpis` | KPI report for a PID and period |
//! | GET    | `/obs/overdue` | Processes past their regulatory deadline |
//! | GET    | `/health/live` | Liveness probe |
//! | GET    | `/health/ready` | Readiness probe |
//!
//! ## Configuration
//!
//! | Flag | Env | Default |
//! |---|---|---|
//! | `--listen` | `OBSD_LISTEN` | `0.0.0.0:8480` |
//! | `--database-url` | `OBSD_DATABASE_URL` | *(required)* |
//! | `--marktd-url` | `OBSD_MARKTD_URL` | `http://localhost:8180` | URL of `marktd` for subscription registration |
//! | `--subscriber-id` | `OBSD_SUBSCRIBER_ID` | `obsd` |
//! | `--webhook-url` | `OBSD_WEBHOOK_URL` | *(required)* |
//! | `--webhook-secret` | `OBSD_WEBHOOK_SECRET` | *(optional)* |
//! | `--inbound-secret` | `OBSD_INBOUND_SECRET` | *(optional)* |

pub mod config;
pub mod handler;
pub mod mcp_server;
pub mod pg;
pub mod server;
