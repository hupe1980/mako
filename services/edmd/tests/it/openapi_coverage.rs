//! Every `/api/v1` route edmd mounts is in its OpenAPI document.
//!
//! Two halves, because neither alone is the claim. The routes mount through
//! utoipa-axum's `OpenApiRouter` (`edmd::api::routes`), whose
//! `.routes(routes!(..))` registers a handler and its document entry together —
//! so a route is undocumented only if it bypasses that router with a plain
//! `.route("/api/v1/…")`, which the text half refuses. The runtime half sends
//! each documented operation to the router as the demo starts it and requires
//! it to reach a handler: a document entry whose path or method the router
//! does not serve is a contract an integrator generates a client for and that
//! answers 404.

use super::router_builds::{app, call};

/// The production text of every source file under `src/`, its `#[cfg(test)]`
/// module cut off.
fn production_sources() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("src is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).expect("a source file");
                let production = src
                    .find("#[cfg(test)]\nmod ")
                    .map_or(src.as_str(), |at| &src[..at])
                    .to_owned();
                out.push((path.display().to_string(), production));
            }
        }
    }
    let mut out = Vec::new();
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut out,
    );
    out
}

/// No `/api/v1` route is mounted with a plain `.route(…)`, so none can be
/// missing from the document. `GET /api/v1/openapi.json` itself is
/// `mako_service::openapi::serve`'s, `GET /api/v1/policy`
/// `mako_service::einrichtung::router`'s, and `/api/v1/iceberg` meterstore's
/// nested catalog facade.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let sources = production_sources();
    assert!(
        sources.len() > 20,
        "only {} source files found",
        sources.len()
    );
    let mut bypassing = Vec::new();
    for (name, src) in &sources {
        let mut rest = src.as_str();
        while let Some(i) = rest.find(".route(") {
            rest = &rest[i + ".route(".len()..];
            let Some(lit) = rest.trim_start().strip_prefix('"') else {
                continue;
            };
            let path = &lit[..lit.find('"').unwrap_or(lit.len())];
            if path.starts_with("/api/v1") {
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
        "malo_id" | "virtual_malo_id" => "51238696012",
        "year" => "2026",
        "month" => "6",
        "community_id" => "X",
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
    let Some((app, _pool, _pg)) = app().await else {
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

    let doc = serde_json::to_value(edmd::api::document()).expect("the document serializes");
    let paths = doc["paths"].as_object().expect("a paths object");
    let mut operations = 0usize;
    let mut unserved = Vec::new();
    for (template, item) in paths {
        // Served by `mako_service::openapi::serve` without authentication.
        if template == "/api/v1/openapi.json" {
            continue;
        }
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
        operations > 60,
        "only {operations} documented operations — the document lost its routes"
    );
    assert!(
        unserved.is_empty(),
        "these documented operations do not reach an authenticating handler: {unserved:#?}"
    );
}
