//! Shared service infrastructure for mako daemons.
//!
//! Provides:
//! - [`load_config`] — TOML config loading with `env:VAR_NAME` resolution
//! - [`ServiceBuilder`] — composable Axum router builder
//! - [`health`] — `/health/live` and `/health/ready` route helpers
//! - [`webhook`] — HMAC-SHA256 signature verification helpers
//! - [`telemetry`] — structured logging + optional OpenTelemetry OTLP export
//! - [`cedar`] — Cedar ABAC policy enforcement (feature-gated: `cedar`)
//! - [`oidc`] — OIDC/JWT verification + `Claims` Axum extractor (feature-gated: `oidc`)
//! - [`metrics`] — Prometheus `/metrics` handler + recording middleware (feature-gated: `metrics`)
//! - [`rate_limit`] — Tower rate-limiter config (feature-gated: `rate-limit`)

#![deny(unsafe_code)]

pub mod builder;
pub mod config;
pub mod event_bus;
pub mod health;
pub mod http;
pub mod shutdown;
pub mod telemetry;
pub mod webhook;

/// Unified MCP server authentication (OIDC+Cedar, API-key, dev mode).
/// Feature-gated: requires both `cedar` and `oidc` features.
#[cfg(all(feature = "cedar", feature = "oidc"))]
pub mod mcp_auth;

#[cfg(feature = "cedar")]
pub mod cedar;

#[cfg(feature = "oidc")]
pub mod oidc;

#[cfg(feature = "metrics")]
pub mod metrics;

#[cfg(feature = "rate-limit")]
pub mod rate_limit;

pub use mako_plugin::{PluginContext, PluginError, PluginManifest, PluginRegistry};

pub use builder::ServiceBuilder;
pub use config::{ConfigError, DatabaseConfig, HttpConfig, load_config};
pub use telemetry::{OtelConfig, OtelGuard, init_tracing, init_tracing_from_env};

#[cfg(feature = "metrics")]
pub use metrics::init_metrics;

#[cfg(feature = "rate-limit")]
pub use rate_limit::RateLimitConfig;
