//! Every `/api/v1` route einsd mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter`, whose `.routes(routes!(..))` registers a
//! handler and its document entry together — so a route is undocumented only if
//! it bypasses that router with a plain `.route("/api/v1/…")`, which the text
//! half refuses. The runtime half sends each documented operation to the router
//! as `mako_service::run` mounts it and requires it to reach a handler: a
//! document entry whose path or method the router does not serve would be a
//! contract the Desk generates a client for and that answers 404.

use tower::ServiceExt as _;

const SOURCES: &[(&str, &str)] = &[
    ("routes.rs", include_str!("../../src/routes.rs")),
    ("reads.rs", include_str!("../../src/reads.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

/// The one `/api/v1` route mounted outside the `OpenApiRouter`: the document
/// itself. `GET /api/v1/policy` is `mako_service::einrichtung::router`'s, shared
/// by every daemon.
const UNDOCUMENTED: &[&str] = &["/api/v1/openapi.json"];

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let mut bypassing = Vec::new();
    for (name, src) in SOURCES {
        let mut rest = *src;
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let arg = rest.trim_start();
            let Some(lit) = arg.strip_prefix('"') else {
                continue;
            };
            let path = &lit[..lit.find('"').unwrap_or(lit.len())];
            if path.starts_with("/api/v1/") && !UNDOCUMENTED.contains(&path) {
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
        "year" => "2026",
        "month" => "1",
        "id" => "00000000-0000-4000-8000-000000000000",
        "malo_id" => "51238696012",
        _ => "X",
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

/// Every documented operation reaches a handler: an unverifiable token is
/// refused 401 by the `Claims` extractor, which runs only inside a routed
/// handler — an unrouted path is 404, an unrouted method 405.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let cfg: einsd::config::EinsdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/einsd"},
        "tenant": "9900357000004",
        "allow_insecure_no_auth": true,
        "oidc": {"mode": "demo"},
    }))
    .expect("the configuration parses");
    let app = mako_service::build_for_test::<einsd::Einsd>(cfg)
        .await
        .expect("the router builds");

    // The probe distinguishes: with the same token, an unrouted path is not a 401.
    let unrouted = app
        .clone()
        .oneshot(
            axum::http::Request::post("/api/v1/kein-solcher-pfad")
                .header("authorization", "Bearer not.a.token")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
        .status();
    assert_eq!(unrouted, axum::http::StatusCode::NOT_FOUND);

    let doc = serde_json::to_value(einsd::routes::openapi_document()).expect("serializes");
    let paths = doc["paths"].as_object().expect("a paths object");
    let mut operations = 0usize;
    let mut unserved = Vec::new();
    for (template, item) in paths {
        for method in ["get", "post", "put", "delete", "patch"] {
            if item.get(method).is_none() {
                continue;
            }
            operations += 1;
            let request = axum::http::Request::builder()
                .method(method.to_uppercase().as_str())
                .uri(concrete(template))
                .header("authorization", "Bearer not.a.token")
                .body(axum::body::Body::empty())
                .unwrap();
            let status = app.clone().oneshot(request).await.unwrap().status();
            if status != axum::http::StatusCode::UNAUTHORIZED {
                unserved.push(format!("{} {template} → {status}", method.to_uppercase()));
            }
        }
    }
    assert!(
        operations > 30,
        "only {operations} documented operations — the document lost its routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach an authenticating handler: {unserved:#?}"
    );
}
