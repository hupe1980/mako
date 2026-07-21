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

/// §14 Abs. 4 Nr. 4 UStG: the handler refuses a second correction of the same
/// original via `count(*) WHERE original_record_id = $1` — this proves that
/// exact detection query sees the first correction.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn a_second_correction_of_the_same_original_is_detected() {
    let Some(pool) = test_pool("second_correction").await else {
        return;
    };
    let original = insert_draft(&pool, dec!(100)).await;
    pg::mark_dispatched(&pool, original, Uuid::new_v4())
        .await
        .expect("dispatch");

    let before: i64 =
        sqlx::query_scalar("SELECT count(*) FROM billing_records WHERE original_record_id = $1")
            .bind(original)
            .fetch_one(&pool)
            .await
            .expect("count before");
    assert_eq!(before, 0, "no correction exists yet");

    pg::insert_correction_record(
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
        Some("erste Korrektur"),
    )
    .await
    .expect("insert first correction");

    let after: i64 =
        sqlx::query_scalar("SELECT count(*) FROM billing_records WHERE original_record_id = $1")
            .bind(original)
            .fetch_one(&pool)
            .await
            .expect("count after");
    assert_eq!(
        after, 1,
        "the guard query the handler runs must see the existing correction \
         so KORR-{{nr}} stays einmalig"
    );
}

/// §40b: the month's `billing_run_log` row accumulates daily sweeps, and a
/// single failed sweep marks the whole month for operator attention.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn the_monthly_run_log_accumulates_daily_sweeps() {
    let Some(pool) = test_pool("run_log").await else {
        return;
    };
    pg::record_billing_run(&pool, "9910000000002", "9910000000002", 2026, 7, 5, 0)
        .await
        .expect("first sweep");
    pg::record_billing_run(&pool, "9910000000002", "9910000000002", 2026, 7, 3, 1)
        .await
        .expect("second sweep");

    let (records, errors, status): (i32, i32, String) = sqlx::query_as(
        "SELECT records_count, errors_count, status FROM billing_run_log
         WHERE tenant = $1 AND billing_year = 2026 AND billing_month = 7",
    )
    .bind("9910000000002")
    .fetch_one(&pool)
    .await
    .expect("one accumulated row");
    assert_eq!(records, 8, "sweeps accumulate");
    assert_eq!(errors, 1);
    assert_eq!(status, "failed", "a failed sweep sticks for the month");

    // A later clean sweep does not launder the failure away.
    pg::record_billing_run(&pool, "9910000000002", "9910000000002", 2026, 7, 2, 0)
        .await
        .expect("third sweep");
    let status: String = sqlx::query_scalar(
        "SELECT status FROM billing_run_log
         WHERE tenant = $1 AND billing_year = 2026 AND billing_month = 7",
    )
    .bind("9910000000002")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "failed");
}

/// §40b Abs. 2: the monthly Abrechnungsinformation is claimed exactly once
/// per MaLo and month — the second daily sweep must not re-send it.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn the_monthly_abrechnungsinfo_is_claimed_exactly_once() {
    let Some(pool) = test_pool("abrechnungsinfo").await else {
        return;
    };
    let first = pg::claim_abrechnungsinfo(&pool, "9910000000002", "51238696781", 2026, 6)
        .await
        .expect("first claim");
    assert!(first, "first sweep claims the month");
    let second = pg::claim_abrechnungsinfo(&pool, "9910000000002", "51238696781", 2026, 6)
        .await
        .expect("second claim");
    assert!(!second, "second sweep must not re-send");
    // A different month claims independently.
    let july = pg::claim_abrechnungsinfo(&pool, "9910000000002", "51238696781", 2026, 7)
        .await
        .expect("july claim");
    assert!(july);
}

// ── Deterministic risk gate ───────────────────────────────────────────────────

async fn insert_period(
    pool: &PgPool,
    from: time::Date,
    to: time::Date,
    brutto: rust_decimal::Decimal,
) -> Uuid {
    pg::insert_billing_record(
        pool,
        "9910000000002",
        "51238696781",
        "9910000000002",
        "STROM-BASIS",
        "STROM",
        from,
        to,
        &serde_json::json!({ "_typ": "RECHNUNG" }),
        brutto / dec!(1.19),
        brutto,
    )
    .await
    .expect("insert record")
}

/// The history context feeds the scorer with the rolling baseline, the
/// previous period end (gap/overlap detection) and the consecutive-estimate
/// count — all from real SQL.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn risk_context_reads_baseline_continuity_and_estimates() {
    let Some(pool) = test_pool("risk_context").await else {
        return;
    };
    let a = insert_period(
        &pool,
        date!(2026 - 04 - 01),
        date!(2026 - 04 - 30),
        dec!(100),
    )
    .await;
    let b = insert_period(
        &pool,
        date!(2026 - 05 - 01),
        date!(2026 - 05 - 31),
        dec!(120),
    )
    .await;

    // Mark both prior invoices as estimate-based via their persisted findings.
    for id in [a, b] {
        pg::set_risk(
            &pool,
            id,
            &billingd::risk::RiskAssessment {
                score: 15,
                band: billingd::risk::RiskBand::AutoReleased,
                findings: vec![billingd::risk::RiskFinding {
                    code: "ESTIMATED_READING".into(),
                    weight: 15,
                    message: "test".into(),
                }],
            },
        )
        .await
        .expect("set risk");
    }

    let ctx = pg::risk_context(&pool, "9910000000002", "51238696781", date!(2026 - 06 - 01))
        .await
        .expect("context");
    assert_eq!(
        ctx.rolling_avg_brutto_eur,
        Some(dec!(110.00)),
        "mean of 100/120"
    );
    assert_eq!(
        ctx.prev_period_to,
        Some(date!(2026 - 05 - 31)),
        "continuity anchor"
    );
    assert_eq!(ctx.recent_estimated_count, 2, "both priors were estimates");
}

/// HELD records enter the review queue and can be released exactly once.
#[tokio::test]
#[ignore = "requires BILLINGD_TEST_DATABASE_URL"]
async fn a_held_record_is_queued_and_released_exactly_once() {
    let Some(pool) = test_pool("risk_release").await else {
        return;
    };
    let id = insert_period(
        &pool,
        date!(2026 - 06 - 01),
        date!(2026 - 06 - 30),
        dec!(500),
    )
    .await;
    pg::set_risk(
        &pool,
        id,
        &billingd::risk::RiskAssessment {
            score: 95,
            band: billingd::risk::RiskBand::Held,
            findings: vec![billingd::risk::RiskFinding {
                code: "PERIOD_OVERLAP".into(),
                weight: 50,
                message: "test".into(),
            }],
        },
    )
    .await
    .expect("set risk");

    let queue = pg::list_review_queue(&pool, "9910000000002", None, 10)
        .await
        .expect("queue");
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].risk_band.as_deref(), Some("HELD"));
    assert_eq!(queue[0].risk_score, Some(95));

    let released = pg::release_held_record(&pool, "9910000000002", id, "analyst@example")
        .await
        .expect("release");
    assert!(released.is_some(), "first release succeeds");
    let again = pg::release_held_record(&pool, "9910000000002", id, "analyst@example")
        .await
        .expect("second release");
    assert!(again.is_none(), "a record releases exactly once");
}
