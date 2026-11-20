//! Each runner fails, naming the check, against a service with one seeded
//! defect per runner — a guard is only worth what it refuses.

use std::time::Duration;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::IntoResponse;
use mako_conformance::report::{Outcome, Runner};
use mako_conformance::suite::Target;
use serde_json::{Value, json};

/// A vertragd lookalike with three defects: it answers every REST call
/// without asking for a token, delivers its test event signed with the wrong
/// secret and accepts an unsigned inbound event, and lists one MCP tool too
/// few.
async fn defective(
    State(callback): State<std::sync::Arc<tokio::sync::Mutex<Option<String>>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> axum::response::Response {
    let path = uri.path().to_owned();
    match (method.as_str(), path.as_str()) {
        ("POST", "/api/v1/webhooks/endpoints") => {
            let b: Value = serde_json::from_slice(&body).unwrap_or_default();
            *callback.lock().await = b["url"].as_str().map(str::to_owned);
            (
                StatusCode::CREATED,
                axum::Json(json!({ "endpoint": { "id": "e1" }, "secret": "whsec_aGVsbG8=" })),
            )
                .into_response()
        }
        ("POST", "/api/v1/webhooks/endpoints/e1/test") => {
            if let Some(url) = callback.lock().await.clone() {
                let ce = json!({ "specversion": "1.0", "id": "t1", "source": "x",
                                 "type": mako_events::mako::WEBHOOK_TEST, "data": {} })
                .to_string();
                let now = time::OffsetDateTime::now_utc().unix_timestamp();
                let mut req = reqwest::Client::new().post(url).body(ce.clone());
                for (h, v) in mako_service::webhook::headers(
                    b"not-the-issued-secret",
                    "t1",
                    now,
                    ce.as_bytes(),
                ) {
                    req = req.header(h, v);
                }
                let _ = req.send().await;
            }
            StatusCode::ACCEPTED.into_response()
        }
        ("POST", "/mcp") => {
            if headers.get("authorization").is_none() {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            axum::Json(json!({ "jsonrpc": "2.0", "id": 1,
                               "result": { "tools": [{ "name": "get_kunde" }] } }))
            .into_response()
        }
        // Every other route, signed or not, with a token or without.
        _ => (StatusCode::OK, axum::Json(json!({}))).into_response(),
    }
}

async fn serve() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .fallback(defective)
        .with_state(std::sync::Arc::new(tokio::sync::Mutex::new(None)));
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    base
}

async fn report(only: Runner) -> mako_conformance::report::Report {
    let base = serve().await;
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let listen = probe.local_addr().unwrap().to_string();
    drop(probe);
    mako_conformance::run(
        mako_conformance::suites::suite("vertragd").unwrap(),
        Target {
            base,
            token: "t".to_owned(),
            refused_token: Some("r".to_owned()),
            inbound_secret: Some("s".to_owned()),
            listen,
            callback: None,
            wait: Duration::from_secs(3),
        },
        Some(only),
    )
    .await
    .unwrap()
}

fn broken(r: &mako_conformance::report::Report, starts: &str) -> bool {
    r.checks()
        .iter()
        .any(|c| c.name.starts_with(starts) && matches!(c.outcome, Outcome::Broken(_)))
}

#[tokio::test]
async fn rest_names_an_unprotected_operation_and_an_admitted_principal() {
    let r = report(Runner::Rest).await;
    assert!(
        broken(&r, "401 without a token: GET /api/v1/vertraege"),
        "{r}"
    );
    assert!(broken(&r, "403 for the refused principal"), "{r}");
}

#[tokio::test]
async fn events_name_a_wrong_signature_and_an_accepted_unsigned_event() {
    let r = report(Runner::Events).await;
    assert!(broken(&r, "the test delivery is signed"), "{r}");
    assert!(broken(&r, "inbound /api/v1/events"), "{r}");
}

#[tokio::test]
async fn mcp_names_a_missing_tool() {
    let r = report(Runner::Mcp).await;
    assert!(broken(&r, "tools/list names the declared tools"), "{r}");
}
