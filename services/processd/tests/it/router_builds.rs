//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with the shipped demo configuration and a lazy pool, and the
//! runner's infra routes are merged in exactly as at boot — so a route axum
//! refuses, a merge collision with the runner's `/metrics` or `/health/*`, or
//! a `build` that panics fails `cargo test` instead of the container start.
//!
//! `build` registers this processd's webhook with `marktd` and refuses to start
//! when that fails, so the test answers the registration from a stand-in.

/// A `marktd` that accepts any subscription.
async fn marktd() -> String {
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
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../demos/nb-stp/processd.toml"))
            .expect("the demo configuration parses");
    cfg["marktd"]["url"] = toml::Value::String(marktd().await);
    let cfg = cfg
        .try_into()
        .expect("the demo configuration is a processd one");
    let _app = mako_service::build_for_test::<processd::Processd>(cfg)
        .await
        .expect("the router builds");
}

/// The router, served on a loopback port, over a pool with no database behind
/// it — so every query fails.
async fn serve_without_a_database() -> String {
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../demos/nb-stp/processd.toml"))
            .expect("the demo configuration parses");
    cfg["marktd"]["url"] = toml::Value::String(marktd().await);
    let app = mako_service::build_for_test::<processd::Processd>(
        cfg.try_into()
            .expect("the demo configuration is a processd one"),
    )
    .await
    .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// A failed query is a 500 with an `error_id` the log line carries, and none
/// of the database driver's text — which names the pool, the host and the
/// cause to whoever called.
#[tokio::test]
async fn a_database_failure_is_a_problem_body_with_an_error_id() {
    let base = serve_without_a_database().await;
    for path in ["/api/v1/decisions", "/api/v1/queue", "/obs/overdue"] {
        let resp = reqwest::get(format!("{base}{path}"))
            .await
            .expect("answered");
        assert_eq!(resp.status().as_u16(), 500, "{path}");
        let body: serde_json::Value = resp.json().await.expect("a JSON problem body");
        assert_eq!(body["detail"], "internal server error", "{path}: {body}");
        assert!(
            body["error_id"]
                .as_str()
                .is_some_and(|id| id.parse::<uuid::Uuid>().is_ok()),
            "{path}: {body}"
        );
        let text = body.to_string().to_lowercase();
        for leaked in ["pool", "timed out", "connection", "127.0.0.1"] {
            assert!(!text.contains(leaked), "{path} leaks {leaked:?}: {body}");
        }
    }
}

/// A refused supply request names its refusal in `code`, beside the sentence.
#[tokio::test]
async fn a_refused_start_supply_is_a_422_with_its_code() {
    let base = serve_without_a_database().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/v1/start-supply"))
        .json(&serde_json::json!({ "lieferbeginn_datum": "2099-01-01" }))
        .send()
        .await
        .expect("answered");
    assert_eq!(resp.status().as_u16(), 422);
    let body: serde_json::Value = resp.json().await.expect("a JSON problem body");
    assert_eq!(body["code"], "MISSING_MALO_ID", "{body}");
    assert!(
        body["detail"]
            .as_str()
            .is_some_and(|d| d.contains("malo_id")),
        "{body}"
    );
}
