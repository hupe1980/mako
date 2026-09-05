#![deny(unsafe_code)]
//! `obsd` — Business-Process Observability daemon.
//!
//! ## Architecture
//!
//! `obsd` is an L3 application service that subscribes to `de.mako.*` events
//! from `marktd` — the types named in `[subscription].event_types`, six by
//! default — and projects them into a `ProcessProjection` read-model stored in
//! PostgreSQL. It **never** connects to `makod` directly.
//!
//! It is also a producer of two `de.obs.*` events — the Antwortfrist warning and
//! the § 7a Abs. 5 EnWG parity alert — emitted by [`worker`] to `marktd`'s
//! ingest, whose fan-out delivers them to `agentd`.
//!
//! ```text
//! makod ──(CloudEvents)──► marktd ──(webhook fan-out, subscribed types)──► obsd POST /webhook
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
//! | Method | Path | Auth | Description |
//! |--------|------|------|-------------|
//! | POST   | `/webhook` | inbound HMAC | `de.mako.*` CloudEvents from `marktd` |
//! | GET    | `/obs/processes` | OIDC + Cedar `read-process` | Query projections |
//! | GET    | `/obs/processes/{process_id}` | OIDC + Cedar `read-process` | One projection |
//! | GET    | `/obs/kpis` | OIDC + Cedar `read-kpi` | Per-PID KPIs for a month |
//! | GET    | `/obs/overdue` | OIDC + Cedar `read-overdue` | Past their Antwortfrist |
//! | GET    | `/api/v1/audit/gleichbehandlung` | OIDC + Cedar `read-kpi` | § 7a Abs. 5 EnWG parity evidence |
//! | GET    | `/obs/metrics` | none — restrict at the ingress | Business gauges, Prometheus text |
//! | POST/GET | `/mcp` | OIDC or API key + Cedar `use-mcp` | MCP Streamable HTTP |
//!
//! `/health/live`, `/health/ready` and the generic `/metrics` are mounted by
//! [`mako_service::run`], not here.
//!
//! ## Configuration
//!
//! `obsd.toml`, resolved by `mako_service::load_config` with an `OBSD_*` env
//! overlay and `env:VAR` indirection for secrets. There are **no command-line
//! flags**: the runner owns the lifecycle and the config file owns the settings.
//! See [`config`] for the annotated minimal file.
//!
//! ## Two deadline clocks
//!
//! Every process carries two, and `obsd` reports them as two numbers. See
//! [`mako_obs::domain`] for why conflating them is the defect this service is
//! shaped to prevent.

pub mod config;
pub mod handler;
pub mod mcp_server;
pub mod pg;
pub mod server;
pub mod worker;
