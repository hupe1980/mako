//! Health check routes for mako services.
//!
//! Provides three standard endpoints:
//!
//! - `GET /health/live`  — liveness probe: returns `200 OK` when the process
//!   is running. Never fails unless the process is dead.
//! - `GET /health/ready` — readiness probe: calls a user-supplied closure that
//!   returns `true` when the service is ready to receive traffic.
//! - `GET /health`       — the same readiness answer with a JSON body, for
//!   people rather than for kubelets.
//!
//! `/health` is not decoration. It is the first thing anyone types, `makod` has
//! always answered it, and mako's own READMEs, operator guides and demo smoke
//! test all documented `curl <service>/health` against services that served
//! only the two probe routes — so the nb-stp demo waited 60 s on a 404 and
//! failed before submitting anything. A probe pair that a kubelet is happy with
//! and a human cannot reach is a probe pair that gets documented wrong.
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
/// `ready_fn` is called on every `/health/ready` and `/health` request. Return
/// `true` when the service is fully initialised and ready to serve traffic.
///
/// The liveness route (`/health/live`) always returns `200 OK`.
pub fn health_routes<F, Fut>(ready_fn: F) -> Router
where
    F: Fn() -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = bool> + Send,
{
    let json_fn = ready_fn.clone();
    Router::new()
        .route("/health/live", get(live))
        .route(
            "/health/ready",
            get(move || {
                let f = ready_fn.clone();
                async move { ready_handler(f).await }
            }),
        )
        .route(
            "/health",
            get(move || {
                let f = json_fn.clone();
                async move { ready_json_handler(f).await }
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

/// The readiness answer with a body, for `curl <service>/health`.
///
/// The status code carries the same meaning as `/health/ready`, so this route
/// works as a probe too; the body is what makes the reply legible to a person
/// and to the `jq '.status'` the demo scripts and operator guides run.
async fn ready_json_handler<F, Fut>(ready_fn: F) -> impl IntoResponse
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    if ready_fn().await {
        (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "ok" })),
        )
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({ "status": "unavailable" })),
        )
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

    /// `GET /health` is routed, not just `/health/live` and `/health/ready`.
    ///
    /// The demo smoke test and every operator guide `curl <service>/health`;
    /// when only the probe pair was routed they got a 404 and the nb-stp demo
    /// stalled 60 s before failing.
    #[tokio::test]
    async fn health_is_routed_and_carries_a_status_body() {
        use tower::ServiceExt as _;

        let app = health_routes(|| async { true });
        let resp = app
            .oneshot(
                axum::http::Request::get("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "ok");
    }

    /// A service that is up but not ready says so on `/health` too, with the
    /// same 503 `/health/ready` uses — so the route is usable as a probe.
    #[tokio::test]
    async fn health_reports_unready_with_503() {
        use tower::ServiceExt as _;

        let app = health_routes(|| async { false });
        let resp = app
            .oneshot(
                axum::http::Request::get("/health")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let bytes = axum::body::to_bytes(resp.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["status"], "unavailable");
    }
}
