//! Real-PostgreSQL integration tests for the transactional outbox.
//!
//! Each test spins a real PostgreSQL via testcontainers and a throwaway axum
//! receiver, so they are `#[ignore]`d by default (Docker required):
//!
//! ```bash
//! cargo test -p mako-service --test outbox_integration -- --include-ignored --test-threads=1
//! ```
//!
//! What they pin:
//! - **Atomicity** — an enqueue on a rolled-back transaction leaves no row
//!   (persist-before-dispatch is only worth anything if it is transactional).
//! - **Happy path** — a committed enqueue is delivered by the worker and marked
//!   `delivered_at`, with the signed CloudEvent bytes actually received.
//! - **Transient retry → dead-letter** — a receiver that always 5xxs drives the
//!   row through its attempts to `dead_lettered_at`, listable + requeueable.
//! - **Permanent 4xx** — dead-lettered immediately, no wasted retries.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mako_service::CloudEvent;
use mako_service::outbox::{self, OutboxConfig, OutboxWorker};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ── PostgreSQL ──────────────────────────────────────────────────────────────

async fn boot() -> sqlx::PgPool {
    let container = Postgres::default().start().await.expect("start postgres");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("postgres port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Box::leak(Box::new(container));

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect");
    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&pool)
        .await
        .expect("pgcrypto");
    outbox::ensure_schema(&pool).await.expect("outbox schema");
    pool
}

// ── Scriptable receiver ─────────────────────────────────────────────────────

#[derive(Default)]
struct Received {
    calls: AtomicUsize,
    statuses: Mutex<Vec<u16>>,
    last_body: Mutex<Vec<u8>>,
    last_sig: Mutex<Option<String>>,
}

async fn receiver(
    axum::extract::State(rx): axum::extract::State<Arc<Received>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    let n = rx.calls.fetch_add(1, Ordering::SeqCst);
    *rx.last_body.lock().unwrap() = body.to_vec();
    *rx.last_sig.lock().unwrap() = headers
        .get("x-mako-signature")
        .and_then(|v| v.to_str().ok())
        .map(String::from);
    let statuses = rx.statuses.lock().unwrap();
    let code = *statuses.get(n).unwrap_or_else(|| statuses.last().unwrap());
    axum::http::StatusCode::from_u16(code).unwrap()
}

async fn spawn_receiver(statuses: Vec<u16>) -> (String, Arc<Received>) {
    let rx = Arc::new(Received {
        statuses: Mutex::new(statuses),
        ..Default::default()
    });
    let app = axum::Router::new()
        .route("/hook", axum::routing::post(receiver))
        .with_state(Arc::clone(&rx));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}/hook"), rx)
}

fn sample_event() -> CloudEvent {
    CloudEvent::new(
        mako_service::source("billingd", "9900000000001"),
        mako_events::billing::RECHNUNG_ERSTELLT,
        "DE0001",
        serde_json::json!({"betrag": "42.00"}),
    )
}

// Fast config so the retry→dead-letter test does not sleep for hours.
fn fast_config() -> OutboxConfig {
    OutboxConfig {
        poll_interval: Duration::from_millis(50),
        batch_size: 50,
        max_attempts: 3,
        backoff: vec![Duration::from_millis(0)],
        lease: Duration::from_secs(30),
        retention: Duration::from_secs(30 * 24 * 3600),
    }
}

async fn pending_count(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE delivered_at IS NULL AND dead_lettered_at IS NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn rolled_back_enqueue_leaves_no_row() {
    let pool = boot().await;
    let ce = sample_event();
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.rollback().await.unwrap();
    // Atomicity: the event vanished with the business transaction.
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "a rolled-back enqueue must not persist the event");
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn committed_event_is_delivered_and_signed() {
    let pool = boot().await;
    let (url, rx) = spawn_receiver(vec![200]).await;
    let ce = sample_event();
    let expected_body = ce.to_bytes().unwrap();

    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();

    let worker =
        OutboxWorker::new(pool.clone(), url, Some("s3cr3t".into())).with_config(fast_config());
    let n = worker.flush_once().await.unwrap();
    assert_eq!(n, 1);

    assert_eq!(rx.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *rx.last_body.lock().unwrap(),
        expected_body,
        "exact CE bytes delivered"
    );
    assert_eq!(
        *rx.last_sig.lock().unwrap(),
        Some(mako_service::webhook::sign(b"s3cr3t", &expected_body))
    );
    assert_eq!(
        pending_count(&pool).await,
        0,
        "delivered → no longer pending"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn duplicate_enqueue_is_idempotent_on_event_id() {
    let pool = boot().await;
    let ce = sample_event();
    for _ in 0..3 {
        let mut tx = pool.begin().await.unwrap();
        outbox::enqueue(&mut tx, &ce).await.unwrap();
        tx.commit().await.unwrap();
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "same CloudEvent id must not double-enqueue");
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn transient_failures_exhaust_to_dead_letter_then_requeue() {
    let pool = boot().await;
    let (url, rx) = spawn_receiver(vec![503]).await; // always 5xx
    let ce = sample_event();

    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();

    let worker = OutboxWorker::new(pool.clone(), url.clone(), None).with_config(fast_config());
    // max_attempts = 3, zero backoff → three flushes drive it to dead-letter.
    for _ in 0..3 {
        worker.flush_once().await.unwrap();
    }
    assert!(
        rx.calls.load(Ordering::SeqCst) >= 3,
        "each attempt hit the receiver"
    );
    let dead = outbox::list_dead_letters(&pool, 10).await.unwrap();
    assert_eq!(dead.len(), 1, "exhausted event is dead-lettered");
    assert_eq!(dead[0].event_id, ce.id);
    assert_eq!(pending_count(&pool).await, 0, "dead-letter is not pending");

    // Requeue → pending again; a now-healthy receiver delivers it.
    let (url2, rx2) = spawn_receiver(vec![200]).await;
    assert!(outbox::requeue(&pool, dead[0].id).await.unwrap());
    assert_eq!(pending_count(&pool).await, 1, "requeue makes it pending");
    let worker2 = OutboxWorker::new(pool.clone(), url2, None).with_config(fast_config());
    worker2.flush_once().await.unwrap();
    assert_eq!(rx2.calls.load(Ordering::SeqCst), 1);
    assert_eq!(pending_count(&pool).await, 0);
    let _ = url; // silence unused in case of edits
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn prune_removes_old_delivered_rows_only() {
    let pool = boot().await;
    let (url, _rx) = spawn_receiver(vec![200]).await;
    let ce = sample_event();
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();
    OutboxWorker::new(pool.clone(), url, None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();

    // Not old enough yet → retained.
    assert_eq!(
        outbox::prune_delivered(&pool, Duration::from_secs(3600))
            .await
            .unwrap(),
        0
    );
    // Backdate the delivery and prune with zero retention → gone.
    sqlx::query("UPDATE event_outbox SET delivered_at = now() - INTERVAL '2 days'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        outbox::prune_delivered(&pool, Duration::from_secs(0))
            .await
            .unwrap(),
        1
    );
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0);
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn permanent_4xx_dead_letters_immediately() {
    let pool = boot().await;
    let (url, rx) = spawn_receiver(vec![400]).await; // permanent
    let ce = sample_event();

    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();

    let worker = OutboxWorker::new(pool.clone(), url, None).with_config(fast_config());
    worker.flush_once().await.unwrap();

    assert_eq!(
        rx.calls.load(Ordering::SeqCst),
        1,
        "one attempt, no retries on 4xx"
    );
    let dead = outbox::list_dead_letters(&pool, 10).await.unwrap();
    assert_eq!(dead.len(), 1);
}
