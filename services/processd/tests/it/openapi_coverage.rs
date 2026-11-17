//! Every `/api/v1` route processd mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter` (`processd::openapi::api`), whose
//! `.routes(routes!(..))` registers a handler and its document entry together —
//! so a route is undocumented only if it bypasses that router with a plain
//! `.route("/api/v1/…")`, which the text half refuses. The runtime half sends
//! each documented operation to the router as `mako_service::run` mounts it and
//! requires it to reach a handler: a document entry whose path or method the
//! router does not serve is a contract an integrator generates a client for and
//! that answers 404.

use tower::ServiceExt as _;

const SOURCES: &[(&str, &str)] = &[
    ("server.rs", include_str!("../../src/server.rs")),
    ("daemon.rs", include_str!("../../src/daemon.rs")),
    ("openapi.rs", include_str!("../../src/openapi.rs")),
    ("mcp_server.rs", include_str!("../../src/mcp_server.rs")),
    ("obs/rest.rs", include_str!("../../src/obs/rest.rs")),
    (
        "obs/counterparty.rs",
        include_str!("../../src/obs/counterparty.rs"),
    ),
    ("sperr/mod.rs", include_str!("../../src/sperr/mod.rs")),
];

/// The production text of `src`: everything before its first `#[cfg(test)]`,
/// where the stand-in upstreams the unit tests serve live.
fn production(src: &str) -> &str {
    &src[..src.find("#[cfg(test)]").unwrap_or(src.len())]
}

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document. `GET /api/v1/openapi.json` itself is
/// `mako_service::openapi::serve`'s, and `GET /api/v1/policy`
/// `mako_service::einrichtung::router`'s.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let mut bypassing = Vec::new();
    for (name, src) in SOURCES {
        let mut rest = production(src);
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let arg = rest.trim_start();
            if arg.starts_with("\"/api/v1") {
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
        "mp_id" => "9900357000004",
        "rolle" => "NB",
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

/// A `marktd` that accepts the self-registration `build` makes.
async fn marktd() -> String {
    let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::OK });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// Every documented operation reaches a handler: an unverifiable token is
/// refused by the `Claims` extractor, which runs only inside a routed handler,
/// with an `ApiError` body.
///
/// The status alone does not tell: the MCP router's auth layer also wraps the
/// merged router's fallback, so an unrouted path with a bearer token is a 401
/// too — but a plain-text one. The `ApiError` body is the handler's mark; an
/// unrouted method is a 405.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../deploy/demo/processd.toml"))
            .expect("the demo configuration parses");
    cfg["marktd"]["url"] = toml::Value::String(marktd().await);
    let app = mako_service::build_for_test::<processd::Processd>(
        cfg.try_into()
            .expect("the demo configuration is a processd one"),
    )
    .await
    .expect("the router builds");

    let probe = |method: &str, uri: &str| {
        axum::http::Request::builder()
            .method(method)
            .uri(uri)
            .header("authorization", "Bearer not.a.token")
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let reached = |status: axum::http::StatusCode, body: &[u8]| {
        status != axum::http::StatusCode::METHOD_NOT_ALLOWED
            && serde_json::from_slice::<serde_json::Value>(body)
                .is_ok_and(|b| b["error"].is_string())
    };
    // The probe distinguishes: an unrouted path does not reach a handler.
    let res = app
        .clone()
        .oneshot(probe("GET", "/api/v1/kein-solcher-pfad"))
        .await
        .unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(
        !reached(status, &body),
        "an unrouted path answered {status}"
    );

    let doc = serde_json::to_value(processd::openapi::document()).expect("serializes");
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
            let res = app
                .clone()
                .oneshot(probe(&method, &concrete(template)))
                .await
                .unwrap();
            let status = res.status();
            let body = axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap();
            if !reached(status, &body) {
                unserved.push(format!("{method} {template} → {status}"));
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
/// processd's document, so an integrator finds them beside processd's own.
#[tokio::test]
async fn the_webhook_routes_are_documented() {
    let doc = serde_json::to_value(processd::openapi::document()).expect("serializes");
    for path in ["/api/v1/webhooks/endpoints", "/api/v1/events"] {
        assert!(doc["paths"].get(path).is_some(), "{path} is not documented");
    }
}
