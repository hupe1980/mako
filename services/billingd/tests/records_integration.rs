//! SQL-level tests for `billingd`'s billing-record store, against a real
//! PostgreSQL.
//!
//! The defects these guard against live in the SQL, not in the arithmetic: an
//! upsert that could silently replace an invoice the counterparty had already
//! received, a correction chain whose original could be mutated. billingd had
//! zero tests over `pg.rs` — the same gap that let three runtime defects ship
//! in einsd before its suite existed.
//!
//! ```bash
//! docker run -d --name billingd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=billingd -p 55435:5432 postgres:17-alpine
//! export BILLINGD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55435/billingd"
//! cargo test -p billingd --test records_integration -- --include-ignored
//! ```
//!
//! Every test provisions its own schema, so they leave nothing behind.

use billingd::pg;
use rust_decimal::dec;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Connect and provision a fresh schema, or skip when no database is configured.
async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("BILLINGD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;

    let schema = format!("t_{test_name}");
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
        .expect("connect to test schema");

    for stmt in split_statements(SCHEMA) {
        sqlx::query(&stmt)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema statement failed: {e}\n{stmt}"));
    }
    Some(pool)
}

/// Split the DDL on `;` at statement level, keeping `$$`-quoted bodies intact.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_dollar = false;
    for line in sql.lines() {
        if line.matches("$$").count() % 2 == 1 {
            in_dollar = !in_dollar;
        }
        current.push_str(line);
        current.push('\n');
        if !in_dollar && line.trim_end().ends_with(';') {
            let stmt = current.trim().to_owned();
            if !stmt.is_empty() && !stmt.lines().all(|l| l.trim().starts_with("--")) {
                out.push(stmt);
            }
            current.clear();
        }
    }
    out
}

async fn insert_draft(pool: &PgPool, netto: rust_decimal::Decimal) -> Uuid {
    pg::insert_billing_record(
        pool,
        "9910000000002",
        "51238696781",
        "9910000000002",
        "STROM-BASIS",
        "STROM",
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
        &serde_json::json!({ "_typ": "RECHNUNG", "gesamtnetto": netto.to_string() }),
        netto,
        netto * dec!(1.19),
    )
    .await
    .expect("insert draft")
}

/// A re-run may replace a draft — same period, same product, new numbers.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn a_rerun_replaces_a_draft() {
    let Some(pool) = test_pool("rerun_draft").await else {
        return;
    };
    let first = insert_draft(&pool, dec!(100)).await;
    let second = insert_draft(&pool, dec!(120)).await;
    assert_eq!(first, second, "same record, updated in place");

    let (count, netto): (i64, rust_decimal::Decimal) =
        sqlx::query_as("SELECT count(*), max(total_netto_eur) FROM billing_records")
            .fetch_one(&pool)
            .await
            .expect("read back");
    assert_eq!(count, 1);
    assert_eq!(netto, dec!(120), "the draft carries the re-run's numbers");
}

/// A dispatched record is never overwritten — the stored Rechnung is what the
/// counterparty received, and a re-run must be told to use the correction path.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn a_dispatched_record_refuses_the_overwrite() {
    let Some(pool) = test_pool("dispatched_guard").await else {
        return;
    };
    let id = insert_draft(&pool, dec!(100)).await;
    pg::mark_dispatched(&pool, id, Uuid::new_v4())
        .await
        .expect("dispatch");

    let err = pg::insert_billing_record(
        &pool,
        "9910000000002",
        "51238696781",
        "9910000000002",
        "STROM-BASIS",
        "STROM",
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
        &serde_json::json!({ "_typ": "RECHNUNG" }),
        dec!(999),
        dec!(999),
    )
    .await
    .expect_err("the guard must refuse");
    assert!(
        err.to_string().contains("correction"),
        "the error points at the correction path: {err}"
    );

    // And the stored record is byte-for-byte what was dispatched.
    let netto: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT total_netto_eur FROM billing_records WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read back");
    assert_eq!(netto, dec!(100));
}

/// A correction is a new row referencing its original; the original survives
/// untouched, and the stated reason is persisted.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn a_correction_references_its_untouched_original() {
    let Some(pool) = test_pool("correction_chain").await else {
        return;
    };
    let original = insert_draft(&pool, dec!(100)).await;
    pg::mark_dispatched(&pool, original, Uuid::new_v4())
        .await
        .expect("dispatch");

    let correction = pg::insert_correction_record(
        &pool,
        "9910000000002",
        "51238696781",
        "9910000000002",
        "STROM-BASIS",
        "STROM",
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
        &serde_json::json!({ "_typ": "RECHNUNG", "rechnungsart": "KORREKTURRECHNUNG" }),
        dec!(-100),
        dec!(-119),
        original,
        Some("Messwertkorrektur: Zaehlerstand revidiert"),
    )
    .await
    .expect("insert correction");
    assert_ne!(correction, original);

    let (is_corr, orig_ref, reason): (bool, Option<Uuid>, Option<String>) = sqlx::query_as(
        "SELECT is_correction, original_record_id, correction_reason \
         FROM billing_records WHERE id = $1",
    )
    .bind(correction)
    .fetch_one(&pool)
    .await
    .expect("read correction");
    assert!(is_corr);
    assert_eq!(orig_ref, Some(original));
    assert_eq!(
        reason.as_deref(),
        Some("Messwertkorrektur: Zaehlerstand revidiert")
    );

    // The original is exactly as dispatched.
    let (netto, outcome): (rust_decimal::Decimal, String) =
        sqlx::query_as("SELECT total_netto_eur, outcome FROM billing_records WHERE id = $1")
            .bind(original)
            .fetch_one(&pool)
            .await
            .expect("read original");
    assert_eq!(netto, dec!(100));
    assert_eq!(outcome, "dispatched");
}
