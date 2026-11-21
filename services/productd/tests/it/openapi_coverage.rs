//! Every `/api/v1` route productd mounts is in its OpenAPI document, and the
//! committed `openapi.json` is that document.
//!
//! Three halves, because none alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter`, whose `.routes(routes!(..))` registers a
//! handler and its document entry together — so a route is undocumented only if
//! it bypasses that router with a plain `.route("/api/v1/…")`, which the text
//! half refuses. The runtime half sends each documented operation to the router
//! as `mako_service::run` mounts it and requires it to reach a handler: a
//! document entry whose path or method the router does not serve would be a
//! contract an integrator generates a client for and that answers 404. The
//! committed half holds `openapi.json`, which clients are generated from.

use tower::ServiceExt as _;

const SOURCES: &[(&str, &str)] = &[
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

/// The routes that answer without a bearer: the two § 41c Abs. 5 EnWG feeds.
const PUBLIC: &[&str] = &["/api/v1/comparison-feed", "/api/v1/comparison-feed/bo4e"];

/// productd's document. The `Subscriptions` value is needed for its routes,
/// not for a database: the pool never connects.
fn openapi() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = mako_service::subscriptions::Subscriptions::new(
        pool,
        "productd",
        "9900357000004",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    productd::daemon::api(&subs).split_for_parts().1
}

fn document() -> serde_json::Value {
    serde_json::to_value(openapi()).expect("the document serializes")
}

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
            if arg.starts_with("\"/api/v1") {
                let path = &arg[1..arg[1..].find('"').map_or(arg.len(), |e| e + 1)];
                bypassing.push(format!("{name}: {path}"));
            }
        }
    }
    assert!(
        bypassing.is_empty(),
        "these `/api/v1` routes are mounted with `.route(…)` rather than through the \
         `OpenApiRouter`, so `/api/v1/openapi.json` does not describe them: {bypassing:#?}"
    );
}

/// The committed `openapi.json` is the document productd serves
/// (`BLESS_MCP_SNAPSHOT=1` rewrites it).
#[tokio::test]
async fn the_openapi_document_matches_the_committed_one() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    mako_service::openapi::assert_committed(&openapi(), &path);
}

/// The webhook routes are part of productd's document.
#[tokio::test]
async fn the_webhook_routes_are_documented() {
    let doc = document();
    for path in [
        "/api/v1/webhooks/endpoints",
        "/api/v1/webhooks/endpoints/{id}/deliveries",
        "/api/v1/events",
    ] {
        assert!(doc["paths"][path].is_object(), "{path} is not documented");
    }
}

/// A sample value for a path parameter, so a typed `Path` extractor accepts it
/// — the request must fail on the token, not on routing.
fn sample(param: &str) -> &'static str {
    match param {
        "year" => "2026",
        "month" => "1",
        "date" => "2026-01-01",
        "id" => "00000000-0000-4000-8000-000000000000",
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
/// handler — an unrouted path is 404, an unrouted method 405. The two public
/// feeds take no token, so they only have to be routed.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let cfg: productd::config::ProductdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": "postgres://startup-test/productd"},
        "tenant": "stadtwerke-musterstadt",
        "lf_mp_id": "9900357000028",
        "oidc": {"mode": "demo"},
    }))
    .expect("the configuration parses");
    let app = mako_service::build_for_test::<productd::Productd>(cfg)
        .await
        .expect("the router builds");

    let doc = document();
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
            let served = if PUBLIC.contains(&template.as_str()) {
                status != axum::http::StatusCode::NOT_FOUND
                    && status != axum::http::StatusCode::METHOD_NOT_ALLOWED
            } else {
                status == axum::http::StatusCode::UNAUTHORIZED
            };
            if !served {
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
