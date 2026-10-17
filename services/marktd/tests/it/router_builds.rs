//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with the dev and the demo configuration and a lazy pool, and
//! the runner's infra routes are merged in exactly as at boot — so a route axum
//! refuses, a merge collision with the runner's `/metrics` or `/health/*`, or a
//! `build` that panics fails `cargo test` instead of the container start.

use marktd::daemon::Marktd;

pub(crate) fn load(path: &str) -> marktd::config::Config {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path);
    mako_service::load_config_file::<marktd::config::Config>("marktd", &path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[tokio::test]
async fn the_router_builds_from_the_dev_config() {
    let _app = mako_service::build_for_test::<Marktd>(load("../../dev/config/marktd.toml"))
        .await
        .expect("the router builds");
}

#[tokio::test]
async fn the_router_builds_from_the_demo_config() {
    let _app = mako_service::build_for_test::<Marktd>(load("../../demos/eeg-billing/marktd.toml"))
        .await
        .expect("the router builds");
}

/// A database failure inside a handler answers the problem shape with an
/// `error_id` the log line carries, and nothing of the driver's text.
///
/// The lazy pool points at a closed port, so the repository read is the fault.
#[tokio::test]
async fn a_database_failure_answers_with_an_error_id_and_no_driver_text() {
    let app = mako_service::build_for_test::<Marktd>(load("../../dev/config/marktd.toml"))
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let resp = reqwest::get(format!(
        "http://{addr}/api/v1/steuerbare-ressourcen/C1234567890"
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
