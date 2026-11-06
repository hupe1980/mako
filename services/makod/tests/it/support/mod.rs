//! Shared fixtures for the renderer conformance tests.

use mako_engine::ids::{ConversationId, CorrelationId, EventId, ProcessId, StreamId, TenantId};
use mako_engine::outbox::OutboxMessage;
use makod::config::PartyConfig;
use makod::party_registry::MpIdRegistry;

/// A registry with one primary party in the NB role.
pub fn registry(mp_id: &str) -> MpIdRegistry {
    let party = PartyConfig {
        mp_id: mp_id.to_owned(),
        roles: vec!["NB".to_owned()],
        primary: true,
        agency: None,
        as4: None,
    };
    MpIdRegistry::from_config(&[party]).expect("test registry")
}

/// An outbox message carrying `payload` for `recipient`.
pub fn outbox_message(
    message_type: &str,
    recipient: &str,
    payload: serde_json::Value,
) -> OutboxMessage {
    OutboxMessage::new(
        StreamId::new("process/conformance"),
        ProcessId::new(),
        TenantId::new(),
        CorrelationId::new(),
        ConversationId::new(),
        EventId::new(),
        message_type,
        recipient,
        payload,
    )
}

// ── API-Webdienste content signature ─────────────────────────────────────────

/// An API-Webdienste caller's signing key, for a request every test signs the
/// way a counterparty does (Regelungen zum Übertragungsweg für API-Webdienste
/// 1.2, Kap. 3.3).
pub const WEBDIENSTE_CALLER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgckWBECvjQWtsSdvR
HoiIfDEkbWbAnYri4WAvp2St+v+hRANCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/a
r/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPr
-----END PRIVATE KEY-----";

/// A self-signed P-256 certificate over [`WEBDIENSTE_CALLER_KEY`]'s public key, DER in
/// standard Base64.
pub const WEBDIENSTE_CALLER_CERT: &str = "MIIBhDCCASugAwIBAgIUd0O+Mx2XBxfhEKRqlUdnmSSHU2UwCgYIKoZIzj0EAwIwGDEWMBQGA1UEAwwNOTkwMDAwMTAwMDAwMTAeFw0yNjEwMDUxNDIzMTlaFw0zNjEwMDIxNDIzMTlaMBgxFjAUBgNVBAMMDTk5MDAwMDEwMDAwMDEwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/ar/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPro1MwUTAdBgNVHQ4EFgQUswL8Izc4762WL12or5ykXpaTBJAwHwYDVR0jBBgwFoAUswL8Izc4762WL12or5ykXpaTBJAwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNHADBEAiBxBiWnEciLutLqz4JaEIF6cpxmbA9BWAKi5qJSxOcTKwIgYUlfsyMjm3jaGMbpPZdu1Br5vFG97Ob7Kwj1Qcc7cyk=";

/// `req` signed with [`WEBDIENSTE_CALLER_KEY`] as an API-Webdienste
/// counterparty signs it: over `https://{Host}{path?query}`, the RFC 8785
/// payload and the `creationDateTime` / `transactionId` headers, which are
/// added when the request does not carry them.
pub async fn webdienste_signed(
    req: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    use base64::Engine as _;
    use energy_api::transport::content_security::{self, SignedInputs};
    let (mut parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.expect("body");
    let headers = &mut parts.headers;
    for (name, default) in [
        ("host", "msb.example.de"),
        ("creationDateTime", "2026-09-01T10:00:00.000Z"),
        ("transactionId", "f81d4fae-7dec-11d0-a765-00a0c91e6bf6"),
    ] {
        if !headers.contains_key(name) {
            headers.insert(name, default.parse().expect("header"));
        }
    }
    let text = |name: &str| headers[name].to_str().expect("ascii header").to_owned();
    let (host, created, tx) = (
        text("host"),
        text("creationDateTime"),
        text("transactionId"),
    );
    let payload = if bytes.is_empty() {
        Vec::new()
    } else {
        let json: serde_json::Value = serde_json::from_slice(&bytes).expect("a JSON body");
        content_security::canonical_json(&json).expect("canonical")
    };
    let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
    let key = content_security::signing_key_from_pem(WEBDIENSTE_CALLER_KEY).expect("test key");
    let (digest, signature) = content_security::sign_request(
        &SignedInputs {
            uri: &format!("https://{host}{path}"),
            payload: &payload,
            creation_date_time: &created,
            transaction_id: &tx,
            initial_transaction_id: None,
        },
        &key,
    )
    .expect("sign");
    let der = base64::engine::general_purpose::STANDARD
        .decode(WEBDIENSTE_CALLER_CERT)
        .expect("fixture");
    let headers = &mut parts.headers;
    headers.insert(
        content_security::HEADER_DIGEST,
        digest.parse().expect("header"),
    );
    headers.insert(
        content_security::HEADER_SIGNATURE,
        signature.parse().expect("header"),
    );
    headers.insert(
        content_security::HEADER_CERT,
        content_security::cert_header_value(&der)
            .parse()
            .expect("header"),
    );
    axum::http::Request::from_parts(parts, axum::body::Body::from(bytes))
}
