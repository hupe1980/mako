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
async fn app() -> Option<(axum::Router, sqlx::PgPool, ContainerAsync<Postgres>)> {
    let (cfg, pool, pg) = database().await?;
    let app = mako_service::build_for_test_on::<edmd::Edmd>(edmd_config(cfg), pool.clone())
        .await
        .expect("the router builds");
    Some((app, pool, pg))
}

/// The shipped demo configuration over a throwaway PostgreSQL, the pool on it
/// and the container. `None` without Docker.
#[expect(
    clippy::disallowed_methods,
    reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
)]
async fn database() -> Option<(toml::Value, sqlx::PgPool, ContainerAsync<Postgres>)> {
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
    cfg["marktd"]["url"] = toml::Value::String(fake_marktd().await);
    // The deployment's own MSB, the operator of a Zählerstand an Ablesung files.
    cfg["identity"]
        .as_table_mut()
        .expect("an [identity] table")
        .insert("own_mp_id".to_owned(), "9900987654329".into());
    // The secret `marktd` signs its fan-out with, so the suite can post to
    // `/webhook` the way `marktd` does.
    cfg["webhook"]
        .as_table_mut()
        .expect("a [webhook] table")
        .insert("inbound_secret".to_owned(), WEBHOOK_SECRET.into());
    Some((cfg, pool, pg))
}

fn edmd_config(cfg: toml::Value) -> edmd::config::Config {
    cfg.try_into()
        .expect("the demo configuration is an edmd one")
}

/// edmd restarts against its own database on a configuration without
/// `[archive]`. Its Iceberg catalog lives in that database and outlives the
/// process, while the in-memory warehouse the catalog's tables point into does
/// not — so a catalog entry left from the last run names files that are gone,
/// and the second start must not trip over it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn edmd_restarts_against_its_own_database() {
    let Some((cfg, pool, _pg)) = database().await else {
        return;
    };
    let first =
        mako_service::build_for_test_on::<edmd::Edmd>(edmd_config(cfg.clone()), pool.clone())
            .await
            .expect("the first start builds");
    drop(first);
    let _restarted = mako_service::build_for_test_on::<edmd::Edmd>(edmd_config(cfg), pool)
        .await
        .expect("a restart against the same database builds");
}

/// The Messlokation the suite's `marktd` places under `51238696012`.
const MELO: &str = "DE0001234567890123456789012345678";

