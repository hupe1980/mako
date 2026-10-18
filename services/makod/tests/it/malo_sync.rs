//! makod's MaLo cache follows marktd through the fan-out.
//!
//! The cache answers MaLo-ID identifications to counterparties, so a record
//! changed in marktd while makod is down — or whose delivery fails — must still
//! reach it. These tests run makod's fan-out receiver and backfill against a
//! stand-in marktd on a loopback port:
//!
//! - a makod that missed the change finds the new MSB after its startup
//!   backfill, and readiness waits for it;
//! - a delivery whose record read fails answers `503`, which the fan-out
//!   redelivers, and the redelivery leaves the fresh record in the cache;
//! - a delivery with a wrong signature is refused;
//! - a backfill page older than a record the webhook already applied does not
//!   overwrite it;
//! - a record marktd no longer has is dropped.

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode},
    routing::{get, put},
};
use mako_engine::store_slatedb::SlateDbStore;
use makod::config::PartyConfig;
use makod::malo_cache::SlateDbMaloCache;
use makod::malo_sync::{MaloSync, WEBHOOK_PATH};
use makod::party_registry::MpIdRegistry;
use secrecy::SecretString;
use serde_json::{Value, json};
use tower::ServiceExt as _;

const TENANT: &str = "9900357000004";
const MALO: &str = "51238696012";
const MSB_OLD: &str = "9900001000001";
const MSB_NEW: &str = "9900001000002";
const SECRET: &str = "marktd-fanout-secret";

/// The stand-in marktd: one MaLo record, a failure budget for its reads, and
/// the subscriptions it was asked to register.
#[derive(Default)]
struct Marktd {
    record: Option<Value>,
    fail_reads: usize,
    subscriptions: Vec<Value>,
}

type Shared = Arc<Mutex<Marktd>>;

fn record(msb: &str, version: i64) -> Value {
    json!({
        "malo_id": MALO,
        "sparte": "STROM",
        "version": version,
        "energierichtung": "AUSSP",
        "data": {},
        "rollenzuordnung": [
            { "zuordnungstyp": "NB",  "rollencodenummer": TENANT, "valid_from": "2020-01-01" },
            { "zuordnungstyp": "MSB", "rollencodenummer": msb,    "valid_from": "2020-01-01" }
        ]
    })
}

async fn get_malo(State(m): State<Shared>, Path(id): Path<String>) -> (StatusCode, Json<Value>) {
    let mut m = m.lock().expect("mock lock");
    if m.fail_reads > 0 {
        m.fail_reads -= 1;
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})));
    }
    match &m.record {
        Some(r) if r["malo_id"] == id => (StatusCode::OK, Json(r.clone())),
        _ => (StatusCode::NOT_FOUND, Json(json!({}))),
    }
}

async fn list_malos(State(m): State<Shared>) -> Json<Value> {
    let m = m.lock().expect("mock lock");
    let items: Vec<Value> = m.record.iter().cloned().collect();
    Json(json!({ "items": items, "total": items.len(), "page": 0, "size": 500 }))
}

async fn put_subscription(
    State(m): State<Shared>,
    Path(id): Path<String>,
    Json(mut body): Json<Value>,
) -> Json<Value> {
    body["subscriber_id"] = json!(id);
    m.lock().expect("mock lock").subscriptions.push(body);
    Json(json!({ "version": 1 }))
}

async fn spawn_marktd(state: Shared) -> String {
    let app = Router::new()
        .route("/api/v1/malos", get(list_malos))
        .route("/api/v1/malos/{id}", get(get_malo))
        .route("/api/v1/subscriptions/{id}", put(put_subscription))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock marktd");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("mock marktd");
    });
    format!("http://{addr}")
}

struct Harness {
    marktd: Shared,
    sync: Arc<MaloSync>,
    cache: SlateDbMaloCache,
}

async fn harness() -> Harness {
    let marktd: Shared = Arc::default();
    let url = spawn_marktd(Arc::clone(&marktd)).await;
    let store = SlateDbStore::open_in_memory().await.expect("store");
    let cache = SlateDbMaloCache::new(store);
    let registry = Arc::new(
        MpIdRegistry::from_config(&[PartyConfig {
            mp_id: TENANT.to_owned(),
            roles: vec!["NB".to_owned()],
            primary: true,
            agency: None,
            as4: None,
        }])
        .expect("registry"),
    );
    let sync = Arc::new(MaloSync::new(
        reqwest::Client::new(),
        url,
        SecretString::from("marktd-key"),
        "http://makod:8080/webhooks/marktd",
        Some(SecretString::from(SECRET)),
        cache.clone(),
        TENANT,
        registry,
    ));
    Harness {
        marktd,
        sync,
        cache,
    }
}

impl Harness {
    async fn cached_msb(&self) -> Option<String> {
        let rec = self.cache.get(TENANT, MALO).await.expect("cache read")?;
        rec.data_market_location
            .data_market_location_measuring_point_operators
            .and_then(|v| v.first().map(|p| p.market_partner_id.to_string()))
    }

    fn set_record(&self, rec: Option<Value>) {
        self.marktd.lock().expect("mock lock").record = rec;
    }

