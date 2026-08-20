//! Every portal route resolves customer ownership before it answers.
//!
//! # Why this is a test
//!
//! `portald` serves one customer's consumption profile, account statement,
//! invoices and contract. A handler that omits the check looks exactly like one
//! that makes it, and reading the wrong customer's data is not an error the
//! caller reports.
//!
//! The gate lives in one place (`portald::auth::authorize`) and hands back a
//! `PortalAuthCtx` that handlers need in order to do their work. This test
//! covers the rest: it drives **every route the router exposes** against a
//! `vertragd` that refuses everything, and fails if any of them answers with
//! upstream data.
//!
//! Adding a route without a check makes it fail. Adding a route and forgetting
//! to list it here makes `the_route_table_covers_the_router` fail.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::IntoResponse as _;
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;

/// Every customer-scoped route, as `(method, path, body)`.
///
/// The MaLo belongs to somebody else; the point of each case is that the answer
/// must not depend on whether it exists.
fn routes() -> Vec<(Method, &'static str, Option<&'static str>)> {
    vec![
        (Method::GET, "/api/v1/portal/51238696012/dashboard", None),
        (Method::GET, "/api/v1/portal/51238696012/lastgang", None),
        (Method::GET, "/api/v1/portal/51238696012/invoices", None),
        (
            Method::GET,
            "/api/v1/portal/51238696012/invoices/0195f6c2-0000-7000-8000-000000000001/download",
            None,
        ),
        (Method::GET, "/api/v1/portal/51238696012/balance", None),
        (Method::GET, "/api/v1/portal/51238696012/kontoauszug", None),
        (
            Method::GET,
            "/api/v1/portal/51238696012/vorauszahlung",
            None,
        ),
        (Method::GET, "/api/v1/portal/51238696012/eeg", None),
        (Method::GET, "/api/v1/portal/51238696012/versorgung", None),
        (Method::GET, "/api/v1/portal/51238696012/vertrag", None),
        (
            Method::GET,
            "/api/v1/portal/51238696012/kuendigungsfrist",
            None,
        ),
        (
            Method::POST,
            "/api/v1/portal/51238696012/tarifwechsel",
            Some(r#"{"new_product_code":"OEKO24","wirksamkeit":"2027-01-01"}"#),
        ),
        (
            Method::POST,
            "/api/v1/portal/51238696012/kuendigen",
            Some(r#"{"lieferende":"2027-01-31"}"#),
        ),
        (
            Method::PUT,
            "/api/v1/portal/51238696012/kontakt",
            Some(r#"{"sepa_erlaubt":true}"#),
        ),
        (
            Method::PUT,
            "/api/v1/portal/51238696012/sepa",
            Some(r#"{"iban":"DE02120300000000202051"}"#),
        ),
    ]
}

/// A `vertragd` that answers every ownership question with `verdict`, and
/// upstreams that would happily hand over data if anything reached them.
///
/// Every upstream URL points at this one server, so a route that skips the gate
/// proxies to it and gets a `200` carrying `"leaked"` — which is exactly the
/// failure these assertions look for.
async fn stub_upstreams(verdict: StatusCode) -> String {
    let app = axum::Router::new().fallback(move |req: Request<Body>| async move {
        if req.uri().path().ends_with("/kunden/authenticate") {
            return (verdict, String::new()).into_response();
        }
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"leaked":true,"malo_id":"51238696012"}"#,
        )
            .into_response()
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

async fn app(vertragd_verdict: StatusCode) -> axum::Router {
    let base = stub_upstreams(vertragd_verdict).await;
    let toml_src = format!(
        r#"
port   = 9480
tenant = "9900357000004"
vertragd_url    = "{base}"
edmd_url        = "{base}"
billingd_url    = "{base}"
accountingd_url = "{base}"
einsd_url       = "{base}"
marktd_url      = "{base}"
"#
    );
    let cfg: portald::config::PortaldConfig = toml::from_str(&toml_src).expect("config parses");
    let ctx = mako_service::ServiceContext {
        pool: None,
        http: mako_service::http::default_client(),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    <portald::server::Portald as mako_service::Daemon>::build(Arc::new(cfg), ctx)
        .await
        .expect("router builds")
}

async fn call(
    app: &axum::Router,
    method: &Method,
    path: &str,
    body: Option<&str>,
    token: bool,
) -> (StatusCode, String) {
    let mut req = Request::builder()
        .method(method.clone())
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json");
    if token {
        req = req.header(header::AUTHORIZATION, "Bearer customer-token");
    }
    let req = req
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_owned())))
        .expect("request builds");
    let resp = app.clone().oneshot(req).await.expect("router responds");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("body collects")
        .to_bytes();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

/// A customer `vertragd` refuses reaches no route and no upstream.
#[tokio::test]
async fn a_refused_customer_reaches_no_route() {
    let app = app(StatusCode::FORBIDDEN).await;
    for (method, path, body) in routes() {
        let (status, body) = call(&app, &method, path, body, true).await;
        assert_eq!(
            status,
            StatusCode::FORBIDDEN,
            "{method} {path} answered {status} for a customer vertragd refused"
        );
        assert!(
            !body.contains("leaked"),
            "{method} {path} reached an upstream despite the refusal: {body}"
        );
    }
}

/// A request with no token gets nothing either — the gate runs before the path
/// parameter is used for anything.
#[tokio::test]
async fn an_anonymous_request_reaches_no_route() {
    let app = app(StatusCode::UNAUTHORIZED).await;
    for (method, path, body) in routes() {
        let (status, body) = call(&app, &method, path, body, false).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} answered {status} without a token"
        );
        assert!(!body.contains("leaked"), "{method} {path}: {body}");
    }
}

/// An authorization service that cannot answer is not an answer of yes.
#[tokio::test]
async fn an_unreachable_vertragd_fails_closed() {
    let app = app(StatusCode::INTERNAL_SERVER_ERROR).await;
    for (method, path, body) in routes() {
        let (status, body) = call(&app, &method, path, body, true).await;
        assert_eq!(
            status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{method} {path} answered {status} while authorization was down"
        );
        assert!(!body.contains("leaked"), "{method} {path}: {body}");
    }
}

/// Starting without `vertragd_url` is refused: nothing else can decide which
/// customer a request is for, so serving anyway hands every customer's ledger
/// to every caller.
#[tokio::test]
async fn a_deployment_without_an_authorization_authority_refuses_to_start() {
    let cfg: portald::config::PortaldConfig =
        toml::from_str("port = 9480\ntenant = \"9900357000004\"\n").expect("config parses");
    let ctx = mako_service::ServiceContext {
        pool: None,
        http: mako_service::http::default_client(),
        shutdown: tokio_util::sync::CancellationToken::new(),
    };
    let err = <portald::server::Portald as mako_service::Daemon>::build(Arc::new(cfg), ctx)
        .await
        .expect_err("must refuse to start");
    let msg = err.to_string();
    assert!(msg.contains("vertragd_url"), "{msg}");
    assert!(
        msg.contains("allow_insecure_no_auth"),
        "the refusal must name the way to opt out deliberately: {msg}"
    );
}

/// The table above must not drift behind the router.
///
/// A route added to `server::router` and not listed here would be covered by
/// none of the assertions above — the silent half of the failure this file
/// exists to prevent.
#[test]
fn the_route_table_covers_the_router() {
    let declared = include_str!("../src/server.rs").matches(".route(").count();
    assert_eq!(
        declared,
        routes().len(),
        "server::router declares {declared} routes but the guard lists {} — every \
         customer-scoped route must be exercised here",
        routes().len()
    );
}
