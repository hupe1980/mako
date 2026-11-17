//! The public Rechnung-nachrechnen page: a valid token renders the verdict and
//! the total, any other answers 404 without saying whether it existed.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse as _;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

const TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAA";

async fn stub_billingd() -> String {
    let app = axum::Router::new().fallback(|req: Request<Body>| async move {
        let path = req.uri().path().to_owned();
        let body = req.into_body().collect().await.expect("body").to_bytes();
        let asked: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if path == "/api/v1/billing/nachrechnen" && asked["token"] == TOKEN {
            return (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                serde_json::json!({
                    "rechnungsnummer": "RE-2026-000001",
                    "malo_id_masked": "512*****012",
                    "period_from": "2026-01-01", "period_to": "2026-01-31",
                    "total_brutto_eur": "132.48", "zu_zahlen_eur": "132.48",
                    "verdict": { "matches": true, "arithmetic_matches": true,
                                 "seal_holds": true, "mismatched_positions": [],
                                 "storniert": false },
                    "lines": [], "totals": []
                })
                .to_string(),
            )
                .into_response();
        }
        (StatusCode::NOT_FOUND, String::new()).into_response()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub");
    let addr = listener.local_addr().expect("stub addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn get(path: &str) -> (StatusCode, String) {
    let (status, headers, body) = get_with_headers(path).await;
    for (name, want) in [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("x-robots-tag", "noindex"),
        ("x-content-type-options", "nosniff"),
    ] {
        assert_eq!(
            headers.get(name).and_then(|v| v.to_str().ok()),
            Some(want),
            "{path}: {name}"
        );
    }
    let csp = headers
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        csp.starts_with("default-src 'none'"),
        "{path}: no script may load: {csp}"
    );
    (status, body)
}

async fn get_with_headers(path: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let base = stub_billingd().await;
    let cfg: portald::config::PortaldConfig = toml::from_str(&format!(
        "tenant = \"9900357000004\"\nvertragd_url = \"{base}\"\nbillingd_url = \"{base}\"\n"
    ))
    .expect("config parses");
    let app = mako_service::build_for_test::<portald::server::Portald>(cfg)
        .await
        .expect("router builds");
    let resp = app
        .oneshot(Request::get(path).body(Body::empty()).expect("request"))
        .await
        .expect("responds");
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        headers,
        String::from_utf8_lossy(&bytes).into_owned(),
    )
}

#[tokio::test]
async fn a_valid_token_renders_the_verdict_and_the_total() {
    let (status, html) = get(&format!("/nachrechnen/{TOKEN}")).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("Stimmt überein"), "{html}");
    assert!(html.contains("132,48 €"), "{html}");
    assert!(html.contains("Ihrer Rechnung"), "„Sie“ by default");
}

#[tokio::test]
async fn an_unknown_or_malformed_token_is_404() {
    for path in [
        "/nachrechnen/BBBBBBBBBBBBBBBBBBBBBB",
        "/nachrechnen/short",
        "/nachrechnen/%3Cscript%3E",
    ] {
        let (status, html) = get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(html.contains("keine Rechnung"), "{html}");
    }
}
