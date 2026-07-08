#![deny(unsafe_code)]
//! `edmd` — Energy Data Management daemon.
//!
//! ## Architecture
//!
//! `edmd` is an L3 application service that receives MSCONS process-completion
//! events from `marktd` via webhook fan-out and stores meter data receipts in
//! a PostgreSQL/TimescaleDB database.  It **never** connects to `makod` directly.
//!
//! ```text
//! makod ──(CloudEvents)──► marktd ──(webhook fan-out)──► edmd POST /webhook
//!                                                             │
//!                                                    filter makopid ∈ MSCONS_PIDS
//!                                                             │
//!                                                   TimeSeriesRepository::store_receipt()
//!                                                             │
//!                                                       PostgreSQL / TimescaleDB
//! ```
//!
//! ## Routes
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | POST   | `/webhook` | Inbound `MarktEvent` from `marktd` |
//! | GET    | `/api/v1/deliveries/{malo_id}` | Meter data receipts for a MaLo |
//! | GET    | `/api/v1/imbalance/{malo_id}/{year}/{month}` | Mehr-/Mindermengen |
//! | GET    | `/health/live` | Liveness probe |
//! | GET    | `/health/ready` | Readiness probe |
//!
//! ## Configuration
//!
//! | Flag | Env | Default |
//! |---|---|---|
//! | `--listen` | `EDMD_LISTEN` | `0.0.0.0:8380` |
//! | `--database-url` | `EDMD_DATABASE_URL` | *(required)* |
//! | `--marktd-url` | `EDMD_MARKTD_URL` | `http://localhost:8180` | URL of `marktd` for subscription registration |
//! | `--subscriber-id` | `EDMD_SUBSCRIBER_ID` | `edmd` |
//! | `--webhook-url` | `EDMD_WEBHOOK_URL` | *(required)* |
//! | `--webhook-secret` | `EDMD_WEBHOOK_SECRET` | *(optional)* |
//! | `--inbound-secret` | `EDMD_INBOUND_SECRET` | *(optional)* |

pub mod config;
pub mod handler;
pub mod mcp_server;
pub mod pg;
pub mod server;
