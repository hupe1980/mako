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

#[tokio::test]
async fn the_router_builds_as_the_runner_mounts_it() {
    let peer = peer().await;
    let cfg: invoicd::config::Config = serde_json::from_value(serde_json::json!({
        "http": {}, "database": {"url": "postgres://startup-test/invoicd"},
        "identity": {"tenant": "9900357000004"},
        "makod": {"url": peer},
        "marktd": {"url": peer, "api_key": "test"},
        "webhook": {"inbound_secret": "s3cret"},
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let _app = mako_service::build_for_test::<invoicd::server::Invoicd>(cfg)
        .await
        .expect("the router builds");
}
