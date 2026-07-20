//! [`ServiceBuilder`] — composable Axum router builder.
//!
//! Services assemble their Axum application by combining the cross-cutting
//! infrastructure routes (health, metrics) provided by this builder with their
//! own domain routes (created separately and merged in via [`ServiceBuilder::merge`]).
//!
//! ## Example
//!
//! ```rust,no_run
//! use axum::{Router, routing::post};
//! use axum::http::StatusCode;
//! use mako_service::ServiceBuilder;
//!
//! async fn my_handler() -> StatusCode { StatusCode::NO_CONTENT }
//!
//! // Service-specific router with its own state
//! let svc: Router = Router::new().route("/webhook", post(my_handler));
//!
//! // Assemble the full application
//! let app: Router = ServiceBuilder::new()
//!     .with_health(|| async { true })
//!     .with_metrics()
//!     .merge(svc)
//!     .build();
//! ```

use std::future::Future;

use axum::{Router, routing::get};
use tower_http::trace::TraceLayer;

use crate::health::health_routes;

/// Composable Axum router builder for mako services.
pub struct ServiceBuilder {
    router: Router,
}

impl ServiceBuilder {
    /// Create a new, empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            router: Router::new(),
        }
    }

    /// Add `/health/live` (always `200 OK`) and `/health/ready` routes.
    #[must_use]
    pub fn with_health<F, Fut>(self, ready_fn: F) -> Self
    where
        F: Fn() -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = bool> + Send,
    {
        Self {
            router: self.router.merge(health_routes(ready_fn)),
        }
    }

    /// Add HTTP request/response tracing via `tower-http` [`TraceLayer`].
    #[must_use]
    pub fn with_trace_layer(self) -> Self {
        Self {
            router: self.router.layer(TraceLayer::new_for_http()),
        }
    }

    /// Add `GET /metrics`.
    ///
    /// When the `metrics` feature is active: mounts the real Prometheus handler
    /// and adds a per-request recording middleware (`mako_http_requests_total`,
    /// `mako_http_request_duration_seconds`).
    ///
    /// Without the `metrics` feature: returns a plain-text stub so callers
    /// compile unconditionally.
    #[must_use]
    pub fn with_metrics(self) -> Self {
        #[cfg(feature = "metrics")]
        let router = {
            crate::metrics::init_metrics();
            self.router
                .route("/metrics", get(crate::metrics::metrics_handler))
                .layer(axum::middleware::from_fn(
                    crate::metrics::recording_middleware,
                ))
        };
        #[cfg(not(feature = "metrics"))]
        let router = self.router.route("/metrics", get(metrics_stub));
        Self { router }
    }

    /// Add a global GCRA rate limiter (requires feature `rate-limit`).
    ///
    /// Responds with `429 Too Many Requests` when the token bucket is empty.
    /// The limiter is global across all inbound requests regardless of client,
    /// so it bounds total load but not any individual caller — pair it with
    /// [`Self::with_tenant_rate_limit`] on a multi-tenant deployment.
    #[must_use]
    #[cfg(feature = "rate-limit")]
    pub fn with_rate_limit(self, config: crate::rate_limit::RateLimitConfig) -> Self {
        use axum::{extract::Request, middleware::Next};
        use governor::{Quota, RateLimiter};
        use std::{num::NonZeroU32, sync::Arc};

        let rps = NonZeroU32::new(config.requests_per_second).unwrap_or(NonZeroU32::MIN);
        let burst = NonZeroU32::new(config.burst.max(config.requests_per_second))
            .unwrap_or(NonZeroU32::MIN);
        let limiter = Arc::new(RateLimiter::direct(
            Quota::per_second(rps).allow_burst(burst),
        ));
        Self {
            router: self.router.layer(axum::middleware::from_fn(
                move |req: Request, next: Next| {
                    let limiter = Arc::clone(&limiter);
                    async move {
                        match limiter.check() {
                            Ok(()) => next.run(req).await,
                            Err(not_until) => crate::rate_limit::too_many_requests(
                                not_until.wait_time_from(governor::clock::Clock::now(
                                    &governor::clock::DefaultClock::default(),
                                )),
                                "service",
                            ),
                        }
                    }
                },
            )),
        }
    }

    /// Add a per-tenant GCRA rate limiter (requires feature `rate-limit`).
    ///
    /// Each caller gets its own bucket, keyed on the authenticated tenant when
    /// the request carries one and on the peer address otherwise. This keeps a
    /// single busy tenant from consuming the whole service allowance.
    ///
    /// Rejections carry `Retry-After`, so a well-behaved client backs off for
    /// the right interval instead of retrying immediately and deepening the
    /// overload.
    #[must_use]
    #[cfg(feature = "rate-limit")]
    pub fn with_tenant_rate_limit(self, config: crate::rate_limit::RateLimitConfig) -> Self {
        use axum::{extract::Request, middleware::Next};
        use governor::{Quota, RateLimiter};
        use std::{num::NonZeroU32, sync::Arc};

        let rps = NonZeroU32::new(config.per_tenant_requests_per_second).unwrap_or(NonZeroU32::MIN);
        let burst = NonZeroU32::new(config.burst.max(config.per_tenant_requests_per_second))
            .unwrap_or(NonZeroU32::MIN);
        let limiter: Arc<governor::DefaultKeyedRateLimiter<String>> = Arc::new(RateLimiter::keyed(
            Quota::per_second(rps).allow_burst(burst),
        ));

        Self {
            router: self.router.layer(axum::middleware::from_fn(
                move |req: Request, next: Next| {
                    let limiter = Arc::clone(&limiter);
                    async move {
                        let key = crate::rate_limit::caller_key(&req);
                        match limiter.check_key(&key) {
                            Ok(()) => next.run(req).await,
                            Err(not_until) => crate::rate_limit::too_many_requests(
                                not_until.wait_time_from(governor::clock::Clock::now(
                                    &governor::clock::DefaultClock::default(),
                                )),
                                &key,
                            ),
                        }
                    }
                },
            )),
        }
    }

    /// Merge an existing [`Router`] into the service router.
    #[must_use]
    pub fn merge(self, other: Router) -> Self {
        Self {
            router: self.router.merge(other),
        }
    }

    /// Consume the builder and return the assembled [`Router`].
    pub fn build(self) -> Router {
        self.router
    }
}

impl Default for ServiceBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(feature = "metrics"))]
async fn metrics_stub() -> impl axum::response::IntoResponse {
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        "# mako metrics — build with feature `metrics` to enable Prometheus export\n",
    )
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn health_live_returns_200() {
        let app = ServiceBuilder::new().with_health(|| async { true }).build();
        let resp = app
            .oneshot(Request::get("/health/live").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn health_ready_false_returns_503() {
        let app = ServiceBuilder::new()
            .with_health(|| async { false })
            .build();
        let resp = app
            .oneshot(Request::get("/health/ready").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn metrics_returns_200() {
        let app = ServiceBuilder::new().with_metrics().build();
        let resp = app
            .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
