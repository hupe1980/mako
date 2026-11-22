//! `GET|PUT /api/v1/netzbetreiber/{nb_mp_id}/grundzustaendiger-msb` — the MP-ID
//! the grundzuständige MSB of a Netzbetreiber's grid acts under.
//!
//! productd reads the `PreisblattMessung` of an offer under this MP-ID, so an
//! unrecorded link must answer `404` — never the NB's own code.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing is a deployment concern"
)]
use marktd::daemon::Marktd;
use tower::ServiceExt as _;

use crate::router_builds::load;

const NB: &str = "9900357000004";
const GMSB: &str = "9900357000035";
/// The undertaking that takes over the Grundzuständigkeit.
const UEBERNEHMER: &str = "9900357000042";

fn uri(nb: &str) -> String {
    format!("/api/v1/netzbetreiber/{nb}/grundzustaendiger-msb")
}

async fn send(
    app: axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let request = axum::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            body.map(|b| b.to_string()).unwrap_or_default(),
        ))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

/// A value that is no MP-ID is refused before the database is reached.
#[tokio::test]
async fn a_value_that_is_no_mp_id_is_refused() {
    let app = mako_service::build_for_test::<Marktd>(load("../../deploy/demo/marktd.toml"))
        .await
        .expect("the router builds");
    let (status, problem) = send(
        app.clone(),
        "PUT",
        &uri(NB),
        Some(serde_json::json!({
            "gmsb_mp_id": "stadtwerke-musterstadt",
            "valid_from": "2020-01-01",
        })),
    )
    .await;
    assert_eq!(status, 422, "{problem}");
    assert_eq!(problem["code"], "mp_id_ungueltig", "{problem}");

    let (status, problem) = send(
        app,
        "PUT",
        &uri(NB),
        Some(serde_json::json!({ "gmsb": GMSB })),
    )
    .await;
    assert_eq!(status, 422, "an unknown key is refused by name: {problem}");
}

/// Unrecorded, the link is `404`; recorded, it is read back under the NB.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_link_is_recorded_and_read_back() {
    use testcontainers::ImageExt as _;
    use testcontainers::runners::AsyncRunner as _;
    use testcontainers_modules::postgres::Postgres;

    let Ok(pg) = Postgres::default().with_tag("17-alpine").start().await else {
        return;
    };
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let mut cfg = load("../../deploy/demo/marktd.toml");
    cfg.database.url = url;
    let app = mako_service::build_for_test_on::<Marktd>(cfg, pool)
        .await
        .expect("the router builds");

    let (status, problem) = send(app.clone(), "GET", &uri(NB), None).await;
    assert_eq!(status, 404, "no link is recorded yet: {problem}");

    let (status, problem) = send(
        app.clone(),
        "PUT",
        &uri(NB),
        Some(serde_json::json!({ "gmsb_mp_id": GMSB, "valid_from": "2020-01-01" })),
    )
    .await;
    assert_eq!(status, 204, "{problem}");

    let (status, answer) = send(app.clone(), "GET", &uri(NB), None).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["nb_mp_id"], NB);
    assert_eq!(answer["gmsb_mp_id"], GMSB);
    assert_eq!(answer["valid_from"], "2020-01-01");
    assert_eq!(answer["valid_to"], serde_json::Value::Null);

    // A transfer of the Grundzuständigkeit (§§ 41, 43 MsbG) from 2027-01-01:
    // the old link ends that day, the new one starts it, and an offer is
    // priced by the link in force on its own day.
    let (status, problem) = send(
        app.clone(),
        "PUT",
        &uri(NB),
        Some(serde_json::json!({ "gmsb_mp_id": UEBERNEHMER, "valid_from": "2027-01-01" })),
    )
    .await;
    assert_eq!(status, 204, "{problem}");
    let am = |tag: &'static str| {
        let app = app.clone();
        async move { send(app, "GET", &format!("{}?at={tag}", uri(NB)), None).await }
    };
    let (_, vorher) = am("2026-12-31").await;
    assert_eq!(vorher["gmsb_mp_id"], GMSB, "{vorher}");
    assert_eq!(vorher["valid_to"], "2027-01-01", "{vorher}");
    let (_, nachher) = am("2027-01-01").await;
    assert_eq!(nachher["gmsb_mp_id"], UEBERNEHMER, "{nachher}");

    let (status, _) = am("2019-12-31").await;
    assert_eq!(status, 404, "no link before the first was recorded");
}
