//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with the dev configuration and a lazy pool, and the runner's
//! infra routes are merged in exactly as at boot — so a route axum refuses, a
//! merge collision with the runner's `/metrics` or `/health/*`, or a `build`
//! that panics fails `cargo test` instead of the container start.

use billingd::daemon::Billingd;

fn load(path: &str) -> billingd::config::BillingdConfig {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    mako_service::load_config_file::<billingd::config::BillingdConfig>("billingd", &path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test]
async fn the_router_builds_from_the_dev_config() {
    let _app = mako_service::build_for_test::<Billingd>(load("../../dev/config/billingd.toml"))
        .await
        .expect("the router builds");
}

#[tokio::test]
async fn the_router_builds_from_the_demo_config() {
    let _app = mako_service::build_for_test::<Billingd>(load("../../demos/o2c/billingd.toml"))
        .await
        .expect("the router builds");
}

/// A database failure inside a handler answers the problem shape with an
/// `error_id` the log line carries, and nothing of the driver's text.
///
/// The lazy pool points at a closed port, so the webhook's idempotency lookup
/// is the fault: without the ledger answer the dispatch must not be settled,
/// and the sender has to see a 500 it retries.
#[tokio::test]
async fn a_database_failure_answers_with_an_error_id_and_no_driver_text() {
    let app = mako_service::build_for_test::<Billingd>(load("../../dev/config/billingd.toml"))
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });

    let resp = reqwest::Client::new()
        .post(format!("http://{addr}/api/v1/webhooks/vpp-dispatch"))
        .header("content-type", "application/json")
        .body(r#"{"id":"evt-1","data":{"tx_id":"tx-1"}}"#)
        .send()
        .await
        .expect("the request is answered");
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.expect("a JSON problem body");
    let error_id = body["error_id"].as_str().expect("an error_id");
    assert!(uuid::Uuid::parse_str(error_id).is_ok(), "{body}");
    let text = body.to_string();
    for leak in ["pool", "timed out", "connect", "127.0.0.1"] {
        assert!(!text.contains(leak), "the body leaks {leak:?}: {text}");
    }
}
