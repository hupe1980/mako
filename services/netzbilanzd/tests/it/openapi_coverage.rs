//! Every `/api/v1` route netzbilanzd mounts is in its OpenAPI document, and the
//! committed `openapi.json` is that document.
//!
//! Three halves, because none alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter`, whose `.routes(routes!(..))` registers a
//! handler and its document entry together — so a route is undocumented only if
//! it bypasses that router with a plain `.route("/api/v1/…")` or a `.nest`,
//! which the text half refuses. The runtime half sends each documented
//! operation to the router as `mako_service::run` mounts it, with `[mabis]`, and
//! requires it to reach a handler: a document entry whose path or method the
//! router does not serve would be a contract an integrator generates a client
//! for and that answers 404. The committed half holds `openapi.json`, which
//! clients are generated from.

use tower::ServiceExt as _;

const SOURCES: &[(&str, &str)] = &[
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
];

/// netzbilanzd's document. The `Subscriptions` value is needed for its routes,
/// not for a database: the pool never connects.
fn openapi() -> utoipa::openapi::OpenApi {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://unused@127.0.0.1:1/unused")
        .expect("a lazy pool opens no connection");
    let subs = mako_service::subscriptions::Subscriptions::new(
        pool,
        "netzbilanzd",
        "9900357000004",
        mako_service::subscriptions::SubscriptionsConfig::default(),
    );
    netzbilanzd::daemon::openapi_document(netzbilanzd::daemon::api(&subs).split_for_parts().1)
}

/// No `/api/v1` route is mounted with a plain `.route(…)` or under a
/// `.nest(…)`, so none can be missing from the document.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let mut bypassing = Vec::new();
    for (name, src) in SOURCES {
        for call in [".route(", ".nest("] {
            let mut rest = *src;
            while let Some(i) = rest.find(call) {
                rest = &rest[i + call.len()..];
                if rest.trim_start().starts_with("\"/api/v1") {
                    let line = rest.lines().next().unwrap_or_default();
                    bypassing.push(format!("{name}: {call}{line}"));
                }
            }
        }
    }
    assert!(
        bypassing.is_empty(),
        "these `/api/v1` routes are mounted outside the `OpenApiRouter`, so \
         `/api/v1/openapi.json` does not describe them: {bypassing:#?}"
    );
}

/// The committed `openapi.json` is the document netzbilanzd serves
/// (`BLESS_MCP_SNAPSHOT=1` rewrites it).
#[tokio::test]
async fn the_openapi_document_matches_the_committed_one() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    mako_service::openapi::assert_committed(&openapi(), &path);
}

/// A sample value for a path parameter, so a typed `Path` extractor accepts it
/// — the request must fail on the token, not on routing.
fn sample(param: &str) -> &'static str {
    match param {
        "year" => "2026",
        "month" => "1",
        "id" | "draft_id" => "00000000-0000-4000-8000-000000000000",
        "malo_id" | "ggv_malo_id" => "51238696012",
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
/// handler — and the REMADV webhook refuses the unsigned body 401 — while an
/// unrouted path is 404 and an unrouted method 405.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let peer = crate::router_builds::peer().await;
    let mut cfg = crate::router_builds::mabis_config(&peer, "biko-bilateral");
    cfg.allow_insecure_no_auth = false;
    cfg.oidc = Some(serde_json::from_value(serde_json::json!({"mode": "demo"})).unwrap());
    let app = mako_service::build_for_test::<netzbilanzd::Netzbilanzd>(cfg)
        .await
        .expect("the router builds");

    let doc = serde_json::to_value(openapi()).expect("serializes");
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
                .header("content-type", "application/json")
                .body(axum::body::Body::from("{}"))
                .unwrap();
            let status = app.clone().oneshot(request).await.unwrap().status();
            if status != axum::http::StatusCode::UNAUTHORIZED {
                unserved.push(format!("{} {template} → {status}", method.to_uppercase()));
            }
        }
    }
    assert!(
        operations > 40,
        "only {operations} documented operations — the document lost its routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach an authenticating handler: {unserved:#?}"
    );
}
