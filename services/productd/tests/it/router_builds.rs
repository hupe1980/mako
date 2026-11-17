//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with a lazy pool and the runner's infra routes are merged in
//! exactly as at boot — so a route axum refuses, a merge collision with the
//! runner's `/metrics` or `/health/*`, or a `build` that panics fails
//! `cargo test` instead of the container start.

/// `deploy/demo/productd.toml` — what `just dev` and `just demo` start — builds a router.
#[tokio::test]
async fn the_demo_config_builds_a_router() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/productd.toml");
    let cfg = mako_service::load_config_file::<productd::config::ProductdConfig>("productd", &path)
        .expect("the demo configuration loads");
    let _app = mako_service::build_for_test::<productd::Productd>(cfg)
        .await
        .expect("the router builds");
}

/// A database failure answers the problem shape with an `error_id`, never the
/// driver's text — the pool here points at a port nothing listens on.
#[tokio::test]
async fn a_database_failure_answers_an_error_id_and_no_driver_text() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let cfg = toml::from_str(include_str!("../../../../deploy/demo/productd.toml"))
        .expect("the demo configuration parses");
    let app = mako_service::build_for_test::<productd::Productd>(cfg)
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let mut conn = tokio::net::TcpStream::connect(addr).await.unwrap();
    let request = format!(
        "POST /api/v1/angebote/{}/ablehnen HTTP/1.1\r\nHost: {addr}\r\n\
         Content-Length: 0\r\nConnection: close\r\n\r\n",
        uuid::Uuid::new_v4()
    );
    conn.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    conn.read_to_string(&mut raw).await.unwrap();

    let (head, body) = raw.split_once("\r\n\r\n").expect("an HTTP response");
    assert!(head.starts_with("HTTP/1.1 500"), "{head}");
    let body: serde_json::Value =
        serde_json::from_str(body.trim()).expect("the body is the JSON problem shape");
    assert!(body["error_id"].is_string(), "{body}");
    assert_eq!(body["detail"], "internal server error", "{body}");
}
