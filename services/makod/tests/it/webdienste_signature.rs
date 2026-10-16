//! An inbound API-Webdienste request whose content signature does not verify is
//! refused.
//!
//! Regelungen zum Übertragungsweg für API-Webdienste 1.2, Kap. 3.3: every API
//! interaction is signed over the URI, the canonical payload and the
//! `creationDateTime` / `transactionId` headers — and `initialTransactionId`
//! when the request carries one; the signature travels in
//! `X-BDEW-SIGNATURE`, the digest in `X-BDEW-DIGEST` and the signer's
//! certificate in `X-BDEW-CERT` (RFC 9440 encoding). A signature checked by
//! nobody proves nothing, so a request carrying one that does not verify must
//! not reach a handler — it would otherwise switch a customer installation on
//! a forged order.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine as _;
use tower::ServiceExt as _;

use makod::webdienste::{self, MakodApiHandler};

const TENANT: &str = "9900357000004";
const CALLER: &str = "9900001000001";
const HOST: &str = "msb.example.de";
const PATH: &str = "/[Post]/steuerbefehl/konfiguration/?locationId=E1234848431&commandControl=%7B%22maximumPowerValue%22%3A%224.2%22%2C%22executionTimeFrom%22%3A%222026-09-01T00%3A00%3A00Z%22%7D";
const CREATED: &str = "2026-09-01T10:00:00.000Z";
const TX: &str = "f81d4fae-7dec-11d0-a765-00a0c91e6bf6";

/// The caller's signing key; `CALLER_CERT` carries its public half.
const CALLER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgckWBECvjQWtsSdvR
HoiIfDEkbWbAnYri4WAvp2St+v+hRANCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/a
r/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPr
-----END PRIVATE KEY-----";

/// A self-signed P-256 certificate over `CALLER_KEY`'s public key, DER in
/// standard Base64.
const CALLER_CERT: &str = "MIIBhDCCASugAwIBAgIUd0O+Mx2XBxfhEKRqlUdnmSSHU2UwCgYIKoZIzj0EAwIwGDEWMBQGA1UEAwwNOTkwMDAwMTAwMDAwMTAeFw0yNjEwMDUxNDIzMTlaFw0zNjEwMDIxNDIzMTlaMBgxFjAUBgNVBAMMDTk5MDAwMDEwMDAwMDEwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAASu1tuAaW6ZKbVOAyetJgFYgZEEYP/ar/Ox8QqIhDVKepRV09o6tWyu3o1V8IeBpSlvcWRWS2J39e8qjUDwmCPro1MwUTAdBgNVHQ4EFgQUswL8Izc4762WL12or5ykXpaTBJAwHwYDVR0jBBgwFoAUswL8Izc4762WL12or5ykXpaTBJAwDwYDVR0TAQH/BAUwAwEB/zAKBggqhkjOPQQDAgNHADBEAiBxBiWnEciLutLqz4JaEIF6cpxmbA9BWAKi5qJSxOcTKwIgYUlfsyMjm3jaGMbpPZdu1Br5vFG97Ob7Kwj1Qcc7cyk=";

/// Somebody else's key.
const OTHER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgr2t8BwvjUy+M8WBQ
4zJBX7eVicBSIdhKHbe18eRxaoahRANCAAR4D1MXBv46lR4KZPHG9k2M+inHQNPo
HllI3m64ULR61sjxN6lkT9mltScv1FHQMfUGte1/1opz1FAZfvbFSBlz
-----END PRIVATE KEY-----";

async fn app() -> axum::Router {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    webdienste::build_app(
        Arc::new(MakodApiHandler {
            store,
            tenant_id: mako_engine::ids::TenantId::from_party_id(TENANT),
        }),
        None,
        1024 * 1024,
        true,
    )
}

fn signed(key_pem: &str) -> Request<Body> {
    signed_with(key_pem, None)
}

