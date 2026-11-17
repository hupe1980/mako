//! The router builds the way `mako_service::run` mounts it.
//!
//! `build` runs with the demo configuration and a lazy pool, and the runner's
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
async fn the_router_builds_from_the_demo_config() {
    let _app = mako_service::build_for_test::<Billingd>(load("../../deploy/demo/billingd.toml"))
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
    let app = mako_service::build_for_test::<Billingd>(load("../../deploy/demo/billingd.toml"))
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

/// The token route admits portald's service principal alone. An LF
/// operator holds `read-billing` and is still refused `403`, and so is any
/// opaque key the demo does not name (the synthetic dev principal); portald's
/// configured demo key reaches the lookup, which answers a malformed token
/// `404`.
#[tokio::test]
async fn only_portald_reads_an_invoice_by_its_verify_token() {
    use mako_service::oidc::demo;

    let app = mako_service::build_for_test::<Billingd>(load("../../deploy/demo/billingd.toml"))
        .await
        .expect("the router builds");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });

    let post = |bearer: String| {
        reqwest::Client::new()
            .post(format!("http://{addr}/api/v1/billing/nachrechnen"))
            .bearer_auth(bearer)
            .json(&serde_json::json!({ "token": "not-a-token" }))
            .send()
    };
    let lf = demo::mint(demo::persona("lf").expect("lf persona"));
    let resp = post(lf).await.expect("answered");
    assert_eq!(resp.status(), 403, "an LF operator is not portald");

    let resp = post("some-other-opaque-key".to_owned())
        .await
        .expect("answered");
    assert_eq!(resp.status(), 403, "the dev principal is not portald");

    let resp = post("dev-portald-key".to_owned()).await.expect("answered");
    assert_eq!(resp.status(), 404, "portald reaches the token lookup");

    // Revoking is a write: a caller who may read invoices may not revoke one.
    let nb = demo::mint(demo::persona("nb").expect("nb persona"));
    let resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/billing/{}/verify-token/revoke",
            uuid::Uuid::new_v4()
        ))
        .bearer_auth(nb)
        .send()
        .await
        .expect("answered");
    assert_eq!(resp.status(), 403, "revoke needs correct-billing");
}

/// An MCP tool's database failure answers an `error_id` and nothing of the
/// driver's text, as the REST routes do.
#[tokio::test]
async fn an_mcp_database_failure_answers_with_an_error_id_and_no_driver_text() {
    let app = mako_service::build_for_test::<Billingd>(load("../../deploy/demo/billingd.toml"))
        .await
        .expect("the router builds");
    let endpoint = mako_service::mcp::conformance::Endpoint::serve(app, "/mcp").await;
    for (tool, arguments) in [
        ("list_review_queue", serde_json::json!({})),
        (
            "get_billing_summary",
            serde_json::json!({ "malo_id": "51238696012" }),
        ),
        (
            "list_billing_records",
            serde_json::json!({ "malo_id": "51238696012" }),
        ),
    ] {
        let answer = endpoint
            .post(
                &[("MCP-Protocol-Version", "2025-11-25")],
                &serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                    "params": { "name": tool, "arguments": arguments },
                }),
            )
            .await;
        let text = answer.message.to_string();
        assert!(
            text.contains("internal error "),
            "{tool}: no error_id answer: {text}"
        );
        for leak in ["pool", "timed out", "connect", "127.0.0.1"] {
            assert!(!text.contains(leak), "{tool} leaks {leak:?}: {text}");
        }
    }
}
