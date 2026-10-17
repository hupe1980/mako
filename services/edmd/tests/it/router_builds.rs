//! The router builds the way `mako_service::run` mounts it, and answers the
//! way its contract says.
//!
//! `build` runs with the shipped demo configuration and the runner's infra
//! routes are merged in exactly as at boot — so a route axum refuses, a merge
//! collision with the runner's `/metrics` or `/health/*`, or a `build` that
//! panics fails a test instead of the container start. This daemon's `build`
//! connects a store of its own from `[database] url`, so it starts against a
//! real PostgreSQL (`just test-edmd-db`).

use testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt as _;

/// The router over a throwaway PostgreSQL, with a pool on the same database
/// and the container that must outlive both. `None` without Docker.
#[expect(
    clippy::disallowed_methods,
    reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
)]
async fn app() -> Option<(axum::Router, sqlx::PgPool, ContainerAsync<Postgres>)> {
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres?sslmode=disable");
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");

    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../demos/eeg-billing/edmd.toml"))
            .expect("the demo configuration parses");
    cfg["database"]["url"] = toml::Value::String(url);
    let cfg = cfg
        .try_into()
        .expect("the demo configuration is an edmd one");
    let app = mako_service::build_for_test_on::<edmd::Edmd>(cfg, pool.clone())
        .await
        .expect("the router builds");
    Some((app, pool, pg))
}

async fn call(
    app: axum::Router,
    request: axum::http::Request<axum::body::Body>,
) -> (axum::http::StatusCode, serde_json::Value) {
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(&bytes).into()));
    (status, body)
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_router_builds_as_the_runner_mounts_it() {
    let Some((_app, _pool, _pg)) = app().await else {
        return;
    };
}

/// A database failure is this service's fault: the problem shape with an
/// `error_id` the log line carries, and none of the database's own text —
/// a relation name in a response body is a map of the schema.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_database_failure_answers_an_error_id_and_echoes_nothing() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    sqlx::query("DROP TABLE ablese_auftraege CASCADE")
        .execute(&pool)
        .await
        .expect("drop the table the handler reads");

    let (status, body) = call(
        app,
        axum::http::Request::get("/api/v1/reading-orders")
            .body(axum::body::Body::empty())
            .expect("request"),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "{body}"
    );
    assert!(body["error_id"].is_string(), "{body}");
    assert!(!body.to_string().contains("ablese_auftraege"), "{body}");
}

/// `POST /api/v1/esa/typ2/{malo_id}` accepts a push carrying fields edmd does
/// not model — the route takes `Lenient`, because a BSI TR-03109 gateway on a
/// newer profile may send keys a strict body would refuse, and refusing them
/// drops valid Kapitel-4.6.2 values.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_smgw_typ2_push_with_an_unknown_field_is_stored() {
    let Some((app, _pool, _pg)) = app().await else {
        return;
    };
    let body = serde_json::json!({
        "bestellung_ref": "ORD-17007-1",
        "msb_mp_id": "9900987654329",
        "melo_id": "DE0001234567890123456789012345678",
        "pruefsumme": "a1b2",
        "werte": [{
            "dtm_from": "2026-07-01T00:00:00Z",
            "dtm_to": "2026-07-01T00:15:00Z",
            "quantity_kwh": "1.25",
            "obis_code": "1-0:1.8.0",
            "zaehlerstand_signatur": "MEUCIQ",
        }],
    });
    let (status, body) = call(
        app,
        axum::http::Request::post("/api/v1/esa/typ2/51238696012")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{body}");
}
