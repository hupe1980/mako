//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with a minimal configuration and a lazy pool, and the runner's
//! infra routes are merged in exactly as at boot — so a route axum refuses, a
//! merge collision with the runner's `/metrics` or `/health/*`, or a `build`
//! that panics fails `cargo test` instead of the container start.

async fn peer() -> String {
    let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::OK });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn config() -> invoicd::config::Config {
    let peer = peer().await;
    serde_json::from_value(serde_json::json!({
        "http": {}, "database": {"url": "postgres://startup-test/invoicd"},
        "identity": {"tenant": "9900357000004"},
        "makod": {"url": peer},
        "marktd": {"url": peer, "api_key": "test"},
        "webhook": {"inbound_secret": "s3cret"},
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses")
}

#[tokio::test]
async fn the_router_builds_as_the_runner_mounts_it() {
    let _app = mako_service::build_for_test::<invoicd::server::Invoicd>(config().await)
        .await
        .expect("the router builds");
}

/// A database failure answers the problem shape with an `error_id` the log
/// line carries, and nothing of the driver's text. The lazy pool points at a
/// closed port, so the receipt lookup is the fault.
#[tokio::test]
async fn a_database_failure_answers_with_an_error_id_and_no_driver_text() {
    let app = mako_service::build_for_test::<invoicd::server::Invoicd>(config().await)
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let resp = reqwest::get(format!(
        "http://{addr}/api/v1/receipts/0195f6c2-0000-4000-8000-000000000001"
    ))
    .await
    .expect("the request is answered");
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.expect("a JSON problem body");
    let error_id = body["error_id"].as_str().expect("an error_id");
    assert!(uuid::Uuid::parse_str(error_id).is_ok(), "{body}");
    let text = body.to_string();
    for leak in ["pool", "timed out", "connect", "startup-test"] {
        assert!(!text.contains(leak), "the body leaks {leak:?}: {text}");
    }
}
