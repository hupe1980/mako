//! Tower rate-limiting middleware backed by the `governor` GCRA algorithm.
//!
//! Enabled by the `rate-limit` Cargo feature.
//!
//! Two limiters are available:
//!
//! - [`crate::ServiceBuilder::with_rate_limit`] — one global bucket across all
//!   requests. Protects the process from total overload.
//! - [`crate::ServiceBuilder::with_caller_rate_limit`] — one bucket per caller,
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
//! per_caller_requests_per_second = 100
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
//!     .with_rate_limit(&RateLimitConfig::default())
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
    /// Metered ingest is bursty by nature — an MSCONS batch or an `IoT` gateway
    /// flushing a backlog arrives all at once — so a burst allowance below the
    /// sustained rate would reject legitimate traffic that fits comfortably
    /// within the hourly budget.
    #[serde(default = "default_burst")]
    pub burst: u32,
    /// Sustained request rate allowed to any single tenant.
    #[serde(default = "default_per_caller_rps")]
    pub per_caller_requests_per_second: u32,
}

fn default_burst() -> u32 {
    1_000
}

fn default_per_caller_rps() -> u32 {
    100
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            requests_per_second: 500,
            burst: default_burst(),
            per_caller_requests_per_second: default_per_caller_rps(),
        }
    }
}

/// Identify the caller a per-caller bucket should be keyed on.
///
/// **The peer address, and nothing the caller can choose.** This layer runs
/// before authentication — that is the point of it, since rejecting a flood
/// after verifying its signatures costs exactly what the flood is trying to
/// spend — so every credential it can see is unverified.
///
/// Keying on the presented bearer token therefore defeats the limiter outright
/// rather than sharpening it: a client sending `Authorization: Bearer
/// <fresh random>` on every request lands in a new bucket every time and is
/// never limited, while also adding a permanent entry to the keyed state on
/// each one. A token is a claim, not an identity, until something checks it.
///
/// Falls back to a single shared key when the peer address is unavailable —
/// bounded together is better than unbounded. A deployment behind a proxy
/// therefore wants the peer address preserved (`ConnectInfo` populated via
/// `into_make_service_with_connect_info`), or it gets one bucket for everyone;
/// the alternative, trusting a forwarded-for header, is caller-chosen again.
///
/// Per-**tenant** limiting is a different layer and belongs after the token is
/// verified, where the tenant is a fact rather than an assertion.
#[cfg(feature = "rate-limit")]
#[must_use]
pub fn caller_key(req: &axum::extract::Request) -> String {
    req.extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        .map_or_else(|| "anonymous".to_owned(), |ci| format!("ip:{}", ci.0.ip()))
}

/// How many admitted checks pass between sweeps of a keyed limiter's state.
///
/// `governor`'s keyed store grows one entry per distinct key and is pruned only
/// by an explicit `retain_recent()`; nothing calls it on a timer. Sweeping on a
/// request count rather than a clock keeps the cost proportional to traffic and
/// needs no background task to own — and therefore no shutdown path to get
/// wrong.
#[cfg(feature = "rate-limit")]
pub const PRUNE_EVERY: u64 = 1024;

/// Whether this call is the one that should sweep the keyed state.
///
/// Counts with `Relaxed` ordering: the sweep is a housekeeping hint, so an
/// occasional lost or duplicated tick costs nothing and the counter must not
/// become a synchronisation point on the hot path.
#[cfg(feature = "rate-limit")]
#[must_use]
pub fn prune_due(calls: &std::sync::atomic::AtomicU64) -> bool {
    calls
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .is_multiple_of(PRUNE_EVERY)
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

#[cfg(all(test, feature = "rate-limit"))]
mod tests {
    use super::*;
    use axum::extract::{ConnectInfo, Request};
    use std::net::SocketAddr;

    fn req(peer: Option<&str>, bearer: Option<&str>) -> Request {
        let mut r = Request::new(axum::body::Body::empty());
        if let Some(b) = bearer {
            r.headers_mut().insert(
                axum::http::header::AUTHORIZATION,
                format!("Bearer {b}").parse().expect("header"),
            );
        }
        if let Some(p) = peer {
            let addr: SocketAddr = p.parse().expect("addr");
            r.extensions_mut().insert(ConnectInfo(addr));
        }
        r
    }

    /// A caller cannot mint a bucket by changing its credential.
    ///
    /// This is the whole property. The limiter runs before authentication, so
    /// the token is an unverified assertion; keying on it means
    /// `Authorization: Bearer <fresh random>` per request lands in a new bucket
    /// every time and the limiter never fires, while adding one permanent entry
    /// to the keyed state per request.
    #[test]
    fn the_key_ignores_the_presented_token() {
        let a = caller_key(&req(Some("203.0.113.7:4444"), Some("token-one")));
        let b = caller_key(&req(Some("203.0.113.7:5555"), Some("token-two")));
        assert_eq!(a, b, "two credentials from one peer share one bucket");
        assert_eq!(a, "ip:203.0.113.7");
        assert!(!a.contains("tok"), "no credential-derived component: {a}");
    }

    /// Distinct peers still get distinct buckets.
    #[test]
    fn distinct_peers_get_distinct_buckets() {
        let a = caller_key(&req(Some("203.0.113.7:4444"), None));
        let b = caller_key(&req(Some("198.51.100.9:4444"), None));
        assert_ne!(a, b);
    }

    /// With no peer address the callers share one bucket rather than escaping.
    ///
    /// Bounded together is the safe direction: a deployment that loses
    /// `ConnectInfo` gets a limiter that is too coarse, not one that is absent.
    #[test]
    fn an_unknown_peer_falls_back_to_one_shared_bucket() {
        assert_eq!(caller_key(&req(None, Some("anything"))), "anonymous");
        assert_eq!(caller_key(&req(None, None)), "anonymous");
    }

    /// The sweep fires on the first call and then every `PRUNE_EVERY`.
    #[test]
    fn the_prune_counter_fires_on_schedule() {
        let calls = std::sync::atomic::AtomicU64::new(0);
        assert!(prune_due(&calls), "the first call sweeps");
        let fired = (1..PRUNE_EVERY).filter(|_| prune_due(&calls)).count();
        assert_eq!(fired, 0, "no sweep inside the interval");
        assert!(prune_due(&calls), "the interval boundary sweeps");
    }
}
