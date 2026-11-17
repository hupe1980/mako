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
    ///
    /// The request span names the matched route template (`route_span`),
    /// never the URI: a path segment can be a credential — portald's
    /// `/nachrechnen/{token}` — and the request log is read by more people
    /// than the token's holder.
    #[must_use]
    pub fn with_trace_layer(self) -> Self {
        Self {
            router: self
                .router
                .layer(TraceLayer::new_for_http().make_span_with(route_span)),
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
    /// [`Self::with_caller_rate_limit`] on a multi-tenant deployment.
    #[must_use]
    #[cfg(feature = "rate-limit")]
    pub fn with_rate_limit(self, config: &crate::rate_limit::RateLimitConfig) -> Self {
        use axum::{extract::Request, middleware::Next};
        use governor::RateLimiter;
        use std::sync::Arc;

        let limiter = Arc::new(RateLimiter::direct(quota(
            config.requests_per_second,
            config.burst,
        )));
        Self {
            router: self.router.layer(axum::middleware::from_fn(
                move |req: Request, next: Next| {
                    let limiter = Arc::clone(&limiter);
                    async move {
                        match limiter.check() {
                            Ok(()) => next.run(req).await,
                            Err(not_until) => crate::rate_limit::too_many_requests(
                                wait_time(not_until),
                                "service",
                            ),
                        }
                    }
                },
            )),
        }
    }

    /// Add a per-caller GCRA rate limiter (requires feature `rate-limit`).
    ///
    /// Each caller gets its own bucket, keyed on the **peer address**. This
    /// layer runs before authentication — which is the point of it, since
    /// rejecting a flood only after verifying its signatures costs exactly what
    /// the flood is trying to spend — so any credential it could read is
    /// unverified and therefore caller-chosen. See
    /// [`crate::rate_limit::caller_key`]: keying on a presented token lets a
    /// client mint a fresh bucket per request and escape the limiter entirely.
    /// Limiting per *tenant* belongs after the token is verified.
    ///
    /// Rejections carry `Retry-After`, so a well-behaved client backs off for
    /// the right interval instead of retrying immediately and deepening the
    /// overload.
    #[must_use]
    #[cfg(feature = "rate-limit")]
    pub fn with_caller_rate_limit(self, config: &crate::rate_limit::RateLimitConfig) -> Self {
        use axum::{extract::Request, middleware::Next};
        use governor::RateLimiter;
        use std::sync::Arc;

        let quota = quota(config.per_caller_requests_per_second, config.burst);
        let limiter: Arc<governor::DefaultKeyedRateLimiter<String>> =
            Arc::new(RateLimiter::keyed(quota));
        // `governor` prunes its keyed store only when asked. Without this the
        // map grows one permanent entry per distinct caller for the process
        // lifetime, which is a slow leak on any public route.
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        Self {
            router: self.router.layer(axum::middleware::from_fn(
                move |req: Request, next: Next| {
                    let limiter = Arc::clone(&limiter);
                    let calls = Arc::clone(&calls);
                    async move {
                        if crate::rate_limit::prune_due(&calls) {
                            limiter.retain_recent();
                        }
                        let key = crate::rate_limit::caller_key(&req);
                        match limiter.check_key(&key) {
                            Ok(()) => next.run(req).await,
                            Err(not_until) => {
                                crate::rate_limit::too_many_requests(wait_time(not_until), &key)
                            }
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

/// A per-second GCRA quota with a burst allowance (never below the steady rate).
/// Shared by both rate-limiter builders.
#[cfg(feature = "rate-limit")]
fn quota(per_second: u32, burst: u32) -> governor::Quota {
    use std::num::NonZeroU32;
    let rps = NonZeroU32::new(per_second).unwrap_or(NonZeroU32::MIN);
    let burst = NonZeroU32::new(burst.max(per_second)).unwrap_or(NonZeroU32::MIN);
    governor::Quota::per_second(rps).allow_burst(burst)
}

/// The back-off implied by a GCRA rejection, for the `Retry-After` header.
#[cfg(feature = "rate-limit")]
fn wait_time(not_until: governor::NotUntil<governor::clock::QuantaInstant>) -> std::time::Duration {
    use governor::clock::Clock as _;
    not_until.wait_time_from(governor::clock::DefaultClock::default().now())
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

/// The DEBUG `request` span of one request: its method, the route template
/// it matched (`{unmatched}` when none did) and the HTTP version — the fields
/// of `tower-http`'s default span with the URI replaced by the template.
fn route_span<B>(req: &axum::http::Request<B>) -> tracing::Span {
    let route = req
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or("{unmatched}", axum::extract::MatchedPath::as_str);
    tracing::debug_span!(
        "request",
        method = %req.method(),
        route = %route,
        version = ?req.version(),
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

    /// Every span field and event field a subscriber sees, as `name=value`.
    #[derive(Clone, Default)]
    struct Recorded(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

    struct Fields<'a>(&'a mut Vec<String>);

    impl tracing::field::Visit for Fields<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            self.0.push(format!("{}={value:?}", field.name()));
        }
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Recorded {
        fn on_new_span(
            &self,
            attrs: &tracing::span::Attributes<'_>,
            _: &tracing::span::Id,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            let mut seen = self.0.lock().unwrap();
            seen.push(format!("span {}", attrs.metadata().name()));
            attrs.record(&mut Fields(&mut seen));
        }

        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            event.record(&mut Fields(&mut self.0.lock().unwrap()));
        }
    }

    /// A path segment can be a credential (portald's
    /// `/nachrechnen/{token}`), so the request span names the matched route
    /// template and never the URI — no span or event of a traced request
    /// carries the token.
    #[tokio::test]
    async fn the_request_span_carries_the_route_template_not_the_path() {
        use tracing_subscriber::layer::SubscriberExt as _;

        let recorded = Recorded::default();
        let subscriber = tracing_subscriber::registry().with(recorded.clone());
        let _guard = tracing::subscriber::set_default(subscriber);

        const TOKEN: &str = "Zq3v9TOKENxY7pL0aBcDeF";
        let app = ServiceBuilder::new()
            .merge(Router::new().route("/nachrechnen/{token}", get(|| async { "ok" })))
            .with_trace_layer()
            .build();
        let resp = app
            .oneshot(
                Request::get(format!("/nachrechnen/{TOKEN}?q={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let seen = recorded.0.lock().unwrap().join("\n");
        assert!(seen.contains("span request"), "no request span: {seen}");
        assert!(
            seen.contains("/nachrechnen/{token}"),
            "the span names the route template: {seen}"
        );
        assert!(!seen.contains(TOKEN), "the token reached a span: {seen}");
    }
}