/// A request signed the way `energy-api` signs one, carrying the three
/// content-signature headers under the names `energy-api` publishes.
fn signed_with(key_pem: &str, initial_tx: Option<&str>) -> Request<Body> {
    use energy_api::transport::content_security::{self, SignedInputs};
    let key = content_security::signing_key_from_pem(key_pem).expect("test key");
    let (digest, signature) = content_security::sign_request(
        &SignedInputs {
            uri: &format!("https://{HOST}{PATH}"),
            payload: &[],
            creation_date_time: CREATED,
            transaction_id: TX,
            initial_transaction_id: initial_tx,
        },
        &key,
    )
    .expect("sign");
    let der = base64::engine::general_purpose::STANDARD
        .decode(CALLER_CERT)
        .expect("fixture");
    let mut req = Request::builder()
        .method("POST")
        .uri(PATH)
        .header("host", HOST)
        .header("x-mako-client-mp-id", CALLER)
        .header("creationDateTime", CREATED)
        .header("transactionId", TX)
        .header(content_security::HEADER_DIGEST, digest)
        .header(content_security::HEADER_SIGNATURE, signature)
        .header(
            content_security::HEADER_CERT,
            content_security::cert_header_value(&der),
        );
    if let Some(id) = initial_tx {
        req = req.header("initialTransactionId", id);
    }
    req.body(Body::empty()).expect("request")
}

/// A retransmission names its first attempt in `initialTransactionId`, and
/// Kap. 3.3 signs it „soweit vorhanden". Such a request, signed as the
/// Regelungen specify, must verify and reach the handler.
#[tokio::test]
async fn a_signed_initial_transaction_id_reaches_the_handler() {
    let res = app()
        .await
        .oneshot(signed_with(
            CALLER_KEY,
            Some("0b6b2d3e-6f0a-4f53-9d1e-2b8a4c1f7e90"),
        ))
        .await
        .expect("service call");
    assert_ne!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a request signed over its initialTransactionId must verify"
    );
}

/// The `initialTransactionId` is a signed input: altering it under a valid
/// signature is refused.
#[tokio::test]
async fn an_altered_initial_transaction_id_is_refused() {
    let mut req = signed_with(CALLER_KEY, Some("0b6b2d3e-6f0a-4f53-9d1e-2b8a4c1f7e90"));
    req.headers_mut().insert(
        "initialTransactionId",
        "11111111-1111-1111-1111-111111111111"
            .parse()
            .expect("header"),
    );
    let res = app().await.oneshot(req).await.expect("service call");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_forged_signature_is_refused() {
    let res = app()
        .await
        .oneshot(signed(OTHER_KEY))
        .await
        .expect("service call");
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "a signature that does not verify against the named certificate is refused"
    );
}

#[tokio::test]
async fn a_tampered_request_is_refused() {
    let mut req = signed(CALLER_KEY);
    req.headers_mut().insert(
        "transactionId",
        "00000000-0000-0000-0000-000000000000"
            .parse()
            .expect("header"),
    );
    let res = app().await.oneshot(req).await.expect("service call");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_valid_signature_reaches_the_handler() {
    let res = app()
        .await
        .oneshot(signed(CALLER_KEY))
        .await
        .expect("service call");
    let status = res.status();
    let body = http_body_util::BodyExt::collect(res.into_body())
        .await
        .expect("body")
        .to_bytes();
    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "a signature that verifies must pass: {}",
        String::from_utf8_lossy(&body)
    );
}

/// The boot proves the signing key and the certificate sent as `X-BDEW-CERT`
/// belong together: a certificate naming another key would make every signed
/// request fail the receiver's check.
#[test]
fn the_signing_pair_must_belong_together() {
    let pem = format!("-----BEGIN CERTIFICATE-----\n{CALLER_CERT}\n-----END CERTIFICATE-----\n");
    assert!(webdienste::ContentSigning::from_pem(CALLER_KEY, &pem).is_ok());
    assert!(webdienste::ContentSigning::from_pem(OTHER_KEY, &pem).is_err());
}
