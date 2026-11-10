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
use testcontainers::ImageExt as _;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ── PostgreSQL ──────────────────────────────────────────────────────────────

async fn boot() -> sqlx::PgPool {
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
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
    last_headers: Mutex<axum::http::HeaderMap>,
}

async fn receiver(
    axum::extract::State(rx): axum::extract::State<Arc<Received>>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> axum::http::StatusCode {
    let n = rx.calls.fetch_add(1, Ordering::SeqCst);
    *rx.last_body.lock().unwrap() = body.to_vec();
    *rx.last_headers.lock().unwrap() = headers;
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
    // Asserted through the shared **verifier** a real receiver would use — and
    // it checks the timestamp too, so this also pins that the outbox stamps a
    // fresh one rather than replaying an old row's.
    assert_eq!(
        mako_service::webhook::verify_request(
            Some(b"s3cr3t"),
            &rx.last_headers.lock().unwrap(),
            &expected_body
        ),
        Ok(Some(mako_service::webhook::WebhookId(ce.id.clone()))),
        "the delivered request verifies, and its webhook-id is the CloudEvent id"
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

/// A receiver that accepts the connection and never answers, so the worker is
/// still inside the POST when the test drops its future — the shape of a worker
/// killed between the POST and the outcome write.
async fn spawn_hanging_receiver() -> String {
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(|| async {
            std::future::pending::<()>().await;
            axum::http::StatusCode::OK
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}/hook")
}

/// The retry budget is spent at the claim, not at the outcome write: a worker
/// that dies after claiming a row and before recording what happened must still
/// spend one attempt, or the row is claimed, POSTed and re-leased forever and
/// never reaches `list_dead_letters`, the trail an operator reads.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_worker_killed_mid_delivery_still_spends_the_budget() {
    let pool = boot().await;
    let url = spawn_hanging_receiver().await;
    let ce = sample_event();
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();

    let cfg = OutboxConfig {
        lease: Duration::from_secs(0),
        ..fast_config()
    };
    let worker = OutboxWorker::new(pool.clone(), url, None).with_config(cfg.clone());
    // Kill the worker inside the POST, past the budget.
    for _ in 0..=cfg.max_attempts {
        let _ = tokio::time::timeout(Duration::from_millis(200), worker.flush_once()).await;
    }
    let attempts: i16 = sqlx::query_scalar("SELECT attempts FROM event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        attempts >= cfg.max_attempts,
        "every claim spends an attempt, got {attempts}"
    );
    // The backstop the next flush runs first dead-letters the exhausted row.
    let _ = tokio::time::timeout(Duration::from_millis(200), worker.flush_once()).await;
    let dead = outbox::list_dead_letters(&pool, 10).await.unwrap();
    assert_eq!(dead.len(), 1, "the exhausted row is dead-lettered");
    assert_eq!(pending_count(&pool).await, 0);
}

/// `sweep_exhausted` is callable on its own and leaves a row with budget left.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn sweep_exhausted_takes_only_spent_rows() {
    let pool = boot().await;
    for _ in 0..2 {
        let mut tx = pool.begin().await.unwrap();
        outbox::enqueue(&mut tx, &sample_event()).await.unwrap();
        tx.commit().await.unwrap();
    }
    sqlx::query(
        "UPDATE event_outbox SET attempts = 3
          WHERE id = (SELECT id FROM event_outbox ORDER BY created_at, id LIMIT 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(outbox::sweep_exhausted(&pool, 3).await.unwrap(), 1);
    assert_eq!(pending_count(&pool).await, 1);
}

fn sample_command(key: &str) -> outbox::Command {
    outbox::Command {
        upstream: "makod".into(),
        label: "esa.abbestellung.beauftragen".into(),
        method: outbox::CommandMethod::Post,
        path: "/hook".into(),
        body: serde_json::json!({"command": "esa.abbestellung.beauftragen"}),
        key: key.into(),
        settled_statuses: vec![409],
    }
}

/// A command owed to an upstream that is down survives the outage and is
/// delivered once when it returns, with its key as the `Idempotency-Key`; an
/// event worker never claims it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_command_survives_its_upstream_being_down() {
    let pool = boot().await;
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_command(&mut tx, &sample_command("abb:1:51238696012"))
        .await
        .unwrap();
    // Enqueued twice under one key: one obligation.
    outbox::enqueue_command(&mut tx, &sample_command("abb:1:51238696012"))
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The event worker leaves the command alone.
    let (ce_url, ce_rx) = spawn_receiver(vec![200]).await;
    let events = OutboxWorker::new(pool.clone(), ce_url, None).with_config(fast_config());
    assert_eq!(events.flush_once().await.unwrap(), 0);
    assert_eq!(ce_rx.calls.load(Ordering::SeqCst), 0);

    // Upstream down: the attempt fails transiently and the row stays pending.
    let (down, _down_rx) = spawn_receiver(vec![503]).await;
    let base_down = down.trim_end_matches("/hook").to_owned();
    OutboxWorker::commands(pool.clone(), "makod", base_down, None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    assert_eq!(pending_count(&pool).await, 1, "the obligation survives");

    // Upstream back: delivered once.
    let (up, up_rx) = spawn_receiver(vec![200]).await;
    let base_up = up.trim_end_matches("/hook").to_owned();
    let worker = OutboxWorker::commands(pool.clone(), "makod", base_up, Some("tok".into()))
        .with_config(fast_config());
    worker.flush_once().await.unwrap();
    worker.flush_once().await.unwrap();
    assert_eq!(
        up_rx.calls.load(Ordering::SeqCst),
        1,
        "delivered exactly once"
    );
    let headers = up_rx.last_headers.lock().unwrap().clone();
    assert_eq!(headers["idempotency-key"], "abb:1:51238696012");
    assert_eq!(headers["authorization"], "Bearer tok");
    assert_eq!(pending_count(&pool).await, 0);
}

/// A settled status discharges a command; any other 4xx dead-letters it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_settled_status_discharges_a_command() {
    let pool = boot().await;
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_command(&mut tx, &sample_command("a"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (url, _rx) = spawn_receiver(vec![409]).await;
    OutboxWorker::commands(pool.clone(), "makod", url.trim_end_matches("/hook"), None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    assert_eq!(pending_count(&pool).await, 0);
    assert!(
        outbox::list_dead_letters(&pool, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_command(&mut tx, &sample_command("b"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (url, _rx) = spawn_receiver(vec![422]).await;
    OutboxWorker::commands(pool.clone(), "makod", url.trim_end_matches("/hook"), None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    let dead = outbox::list_dead_letters(&pool, 10).await.unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].kind, "command");
    assert_eq!(dead[0].upstream.as_deref(), Some("makod"));
}

/// The `on_delivered` handler runs in the transaction that marks the row
/// delivered and sees the receiver's answer; a handler that fails leaves the
/// row pending, so the follow-up is never lost behind a delivered mark.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_delivery_handler_commits_with_the_delivered_mark() {
    let pool = boot().await;
    sqlx::query("CREATE TABLE links (subject TEXT, reply TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    let app = axum::Router::new().route(
        "/hook",
        axum::routing::post(|| async { axum::Json(serde_json::json!({"ref": "R-1"})) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let ce = sample_event();
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue(&mut tx, &ce).await.unwrap();
    tx.commit().await.unwrap();

    let failing: outbox::OnDelivered = Arc::new(|_conn, _d| {
        Box::pin(async { Err(sqlx::Error::Protocol("follow-up failed".into())) })
    });
    OutboxWorker::new(pool.clone(), url.clone(), None)
        .with_config(OutboxConfig {
            lease: Duration::ZERO,
            ..fast_config()
        })
        .on_delivered(failing)
        .flush_once()
        .await
        .unwrap();
    assert_eq!(
        pending_count(&pool).await,
        1,
        "no delivered mark without its follow-up"
    );

    let linking: outbox::OnDelivered = Arc::new(|conn, d| {
        Box::pin(async move {
            sqlx::query("INSERT INTO links (subject, reply) VALUES ($1, $2)")
                .bind(&d.subject)
                .bind(d.reply.as_ref().and_then(|r| r["ref"].as_str()))
                .execute(conn)
                .await
                .map(|_| ())
        })
    });
    OutboxWorker::new(pool.clone(), url, None)
        .with_config(fast_config())
        .on_delivered(linking)
        .flush_once()
        .await
        .unwrap();
    assert_eq!(pending_count(&pool).await, 0);
    let (subject, reply): (String, String) = sqlx::query_as("SELECT subject, reply FROM links")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((subject.as_str(), reply.as_str()), ("DE0001", "R-1"));
}

/// An event for a named receiver is drained by that receiver's worker only.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_targeted_event_reaches_only_its_receiver() {
    let pool = boot().await;
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_to(&mut tx, "eog", &sample_event())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (default_url, default_rx) = spawn_receiver(vec![200]).await;
    OutboxWorker::new(pool.clone(), default_url, None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    assert_eq!(default_rx.calls.load(Ordering::SeqCst), 0);

    let (eog_url, eog_rx) = spawn_receiver(vec![200]).await;
    OutboxWorker::for_target(pool.clone(), "eog", eog_url, None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    assert_eq!(eog_rx.calls.load(Ordering::SeqCst), 1);
    assert_eq!(pending_count(&pool).await, 0);
}

/// A signed command worker signs each command with Standard Webhooks under the
/// command's key, and its follow-up sees the key and the receiver's answer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_signed_command_verifies_and_its_follow_up_sees_the_reply() {
    let pool = boot().await;
    sqlx::query("CREATE TABLE seen (key TEXT, reply TEXT)")
        .execute(&pool)
        .await
        .unwrap();
    type Capture = Option<(axum::http::HeaderMap, Vec<u8>)>;
    let captured: Arc<Mutex<Capture>> = Arc::default();
    let cap = Arc::clone(&captured);
    let app = axum::Router::new().route(
        "/relay",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let cap = Arc::clone(&cap);
                async move {
                    *cap.lock().unwrap() = Some((headers, body.to_vec()));
                    axum::Json(serde_json::json!({"message_id": "M-1"}))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/relay", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let mut cmd = sample_command("de.output.delivery/1");
    cmd.upstream = "relay".into();
    cmd.path = String::new();
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_command(&mut tx, &cmd).await.unwrap();
    tx.commit().await.unwrap();

    let follow_up: outbox::OnDelivered = Arc::new(|conn, d| {
        Box::pin(async move {
            sqlx::query("INSERT INTO seen (key, reply) VALUES ($1, $2)")
                .bind(&d.subject)
                .bind(d.reply.as_ref().and_then(|r| r["message_id"].as_str()))
                .execute(conn)
                .await
                .map(|_| ())
        })
    });
    OutboxWorker::commands(pool.clone(), "relay", url, Some("relay-key".into()))
        .signed()
        .on_delivered(follow_up)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();

    let (headers, body) = captured.lock().unwrap().take().expect("request");
    assert_eq!(
        mako_service::webhook::verify_request(Some(b"relay-key"), &headers, &body),
        Ok(Some(mako_service::webhook::WebhookId(
            "de.output.delivery/1".into()
        )))
    );
    let (key, reply): (String, String) = sqlx::query_as("SELECT key, reply FROM seen")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        (key.as_str(), reply.as_str()),
        ("de.output.delivery/1", "M-1")
    );
}

/// A receiver that timed out (408) or is throttling or briefly unavailable
/// (429, 502, 503, 504) has not refused the call: the command stays owed and
/// is retried. A refusal of the call itself (422) is permanent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_timeout_is_retried_and_a_refusal_is_not() {
    let pool = boot().await;
    for (status, key) in [
        (408u16, "t408"),
        (429, "t429"),
        (502, "t502"),
        (503, "t503"),
        (504, "t504"),
    ] {
        let mut tx = pool.begin().await.unwrap();
        outbox::enqueue_command(&mut tx, &sample_command(key))
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let (url, _rx) = spawn_receiver(vec![status]).await;
        OutboxWorker::commands(pool.clone(), "makod", url.trim_end_matches("/hook"), None)
            .with_config(fast_config())
            .flush_once()
            .await
            .unwrap();
        assert!(
            outbox::list_dead_letters(&pool, 10)
                .await
                .unwrap()
                .is_empty(),
            "HTTP {status} must be retried, not dead-lettered"
        );
        sqlx::query("DELETE FROM event_outbox")
            .execute(&pool)
            .await
            .unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    outbox::enqueue_command(&mut tx, &sample_command("t422"))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (url, _rx) = spawn_receiver(vec![422]).await;
    OutboxWorker::commands(pool.clone(), "makod", url.trim_end_matches("/hook"), None)
        .with_config(fast_config())
        .flush_once()
        .await
        .unwrap();
    assert_eq!(outbox::list_dead_letters(&pool, 10).await.unwrap().len(), 1);
}