/// A `marktd` holding one Lokationszuordnung — [`MELO`] under the MaLo
/// `51238696012` — and the address of that MaLo; it accepts edmd's
/// subscription. Returns its base URL.
async fn fake_marktd() -> String {
    use axum::routing::{get, put};
    let edge = || {
        serde_json::json!([{
            "id": "00000000-0000-4000-8000-000000000001",
            "tenant": "9900357000004",
            "von_id": "51238696012", "von_typ": "MALO",
            "nach_id": MELO, "nach_typ": "MELO",
            "valid_from": null, "valid_to": null, "data": {}, "depth": 0,
        }])
    };
    let app = axum::Router::new()
        .route(
            "/api/v1/melos/{id}/lokationen",
            get(
                move |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    axum::Json(if id == MELO {
                        edge()
                    } else {
                        serde_json::json!([])
                    })
                },
            ),
        )
        .route(
            "/api/v1/malos/{id}/lokationen",
            get(
                move |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    axum::Json(if id == "51238696012" {
                        edge()
                    } else {
                        serde_json::json!([])
                    })
                },
            ),
        )
        .route(
            "/api/v1/malos/{id}/bundesland",
            get(
                |axum::extract::Path(id): axum::extract::Path<String>| async move {
                    use axum::response::IntoResponse as _;
                    if id == "51238696012" {
                        axum::Json(serde_json::json!({
                        "malo_id": id, "bundesland": "NW", "postleitzahl": "50667",
                        "quelle": "GeoNames postal codes DE (https://www.geonames.org), CC BY 4.0",
                    }))
                    .into_response()
                    } else {
                        (
                            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
                            "PLZ 14715 lies in BB, ST — no single Bundesland",
                        )
                            .into_response()
                    }
                },
            ),
        )
        .route(
            "/api/v1/subscriptions/{id}",
            put(|| async { axum::http::StatusCode::OK }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the fake marktd");
    let addr = listener.local_addr().expect("address");
    tokio::spawn(async move {
        let _served = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

/// The `[webhook] inbound_secret` the suite configures.
const WEBHOOK_SECRET: &str = "edmd-router-test-webhook-secret";

/// A `de.mako.process.completed` for an MSCONS delivery, signed as `marktd`
/// signs its fan-out.
fn process_completed(pid: u32, data: &serde_json::Value) -> axum::http::Request<axum::body::Body> {
    signed_event("de.mako.process.completed", pid, data)
}

/// A CloudEvent of `ce_type`, signed as `marktd` signs its fan-out.
fn signed_event(
    ce_type: &str,
    pid: u32,
    data: &serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    let event = serde_json::json!({
        "specversion": "1.0",
        "type": ce_type,
        "source": "/makod/test",
        "id": uuid::Uuid::new_v4().to_string(),
        "subject": uuid::Uuid::new_v4().to_string(),
        "time": "2026-07-02T08:00:00Z",
        "makopid": pid,
        "data": data,
    });
    let body = event.to_string();
    let id = uuid::Uuid::new_v4().to_string();
    let ts = time::OffsetDateTime::now_utc().unix_timestamp();
    let mut request =
        axum::http::Request::post("/webhook").header("content-type", "application/json");
    for (name, value) in
        mako_service::webhook::headers(WEBHOOK_SECRET.as_bytes(), &id, ts, body.as_bytes())
    {
        request = request.header(name, value);
    }
    request.body(axum::body::Body::from(body)).expect("request")
}

/// One quarter-hour of a Messwesen MSCONS payload, as `makod` builds it.
fn mscons_read(from: &str, to: &str, kwh: &str) -> serde_json::Value {
    serde_json::json!({
        "dtm_from": from, "dtm_to": to, "quantity_kwh": kwh,
        "quality": "MEASURED", "obis_code": "1-0:1.29.0",
    })
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
             (malo_id, melo_id, tenant, anlass, auftraggeber_rolle, geplant_am, obis_code, sparte)
         VALUES ('51238696012', 'DE0001234567890123456789012345678', '9900357000004',
                 'ZWISCHENABLESUNG', 'LF', DATE '2026-07-01', $1, 'STROM')
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

/// A reading order names its Sparte or is refused: the Sparte decides the unit
/// the Zählerstand is reported in, and an order filed as electricity for a gas
/// meter files its reading in the wrong dimension.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_reading_order_without_a_sparte_is_refused() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let order = serde_json::json!({
        "malo_id": "51238696012",
        "anlass": "ZWISCHENABLESUNG",
        "auftraggeber_rolle": "LF",
        "geplant_am": "2026-07-01",
    });
    let (status, problem) = call(app, json_request("POST", "/api/v1/reading-orders", &order)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{problem}"
    );
    assert!(problem.to_string().contains("sparte"), "{problem}");
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM ablese_auftraege")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(stored, 0, "a refused order is not stored");
}

/// The schema holds the same rule: a row without a Sparte cannot be written,
/// so a read-back never has one to default.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_stored_reading_order_cannot_lack_a_sparte() {
    let Some((_app, pool, _pg)) = app().await else {
        return;
    };
    let inserted = sqlx::query(
        "INSERT INTO ablese_auftraege
             (malo_id, tenant, anlass, auftraggeber_rolle, geplant_am)
         VALUES ('51238696012', '9900357000004', 'ZWISCHENABLESUNG', 'LF', DATE '2026-07-01')",
    )
    .execute(&pool)
    .await;
    assert!(inserted.is_err(), "an order without a Sparte was stored");
}

/// An MSCONS delivery that names no Sparte is refused, not stored as Strom:
/// the Sparte decides the stored unit, and a redelivery of the same payload
/// cannot succeed, so the answer is a `422`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_mscons_delivery_without_a_sparte_is_refused() {
    let Some((app, _pool, _pg)) = app().await else {
        return;
    };
    let data = serde_json::json!({
        "pid": 13025,
        "malo_id": "51238696012",
        "sender": "9900987654329",
        "message_ref": "MSG-1",
        "reads": [mscons_read("2026-07-01T00:00:00Z", "2026-07-01T00:15:00Z", "1.5")],
    });
    let (status, body) = call(app, process_completed(13025, &data)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
}

/// The billing period of a MaLo, as `billingd` reads it.
async fn billing_day(app: axum::Router, malo: &str) -> (axum::http::StatusCode, serde_json::Value) {
    call(
        app,
        axum::http::Request::get(format!(
            "/api/v1/billing-period/{malo}?from=2026-07-01&to=2026-07-01&sparte=STROM"
        ))
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await
}

/// The origin a delivery states (`STS+Z33`/`Z83`, carried by `makod` as
/// `herkunft`) is stored beside the quality flag and served on the billing
/// period; a delivery that states none serves none.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_self_read_reaches_the_billing_period_as_its_origin() {
    let Some((app, _pool, _pg)) = app().await else {
        return;
    };
    let delivery = |malo: &str, herkunft: Option<&str>| {
        let mut read = mscons_read("2026-06-30T22:00:00Z", "2026-06-30T22:15:00Z", "1.5");
        if let Some(h) = herkunft {
            read["herkunft"] = h.into();
        }
        serde_json::json!({
            "pid": 13025, "malo_id": malo, "sender": "9900987654329",
            "message_ref": "MSG-1", "sparte": "STROM", "reads": [read],
        })
    };
    let (status, body) = call(
        app.clone(),
        process_completed(
            13025,
            &delivery("51238696012", Some("KUNDENSELBSTABLESUNG")),
        ),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");
    let (status, body) = call(
        app.clone(),
        process_completed(13025, &delivery("51238696781", None)),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");

    let (status, period) = billing_day(app.clone(), "51238696012").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{period}");
    assert_eq!(
        period["ablesung_herkunft"], "KUNDENSELBSTABLESUNG",
        "{period}"
    );
    // The cached row answers the same.
    let (_, cached) = billing_day(app.clone(), "51238696012").await;
    assert_eq!(
        cached["ablesung_herkunft"], "KUNDENSELBSTABLESUNG",
        "{cached}"
    );

    let (status, period) = billing_day(app, "51238696781").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{period}");
    assert!(period["ablesung_herkunft"].is_null(), "{period}");
}

/// Two Lastgang deliveries for one interval resolve by the version `makod`
/// reads off their `DTM+137` Nachrichtendatum, whatever order they arrive in:
/// the correction delivered first still outranks the original replayed after
/// it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_outranks_an_original_that_arrives_after_it() {
    let Some((app, _pool, _pg)) = app().await else {
        return;
    };
    let delivery = |kwh: &str, version: &str| {
        serde_json::json!({
            "pid": 13025, "malo_id": "51238696012", "sender": "9900987654329",
            "message_ref": "MSG-1", "sparte": "STROM", "mscons_version": version,
            "reads": [mscons_read("2026-06-30T22:00:00Z", "2026-06-30T22:15:00Z", kwh)],
        })
    };
    for (kwh, version) in [("2.0", "20260705080000"), ("1.0", "20260702080000")] {
        let (status, body) = call(
            app.clone(),
            process_completed(13025, &delivery(kwh, version)),
        )
        .await;
        assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");
    }
    let (status, period) = billing_day(app, "51238696012").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{period}");
    assert_eq!(
        period["arbeitsmenge_kwh"]
            .as_str()
            .and_then(|s| s.parse::<rust_decimal::Decimal>().ok()),
        Some(rust_decimal::Decimal::from(2)),
        "{period}"
    );
}

/// A settlement read without `?sparte` runs on the Sparte the MaLo's readings
/// are stored under, not on Strom: a gas period is a run of Gastage, and an
/// hour on 2 July before 06:00 belongs to the Gastag of 1 July.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_billing_period_without_a_sparte_runs_on_the_stored_one() {
    let Some((app, _pool, _pg)) = app().await else {
        return;
    };
    let delivery = serde_json::json!({
        "pid": 13008, "malo_id": "51238696012", "sender": "9900987654329",
        "message_ref": "MSG-1", "sparte": "GAS",
        "reads": [{
            "dtm_from": "2026-07-02T02:00:00Z", "dtm_to": "2026-07-02T03:00:00Z",
            "quantity_kwh": "10.0", "quality": "MEASURED", "obis_code": "7-20:99.33.17",
        }],
    });
    let (status, body) = call(app.clone(), process_completed(13008, &delivery)).await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");

    let (status, period) = call(
        app.clone(),
        axum::http::Request::get(
            "/api/v1/billing-period/51238696012?from=2026-07-01&to=2026-07-01",
        )
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{period}");
    assert_eq!(period["sparte"], "GAS", "{period}");

    // A MaLo with no stored reading names no Sparte to run on.
    let (status, problem) = call(
        app,
        axum::http::Request::get(
            "/api/v1/billing-period/51238696781?from=2026-07-01&to=2026-07-01",
        )
        .body(axum::body::Body::empty())
        .expect("request"),
    )
    .await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{problem}"
    );
}

/// A delivery edmd refuses leaves no receipt: the receipt says the delivery
/// was taken, and a refused one was not.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_refused_mscons_delivery_leaves_no_receipt() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let mut read = mscons_read("2026-06-30T22:00:00Z", "2026-06-30T22:15:00Z", "1.5");
    read["herkunft"] = "NACHBARSCHAFTSABLESUNG".into();
    for data in [
        serde_json::json!({
            "pid": 13025, "malo_id": "51238696012", "sender": "9900987654329",
            "message_ref": "MSG-1", "sparte": "STROM", "reads": [read],
        }),
        serde_json::json!({
            "pid": 13025, "malo_id": "51238696012", "sender": "9900987654329",
            "message_ref": "MSG-2",
            "reads": [mscons_read("2026-06-30T22:00:00Z", "2026-06-30T22:15:00Z", "1.5")],
        }),
    ] {
        let (status, body) = call(app.clone(), process_completed(13025, &data)).await;
        assert_eq!(
            status,
            axum::http::StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
    }
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM meter_data_receipts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(receipts, 0, "a refused delivery is not receipted");
}

/// An INSRPT orders no reading. The Störungsbehebung in der Messlokation (WiM
/// Teil 2 Kap. 1) leaves the Zählerstand where it was — „Der von der
/// Messeinrichtung abgelesene Zählerstand wird nicht korrigiert." — and
/// transmits it through the Aufbereitung und Übermittlung von Werten, so a
/// process initiation is not a reading order.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_insrpt_orders_no_reading() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let data = serde_json::json!({
        "malo_id": "51238696012", "sparte": "STROM",
        "melo_id": "DE0001234567890123456789012345678",
    });
    let (status, body) = call(app, signed_event("de.mako.process.initiated", 23005, &data)).await;
    assert!(status.is_success(), "{status}: {body}");
    let orders: i64 = sqlx::query_scalar("SELECT count(*) FROM ablese_auftraege")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(orders, 0);
}

