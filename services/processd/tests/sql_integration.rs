//! Integration tests for `processd` PostgreSQL queries using testcontainers.
//!
//! # Why testcontainers?
//!
//! All processd SQL queries use `sqlx::query(…)` — dynamic, untype-checked.
//! This test module spins up a real PostgreSQL container, applies all migrations,
//! and exercises every query in the `pg` module to catch:
//!
//! - Column name mismatches (e.g. after a rename migration)
//! - Missing NOT NULL constraints surfaced at insert time
//! - Type-binding errors (e.g. wrong type for a UUID column)
//! - Missing rows / incorrect WHERE clauses
//!
//! These are exactly the errors that `sqlx::query!()` compile-time macros would
//! catch statically. Until the `.sqlx/` cache is generated (see `just sqlx-prepare`)
//! and the queries are migrated to `query!()`, this test suite is the safety net.
//!
//! # Running
//!
//! ```bash
//! # Requires Docker running locally:
//! cargo test --test sql_integration -p processd
//! ```
//!
//! # sqlx-prepare alternative
//!
//! Run `just sqlx-prepare` to generate the `.sqlx/` offline cache, then
//! migrate queries to `sqlx::query_as!()` for compile-time checking.

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

// ── Container lifecycle helper ────────────────────────────────────────────────

async fn pg_pool() -> sqlx::PgPool {
    let container = Postgres::default().start().await.expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("get port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    // Leak the container so it lives for the duration of the test process.
    Box::leak(Box::new(container));

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");

    sqlx::query("CREATE EXTENSION IF NOT EXISTS pgcrypto")
        .execute(&pool)
        .await
        .expect("enable pgcrypto for gen_random_uuid()");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    pool
}

// ── Approval queue ────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_queue_enqueue_list_approve() {
    let pool = pg_pool().await;
    let queue = processd::pg::PgApprovalQueue::new(pool.clone());

    let id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    use processd::pg::approval::{ApprovalQueueEntry, QueueStatus};

    // Enqueue
    queue
        .enqueue(&ApprovalQueueEntry {
            id,
            process_id,
            pid: 55001,
            malo_id: Some("12345678901".to_owned()),
            reason: "test E_0624 event".to_owned(),
            status: QueueStatus::Pending,
            expires_at: now + time::Duration::minutes(45),
            created_at: now,
            decided_at: None,
            tenant: "9900357000004".to_owned(),
        })
        .await
        .expect("enqueue approval entry");

    // List pending
    let pending = queue
        .list("9900357000004", Some(QueueStatus::Pending), 50)
        .await
        .expect("list pending");
    assert_eq!(pending.len(), 1, "expected 1 pending entry");
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].pid, 55001);
    assert!(matches!(pending[0].status, QueueStatus::Pending));

    // Find by ID
    let entry = queue
        .find_by_id(id, "9900357000004")
        .await
        .expect("find_by_id")
        .expect("entry exists");
    assert_eq!(entry.process_id, process_id);

    // Approve
    let affected = queue.approve(id, "9900357000004").await.expect("approve");
    assert!(affected, "approve must affect 1 row");

    let approved = queue
        .find_by_id(id, "9900357000004")
        .await
        .expect("find approved")
        .expect("entry still exists");
    assert!(matches!(approved.status, QueueStatus::Approved));
    assert!(
        approved.decided_at.is_some(),
        "decided_at must be set after approve"
    );

    // Expire stale (no stale entries — all decided)
    let expired_count = queue.expire_stale().await.expect("expire stale");
    assert_eq!(expired_count, 0, "decided entries must not be expired");
}

// ── Anmeldung decisions ───────────────────────────────────────────────────────

#[tokio::test]
async fn anmeldung_decisions_insert_and_list() {
    let pool = pg_pool().await;
    let repo = processd::pg::PgAnmeldungRepository::new(pool.clone());

    use processd::pg::anmeldung::{AnmeldungDecision, AnmeldungDecisionRecord};

    let process_id = Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    repo.insert(&AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: 55001,
        malo_id: "12345678901".to_owned(),
        lf_mp_id: "9900100000001".to_owned(),
        decision: AnmeldungDecision::Accept,
        erc_code: None,
        detail: None,
        initiator_is_affiliate: false,
        decided_at: now,
        tenant: "9900357000004".to_owned(),
    })
    .await
    .expect("insert Accept decision");

    let records = repo
        .list("9900357000004", 50)
        .await
        .expect("list decisions");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].process_id, process_id);
    assert!(matches!(records[0].decision, AnmeldungDecision::Accept));

    // STP rate: 1 accept out of 1 → 100%
    let rate = repo
        .stp_rate("9900357000004", 7)
        .await
        .expect("stp_rate")
        .unwrap_or(0.0);
    assert!(
        (rate - 1.0).abs() < f64::EPSILON,
        "100% STP rate for single Accept, got {rate}"
    );
}
