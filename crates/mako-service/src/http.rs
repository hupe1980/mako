//! Shared HTTP client construction for inter-service calls.
//!
//! All mako daemons that call peer services (e.g. `processd` → `makod`, `edmd` → `marktd`)
//! should use [`default_client`] rather than `reqwest::Client::new()`.
//!
//! `reqwest::Client::new()` has no connection timeout — a SYN to an unreachable
//! host can block for several minutes, stalling pod startup and preventing
//! the liveness probe from responding.  [`default_client`] sets conservative
//! timeouts suitable for cluster-internal traffic.

/// Build the default inter-service `reqwest::Client`.
///
/// Settings:
/// - **Request timeout**: 30 s (including response-body read)
/// - **Connect timeout**: 5 s (TCP handshake deadline)
/// - **Pool max idle per host**: 4 (sufficient for low-concurrency service calls)
///
/// # Panics
///
/// Panics only if the underlying TLS/native-TLS stack fails to initialise,
/// which cannot happen with the default `reqwest` feature set on any supported
/// platform.
#[must_use]
pub fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(5))
        .pool_max_idle_per_host(4)
        .build()
        .expect("reqwest default_client: TLS initialisation is infallible on supported platforms")
}