    /// One fan-out delivery of `de.markt.malo.updated`, signed with `secret`.
    async fn deliver(&self, secret: &str) -> StatusCode {
        let body = serde_json::to_vec(&json!({
            "specversion": "1.0",
            "id": "evt-1",
            "source": "marktd",
            "type": mako_events::markt::MALO_UPDATED,
            "subject": MALO,
            "marktmaloid": MALO,
            "data": { "version": 2 }
        }))
        .expect("body");
        let mut req =
            Request::post(WEBHOOK_PATH).header("content-type", "application/cloudevents+json");
        for (name, value) in mako_service::webhook::headers(
            secret.as_bytes(),
            "evt-1",
            time::OffsetDateTime::now_utc().unix_timestamp(),
            &body,
        ) {
            req = req.header(name, value);
        }
        makod::malo_sync::router(Arc::clone(&self.sync))
            .oneshot(req.body(Body::from(body)).expect("request"))
            .await
            .expect("router answers")
            .status()
    }
}

/// A makod that was down when marktd changed the MSB serves the new one after
/// its startup backfill, and readiness waits for the backfill.
#[tokio::test]
async fn a_change_made_while_makod_was_down_reaches_the_cache_on_startup() {
    let h = harness().await;
    h.set_record(Some(record(MSB_OLD, 1)));
    h.sync.backfill().await.expect("first backfill");
    assert_eq!(h.cached_msb().await.as_deref(), Some(MSB_OLD));

    // makod is down; marktd changes the MSB and its deliveries go nowhere.
    h.set_record(Some(record(MSB_NEW, 2)));

    let store = SlateDbStore::open_in_memory().await.expect("store");
    let health = makod::health::HealthState::new(store);
    let gate = health.readiness_gate("malo_backfill");
    let ready = || async {
        let app = makod::health::router(health.clone());
        app.oneshot(
            Request::get("/health/ready")
                .body(Body::empty())
                .expect("req"),
        )
        .await
        .expect("health answers")
        .status()
    };
    assert_eq!(
        ready().await,
        StatusCode::SERVICE_UNAVAILABLE,
        "not ready before the backfill"
    );

    Arc::clone(&h.sync)
        .run_startup(gate, tokio_util::sync::CancellationToken::new())
        .await;

    assert_eq!(h.cached_msb().await.as_deref(), Some(MSB_NEW));
    assert_eq!(
        ready().await,
        StatusCode::OK,
        "ready once subscribed and backfilled"
    );
    let subs = h.marktd.lock().expect("mock lock").subscriptions.clone();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["subscriber_id"], "makod");
    assert_eq!(
        subs[0]["event_types"],
        json!([mako_events::markt::MALO_UPDATED])
    );
    assert_eq!(subs[0]["webhook_secret"], SECRET);
}

/// A delivery whose record read fails answers 503 — the fan-out redelivers —
/// and the redelivery leaves the fresh record in the cache.
#[tokio::test]
async fn a_failed_delivery_is_answered_503_and_its_redelivery_applies_the_record() {
    let h = harness().await;
    h.set_record(Some(record(MSB_OLD, 1)));
    h.sync.backfill().await.expect("backfill");

    h.set_record(Some(record(MSB_NEW, 2)));
    h.marktd.lock().expect("mock lock").fail_reads = 1;
    assert_eq!(h.deliver(SECRET).await, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        h.cached_msb().await.as_deref(),
        Some(MSB_OLD),
        "nothing applied while marktd could not be read"
    );

    assert_eq!(h.deliver(SECRET).await, StatusCode::NO_CONTENT);
    assert_eq!(h.cached_msb().await.as_deref(), Some(MSB_NEW));
}

/// A delivery signed with another secret is refused and changes nothing.
#[tokio::test]
async fn a_delivery_with_a_wrong_signature_is_refused() {
    let h = harness().await;
    h.set_record(Some(record(MSB_NEW, 2)));
    assert_eq!(h.deliver("not-the-secret").await, StatusCode::UNAUTHORIZED);
    assert_eq!(h.cached_msb().await, None);
}

/// A backfill page read before a change does not overwrite the newer record
/// the webhook already applied.
#[tokio::test]
async fn an_older_backfill_page_does_not_overwrite_a_newer_delivery() {
    let h = harness().await;
    h.set_record(Some(record(MSB_NEW, 3)));
    assert_eq!(h.deliver(SECRET).await, StatusCode::NO_CONTENT);

    h.set_record(Some(record(MSB_OLD, 2)));
    h.sync.backfill().await.expect("backfill");
    assert_eq!(h.cached_msb().await.as_deref(), Some(MSB_NEW));
}

/// A record marktd no longer has is dropped from the cache.
#[tokio::test]
async fn a_record_marktd_no_longer_has_is_dropped() {
    let h = harness().await;
    h.set_record(Some(record(MSB_OLD, 1)));
    h.sync.backfill().await.expect("backfill");
    h.set_record(None);
    assert_eq!(h.deliver(SECRET).await, StatusCode::NO_CONTENT);
    assert!(h.cache.get(TENANT, MALO).await.expect("read").is_none());
}