/// A Zählerstand the customer read (`STS+Z33`/`Z83`, carried as `herkunft`)
/// is filed as a register reading under the Marktlokation `marktd` places its
/// Messlokation in, and when both Zählerstände a billing period states are
/// self-reads the period's Ablesungsart is the customer's — whatever its
/// interval values state.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_self_read_zaehlerstand_reaches_the_billing_period() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let lastgang = serde_json::json!({
        "pid": 13025, "malo_id": "51238696012", "sender": "9900987654329",
        "message_ref": "MSG-1", "sparte": "STROM",
        "reads": [mscons_read("2026-06-30T22:00:00Z", "2026-06-30T22:15:00Z", "1.5")],
    });
    let (status, body) = call(app.clone(), process_completed(13025, &lastgang)).await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");

    let stand = |at: &str, value: &str| {
        serde_json::json!({
            "at": at, "value": value, "quality": "MEASURED",
            "obis_code": "1-0:1.8.0", "melo_id": MELO, "herkunft": "KUNDENSELBSTABLESUNG",
        })
    };
    let zaehlerstaende = serde_json::json!({
        "pid": 13017, "malo_id": MELO, "sender": "9900987654329",
        "message_ref": "MSG-2", "sparte": "STROM",
        "zaehlerstaende": [
            stand("2026-06-30T22:00:00Z", "14230.5"),
            stand("2026-07-01T22:00:00Z", "14236.0"),
        ],
    });
    let (status, body) = call(app.clone(), process_completed(13017, &zaehlerstaende)).await;
    assert_eq!(status, axum::http::StatusCode::NO_CONTENT, "{body}");
    let receipt: String =
        sqlx::query_scalar("SELECT malo_id FROM meter_data_receipts WHERE pid = 13017")
            .fetch_one(&pool)
            .await
            .expect("the Zählerstand delivery is receipted");
    assert_eq!(receipt, "51238696012", "filed under the Marktlokation");

    let (status, period) = billing_day(app, "51238696012").await;
    assert_eq!(status, axum::http::StatusCode::OK, "{period}");
    assert_eq!(
        period["ablesung_herkunft"], "KUNDENSELBSTABLESUNG",
        "{period}"
    );
    assert!(
        period["zaehlerstand_ende"].is_string() || period["zaehlerstand_ende"].is_number(),
        "{period}"
    );
}

