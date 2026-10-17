//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with a minimal configuration and a lazy pool, and the runner's
//! infra routes are merged in exactly as at boot — so a route axum refuses, a
//! merge collision with the runner's `/metrics` or `/health/*`, or a `build`
//! that panics fails `cargo test` instead of the container start.

/// A peer that answers every request `200` — the subscription `build`
/// registers, if it registers one.
pub(crate) async fn peer() -> String {
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
    let cfg = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/netzbilanzd"},
        "tenant": "9900357000004",
        "marktd_url": peer, "marktd_api_key": "test",
        "makod_url": peer, "makod_api_key": "test",
        "inbound_secret": "s3cret",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    let _app = mako_service::build_for_test::<netzbilanzd::Netzbilanzd>(cfg)
        .await
        .expect("the router builds");
}

pub(crate) fn mabis_config(peer: &str, target: &str) -> netzbilanzd::config::NetzbilanzConfig {
    serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/netzbilanzd"},
        "tenant": "9900357000004",
        "marktd_url": peer, "marktd_api_key": "test",
        "makod_url": peer, "makod_api_key": "test",
        "edmd_url": peer, "edmd_api_key": "test",
        "inbound_secret": "s3cret",
        "allow_insecure_no_auth": true,
        "mabis": {
            "sender_mp_id": "9900357000004",
            "receiver_mp_id": "9900077000006",
            "bilanzierungsgebiet_id": "11YMAKO-TEST-01U",
            "submission_target": target,
        },
    }))
    .expect("the configuration parses")
}

/// With `[mabis]`, the router builds with the MaBiS routes mounted.
#[tokio::test]
async fn the_router_builds_with_the_mabis_module() {
    let peer = peer().await;
    let _app = mako_service::build_for_test::<netzbilanzd::Netzbilanzd>(mabis_config(
        &peer,
        "biko-bilateral",
    ))
    .await
    .expect("the router builds");
}

/// The Hub target refuses in `build`, the one start-up path there is.
#[tokio::test]
async fn the_mabis_hub_target_refuses_at_startup() {
    let peer = peer().await;
    let Err(err) =
        mako_service::build_for_test::<netzbilanzd::Netzbilanzd>(mabis_config(&peer, "mabis-hub"))
            .await
    else {
        panic!("a mabis-hub target must not start");
    };
    let msg = format!("{err:#}");
    assert!(
        msg.contains("BK6-24-210"),
        "the refusal must cite why: {msg}"
    );
    assert!(
        msg.contains("biko-bilateral"),
        "and name the way forward: {msg}"
    );
}
