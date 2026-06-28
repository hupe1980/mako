//! BDEW MaKo AS4 Axum server helpers.
//!
//! Requires the **`server`** feature (`asx-rs/server`).
//!
//! [`bdew_router_config`] returns the [`RouterConfig`] recommended for BDEW
//! MaKo AS4 deployments.  Pass it to your `as4_router` call:
//!
//! ```rust,ignore
//! use mako_as4::server::bdew_router_config;
//! use asx_rs::transport::server::as4_router;
//!
//! let app = as4_router(handler, "/as4/inbox", bdew_router_config());
//! ```

use std::time::Duration;

pub use asx_rs::transport::server::RouterConfig;

/// Returns the [`RouterConfig`] recommended for BDEW MaKo AS4 inbound endpoints.
///
/// | Parameter | Value | Rationale |
/// |---|---|---|
/// | `body_read_timeout` | 120 s | Accommodates large EDIFACT Fahrplan payloads over BDEW VPN/HTTPS (~50 MiB at 10 Mbit/s) |
/// | `max_body_bytes` | 64 MiB | BDEW AS4 recommendation; increase for Redispatch 2.0 payloads if needed |
pub fn bdew_router_config() -> RouterConfig {
    RouterConfig {
        body_read_timeout: Duration::from_secs(120),
        max_body_bytes: 64 * 1024 * 1024,
    }
}
