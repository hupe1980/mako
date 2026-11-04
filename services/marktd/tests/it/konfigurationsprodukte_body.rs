//! `PUT /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` reads a
//! typed body, `{"konfigurationsprodukte": [...]}`.
//!
//! An untyped `serde_json::Value` body has no fields, so `mako_service::Json`'s
//! unknown-key refusal has nothing to refuse: a misspelt key is read as absent.
//! These tests fail if the handler goes back to one.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing is a deployment concern"
)]

use mako_markt::repository::SteuerbareRessourceRepository as _;
use marktd::daemon::Marktd;
use marktd::pg::PgSteuerbareRessourceRepository;
use tower::ServiceExt as _;

use crate::router_builds::load;

const SR: &str = "C1234567890";
/// The dev configuration's tenant — the one the router writes under.
const TENANT: &str = "9900357000004";
const URI: &str = "/api/v1/steuerbare-ressourcen/C1234567890/konfigurationsprodukte";

async fn put(app: axum::Router, body: &serde_json::Value) -> (u16, serde_json::Value) {
    let request = axum::http::Request::put(URI)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, body)
}

async fn lazy_app() -> axum::Router {
    mako_service::build_for_test::<Marktd>(load("../../dev/config/marktd.toml"))
        .await
        .expect("the router builds")
}

/// A misspelt top-level key is a `422` naming it — refused before the handler
/// reaches the (unreachable) database.
#[tokio::test]
async fn an_unknown_top_level_key_is_refused() {
    let body = serde_json::json!({
        "konfigurationsprodukte": [],
        "konfigurationsprodukt": [{ "produktcode": "FLEX-PRODUCT-001" }],
    });
    let (status, problem) = put(lazy_app().await, &body).await;
    assert_eq!(status, 422, "{problem}");
    assert!(
        problem.to_string().contains("konfigurationsprodukt\""),
        "the problem names the key: {problem}"
    );
}

/// The list is mandatory: an object without it is refused, never read as `[]`
/// — which would clear every contracted product.
#[tokio::test]
async fn a_missing_list_is_refused_not_read_as_empty() {
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "konfigurationsprodukt": [] }),
    ] {
        let (status, problem) = put(lazy_app().await, &body).await;
        assert_eq!(status, 422, "{body} → {problem}");
    }
}

/// A key BO4E does not define inside an element is a `422` naming its path —
/// the gate alone would keep it under `_additional` and store it, with the
/// field it was meant to fill absent.
#[tokio::test]
async fn an_undefined_key_inside_an_element_is_refused() {
    let body = serde_json::json!({
        "konfigurationsprodukte": [
            { "produktcode": "FLEX-PRODUCT-001", "leistungskurvendefinitio": "LK-1" },
        ],
    });
    let (status, problem) = put(lazy_app().await, &body).await;
    assert_eq!(status, 422, "{problem}");
    assert_eq!(problem["code"], "bo4e.unknown_field", "{problem}");
    assert!(
        problem.to_string().contains("leistungskurvendefinitio"),
        "the problem names the path: {problem}"
    );
}

/// The body is an object naming its products; a bare array is refused.
#[tokio::test]
async fn a_bare_array_is_refused() {
    let body = serde_json::json!([{ "produktcode": "FLEX-PRODUCT-001" }]);
    let (status, problem) = put(lazy_app().await, &body).await;
    assert_eq!(status, 422, "{problem}");
}

/// A valid body replaces the stored list; an element without `produktcode` is
/// refused and leaves it untouched.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_valid_body_replaces_the_list() {
    use testcontainers::ImageExt as _;
    use testcontainers::runners::AsyncRunner as _;
    use testcontainers_modules::postgres::Postgres;

    let Ok(pg) = Postgres::default().with_tag("17-alpine").start().await else {
        return;
    };
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let mut cfg = load("../../dev/config/marktd.toml");
    cfg.database.url = url;
    let app = mako_service::build_for_test_on::<Marktd>(cfg, pool.clone())
        .await
        .expect("the router builds");

    let repo = PgSteuerbareRessourceRepository::new(pool.clone());
    repo.upsert_sr(
        SR,
        TENANT,
        None,
        None,
        serde_json::json!({ "_typ": "STEUERBARERESSOURCE" }),
        "v202607.0.0",
        None,
    )
    .await
    .expect("seed the SR");

    let (status, problem) = put(
        app.clone(),
        &serde_json::json!({ "konfigurationsprodukte": [{ "produktcode": "" }] }),
    )
    .await;
    assert_eq!(status, 422, "{problem}");

    let (status, answer) = put(
        app,
        &serde_json::json!({
            "konfigurationsprodukte": [{ "produktcode": "FLEX-PRODUCT-001" }],
        }),
    )
    .await;
    assert_eq!(status, 204, "{answer}");

    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT konfigurationsprodukte FROM steuerbare_ressourcen WHERE sr_id = $1 AND tenant = $2",
    )
    .bind(SR)
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .expect("the SR row");
    assert_eq!(stored[0]["produktcode"], "FLEX-PRODUCT-001", "{stored}");
    assert_eq!(stored.as_array().map(Vec::len), Some(1), "{stored}");
}
