//! Real-PostgreSQL proof for the durable, crash-safe fan-out.
//!
//! ```bash
//! docker run -d --name marktd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=marktd -p 55438:5432 postgres:17-alpine
//! export MARKTD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55438/marktd"
//! cargo test -p marktd --test fanout_durable_integration -- --include-ignored
//! ```
//!
//! Proves:
//! 1. `outbox::enqueue` → worker Phase 1 creates `event_delivery` rows → Phase 2
//!    POSTs and marks `delivered_at` (with a per-subscriber HMAC signature).
//! 2. An `event_log` row written directly (as if by a producer that crashed
//!    before fan-out) is picked up and delivered by a *fresh* worker with no
//!    in-memory state — the crash-recovery guarantee.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use axum::{Router, http::HeaderMap, routing::post};
use mako_markt::cloudevents::MarktEvent;
use marktd::fanout::{self, FanoutConfig};
use marktd::pg::subscription::PgSubscriptionRepository;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const CE_TYPE: &str = "de.markt.malo.updated";
const SECRET: &str = "s3cr3t-hmac-key";

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("MARKTD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;
    let schema = format!("fanout_{test_name}");
    sqlx::query(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .execute(&admin)
        .await
        .expect("drop schema");
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .expect("create schema");
    admin.close().await;
    let opts: sqlx::postgres::PgConnectOptions = base.parse().expect("parse url");
    let pool = PgPool::connect_with(opts.options([("search_path", schema.as_str())]))
        .await
        .expect("connect schema");
    let stripped: String = SCHEMA
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in stripped.split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        sqlx::query(s)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema stmt failed: {e}\n{s}"));
    }
    Some(pool)
}

/// Shared capture state for the mock webhook receiver.
#[derive(Default)]
struct Captured {
    hits: AtomicUsize,
    signed: AtomicUsize,
}

async fn spawn_mock_webhook(state: Arc<Captured>) -> String {
    async fn handler(
        axum::extract::State(state): axum::extract::State<Arc<Captured>>,
        headers: HeaderMap,
        _body: axum::body::Bytes,
    ) -> axum::http::StatusCode {
        state.hits.fetch_add(1, Ordering::SeqCst);
        if headers.contains_key("x-mako-signature") {
            state.signed.fetch_add(1, Ordering::SeqCst);
        }
        axum::http::StatusCode::OK
    }
    let app = Router::new()
        .route("/hook", post(handler))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/hook")
}

async fn insert_subscription(pool: &PgPool, url: &str) {
    sqlx::query(
        "INSERT INTO subscriptions
             (subscriber_id, webhook_url, webhook_secret, roles, event_types, sparten, active)
         VALUES ($1, $2, $3, '{}', $4, '{}', true)",
    )
    .bind("erp-1")
    .bind(url)
    .bind(SECRET)
    .bind(vec![CE_TYPE.to_owned()])
    .execute(pool)
    .await
    .expect("insert subscription");
}

async fn wait_delivered(pool: &PgPool, event_id: &str) -> bool {
    for _ in 0..100 {
        let done: Option<time::OffsetDateTime> = sqlx::query_scalar(
            "SELECT delivered_at FROM event_delivery WHERE event_id = $1 AND subscriber_id = 'erp-1'",
        )
        .bind(event_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten();
        if done.is_some() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn enqueue_then_worker_fans_out_and_delivers() {
    let Some(pool) = test_pool("deliver").await else {
        eprintln!("skipping: MARKTD_TEST_DATABASE_URL not set");
        return;
    };
    let captured = Arc::new(Captured::default());
    let url = spawn_mock_webhook(Arc::clone(&captured)).await;
    insert_subscription(&pool, &url).await;

    let notify = Arc::new(tokio::sync::Notify::new());
    let shutdown = CancellationToken::new();
    fanout::spawn(
        pool.clone(),
        PgSubscriptionRepository::new(pool.clone()),
        mako_service::http::default_client(),
        FanoutConfig::default(),
        Arc::clone(&notify),
        shutdown.clone(),
    );

    // Durable enqueue (persist-before-fan-out) + wake-up hint.
    let ev = MarktEvent::new(
        TENANT,
        CE_TYPE,
        "MALO-1".to_owned(),
        serde_json::json!({ "version": 1 }),
    );
    marktd::outbox::enqueue(&pool, &ev, &notify)
        .await
        .expect("enqueue");

    assert!(
        wait_delivered(&pool, &ev.id).await,
        "delivery row should be marked delivered"
    );
    assert_eq!(captured.hits.load(Ordering::SeqCst), 1, "webhook hit once");
    assert_eq!(
        captured.signed.load(Ordering::SeqCst),
        1,
        "delivery must carry the per-subscriber HMAC signature"
    );

    // The event_log row is stamped fanned_out_at.
    let fanned: Option<time::OffsetDateTime> =
        sqlx::query_scalar("SELECT fanned_out_at FROM event_log WHERE event_id = $1")
            .bind(&ev.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(fanned.is_some(), "event_log must be stamped fanned_out_at");

    shutdown.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn undelivered_event_log_recovered_by_fresh_worker() {
    let Some(pool) = test_pool("recovery").await else {
        eprintln!("skipping: MARKTD_TEST_DATABASE_URL not set");
        return;
    };
    let captured = Arc::new(Captured::default());
    let url = spawn_mock_webhook(Arc::clone(&captured)).await;
    insert_subscription(&pool, &url).await;

    // Simulate a producer that persisted to the outbox and then CRASHED before
    // any fan-out: an event_log row with fanned_out_at IS NULL, and no notify.
    let ev = MarktEvent::new(
        TENANT,
        CE_TYPE,
        "MALO-2".to_owned(),
        serde_json::json!({ "version": 7 }),
    );
    let envelope = serde_json::to_value(&ev).unwrap();
    sqlx::query(
        "INSERT INTO event_log (event_id, ce_type, marktrole, sparte, envelope)
         VALUES ($1, $2, NULL, NULL, $3)",
    )
    .bind(&ev.id)
    .bind(CE_TYPE)
    .bind(&envelope)
    .execute(&pool)
    .await
    .expect("seed event_log");

    // A FRESH worker with a never-signalled notify: recovery must come purely
    // from the durable table via the worker's first poll tick.
    let notify = Arc::new(tokio::sync::Notify::new());
    let shutdown = CancellationToken::new();
    fanout::spawn(
        pool.clone(),
        PgSubscriptionRepository::new(pool.clone()),
        mako_service::http::default_client(),
        FanoutConfig::default(),
        notify,
        shutdown.clone(),
    );

    assert!(
        wait_delivered(&pool, &ev.id).await,
        "a pre-existing undelivered event_log row must be recovered and delivered"
    );
    assert_eq!(captured.hits.load(Ordering::SeqCst), 1);

    shutdown.cancel();
}