/// A Messlokation `marktd` places under no Marktlokation cannot be filed and
/// is refused by name, before anything is written.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_zaehlerstand_of_an_unplaced_messlokation_is_refused() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let data = serde_json::json!({
        "pid": 13017, "malo_id": "DE0009999999999999999999999999999", "sender": "9900987654329",
        "message_ref": "MSG-3", "sparte": "STROM",
        "zaehlerstaende": [{
            "at": "2026-06-30T22:00:00Z", "value": "1", "quality": "MEASURED",
            "obis_code": "1-0:1.8.0",
        }],
    });
    let (status, body) = call(app, process_completed(13017, &data)).await;
    assert_eq!(
        status,
        axum::http::StatusCode::UNPROCESSABLE_ENTITY,
        "{body}"
    );
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM meter_data_receipts")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(receipts, 0);
}

/// `marktd` announcing a MaLo refreshes the Land edmd keeps for it: a Land
/// is cached, and a refusal to name one removes the cached row, so a MaLo that
/// moved into a border PLZ does not keep the Land it left.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_malo_update_refreshes_its_cached_bundesland() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let cached = |malo: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT bundesland FROM malo_bundesland WHERE malo_id = $1",
            )
            .bind(malo)
            .fetch_optional(&pool)
            .await
            .expect("read the cache")
        }
    };
    let updated = |malo: &str| {
        let body = serde_json::json!({
            "specversion": "1.0", "type": "de.markt.malo.updated",
            "source": "/marktd/test", "id": uuid::Uuid::new_v4().to_string(),
            "subject": malo, "data": { "version": 1 },
        })
        .to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut request =
            axum::http::Request::post("/webhook").header("content-type", "application/json");
        for (name, value) in
            mako_service::webhook::headers(WEBHOOK_SECRET.as_bytes(), &id, ts, body.as_bytes())
        {
            request = request.header(name, value);
        }
        request.body(axum::body::Body::from(body)).expect("request")
    };

    let (status, body) = call(app.clone(), updated("51238696012")).await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(cached("51238696012").await.as_deref(), Some("NW"));

    sqlx::query(
        "INSERT INTO malo_bundesland (tenant, malo_id, bundesland, postleitzahl, quelle)
         VALUES ('9900357000004', '51238696781', 'BY', '80331', 'test')",
    )
    .execute(&pool)
    .await
    .expect("a Land cached before the MaLo moved");
    let (status, body) = call(app, updated("51238696781")).await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(
        cached("51238696781").await,
        None,
        "a refused Land is not kept"
    );
}

