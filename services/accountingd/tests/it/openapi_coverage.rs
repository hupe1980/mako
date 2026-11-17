//! Every `/api/v1` route accountingd mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter` (`openapi::api_router`), whose
//! `.routes(routes!(..))` registers a handler and its document entry together —
//! so a route is undocumented only if it bypasses that router with a plain
//! `.route("/api/v1/…")`, which the text half refuses. The runtime half sends
//! each documented operation to the router as the demo starts it and requires
//! it to reach a handler: a document entry whose path or method the router
//! does not serve is a contract the Desk generates a client for and that
//! answers 404.

use super::router_builds::{call, dev_app};

const SOURCES: &[(&str, &str)] = &[
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("openapi.rs", include_str!("../../src/openapi.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
    ("reads.rs", include_str!("../../src/reads.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document. `GET /api/v1/openapi.json` itself is
/// `mako_service::openapi::serve`'s, and `GET /api/v1/policy`
/// `mako_service::einrichtung::router`'s.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let mut bypassing = Vec::new();
    for (name, src) in SOURCES {
        let mut rest = *src;
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let Some(lit) = rest.trim_start().strip_prefix('"') else {
                continue;
            };
            let path = &lit[..lit.find('"').unwrap_or(lit.len())];
            if path.starts_with("/api/v1/") {
                bypassing.push(format!("{name}: {path}"));
            }
        }
    }
    assert!(
        bypassing.is_empty(),
        "these `/api/v1` routes are mounted with `.route(…)` rather than through \
         the `OpenApiRouter`, so `/api/v1/openapi.json` does not describe them: \
         {bypassing:#?}"
    );
}

/// A sample value for a path parameter, so a typed `Path` extractor would
/// accept it — the request must fail on the token, not on routing.
fn sample(param: &str) -> &'static str {
    match param {
        "malo_id" => "51238696012",
        "period_id" => "2026-01",
        "position" => "1",
        "kunden_nr" | "key" | "entry_id" => "X",
        _ => "00000000-0000-4000-8000-000000000000",
    }
}

fn concrete(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').expect("a closed parameter") + open;
        out.push_str(sample(&rest[open + 1..close]));
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

fn probe(method: &str, uri: &str) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", "Bearer not.a.token")
        .body(axum::body::Body::empty())
        .expect("request")
}

/// Every documented operation reaches a handler: under the demo's `[oidc]`, an
/// unverifiable token is refused by the `Claims` extractor, which runs only
/// inside a routed handler, with a 401 `ApiError` body.
///
/// The status alone does not tell: the MCP router's auth layer also wraps the
/// merged router's fallback, so an unrouted path with a bearer token is a 401
/// too — but a plain-text one. The `ApiError` body is the handler's mark.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn every_documented_operation_is_served() {
    let Some((app, _pool, _pg)) = dev_app().await else {
        return;
    };
    let reached = |status: axum::http::StatusCode, body: &serde_json::Value| {
        status == axum::http::StatusCode::UNAUTHORIZED && body["error"].is_string()
    };
    // The probe distinguishes: with the same token, an unrouted path is not
    // answered by a handler.
    let (status, body) = call(app.clone(), probe("POST", "/api/v1/kein-solcher-pfad")).await;
    assert!(
        !reached(status, &body),
        "an unrouted path answered {status} {body}"
    );

    let doc = accountingd::openapi::document();
    let paths = doc["paths"].as_object().expect("a paths object");
    let mut operations = 0usize;
    let mut unserved = Vec::new();
    for (template, item) in paths {
        for method in ["get", "post", "put", "delete", "patch"] {
            if item.get(method).is_none() {
                continue;
            }
            operations += 1;
            let method = method.to_uppercase();
            let (status, body) = call(app.clone(), probe(&method, &concrete(template))).await;
            if !reached(status, &body) {
                unserved.push(format!("{method} {template} → {status} {body}"));
            }
        }
    }
    assert!(
        operations > 70,
        "only {operations} documented operations — the document lost its routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach an authenticating handler: {unserved:#?}"
    );
}
