//! marktd's events reach its in-cluster receivers through the shared webhook
//! module: a MaLo written over REST is enqueued to `event_outbox` with its
//! write, fanned out to the `[[subscriptions.endpoints]]` whose filter matches,
//! and delivered signed with the endpoint's configured secret — the shape
//! makod's MaLo cache (`/webhooks/marktd`) verifies and reads.
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use std::sync::{Arc, Mutex};

use marktd::daemon::Marktd;
use sqlx::PgPool;

const SECRET: &str = "makod-marktd-webhook-secret";

/// What a receiver got: the three Standard Webhooks headers and the body.
type Got = (axum::http::HeaderMap, Vec<u8>);

async fn receiver() -> (Arc<Mutex<Vec<Got>>>, String) {
    let got: Arc<Mutex<Vec<Got>>> = Arc::default();
    let sink = Arc::clone(&got);
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let sink = Arc::clone(&sink);
                async move {
                    sink.lock().unwrap().push((headers, body.to_vec()));
                    axum::http::StatusCode::NO_CONTENT
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (got, format!("http://127.0.0.1:{}/hook", addr.port()))
}

struct Harness {
    pool: PgPool,
    base: String,
    subs: mako_service::subscriptions::Subscriptions,
    _pg: testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>,
}

/// marktd on a fresh database with `endpoints` as its
/// `[[subscriptions.endpoints]]`, and the module it delivers through, driven
/// here pass by pass rather than by the background worker.
async fn harness(endpoints: &str) -> Harness {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPool::connect(&url).await.expect("connect");
    let mut cfg = super::router_builds::load("../../deploy/demo/marktd.toml");
    cfg.database.url = url;
    cfg.subscriptions = toml::from_str(endpoints).expect("the [subscriptions] section parses");
    let subs = mako_service::subscriptions::Subscriptions::new(
        pool.clone(),
        "marktd",
        cfg.markt.tenant.clone(),
        cfg.subscriptions.clone(),
    )
    .with_policy(mako_service::outbox::OutboxConfig {
        backoff: vec![std::time::Duration::ZERO],
        lease: std::time::Duration::ZERO,
        ..mako_service::outbox::OutboxConfig::default()
    });
    let app = mako_service::build_for_test_on::<Marktd>(cfg, pool.clone())
        .await
        .expect("the router builds on the database");
    subs.sync_config()
        .await
        .expect("the config endpoints are written");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Harness {
        pool,
        base: format!("http://{addr}"),
        subs,
        _pg: pg,
    }
}

fn world(file: &str) -> serde_json::Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/mako-seed/world")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
}

impl Harness {
    async fn put_malo(&self, file: &str) -> String {
        let body = world(file);
        let id = body["data"]["marktlokationsId"]
            .as_str()
            .expect("the fixture names its MaLo")
            .to_owned();
        let status = reqwest::Client::new()
            .put(format!("{}/api/v1/malos/{id}", self.base))
            .json(&body)
            .send()
            .await
            .expect("marktd answers")
            .status();
        assert!(status.is_success(), "PUT /api/v1/malos/{id}: {status}");
        id
    }
}

fn event(got: &Got) -> serde_json::Value {
    serde_json::from_slice(&got.1).expect("a JSON CloudEvent")
}

/// A MaLo change reaches makod's endpoint as `de.markt.malo.updated`, signed
/// with the endpoint's secret and carrying `marktmaloid` — what makod's
/// `/webhooks/marktd` verifies, then reads the record back by.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn makods_malo_cache_follows_a_change_through_the_shared_fanout() {
    let (got, url) = receiver().await;
    let h = harness(&format!(
        r#"
        [[endpoints]]
        name = "makod"
        url = "{url}"
        secret = "{SECRET}"
        event_types = ["de.markt.malo.updated"]
        "#
    ))
    .await;
    let malo = h.put_malo("malo.json").await;
    h.subs.drain().await;

    let got = got.lock().unwrap().clone();
    assert_eq!(got.len(), 1, "one delivery for one change");
    let (headers, body) = &got[0];
    mako_service::webhook::verify_request(Some(SECRET.as_bytes()), headers, body)
        .expect("signed with the endpoint's secret");
    let ce = event(&got[0]);
    assert_eq!(ce["type"], mako_events::markt::MALO_UPDATED);
    assert_eq!(ce["marktmaloid"], malo.as_str());
    assert_eq!(
        headers["webhook-id"].to_str().unwrap(),
        ce["id"].as_str().unwrap(),
        "the receiver deduplicates on the CloudEvent id"
    );

    let outboxed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE ce_type = $1 AND ordering_key = $2",
    )
    .bind(mako_events::markt::MALO_UPDATED)
    .bind(&malo)
    .fetch_one(&h.pool)
    .await
    .unwrap();
    assert_eq!(outboxed, 1, "ordered by the Marktlokation");
}

/// An endpoint narrowed on `marktsparte` receives its Sparte's events and not
/// the other's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_sparte_filter_drops_the_other_sparte() {
    let (got, url) = receiver().await;
    let h = harness(&format!(
        r#"
        [[endpoints]]
        name = "gas-only"
        url = "{url}"
        event_types = ["de.markt.malo.*"]
        extensions = {{ marktsparte = "GAS" }}
        "#
    ))
    .await;
    h.put_malo("malo.json").await;
    let gas = h.put_malo("malo-gas.json").await;
    h.subs.drain().await;

    let got = got.lock().unwrap().clone();
    let delivered: Vec<serde_json::Value> = got.iter().map(event).collect();
    assert!(!delivered.is_empty(), "the Gas MaLo's event is delivered");
    assert!(
        delivered
            .iter()
            .all(|ce| ce["marktsparte"] == "GAS" && ce["marktmaloid"] == gas.as_str()),
        "only Gas events reach a Gas endpoint: {delivered:#?}"
    );
}