/// A reading order names the role that ordered it — the NB on the 55007 it
/// sent when the LF did not answer (GPKE Teil 2 Kap. 2.5.2.2 Nr. 3, S. 62), the
/// LF on its own Zustimmung 55008 (Nr. 2) — as marktd's `marktrole` states it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_reading_order_names_the_role_that_ordered_it() {
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let as_role = |pid: u32, malo: &str, role: &str, ohne_antwort: bool| {
        let body = serde_json::json!({
            "specversion": "1.0",
            "type": mako_events::mako::PROCESS_COMPLETED,
            "source": "/makod/test",
            "id": uuid::Uuid::new_v4().to_string(),
            "subject": uuid::Uuid::new_v4().to_string(),
            "makopid": pid,
            "marktrole": role,
            "data": {
                "pid": pid, "malo_id": malo, "process_date": "20270131",
                "accepted": true, "outcome": "accepted", "ohne_antwort": ohne_antwort,
            },
        })
        .to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let ts = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut request =
            axum::http::Request::post("/webhook").header("content-type", "application/json");
        for (name, value) in
            mako_service::webhook::headers(WEBHOOK_SECRET.as_bytes(), &id, ts, body.as_bytes())
        {
            request = request.header(name, value);
        }
        request.body(axum::body::Body::from(body)).expect("request")
    };
    let rolle = |malo: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT auftraggeber_rolle FROM ablese_auftraege
                 WHERE malo_id = $1 AND anlass = 'LIEFERENDE'",
            )
            .bind(malo)
            .fetch_optional(&pool)
            .await
            .expect("read the order")
        }
    };

    let (status, body) = call(app.clone(), as_role(55_007, "51238696012", "NB", true)).await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(rolle("51238696012").await.as_deref(), Some("NB"));

    let (status, body) = call(app, as_role(55_008, "51238696781", "LF", false)).await;
    assert!(status.is_success(), "{status}: {body}");
    assert_eq!(rolle("51238696781").await.as_deref(), Some("LF"));
}
