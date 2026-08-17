//! Real-PostgreSQL tests for `invoic_receipts` — persistence and the ERP outbox.
//!
//! Uses testcontainers (self-managing Docker) — runs under `cargo test` when a
//! Docker daemon is available, skipped gracefully otherwise.
//!
//! # Why this exists
//!
//! The receipt table is the § 147 AO / GoBD audit trail, and everything keyed on
//! it (ERP outbox rows, dispute tracking) is only as good as the INSERT. Two of
//! its statements were rejected by the schema at runtime while compiling fine:
//!
//! - `direction` was written capitalised (`"Inbound"`) against a
//!   `CHECK (direction IN ('inbound','outbound'))`, so every INSERT failed.
//! - the dead-letter UPDATE set `erp_next_attempt_at = NULL` on a `NOT NULL`
//!   column, and the terminal backoff overflowed `INTERVAL`, so a poisoned row
//!   never reached the attempt cap and was retried forever.
//!
//! Both are runtime-checked SQL strings; only a live database catches them.

use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

use invoicd::pg::ReceiptRow;
use invoicd::pg::receipts::{
    DEAD_LETTER_ATTEMPTS, DIRECTION_INBOUND, DIRECTION_OUTBOUND, dead_letter_erp,
    fetch_erp_pending, record_erp_failure, upsert_receipt,
};

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "9900111000002";

async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
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

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply invoicd schema");
    Some((pool, container))
}

fn receipt(process_id: Uuid, direction: &str) -> ReceiptRow {
    ReceiptRow {
        process_id,
        pid: 31002,
        direction: direction.to_owned(),
        sender_mp_id: "9900357000004".to_owned(),
        receiver_gln: TENANT.to_owned(),
        malo_id: Some("51238696012".to_owned()),
        rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
        bo4e_version: "202401.4.0".to_owned(),
        outcome: "Ok".to_owned(),
        findings: serde_json::json!([]),
        pay_by: Some(datetime!(2026-03-01 00:00 UTC)),
        received_at: OffsetDateTime::now_utc(),
        checked_at: OffsetDateTime::now_utc(),
        dispatched_at: None,
        tenant: TENANT.to_owned(),
    }
}

/// Both direction constants satisfy the `invoic_receipts.direction` CHECK — the
/// writers and the schema agree. A capitalised literal rejects every INSERT.
#[tokio::test]
async fn both_direction_constants_pass_the_check() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    for direction in [DIRECTION_INBOUND, DIRECTION_OUTBOUND] {
        let id = Uuid::new_v4();
        upsert_receipt(&pool, &receipt(id, direction))
            .await
            .unwrap_or_else(|e| panic!("{direction} must satisfy the direction CHECK: {e}"));

        let stored: String =
            sqlx::query_scalar("SELECT direction FROM invoic_receipts WHERE process_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("read the receipt back");
        assert_eq!(stored, direction);
    }

    // The capitalised form the writers used before is genuinely rejected, so the
    // constants are load-bearing rather than cosmetic.
    let err = upsert_receipt(&pool, &receipt(Uuid::new_v4(), "Inbound")).await;
    assert!(err.is_err(), "'Inbound' violates the direction CHECK");
}

/// A permanently rejected receipt (4xx from the ERP) leaves the outbox for good.
/// Setting `erp_next_attempt_at = NULL` used to abort the UPDATE — the column is
/// `NOT NULL` — so the row kept being claimed.
#[tokio::test]
async fn a_dead_lettered_receipt_is_never_claimed_again() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("persist the receipt");

    let pending = fetch_erp_pending(&pool, TENANT, 10).await.expect("claim");
    assert_eq!(pending.len(), 1, "a fresh receipt is due immediately");

    dead_letter_erp(&pool, id).await.expect("dead-letter");

    let attempts: i16 =
        sqlx::query_scalar("SELECT erp_attempts FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read attempts");
    assert_eq!(attempts, DEAD_LETTER_ATTEMPTS);

    let pending = fetch_erp_pending(&pool, TENANT, 10).await.expect("claim");
    assert!(
        pending.is_empty(),
        "a dead-lettered row is out of the outbox"
    );
}

/// The retry budget actually terminates: five failures walk `erp_attempts` up to
/// the dead-letter mark. The terminal backoff used to be `i64::MAX / 2` seconds,
/// which PostgreSQL rejects as `interval out of range` — the UPDATE that would
/// have raised the counter past 4 aborted, so the budget never ran out.
#[tokio::test]
async fn the_retry_budget_terminates() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("persist the receipt");

    for attempt in 0..DEAD_LETTER_ATTEMPTS {
        record_erp_failure(&pool, id, attempt)
            .await
            .unwrap_or_else(|e| panic!("attempt {attempt} must be recordable: {e}"));
    }

    let attempts: i16 =
        sqlx::query_scalar("SELECT erp_attempts FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read attempts");
    assert_eq!(attempts, DEAD_LETTER_ATTEMPTS, "the budget is exhausted");

    // Due-time alone would still let it through; the attempt cap is what stops it.
    sqlx::query("UPDATE invoic_receipts SET erp_next_attempt_at = now() WHERE process_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .expect("force the row due");
    let pending = fetch_erp_pending(&pool, TENANT, 10).await.expect("claim");
    assert!(pending.is_empty(), "an exhausted row is out of the outbox");
}
