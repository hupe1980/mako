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
        (Method::GET, "/api/v1/portal/51238696012/dokumente", None),
        (
            Method::GET,
            "/api/v1/portal/51238696012/dokumente/0195f6c2-0000-7000-8000-000000000002",
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
        (Method::GET, "/api/v1/portal/51238696012/kontakt", None),
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

pub(super) async fn app(vertragd_verdict: StatusCode) -> axum::Router {
    let base = stub_upstreams(vertragd_verdict).await;
    let toml_src = format!(
        r#"
tenant = "stadtwerke-musterstadt"
lf_mp_id = "9900357000028"
vertragd_url    = "{base}"
edmd_url        = "{base}"
billingd_url    = "{base}"
accountingd_url = "{base}"
einsd_url       = "{base}"
marktd_url      = "{base}"
"#
    );
    let cfg: portald::config::PortaldConfig = toml::from_str(&toml_src).expect("config parses");
    mako_service::build_for_test::<portald::server::Portald>(cfg)
        .await
        .expect("router builds")
}

pub(super) async fn call(
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
        toml::from_str("tenant = \"stadtwerke-musterstadt\"\nlf_mp_id = \"9900357000028\"\n")
            .expect("config parses");
    let err = mako_service::build_for_test::<portald::server::Portald>(cfg)
        .await
        .expect_err("must refuse to start");
    let msg = format!("{err:#}");
    assert!(msg.contains("vertragd_url"), "{msg}");
    assert!(
        msg.contains("allow_insecure_no_auth"),
        "the refusal must name the way to opt out deliberately: {msg}"
    );
}

/// A self-service SEPA mandate is registered under the Lieferant's own MP-ID:
/// the key is required, and the tenant in its place refuses the start.
#[test]
fn the_lf_mp_id_is_required_and_never_the_tenant() {
    use mako_service::Daemon as _;
    assert!(
        toml::from_str::<portald::config::PortaldConfig>("tenant = \"stadtwerke-musterstadt\"\n")
            .is_err(),
        "a config without lf_mp_id does not parse"
    );
    let cfg: portald::config::PortaldConfig = toml::from_str(
        "tenant = \"stadtwerke-musterstadt\"\nlf_mp_id = \"stadtwerke-musterstadt\"\n\
         allow_insecure_no_auth = true\n",
    )
    .expect("config parses");
    let pfade: Vec<String> = portald::server::Portald::validate(&cfg)
        .into_iter()
        .map(|b| b.pfad)
        .collect();
    assert_eq!(pfade, ["lf_mp_id"]);
}

/// `portald` serves no MCP.
///
/// An MCP tool takes a `malo_id` and carries no customer token, so it cannot go
/// through the `vertragd` ownership gate every portal route goes through: a
/// `/mcp` here would read every customer in the tenant. Agents read the same
/// data from the service that owns it, behind that service's Cedar policy.
#[tokio::test]
async fn mcp_is_not_served() {
    let app = app(StatusCode::OK).await;
    for method in [Method::GET, Method::POST] {
        let (status, body) = call(
            &app,
            &method,
            "/mcp",
            Some(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#),
            true,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} /mcp: {body}");
    }
}

/// The table above must not drift behind the router.
///
/// An operation added to the API and not listed here would be covered by none
/// of the assertions above — the silent half of the failure this file exists to
/// prevent. Every `/api/v1` route is mounted through `openapi::api_router`, so
/// the document's operations are the router's: each must have an entry here,
/// with the same method and a path its template matches.
#[test]
fn the_route_table_covers_the_router() {
    let doc = portald::openapi::document();
    let paths = doc["paths"].as_object().expect("a paths object");
    let table = routes();
    let mut operations = 0usize;
    let mut missing = Vec::new();
    for (template, item) in paths {
        for method in ["get", "post", "put", "delete", "patch"] {
            if item.get(method).is_none() {
                continue;
            }
            operations += 1;
            let covered = table.iter().any(|(m, path, _)| {
                m.as_str().eq_ignore_ascii_case(method) && matches_template(template, path)
            });
            if !covered {
                missing.push(format!("{} {template}", method.to_uppercase()));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "these operations are served but not exercised here: {missing:#?}"
    );
    assert_eq!(
        operations,
        table.len(),
        "the API serves {operations} operations but the guard lists {}",
        table.len()
    );
}

/// `path` is an instance of the OpenAPI path `template`.
fn matches_template(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    t.len() == p.len()
        && t.iter()
            .zip(&p)
            .all(|(t, p)| (t.starts_with('{') && t.ends_with('}')) || t == p)
}
