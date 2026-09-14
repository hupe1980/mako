//! A document push to a relay is signed, and a relay with no credential is
//! refused at startup.
//!
//! The body of one of these POSTs is the customer's document itself — the
//! invoice or Mahnung base64-encoded — with their name, e-mail address, MaLo
//! and Kundennummer beside it. Unsigned, the relay cannot tell a mako push from
//! any other POST that reached its URL, and a captured one replays forever.

use std::sync::{Arc, Mutex};

use axum::{Router, body::Bytes, extract::State, http::HeaderMap, http::StatusCode, routing::post};
use outputd::config::DeliveryConfig;
use outputd::delivery::channel::{Relay, send_to_relay};

const SECRET: &str = "relay-shared-secret";

#[derive(Default)]
struct Seen {
    authorization: Option<String>,
    webhook_id: Option<String>,
    verified: bool,
}

/// A relay that checks the push the way an operator's adapter would: the shared
/// verifier, which is the signature *and* the freshness of the timestamp.
async fn relay_endpoint(
    State(seen): State<Arc<Mutex<Seen>>>,
    headers: HeaderMap,
    body: Bytes,
) -> (StatusCode, &'static str) {
    let verified =
        mako_service::webhook::verify_request(Some(SECRET.as_bytes()), &headers, &body).is_ok();
    let mut guard = seen.lock().expect("the capture mutex is never poisoned");
    guard.verified = verified;
    guard.authorization = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    guard.webhook_id = headers
        .get(mako_service::webhook::ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    (StatusCode::OK, r#"{"message_id":"relay-1"}"#)
}

#[tokio::test]
async fn a_document_push_carries_a_standard_webhooks_signature() {
    let seen = Arc::new(Mutex::new(Seen::default()));
    let app = Router::new()
        .route("/push", post(relay_endpoint))
        .with_state(Arc::clone(&seen));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let relay = Relay {
        url: format!("http://{addr}/push"),
        api_key: Some(secrecy::SecretString::from(SECRET)),
    };
    let body = serde_json::json!({
        "delivery_id":    "11111111-1111-1111-1111-111111111111",
        "recipient_name": "Erika Mustermann",
        "content_base64": "JVBERi0=",
    });
    let outcome = send_to_relay(
        &reqwest::Client::new(),
        &relay,
        "de.output.delivery/11111111-1111-1111-1111-111111111111",
        &body,
    )
    .await
    .expect("the relay accepted the push");
    assert_eq!(
        outcome
            .evidence
            .as_ref()
            .and_then(|e| e["message_id"].as_str()),
        Some("relay-1")
    );

    let guard = seen.lock().expect("the capture mutex is never poisoned");
    assert!(
        guard.verified,
        "the relay could not verify the push — it went out unsigned"
    );
    assert_eq!(
        guard.webhook_id.as_deref(),
        Some("de.output.delivery/11111111-1111-1111-1111-111111111111"),
        "the message id a relay deduplicates on must be the delivery, not the attempt"
    );
    assert_eq!(
        guard.authorization.as_deref(),
        Some(format!("Bearer {SECRET}").as_str())
    );

    server.abort();
}

/// A relay URL with no credential is named at startup, not discovered when the
/// first invoice has already been POSTed to it.
#[test]
fn a_relay_url_without_a_credential_is_reported() {
    let mut cfg = DeliveryConfig::default();
    assert!(
        cfg.unsigned_relays().is_empty(),
        "a deployment that configures no relay has nothing to report"
    );

    cfg.email_relay_url = Some("https://mail.example.test/send".to_owned());
    assert_eq!(cfg.unsigned_relays(), vec!["email_relay_api_key"]);

    cfg.email_relay_api_key = Some(secrecy::SecretString::from("k"));
    assert!(cfg.unsigned_relays().is_empty());

    cfg.postal_relay_url = Some("https://print.example.test/spool".to_owned());
    cfg.erp_webhook_url = Some("https://erp.example.test/hook".to_owned());
    assert_eq!(
        cfg.unsigned_relays(),
        vec!["postal_relay_api_key", "erp_api_key"]
    );

    // An empty URL is not a configured relay — the same emptiness rule the
    // worker applies before it claims a row.
    let empty = DeliveryConfig {
        erp_webhook_url: Some(String::new()),
        ..Default::default()
    };
    assert!(empty.unsigned_relays().is_empty());
}
