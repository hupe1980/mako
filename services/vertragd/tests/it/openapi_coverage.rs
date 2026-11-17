//! Every `/api/v1` route vertragd mounts is in its OpenAPI document, and the
//! committed `openapi.json` is that document.
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
    ("handlers/mod.rs", include_str!("../../src/handlers/mod.rs")),
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

/// The webhook subscription module on a pool that never connects — enough to
/// build its routes and their document.
fn lazy_subscriptions() -> mako_service::subscriptions::Subscriptions {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    mako_service::subscriptions::Subscriptions::new(
        pool,
        "vertragd",
        "9900357000004",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    )
}

fn document() -> utoipa::openapi::OpenApi {
    vertragd::handlers::documented(&lazy_subscriptions()).1
}

/// The committed `openapi.json` is the document vertragd serves
/// (`BLESS_MCP_SNAPSHOT=1` rewrites it); integrators and the Desk generate
/// their clients from the committed file.
#[tokio::test]
async fn the_openapi_document_matches_the_committed_one() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    mako_service::openapi::assert_committed(&document(), &path);
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
            let Some(lit) = arg.strip_prefix('"') else {
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

/// The webhook subscription routes are part of the document.
#[tokio::test]
async fn the_document_carries_the_webhook_routes() {
    let doc = serde_json::to_value(document()).expect("serializes");
    for path in ["/api/v1/webhooks/endpoints", "/api/v1/events"] {
        assert!(doc["paths"][path].is_object(), "{path} is not documented");
    }
    // The feed's GET and the inbound CloudEvent POST share one path.
    assert!(doc["paths"]["/api/v1/events"]["get"].is_object());
    assert!(doc["paths"]["/api/v1/events"]["post"].is_object());
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

/// Every documented operation reaches a handler. An unverifiable token is
/// refused 401 by the `Claims` extractor, which runs only inside a routed
/// handler — an unrouted path is 404, an unrouted method 405. The two inbound
/// webhooks carry no token; they must merely be routed.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let cfg = toml::from_str(include_str!("../../../../deploy/demo/vertragd.toml"))
        .expect("the demo configuration parses");
    let app = mako_service::build_for_test::<vertragd::Vertragd>(cfg)
        .await
        .expect("the router builds");

    let request = |method: &str, uri: &str| {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer not.a.token")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let unrouted = app
        .clone()
        .oneshot(request("POST", "/api/v1/kein-solcher-pfad"))
        .await
        .unwrap()
        .status();
    assert_eq!(unrouted, axum::http::StatusCode::NOT_FOUND);

    let doc = serde_json::to_value(document()).expect("serializes");
    let paths = doc["paths"].as_object().expect("a paths object");
    let mut operations = 0usize;
    let mut unserved = Vec::new();
    for (template, item) in paths {
        for method in ["get", "post", "put", "delete", "patch"] {
            let Some(op) = item.get(method) else {
                continue;
            };
            operations += 1;
            let inbound = op["tags"][0] == "inbound";
            let status = app
                .clone()
                .oneshot(request(&method.to_uppercase(), &concrete(template)))
                .await
                .unwrap()
                .status();
            let served = if inbound {
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
        operations > 60,
        "only {operations} documented operations — the document lost its routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach their handler: {unserved:#?}"
    );
}
