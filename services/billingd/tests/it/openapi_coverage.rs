//! Every `/api/v1` route billingd mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter`, whose `.routes(routes!(..))` registers a
//! handler and its document entry together — so a route is undocumented only if
//! it bypasses that router with a plain `.route("/api/v1/…")`, which the text
//! half refuses. The runtime half sends each documented operation to the router
//! as `mako_service::run` mounts it and requires it to reach a handler: a
//! document entry whose path or method the router does not serve would be a
//! contract an integrator generates a client for and that answers 404.

use tower::ServiceExt as _;

const SOURCES: &[(&str, &str)] = &[
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("openapi.rs", include_str!("../../src/openapi.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

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
            if arg.starts_with("\"/api/v1/") {
                bypassing.push(format!(
                    "{name}: {}",
                    &arg[..arg.find(',').unwrap_or(arg.len())]
                ));
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

/// Every documented operation reaches a handler: an unrouted path answers 404
/// and an unrouted method 405 before any handler runs, so neither may appear.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/demo/billingd.toml");
    let cfg = mako_service::load_config_file::<billingd::config::BillingdConfig>("billingd", &path)
        .expect("the demo config loads");
    let app = mako_service::build_for_test::<billingd::daemon::Billingd>(cfg)
        .await
        .expect("the router builds");

    let doc = serde_json::to_value(billingd::openapi::document()).expect("serializes");
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
            if status == axum::http::StatusCode::NOT_FOUND
                || status == axum::http::StatusCode::METHOD_NOT_ALLOWED
            {
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
        "these documented operations do not reach a handler: {unserved:#?}"
    );
}

/// The webhook routes of the shared subscription module are part of
/// billingd's document, so an integrator finds them beside the billing routes.
#[tokio::test]
async fn the_webhook_routes_are_documented() {
    let doc = serde_json::to_value(billingd::openapi::document()).expect("serializes");
    for path in ["/api/v1/webhooks/endpoints", "/api/v1/events"] {
        assert!(doc["paths"].get(path).is_some(), "{path} is not documented");
    }
}
