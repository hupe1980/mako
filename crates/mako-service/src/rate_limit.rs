//! Tower rate-limiting middleware backed by the `governor` GCRA algorithm.
//!
//! Enabled by the `rate-limit` Cargo feature.
//!
//! Two limiters are available:
//!
//! - [`crate::ServiceBuilder::with_rate_limit`] — one global bucket across all
//!   requests. Protects the process from total overload.
//! - [`crate::ServiceBuilder::with_tenant_rate_limit`] — one bucket per caller,
//!   keyed on a hash of the bearer token (falling back to peer address for
//!   unauthenticated routes). A global bucket alone lets one busy tenant consume
//!   the whole allowance and starve every other tenant on a shared deployment,
//!   which a keyed bucket prevents.
//!
//! Apply both: the per-tenant limit bounds any single caller, the global limit
//! bounds their sum.
//!
//! ## TOML configuration
//!
//! ```toml
//! [rate_limit]
//! requests_per_second = 500
//! burst = 1000
//! per_tenant_requests_per_second = 100
//! ```
//!
//! ## Usage
//!
//! ```rust,no_run
//! use mako_service::ServiceBuilder;
//! use mako_service::RateLimitConfig;
//!
//! let app = ServiceBuilder::new()
//!     .with_health(|| async { true })
//!     .with_rate_limit(RateLimitConfig::default())
//!     .build();
//! ```

use serde::{Deserialize, Serialize};

/// Rate-limiting configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum sustained request rate across the whole service.
    pub requests_per_second: u32,
    /// Requests admitted in a burst before the sustained rate applies.
    ///
    /// Metered ingest is bursty by nature — an MSCONS batch or an IoT gateway
    /// flushing a backlog arrives all at once — so a burst allowance below the
    /// sustained rate would reject legitimate traffic that fits comfortably
    /// within the hourly budget.
    #[serde(default = "default_burst")]
    pub burst: u32,
    /// Sustained request rate allowed to any single tenant.
    #[serde(default = "default_per_tenant_rps")]
    pub per_tenant_requests_per_second: u32,
}

fn default_burst() -> u32 {
    1_000
}

fn default_per_tenant_rps() -> u32 {
    100
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 500,
            burst: default_burst(),
            per_tenant_requests_per_second: default_per_tenant_rps(),
        }
    }
}

/// Identify the caller a per-caller bucket should be keyed on.
///
/// Keys on a hash of the presented bearer token — one bucket per credential —
/// so a client cannot escape its bucket by changing source address. (This is
/// finer-grained than a per-tenant key: two tokens of the same tenant get two
/// buckets.) Falls back to the peer address for unauthenticated routes, and to
/// a single shared key when neither is available — bounded together is better
/// than unbounded.
#[cfg(feature = "rate-limit")]
#[must_use]
pub fn caller_key(req: &axum::extract::Request) -> String {
    use axum::http::header::AUTHORIZATION;

    // The bearer token is hashed, not stored: the key lives in a map for the
    // process lifetime, and a raw credential does not belong there.
    if let Some(auth) = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        use std::hash::{Hash as _, Hasher as _};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        auth.hash(&mut h);
        return format!("tok:{:016x}", h.finish());
    }

    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map_or_else(|| "anonymous".to_owned(), |ci| format!("ip:{}", ci.0.ip()))
}

/// Build the `429` a rejected request receives.
///
/// `Retry-After` is rounded up to whole seconds, since the header has
/// second granularity and rounding down would invite an early retry that is
/// rejected again.
#[cfg(feature = "rate-limit")]
#[must_use]
pub fn too_many_requests(wait: std::time::Duration, key: &str) -> axum::response::Response {
    use axum::response::IntoResponse as _;

    let secs = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
    tracing::warn!(
        rate_limit_key = %key,
        retry_after_secs = secs,
        "rate limit exceeded"
    );

    (
        axum::http::StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, secs.to_string())],
        axum::Json(serde_json::json!({
            "error": "rate limit exceeded",
            "retry_after_secs": secs,
        })),
    )
        .into_response()
}
