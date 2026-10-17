//! An upstream failure reaches the customer as the service's name and an
//! `error_id`, never as the upstream's own error text.
//!
//! An upstream's 5xx body is that service's internals — a SQL fragment, a
//! connection string — and a portal answers the public internet.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse as _;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

const SECRET: &str = "password=hunter2 relation \"kunden\" does not exist";

/// `vertragd` authorises the customer; every other upstream fails with a body
/// carrying [`SECRET`].
async fn failing_upstreams() -> String {
    let app = axum::Router::new().fallback(|req: Request<Body>| async move {
        if req.uri().path().ends_with("/kunden/authenticate") {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                r#"{"kunden_id":"0195f6c2-0000-4000-8000-000000000001","kundentyp":"B2C"}"#,
            )
                .into_response();
        }
        (StatusCode::INTERNAL_SERVER_ERROR, SECRET).into_response()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_upstream_5xx_answers_502_with_an_error_id_and_echoes_nothing() {
    let base = failing_upstreams().await;
    let cfg: portald::config::PortaldConfig = toml::from_str(&format!(
        "tenant = \"9900357000004\"\nvertragd_url = \"{base}\"\naccountingd_url = \"{base}\"\n"
    ))
    .expect("config parses");
    let app = mako_service::build_for_test::<portald::server::Portald>(cfg)
        .await
        .expect("router builds");

    let resp = app
        .oneshot(
            Request::get("/api/v1/portal/51238696012/balance")
                .header(header::AUTHORIZATION, "Bearer customer-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON problem body");
    assert!(body["error_id"].as_str().is_some(), "{body}");
    let text = body.to_string();
    assert!(
        text.contains("accountingd"),
        "the failing service is named: {text}"
    );
    assert!(
        !text.contains("hunter2") && !text.contains("kunden"),
        "{text}"
    );
}
