//! Shared service infrastructure for mako daemons.
//!
//! The one entry point most services need is [`run`] — it owns the whole daemon
//! lifecycle (tracing, tuned pool, migrations, real readiness, graceful serve),
//! so a `main` is just `run::<MyDaemon>().await`. See [`service`].
//!
//! The rest are composable pieces:
//! - [`service`] — the [`Daemon`] trait + [`run`] lifecycle owner
//! - [`config`] — layered TOML/env config ([`load_config`], [`DatabaseConfig`])
//! - [`error`] — shared [`ApiError`] / [`ApiResult`] with `IntoResponse`
//! - [`cloudevent`] — canonical `CloudEvent` envelope + signed publisher
//! - [`outbox`] — transactional outbox (persist-before-dispatch) + drain worker
//! - [`webhook`] — the one HMAC-SHA256 signer/verifier
//! - [`ServiceBuilder`] — composable Axum router builder (infra routes)
//! - [`health`] — `/health/live` and `/health/ready` route helpers
//! - [`telemetry`] — structured logging + optional OpenTelemetry OTLP export
//! - [`cedar`] — Cedar ABAC policy enforcement (feature-gated: `cedar`)
//! - [`oidc`] — OIDC/JWT verification + `Claims` Axum extractor (feature-gated: `oidc`)
//! - [`metrics`] — Prometheus `/metrics` handler + middleware (feature-gated: `metrics`)
//! - [`rate_limit`] — Tower rate-limiter config (feature-gated: `rate-limit`)

#![deny(unsafe_code)]
#![warn(clippy::pedantic)]
// Curated allows: these categories are pervasive across the pre-existing auth /
// cedar / oidc modules or are subjective; pedantic stays on to catch everything
// else in new code.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::large_futures,
    clippy::manual_let_else,
    clippy::items_after_statements,
    clippy::module_name_repetitions,
    clippy::must_use_candidate,
    clippy::doc_markdown
)]

pub mod builder;
pub mod cloudevent;
pub mod config;
pub mod error;
pub mod health;
pub mod http;
pub mod outbox;
pub mod service;
pub mod shutdown;
pub mod telemetry;
pub mod webhook;

/// Unified MCP server authentication (OIDC+Cedar, API-key, dev mode).
/// Feature-gated: requires both `cedar` and `oidc` features.
#[cfg(all(feature = "cedar", feature = "oidc"))]
pub mod mcp_auth;

/// Schema-validated Cedar engine with named-key / OIDC bearer authentication.
/// Feature-gated: requires both `cedar` and `oidc` features.
#[cfg(all(feature = "cedar", feature = "oidc"))]
pub mod cedar_schema;

#[cfg(feature = "cedar")]
pub mod cedar;

#[cfg(feature = "oidc")]
pub mod oidc;

#[cfg(feature = "metrics")]
pub mod metrics;

#[cfg(feature = "rate-limit")]
pub mod rate_limit;

pub use builder::ServiceBuilder;
pub use cloudevent::{CloudEvent, PublishError, post_ce_with_retry, source};
pub use config::{ConfigError, DatabaseConfig, HttpConfig, load_config};
pub use error::{ApiError, ApiResult};
pub use service::{Daemon, ServiceConfig, ServiceContext, run};
pub use telemetry::{
    ExtraLayer, OtelConfig, OtelGuard, init_tracing, init_tracing_from_env,
    init_tracing_from_env_with, init_tracing_with,
};

#[cfg(feature = "metrics")]
pub use metrics::init_metrics;

#[cfg(feature = "rate-limit")]
pub use rate_limit::RateLimitConfig;
