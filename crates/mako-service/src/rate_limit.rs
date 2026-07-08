//! Tower rate-limiting middleware backed by the `governor` GCRA algorithm.
//!
//! Enabled by the `rate-limit` Cargo feature.
//!
//! Adds a global GCRA rate limiter to the service. The limiter applies
//! across **all** requests — use it to protect against accidental runaway
//! clients, not as a per-tenant quota system.
//!
//! ## TOML configuration
//!
//! ```toml
//! [rate_limit]
//! requests_per_second = 500
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
///
/// Defaults to 500 requests per second with a burst tolerance of ×2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    /// Maximum sustained request rate (requests per second).
    pub requests_per_second: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 500,
        }
    }
}
