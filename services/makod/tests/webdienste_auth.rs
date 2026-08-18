//! The API-Webdienste Strom port (`:8090`) is authenticated.
//!
//! # Why this is a test
//!
//! `:8090` carries iMSys Steuerbefehle — the REST/JSON path by which a
//! Steuerbefehl reaches a controllable consumption device. An unauthenticated
//! caller reaching these routes could switch customer installations. The BDEW
//! API-Webdienste specification accordingly requires authenticated access, and
//! `makod` refuses to enable the port unless `--auth-key` or `--oidc-issuer` is
//! configured.
//!
//! The auth layer itself was never in doubt — `webdienste_auth_middleware`
//! reads correctly. What was untested is whether it is *attached*. A
//! middleware that is written, exported, and never layered onto the router
//! leaves the port wide open while every symbol involved looks right, so these
//! tests drive [`webdienste::build_app`] — the same function `main.rs` calls to
//! assemble the port — rather than rebuilding the layer stack here, which would
//! only prove the test's own copy correct.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use secrecy::SecretString;
use tower::ServiceExt as _;

use makod::cedar_authz::{CedarAuthorizer, DefaultPolicy, NamedKey};
use makod::webdienste::{self, MakodApiHandler, WebdiensteAuthState};

/// Any `:8090` route — the assertions below are about the auth layer in front
/// of it, which runs before routing reaches a handler.
const ROUTE: &str = "/[Post]/steuerbefehl/konfiguration/";

const TENANT: &str = "9900357000004";

/// A well-formed Control Measures konfiguration call. The API passes the
/// command and location as query parameters, not as a JSON body.
const CONTROL_URI: &str = "/[Post]/steuerbefehl/konfiguration/?locationId=E1234848431&commandControl=%7B%22maximumPowerValue%22%3A%224.2%22%2C%22executionTimeFrom%22%3A%222026-09-01T00%3A00%3A00Z%22%7D";

async fn handler() -> Arc<MakodApiHandler> {
    let store = mako_engine::store_slatedb::SlateDbStore::open_in_memory()
        .await
        .expect("open in-memory SlateDB");
    Arc::new(MakodApiHandler {
        store,
        tenant_id: mako_engine::ids::TenantId::from_party_id(TENANT),
    })
}

/// An authorizer holding one named key, plus whatever extra policy is given.
fn authorizer(extra: Option<&str>, default_policy: DefaultPolicy) -> Arc<CedarAuthorizer> {
    Arc::new(
        CedarAuthorizer::new(
            vec![NamedKey {
                name: Arc::from("api-webdienste-client"),
                token: SecretString::new("s3cret".to_owned().into()),
            }],
            extra.map(str::to_owned),
            None,
            None,
            default_policy,
        )
        .expect("authorizer construction"),
    )
}

fn auth_state(cedar: Arc<CedarAuthorizer>) -> WebdiensteAuthState {
    WebdiensteAuthState {
        cedar,
        tenant: Arc::from(TENANT),
    }
}

fn post(token: Option<&str>) -> Request<Body> {
    let mut req = Request::builder().method("POST").uri(ROUTE);
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    req.header("content-type", "application/json")
        .body(Body::from("{}"))
        .expect("build request")
}

/// No `Authorization` header — the port must answer 401 before routing.
#[tokio::test]
async fn an_anonymous_request_is_rejected() {
    let app = webdienste::build_app(
        handler().await,
        Some(auth_state(authorizer(None, DefaultPolicy::PermitAll))),
        1024 * 1024,
    );
    let res = app.oneshot(post(None)).await.expect("service call");
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must not reach a Steuerbefehl route"
    );
}

/// A token that matches no configured key is 401, not a pass-through.
#[tokio::test]
async fn an_unknown_token_is_rejected() {
    let app = webdienste::build_app(
        handler().await,
        Some(auth_state(authorizer(None, DefaultPolicy::PermitAll))),
        1024 * 1024,
    );
    let res = app
        .oneshot(post(Some("not-the-configured-token")))
        .await
        .expect("service call");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// A valid key whose principal is not granted `UseWebdienste` is 403.
///
/// Uses `DefaultPolicy::Deny` with a policy set that grants the principal
/// something *other* than `UseWebdienste`, so the denial comes from the action
/// check rather than from the principal being unknown.
#[tokio::test]
async fn a_principal_without_use_webdienste_is_forbidden() {
    let extra = r#"
permit(
  principal == MaKo::Principal::"api-webdienste-client",
  action    == MaKo::Action::"AdminMaloRead",
  resource
);
    "#;
    let app = webdienste::build_app(
        handler().await,
        Some(auth_state(authorizer(Some(extra), DefaultPolicy::Deny))),
        1024 * 1024,
    );
    let res = app
        .oneshot(post(Some("s3cret")))
        .await
        .expect("service call");
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "authentication alone must not grant Steuerbefehl access"
    );
}

