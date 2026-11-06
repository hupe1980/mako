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
    // The deployment's own MSB, the operator of a Zählerstand an Ablesung files.
    cfg["identity"]
        .as_table_mut()
        .expect("an [identity] table")
        .insert("own_mp_id".to_owned(), "9900987654329".into());
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

fn json_request(
    method: &str,
    uri: &str,
    body: &serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .expect("request")
}

/// An `SmgwSession` the compliance check finds nothing wrong with.
fn smgw_session() -> serde_json::Value {
    serde_json::json!({
        "device_id": "SMGW-2026-001", "firmware_version": "3.1.2",
        "msb_mp_id": "9900000000003", "malo_id": "51238696012", "status": "OPERATIONAL",
        "certificates": [], "cls_channels": [],
        "last_contact_at": null, "installed_at": "2025-06-01",
    })
}

/// `PUT /api/v1/smgw/{malo_id}` reads a typed `SmgwSession`: a key the session
/// does not declare is a `422` naming it, never a field silently read as
/// absent. Read as an untyped `serde_json::Value` the misspelt key would pass
/// through and the session would be stored.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_smgw_session_with_an_unknown_key_is_refused() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let mut body = smgw_session();
    body["firmware_versoin"] = serde_json::json!("3.1.3");
    let (status, problem) = call(
        app.clone(),
        json_request("PUT", "/api/v1/smgw/51238696012", &body),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{problem}"
    );
    assert!(
        problem.to_string().contains("firmware_versoin"),
        "{problem}"
    );
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM smgw_sessions")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(stored, 0, "a refused session is not stored");

    let (status, answer) = call(
        app,
        json_request("PUT", "/api/v1/smgw/51238696012", &smgw_session()),
    )
    .await;
    assert!(status.is_success(), "{status}: {answer}");
    let gateway_status: String = sqlx::query_scalar(
        "SELECT gateway_status FROM smgw_sessions WHERE malo_id = '51238696012'",
    )
    .fetch_one(&pool)
    .await
    .expect("the session is stored");
    assert_eq!(gateway_status, "OPERATIONAL");
}

/// A `COMMUNITY_SHARE` rule whose plant and tenant both have a source.
fn virtual_meter() -> serde_json::Value {
    serde_json::json!({
        "virtual_malo_id": "10001234558",
        "display_name": "GGV MaLo2",
        "sparte": "STROM",
        "legal_basis": "§42b EnWG",
        "rule_json": { "COMMUNITY_SHARE": {
            "plant": "DE0001234567890000000000000000001",
            "tenant": "DE0001234567890000000000000000002",
            "key": { "kind": "EQUAL_SHARES" },
            "sources": {
                "DE0001234567890000000000000000001": "51238696012",
                "DE0001234567890000000000000000002": "51238696012",
            },
        } },
    })
}

/// `POST /api/v1/virtual` reads a typed request: a misspelt top-level key is a
/// `422`, not a `legal_basis` stored as absent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_virtual_meter_with_an_unknown_key_is_refused() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let mut body = virtual_meter();
    let legal_basis = body
        .as_object_mut()
        .expect("an object")
        .remove("legal_basis")
        .expect("legal_basis");
    body["legal_bases"] = legal_basis;
    let (status, problem) = call(app.clone(), json_request("POST", "/api/v1/virtual", &body)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{problem}"
    );
    assert!(problem.to_string().contains("legal_bases"), "{problem}");

    let (status, answer) = call(
        app,
        json_request("POST", "/api/v1/virtual", &virtual_meter()),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED, "{answer}");
    assert_eq!(answer["rule_type"], "COMMUNITY_SHARE", "{answer}");
    let legal_basis: Option<String> = sqlx::query_scalar(
        "SELECT legal_basis FROM virtual_meter_configs WHERE virtual_malo_id = '10001234558'",
    )
    .fetch_one(&pool)
    .await
    .expect("the configuration is stored");
    assert_eq!(legal_basis.as_deref(), Some("§42b EnWG"));
}

/// Insert an open Ablesung for `obis_code` on a meter, returning its id.
async fn open_reading_order(pool: &sqlx::PgPool, obis_code: &str) -> uuid::Uuid {
    sqlx::query_scalar(
        "INSERT INTO ablese_auftraege
             (malo_id, melo_id, tenant, anlass, auftraggeber_rolle, geplant_am, obis_code)
         VALUES ('51238696012', 'DE0001234567890123456789012345678', '9900357000004',
                 'ZWISCHENABLESUNG', 'LF', DATE '2026-07-01', $1)
         RETURNING id",
    )
    .bind(obis_code)
    .fetch_one(pool)
    .await
    .expect("insert the reading order")
}

async fn order_status(pool: &sqlx::PgPool, id: uuid::Uuid) -> String {
    sqlx::query_scalar("SELECT status FROM ablese_auftraege WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("the order is there")
}

/// Completing an Ablesung sets the order `AUSGEFUEHRT` and files its
/// Zählerstand as one write: when the Zählerstand cannot be filed, the order
/// stays open. A completed order whose reading never reached the reading store
/// is invisible to billing and is never re-dispatched, because nothing marks it
/// as owing a value. The fault is an order whose register is not an OBIS code,
/// which the reading store refuses after the order row has been updated.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_reading_that_cannot_be_filed_leaves_the_order_open() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let id = open_reading_order(&pool, "not-an-obis-code").await;

    let (status, body) = call(
        app,
        json_request(
            "PUT",
            &format!("/api/v1/reading-orders/{id}/complete"),
            &serde_json::json!({
                "zaehlerstand_kwh": "12345.6",
                "abgelesen_am": "2026-07-01T08:00:00Z",
            }),
        ),
    )
    .await;
    assert!(!status.is_success(), "{status}: {body}");
    assert_eq!(order_status(&pool, id).await, "OFFEN", "{body}");
}

/// The other half: a Zählerstand that is filed completes the order.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_filed_reading_completes_the_order() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let id = open_reading_order(&pool, "1-0:1.8.0").await;

    let (status, body) = call(
        app,
        json_request(
            "PUT",
            &format!("/api/v1/reading-orders/{id}/complete"),
            &serde_json::json!({
                "zaehlerstand_kwh": "12345.6",
                "abgelesen_am": "2026-07-01T08:00:00Z",
            }),
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");
    assert_eq!(order_status(&pool, id).await, "AUSGEFUEHRT");
}

/// A refused completion changes nothing: a Strom order reported with a volume
/// register is a decode fault, answered 422, and the order stays open for the
/// reading it is still owed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_refused_completion_leaves_the_order_open() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let id = open_reading_order(&pool, "1-0:1.8.0").await;

    let (status, body) = call(
        app,
        json_request(
            "PUT",
            &format!("/api/v1/reading-orders/{id}/complete"),
            &serde_json::json!({ "zaehlerstand_qm3": "812.4" }),
        ),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    assert_eq!(order_status(&pool, id).await, "OFFEN", "{body}");
}
