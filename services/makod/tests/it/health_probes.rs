//! The health probes are separate, unauthenticated, and never rate-limited.
//!
//! # Why this is a test
//!
//! One endpoint serving both Kubernetes probes cannot report dependency state:
//! a stale worker heartbeat or an unreachable object store would flip it to 503,
//! and Kubernetes answers a *liveness* failure with a restart. A transient
//! object-store outage would restart the container mid-delivery, which costs an
//! AS4 retry cycle and does not fix the object store.
//! Liveness must therefore report only that the process is up, and readiness is
//! what carries the dependency state.
//!
//! The probes also sat behind the per-peer rate limiter. The limiter keys on the
//! peer address, which behind a proxy or a shared NAT is the same address the
//! orchestrator probes from — so a chatty counterparty could 429 the kubelet and
//! have a healthy pod restarted for it.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt as _;

use makod::health::{self, HealthState};

async fn app() -> axum::Router {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    health::router(HealthState::new(store))
}

async fn status_of(path: &str) -> StatusCode {
    app()
        .await
        .oneshot(
            Request::builder()
                .uri(path)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds")
        .status()
}

#[tokio::test]
async fn all_three_probe_routes_are_mounted() {
    for path in ["/health", "/health/live", "/health/ready"] {
        assert_eq!(
            status_of(path).await,
            StatusCode::OK,
            "{path} must be served on a healthy instance",
        );
    }
}

/// The probes carry no credential requirement — a load balancer must be able to
/// reach them before any key is provisioned.
#[tokio::test]
async fn the_probes_need_no_credential() {
    // `health::router` is merged before every auth layer; reaching 200 with no
    // Authorization header on a bare router is what the merge order promises.
    assert_eq!(status_of("/health/live").await, StatusCode::OK);
}

#[tokio::test]
async fn the_response_names_the_instance_and_version() {
    let body = app()
        .await
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("router responds")
        .into_body();
    let bytes = axum::body::to_bytes(body, 64 * 1024)
        .await
        .expect("body reads");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body is JSON");

    assert_eq!(json["status"], "ok");
    assert_eq!(
        json["version"],
        env!("CARGO_PKG_VERSION"),
        "the documented response shape includes the daemon version",
    );
    assert!(
        json["instance_id"].is_string(),
        "instance_id identifies the replica that answered: {json}",
    );
    assert!(
        json.get("reason").is_none(),
        "a healthy response carries no reason: {json}",
    );
}

/// The rate limiters must skip exactly the health routes and nothing else.
#[test]
fn only_the_health_routes_are_exempt_from_rate_limiting() {
    for path in ["/health", "/health/live", "/health/ready"] {
        assert!(health::is_health_path(path), "{path} must be exempt");
    }
    for path in [
        "/api/v1/commands",
        "/edifact",
        "/as4/inbox",
        "/mcp",
        "/admin/partners",
        // Near-misses: a prefix match here would exempt real endpoints.
        "/health/../api/v1/commands",
        "/healthz",
        "/health/live/extra",
    ] {
        assert!(
            !health::is_health_path(path),
            "{path} must stay rate-limited",
        );
    }
}
