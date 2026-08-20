//! Real-PostgreSQL proof for the durable, crash-safe fan-out.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```
//!
//! Proves:
//! 1. `outbox::enqueue` → worker Phase 1 creates `event_delivery` rows → Phase 2
//!    POSTs and marks `delivered_at` (with a per-subscriber HMAC signature).
//! 2. An `event_log` row written directly (as if by a producer that crashed
//!    before fan-out) is picked up and delivered by a *fresh* worker with no
//!    in-memory state — the crash-recovery guarantee.
//! 3. Events about one Marktlokation reach a subscriber in publication order
//!    even when the first delivery fails — the per-aggregate ordering guarantee.
//! 4. A subscriber's `sparten` filter actually filters.

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

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
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
        if headers.contains_key(mako_service::webhook::SIGNATURE_HEADER) {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn enqueue_then_worker_fans_out_and_delivers() {
    let Some((pool, _pg)) = test_pool("deliver").await else {
        eprintln!("skipping: Docker unavailable");
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn undelivered_event_log_recovered_by_fresh_worker() {
    let Some((pool, _pg)) = test_pool("recovery").await else {
        eprintln!("skipping: Docker unavailable");
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
/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine` and return its URL plus the
/// container guard. `None` when Docker is unavailable (tests skip gracefully).
async fn pg_container() -> Option<(String, PgContainer)> {
    use testcontainers::ImageExt;
    use testcontainers::runners::AsyncRunner;
    use testcontainers_modules::postgres::Postgres;
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Some((url, container))
}

// ── Per-aggregate ordering ────────────────────────────────────────────────────

/// A receiver that rejects the first `fail_first` requests, then records the
/// subjects it accepts in arrival order.
#[derive(Default)]
struct OrderCapture {
    seen: std::sync::Mutex<Vec<String>>,
    remaining_failures: AtomicUsize,
}

async fn spawn_ordering_webhook(state: Arc<OrderCapture>) -> String {
    async fn handler(
        axum::extract::State(state): axum::extract::State<Arc<OrderCapture>>,
        body: axum::body::Bytes,
    ) -> axum::http::StatusCode {
        if state
            .remaining_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| {
                (n > 0).then(|| n - 1)
            })
            .is_ok()
        {
            return axum::http::StatusCode::INTERNAL_SERVER_ERROR;
        }
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        state
            .seen
            .lock()
            .unwrap()
            .push(v["subject"].as_str().unwrap_or_default().to_owned());
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

/// Three events about **one** Marktlokation, where the first delivery fails
/// once. Unordered delivery would hand the subscriber #2 and #3 while #1 backed
/// off, so an ERP would persist the superseded supply state last and keep it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn events_about_one_malo_arrive_in_order_even_when_the_first_fails() {
    let Some((pool, _pg)) = test_pool("ordering").await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let captured = Arc::new(OrderCapture::default());
    captured.remaining_failures.store(1, Ordering::SeqCst);
    let url = spawn_ordering_webhook(Arc::clone(&captured)).await;
    insert_subscription(&pool, &url).await;

    let notify = Arc::new(tokio::sync::Notify::new());
    let shutdown = CancellationToken::new();
    fanout::spawn(
        pool.clone(),
        PgSubscriptionRepository::new(pool.clone()),
        mako_service::http::default_client(),
        FanoutConfig {
            // Retry the failed first event promptly so the test does not sit out
            // the production 30 s back-off.
            backoff: vec![Duration::from_secs(1)],
            poll_interval: Duration::from_millis(200),
            ..FanoutConfig::default()
        },
        Arc::clone(&notify),
        shutdown.clone(),
    );

    let malo = "51238696012";
    let mut ids = Vec::new();
    for version in 1..=3 {
        let ev = MarktEvent::new(
            TENANT,
            CE_TYPE,
            malo.to_owned(),
            serde_json::json!({ "version": version }),
        )
        .with_extensions(mako_markt::cloudevents::EventExtensions {
            marktmaloid: Some(malo.to_owned()),
            ..Default::default()
        });
        marktd::outbox::enqueue(&pool, &ev, &notify)
            .await
            .expect("enqueue");
        ids.push(ev.id.clone());
    }

    for id in &ids {
        assert!(
            wait_delivered(&pool, id).await,
            "event {id} never delivered"
        );
    }

    let seen = captured.seen.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        3,
        "every event about the MaLo must be delivered exactly once"
    );
    // All three carry the same subject; what the ordering guarantee actually
    // pins is that none was delivered before its predecessor, which the
    // delivery timestamps record.
    let order: Vec<(i64, Option<time::OffsetDateTime>)> =
        sqlx::query_as("SELECT d.seq, d.delivered_at FROM event_delivery d ORDER BY d.seq")
            .fetch_all(&pool)
            .await
            .unwrap();
    for pair in order.windows(2) {
        let (prev, next) = (&pair[0], &pair[1]);
        assert!(
            prev.1.unwrap() <= next.1.unwrap(),
            "seq {} was delivered after seq {} — per-aggregate order was not held",
            prev.0,
            next.0
        );
    }

    shutdown.cancel();
}

/// A STROM-only subscriber must not receive the Gas stream. The `sparten` filter
/// was documented and inert: only one event type ever populated the column it
/// matched on, so every other event fanned out to every subscriber.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_strom_only_subscriber_is_not_woken_by_a_gas_event() {
    let Some((pool, _pg)) = test_pool("sparten").await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let captured = Arc::new(Captured::default());
    let url = spawn_mock_webhook(Arc::clone(&captured)).await;
    sqlx::query(
        "INSERT INTO subscriptions
             (subscriber_id, webhook_url, webhook_secret, roles, event_types, sparten, active)
         VALUES ('erp-1', $1, $2, '{}', $3, ARRAY['STROM'], true)",
    )
    .bind(&url)
    .bind(SECRET)
    .bind(vec![CE_TYPE.to_owned()])
    .execute(&pool)
    .await
    .expect("insert subscription");

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

    let gas = MarktEvent::new(
        TENANT,
        CE_TYPE,
        "GAS-MALO".to_owned(),
        serde_json::json!({}),
    )
    .with_extensions(mako_markt::cloudevents::EventExtensions {
        marktsparte: Some("GAS".to_owned()),
        ..Default::default()
    });
    marktd::outbox::enqueue(&pool, &gas, &notify)
        .await
        .expect("enqueue gas");

    let strom = MarktEvent::new(
        TENANT,
        CE_TYPE,
        "STROM-MALO".to_owned(),
        serde_json::json!({}),
    )
    .with_extensions(mako_markt::cloudevents::EventExtensions {
        marktsparte: Some("STROM".to_owned()),
        ..Default::default()
    });
    marktd::outbox::enqueue(&pool, &strom, &notify)
        .await
        .expect("enqueue strom");

    assert!(
        wait_delivered(&pool, &strom.id).await,
        "the STROM event must be delivered"
    );
    let gas_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM event_delivery WHERE event_id = $1")
            .bind(&gas.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        gas_rows, 0,
        "a STROM-only subscriber must get no delivery row for a GAS event"
    );
    assert_eq!(captured.hits.load(Ordering::SeqCst), 1);

    shutdown.cancel();
}
