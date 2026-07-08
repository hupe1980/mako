//! Health check routes for mako services.
//!
//! Provides two standard endpoints:
//!
//! - `GET /health/live`  — liveness probe: returns `200 OK` when the process
//!   is running. Never fails unless the process is dead.
//! - `GET /health/ready` — readiness probe: calls a user-supplied closure that
//!   returns `true` when the service is ready to receive traffic.
//!
//! # Usage
//!
//! ```rust,no_run
//! use axum::Router;
//! use mako_service::health::health_routes;
//!
//! let app: Router = Router::new()
//!     .merge(health_routes(|| async { true }));
//! ```

use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
use std::future::Future;

/// Build standard health routes and merge them into a [`Router`].
///
/// `ready_fn` is called on every `/health/ready` request.  Return `true` when
/// the service is fully initialised and ready to serve traffic.
///
/// The liveness route (`/health/live`) always returns `200 OK`.
pub fn health_routes<F, Fut>(ready_fn: F) -> Router
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send,
{
    Router::new().route("/health/live", get(live)).route(
        "/health/ready",
        get(move || {
            let f = ready_fn.clone();
            async move { ready_handler(f).await }
        }),
    )
}

async fn live() -> impl IntoResponse {
    StatusCode::OK
}

async fn ready_handler<F, Fut>(ready_fn: F) -> impl IntoResponse
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    if ready_fn().await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_handler_always_ok() {
        let resp = live().await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_handler_true_returns_ok() {
        let resp = ready_handler(|| async { true }).await.into_response();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn ready_handler_false_returns_503() {
        let resp = ready_handler(|| async { false }).await.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
