//! Every `/api/v1` route portald mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter` (`openapi::api_router`), whose
//! `.routes(routes!(..))` registers a handler and its document entry together —
//! so a route is undocumented only if it bypasses that router with a plain
//! `.route("/api/v1/…")`, which the text half refuses. The runtime half sends
//! each documented operation to the router as the service builds it and
//! requires it to reach a handler: a document entry whose path or method the
//! router does not serve is a contract a client is generated for and that
//! answers 404.

use axum::http::{Method, StatusCode};

use super::authorization_guard::{app, call};

const SOURCES: &[(&str, &str)] = &[
    ("server.rs", include_str!("../../src/server.rs")),
    ("openapi.rs", include_str!("../../src/openapi.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
    ("nachrechnen.rs", include_str!("../../src/nachrechnen.rs")),
];

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document. `GET /api/v1/openapi.json` itself is
/// `mako_service::openapi::serve`'s; `/nachrechnen/{token}` is a public HTML
/// page and deliberately outside the document.
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

/// A sample value for a path parameter.
fn concrete(template: &str) -> String {
    template
        .replace("{malo_id}", "51238696012")
        .replace("{record_id}", "0195f6c2-0000-7000-8000-000000000001")
        .replace("{document_id}", "0195f6c2-0000-7000-8000-000000000002")
}

/// Every documented operation reaches a handler, and the document is served.
///
/// Without a token, a routed handler answers with an `ApiError` body — a 401
/// from the ownership gate, or the JSON extractor's refusal of an empty body,
/// which runs only inside a routed handler. An unrouted path or method answers
/// axum's bare 404/405.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let app = app(StatusCode::UNAUTHORIZED).await;
    let reached = |status: StatusCode, body: &str| {
        status != StatusCode::NOT_FOUND
            && status != StatusCode::METHOD_NOT_ALLOWED
            && serde_json::from_str::<serde_json::Value>(body).is_ok_and(|b| b["error"].is_string())
    };
    let (status, body) = call(&app, &Method::GET, "/api/v1/kein-solcher-pfad", None, false).await;
    assert!(
        !reached(status, &body),
        "an unrouted path answered {status} {body}"
    );

    let (status, served) = call(&app, &Method::GET, "/api/v1/openapi.json", None, false).await;
    assert_eq!(status, StatusCode::OK, "{served}");
    let served: serde_json::Value = serde_json::from_str(&served).expect("JSON document");
    let doc = portald::openapi::document();
    assert_eq!(served, doc, "the served document is not the built one");
    assert!(
        doc["components"]["securitySchemes"][portald::openapi::CUSTOMER_SCHEME].is_object(),
        "the customer bearer scheme is not documented"
    );

    let paths = doc["paths"].as_object().expect("a paths object");
    assert!(
        !paths.contains_key("/nachrechnen/{token}"),
        "the public HTML page is not part of the API document"
    );
    let mut operations = 0usize;
    let mut unserved = Vec::new();
    for (template, item) in paths {
        for method in ["get", "post", "put", "delete", "patch"] {
            if item.get(method).is_none() {
                continue;
            }
            operations += 1;
            let m = Method::from_bytes(method.to_uppercase().as_bytes()).expect("method");
            let (status, body) = call(&app, &m, &concrete(template), None, false).await;
            if !reached(status, &body) {
                unserved.push(format!("{m} {template} → {status} {body}"));
            }
        }
    }
    assert_eq!(
        operations, 18,
        "the document lists {operations} operations — it lost or gained routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach a handler: {unserved:#?}"
    );
}
