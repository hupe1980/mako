//! makod's `/api/v1` routes are its OpenAPI document: each is mounted through
//! `OpenApiRouter` with its entry (`openapi::api_router`), the committed
//! `services/makod/openapi.json` is that document, and every documented
//! `/api/v1` operation reaches a handler.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use mako_engine::{ids::TenantId, store_slatedb::SlateDbStore};
use makod::commands_api::CommandsApiState;
use tower::ServiceExt as _;

const TENANT: &str = "9900357000004";

async fn api() -> (axum::Router, utoipa::openapi::OpenApi) {
    let store = Arc::new(
        SlateDbStore::open_in_memory()
            .await
            .expect("in-memory store"),
    );
    let cedar =
        Arc::new(makod::cedar_authz::CedarAuthorizer::unauthenticated().expect("infallible"));
    let tenant_id = TenantId::from_tenant(TENANT);
    let malo_cache = Arc::new(makod::malo_cache::SlateDbMaloCache::new(
        (*store).clone(),
        tenant_id,
    ));
    let commands = Arc::new(CommandsApiState {
        tenant_id,
        tenant: "stadtwerke-test".to_owned(),
        parties: crate::support::single_party(TENANT),
        configured_marktrollen: vec!["LF".to_owned()],
        max_body_bytes: 1_048_576,
        snapshot_interval: 100,
        cedar: Arc::clone(&cedar),
        snapshot_store: store.as_snapshot_store(),
        malo_cache,
        maloid_result_cache: makod::malo_cache::MaloIdentResultCache::new((*store).clone()),
        store: Arc::clone(&store),
        marktd_client: None,
    });
    let invoic = Arc::new(makod::invoic_api::InvoicApiState {
        store: Arc::clone(&store),
        tenant_id,
        cedar: Arc::clone(&cedar),
        tenant: TENANT.to_owned(),
    });
    let einrichtung = Arc::new(makod::einrichtung_api::EinrichtungState {
        einrichtung: Arc::new(mako_service::einrichtung::Einrichtung {
            dienst: "makod".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            bereit: true,
            befunde: Vec::new(),
            policy: mako_service::policy::PolicyReport::default(),
            parteien: Vec::new(),
        }),
        dienste: Vec::new(),
        http: mako_service::http::default_client(),
        cedar: Arc::clone(&cedar),
        tenant: TENANT.to_owned(),
        zertifikate: None,
    });
    let format_impact = Arc::new(makod::api::format_impact_api::FormatImpactApiState {
        impact: None,
        cedar,
        tenant: TENANT.to_owned(),
    });
    makod::openapi::api_router(commands, invoic, einrichtung, format_impact).split_for_parts()
}

/// `services/makod/openapi.json` is the document makod serves
/// (`BLESS_MCP_SNAPSHOT=1` rewrites it).
#[tokio::test]
async fn the_committed_document_is_the_served_one() {
    let (_, doc) = api().await;
    mako_service::openapi::assert_committed(
        &doc,
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json"),
    );
}

/// Every documented `/api/v1` operation is answered by its handler, not by
/// the router's fallback: an unrouted request is a plain `404` with no
/// problem body, and a handler's own refusal always carries one.
#[tokio::test]
async fn every_documented_operation_is_served() {
    let (router, doc) = api().await;
    let value = serde_json::to_value(&doc).expect("serializes");
    let mut served = 0;
    for (path, ops) in value["paths"].as_object().expect("paths") {
        if !path.starts_with("/api/v1/") {
            continue;
        }
        for method in ops.as_object().expect("operations").keys() {
            let Ok(method) = method.to_uppercase().parse::<Method>() else {
                continue;
            };
            let uri = path
                .replace("{process_id}", "00000000-0000-4000-8000-000000000000")
                .replace("{command}", "gpke.lieferbeginn.anmelden")
                .replace("{fv}", "fv20270401");
            let resp = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(method.clone())
                        .uri(&uri)
                        .header("content-type", "application/json")
                        .body(Body::from("{}"))
                        .expect("request"),
                )
                .await
                .expect("answered");
            let status = resp.status();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .expect("body");
            assert!(
                status != StatusCode::NOT_FOUND || !bytes.is_empty(),
                "{method} {path} fell through to the router's fallback"
            );
            assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{method} {path}");
            served += 1;
        }
    }
    assert!(served >= 5, "only {served} /api/v1 operations documented");
}

/// No `/api/v1` route is mounted beside the documented router.
#[test]
fn no_api_route_bypasses_the_openapi_router() {
    let mut files = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).expect("readable").flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs")
                // `*_tests.rs` is a `#[cfg(test)]` module of its parent.
                && !p.to_string_lossy().ends_with("tests.rs")
            {
                out.push(p);
            }
        }
    }
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    for f in files {
        let src = std::fs::read_to_string(&f).expect("readable");
        let code = src.split("#[cfg(test)]").next().unwrap_or(&src);
        let mut rest = code;
        while let Some(at) = rest.find(".route(") {
            rest = &rest[at + ".route(".len()..];
            assert!(
                !rest.trim_start().starts_with("\"/api/v1"),
                "{} mounts a bare /api/v1 route",
                f.display()
            );
        }
    }
}

/// Without `[format_impact]` the documented route is there and refuses with
/// its code, rather than answering an empty report.
#[tokio::test]
async fn an_unconfigured_format_impact_is_refused_by_name() {
    let (router, _) = api().await;
    let resp = router
        .oneshot(
            Request::get("/api/v1/format-candidates/fv20270401/impact")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("answered");
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("a problem body");
    assert_eq!(body["code"], "format_impact_not_configured", "{body}");
}