/// A valid key granted `UseWebdienste` passes the auth layer.
///
/// The assertion is deliberately negative: what the handler then does with the
/// body is the API's business, but it must not be turned away as 401/403.
#[tokio::test]
async fn a_granted_principal_passes_the_auth_layer() {
    let extra = r#"
permit(
  principal == MaKo::Principal::"api-webdienste-client",
  action    == MaKo::Action::"UseWebdienste",
  resource
);
    "#;
    let app = webdienste::build_app(
        handler().await,
        Some(auth_state(authorizer(Some(extra), DefaultPolicy::Deny))),
        1024 * 1024,
    );
    let res = app
        .oneshot(post(Some("s3cret")))
        .await
        .expect("service call");
    assert!(
        res.status() != StatusCode::UNAUTHORIZED && res.status() != StatusCode::FORBIDDEN,
        "a principal granted UseWebdienste must pass the auth layer, got {}",
        res.status()
    );
}

/// `--webdienste-allow-unauthenticated` (`auth: None`) really does remove the
/// layer.
///
/// This pins the escape hatch as the *only* way to reach a route without a
/// token: if the two branches ever converge, the tests above stop meaning
/// anything, because they would pass whether or not auth were wired.
#[tokio::test]
async fn the_opt_out_removes_the_layer() {
    let app = webdienste::build_app(handler().await, None, 1024 * 1024);
    let res = app.oneshot(post(None)).await.expect("service call");
    assert_ne!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "with auth disabled an anonymous request must reach the router"
    );
}

// ── Caller identity ──────────────────────────────────────────────────────────

/// A Steuerungsauftrag whose originator is unknown must be refused, not
/// attributed to the operator.
///
/// # Why this is a test
///
/// The Control Measures request body carries no sending party — BDEW identifies
/// the caller by their mTLS client certificate, terminated at the proxy. This
/// code used to fill the workflow's `sender_mp_id` with the operator's *own*
/// Marktpartner-ID, and that field is what the `DispatchConfirmed` outbox entry
/// is addressed to. The §14a EnWG confirmation was therefore sent to ourselves:
/// combined with the loopback path for own MP-IDs it never left the process, so
/// the NB or LF that ordered the control action was never told it had been
/// carried out, and the §14a billing event named the wrong party.
#[tokio::test]
async fn a_control_order_without_a_caller_mp_id_is_refused() {
    let app = webdienste::build_app(
        handler().await,
        Some(WebdiensteAuthState {
            cedar: authorizer(None, DefaultPolicy::PermitAll),
            tenant: Arc::from(TENANT),
        }),
        1024 * 1024,
    );
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(CONTROL_URI)
                .header("authorization", "Bearer s3cret")
                .header("transactionId", "tx-identity-1")
                .header("creationDateTime", "2026-08-18T10:00:00Z")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("service call");

    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "an order with no identifiable sender must be refused — the Endantwort \
         has nowhere to go"
    );
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .expect("body");
    assert!(
        body.contains(webdienste::CLIENT_MP_ID_HEADER),
        "the refusal must name the header that fixes it: {body}"
    );
}

/// The same request with the proxy-supplied Marktpartner-ID gets past the
/// identity gate, so the test above is about the missing header and not about
/// the body being rejected for some other reason.
#[tokio::test]
async fn a_control_order_with_a_caller_mp_id_passes_the_identity_gate() {
    let app = webdienste::build_app(
        handler().await,
        Some(WebdiensteAuthState {
            cedar: authorizer(None, DefaultPolicy::PermitAll),
            tenant: Arc::from(TENANT),
        }),
        1024 * 1024,
    );
    let res = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(CONTROL_URI)
                .header("authorization", "Bearer s3cret")
                .header("transactionId", "tx-identity-2")
                .header("creationDateTime", "2026-08-18T10:00:00Z")
                .header(webdienste::CLIENT_MP_ID_HEADER, "9900001000002")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("service call");

    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024)
        .await
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    assert!(
        !body.contains(webdienste::CLIENT_MP_ID_HEADER),
        "the identity gate must not reject a request carrying the header \
         (status {status}): {body}"
    );
}
