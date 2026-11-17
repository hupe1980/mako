//! Every `/api/v1` route agentd serves itself is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter` (`openapi::api_router`), whose
//! `.routes(routes!(..))` registers a handler and its document entry together —
//! so a route is undocumented only if it bypasses that router with a plain
//! `.route("/api/v1/…")`, which the text half refuses. The runtime half sends
//! each documented operation to the router as `build` returns it and requires
//! it to reach a handler: a document entry whose path or method the router
//! does not serve is a contract a client is generated for and that answers 404.

const SOURCES: &[(&str, &str)] = &[
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("openapi.rs", include_str!("../../src/openapi.rs")),
    ("handlers.rs", include_str!("../../src/handlers.rs")),
];

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document. `GET /api/v1/openapi.json` itself is
/// `mako_service::openapi::serve`'s, `GET /api/v1/policy`
/// `mako_service::einrichtung::router`'s, and `/api/v1/oversight` agentplane's,
/// nested under `plane::oversight::MOUNT` and described by agentplane's own
/// document.
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

fn concrete(template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}').expect("a closed parameter") + open;
        out.push_str("00000000-0000-4000-8000-000000000000");
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

async fn call(base: &str, method: &str, uri: &str) -> (u16, serde_json::Value) {
    let method = reqwest::Method::from_bytes(method.as_bytes()).expect("a method");
    let res = reqwest::Client::new()
        .request(method, format!("{base}{uri}"))
        .send()
        .await
        .expect("the router answers");
    let status = res.status().as_u16();
    let body = res.bytes().await.unwrap_or_default();
    (
        status,
        serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
    )
}

/// Every documented operation reaches a handler. A handler answers with a
/// status other than 404/405 or with an `ApiError` body (its own 404 names the
/// run); an unrouted path or method is axum's bodiless 404/405.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let app = super::router_builds::built_app().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let app = base.as_str();
    let reached = |status: u16, body: &serde_json::Value| {
        !matches!(status, 404 | 405) || body["error"].is_string()
    };
    let (status, body) = call(app, "POST", "/api/v1/kein-solcher-pfad").await;
    assert!(
        !reached(status, &body),
        "an unrouted path answered {status} {body}"
    );

    let (status, served) = call(app, "GET", mako_service::openapi::PATH).await;
    assert_eq!(status, 200, "the document is served");
    let doc = agentd::openapi::document();
    assert_eq!(served, doc, "the served document is the built one");

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
            let (status, body) = call(app, &method, &concrete(template)).await;
            if !reached(status, &body) {
                unserved.push(format!("{method} {template} → {status} {body}"));
            }
        }
    }
    assert_eq!(
        operations, 10,
        "agentd documents ten operations of its own — the document lost or gained routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach a handler: {unserved:#?}"
    );
}
