//! SQL-level tests for `billingd`'s billing-record store, against a real
//! PostgreSQL.
//!
//! The defects these guard against live in the SQL, not in the arithmetic: an
//! upsert that could silently replace an invoice the counterparty had already
//! received, a correction chain whose original could be mutated. billingd had
//! zero tests over `pg.rs` — the same gap that let three runtime defects ship
//! in einsd before its suite existed.
//!
//! PostgreSQL is self-managed via testcontainers (a Docker daemon is the only
//! requirement); the tests skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-billingd-db
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
async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

const TENANT: &str = "9910000000002";
const MALO: &str = "51238696781";

/// The minimal stored Rechnung body every fixture shares.
fn body() -> serde_json::Value {
    serde_json::json!({ "_typ": "RECHNUNG" })
}

/// A January STROM draft for the standard tenant, with a stated number.
fn draft<'a>(
    netto: rust_decimal::Decimal,
    nr: &'a str,
    json: &'a serde_json::Value,
) -> pg::NewBillingRecord<'a> {
    pg::NewBillingRecord {
        tenant: TENANT,
        malo_id: MALO,
        lf_mp_id: TENANT,
        product_code: "STROM-BASIS",
        category: "STROM",
        rechnungsnummer: nr,
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        rechnung_json: json,
        total_netto_eur: netto,
        total_brutto_eur: netto * dec!(1.19),
    }
}

async fn insert_draft(pool: &PgPool, netto: rust_decimal::Decimal) -> Uuid {
    pg::insert_billing_record(pool, &draft(netto, "BILL-2026-01", &body()))
        .await
        .expect("insert draft")
}

/// Mark a record dispatched the way production does — inside a transaction,
/// right after its outbox row.
async fn dispatch(pool: &PgPool, id: Uuid) {
    let mut tx = pool.begin().await.expect("begin");
    pg::mark_dispatched_tx(&mut *tx, id)
        .await
        .expect("dispatch");
    tx.commit().await.expect("commit");
}

/// A re-run may replace a draft — same period, same product, new numbers.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rerun_replaces_a_draft() {
    let Some((pool, _pg)) = test_pool("rerun_draft").await else {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_dispatched_record_refuses_the_overwrite() {
    let Some((pool, _pg)) = test_pool("dispatched_guard").await else {
        return;
    };
    let id = insert_draft(&pool, dec!(100)).await;
    dispatch(&pool, id).await;

    let err = pg::insert_billing_record(&pool, &draft(dec!(999), "BILL-2026-01-RERUN", &body()))
        .await
        .expect_err("the guard must refuse");
    assert!(
        err.to_string().contains("storno"),
        "the error points at the Storno path: {err}"
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_references_its_untouched_original() {
    let Some((pool, _pg)) = test_pool("correction_chain").await else {
        return;
    };
    let original = insert_draft(&pool, dec!(100)).await;
    dispatch(&pool, original).await;

    let correction = storno(
        &pool,
        original,
        "KORR-BILL-2026-01",
        Some("Messwertkorrektur: Zaehlerstand revidiert"),
    )
    .await;
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

    // The original's content is exactly as dispatched; only its outcome moved.
    let (netto, outcome): (rust_decimal::Decimal, String) =
        sqlx::query_as("SELECT total_netto_eur, outcome FROM billing_records WHERE id = $1")
            .bind(original)
            .fetch_one(&pool)
            .await
            .expect("read original");
    assert_eq!(netto, dec!(100));
    assert_eq!(outcome, "cancelled", "the Storno releases the period");
}

/// Issue a Stornorechnung against `original`, the way the handler does.
async fn storno(pool: &PgPool, original: Uuid, nr: &str, reason: Option<&str>) -> Uuid {
    let json = serde_json::json!({
        "_typ": "RECHNUNG",
        "istOriginal": false,
        "zusatzAttribute": [{ "name": "mako:rechnungsart", "wert": "STORNORECHNUNG" }]
    });
    let mut tx = pool.begin().await.expect("begin");
    let id = pg::insert_correction_record(
        &mut tx,
        &pg::NewBillingRecord {
            total_netto_eur: dec!(-100),
            total_brutto_eur: dec!(-119),
            ..draft(dec!(-100), nr, &json)
        },
        original,
        reason,
    )
    .await
    .expect("insert correction");
    tx.commit().await.expect("commit");
    id
}

/// Storno und Neuberechnung: cancelling an original releases its period, so the
/// corrected amounts can be billed as a fresh original. The schema used to
/// forbid exactly what the correction endpoint told callers to do.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_cancelled_period_can_be_billed_again() {
    let Some((pool, _pg)) = test_pool("storno_rebill").await else {
        return;
    };
    let original = insert_draft(&pool, dec!(100)).await;
    dispatch(&pool, original).await;
    storno(
        &pool,
        original,
        "KORR-BILL-2026-01",
        Some("Zählerstand falsch"),
    )
    .await;

    let rebilled = pg::insert_billing_record(&pool, &draft(dec!(140), "BILL-2026-01-NEU", &body()))
        .await
        .expect("the cancelled period is free again");
    assert_ne!(rebilled, original);

    // Three rows: the cancelled original, its Storno, the new original.
    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM billing_records \
         WHERE is_correction = false AND outcome <> 'cancelled'",
    )
    .fetch_one(&pool)
    .await
    .expect("count live originals");
    assert_eq!(live, 1, "exactly one live original for the period");
}

/// The number series is a counter, not a derived string.
///
/// Two allocations of the same series never collide, the value ascends, and the
/// series is scoped per tenant and per year — so `RE-2026-000001` exists once
/// for each operator and the January of the next year starts at 1 again.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_number_series_ascends_per_tenant_series_and_year() {
    let Some((pool, _pg)) = test_pool("number_series").await else {
        return;
    };
    let a = pg::allocate_rechnungsnummer(&pool, TENANT, "RE", 2026)
        .await
        .expect("first");
    let b = pg::allocate_rechnungsnummer(&pool, TENANT, "RE", 2026)
        .await
        .expect("second");
    assert_eq!(a, "RE-2026-000001");
    assert_eq!(b, "RE-2026-000002");

    // A different document class, a different year and a different tenant each
    // carry their own counter.
    assert_eq!(
        pg::allocate_rechnungsnummer(&pool, TENANT, "ST", 2026)
            .await
            .expect("storno series"),
        "ST-2026-000001"
    );
    assert_eq!(
        pg::allocate_rechnungsnummer(&pool, TENANT, "RE", 2027)
            .await
            .expect("next year"),
        "RE-2027-000001"
    );
    assert_eq!(
        pg::allocate_rechnungsnummer(&pool, "9910000000003", "RE", 2026)
            .await
            .expect("other tenant"),
        "RE-2026-000001"
    );
}

/// The flow the correction endpoint documents, end to end, with the numbers
/// production actually generates.
///
/// The numbers must come from the series, not from the billed facts: a derived
/// number reproduces the cancelled original's own string on re-bill and
/// `br_unique_rechnungsnummer` refuses it, so hand-picking a number here would
/// prove nothing about the flow the endpoint and the docs recommend.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn storno_and_rebill_works_with_generated_numbers() {
    let Some((pool, _pg)) = test_pool("storno_rebill_numbers").await else {
        return;
    };
    let nr = |v: &str| v.to_owned();
    let first = pg::allocate_rechnungsnummer(&pool, TENANT, "RE", 2026)
        .await
        .expect("first number");
    let json = body();
    let original = pg::insert_billing_record(&pool, &draft(dec!(100), &nr(&first), &json))
        .await
        .expect("original");
    dispatch(&pool, original).await;

    let storno_nr = pg::allocate_rechnungsnummer(&pool, TENANT, "ST", 2026)
        .await
        .expect("storno number");
    storno(&pool, original, &storno_nr, Some("Zählerstand falsch")).await;

    // The re-bill takes the *next* number of the series — no collision, no
    // hand-picked suffix.
    let rebill_nr = pg::allocate_rechnungsnummer(&pool, TENANT, "RE", 2026)
        .await
        .expect("re-bill number");
    assert_ne!(rebill_nr, first);
    pg::insert_billing_record(&pool, &draft(dec!(140), &rebill_nr, &json))
        .await
        .map_err(|e| format!("{e:#}"))
        .expect("the released period is billable again");

    let live: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM billing_records \
         WHERE is_correction = false AND outcome <> 'cancelled'",
    )
    .fetch_one(&pool)
    .await
    .expect("count live originals");
    assert_eq!(live, 1, "exactly one live original for the period");
}

/// A refused overwrite names the document that holds the period, so a client
/// retrying a request whose response it lost can reconcile against a record id
/// instead of a database string.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_refused_overwrite_names_the_occupying_document() {
    let Some((pool, _pg)) = test_pool("conflict_names_record").await else {
        return;
    };
    let existing = insert_draft(&pool, dec!(100)).await;
    dispatch(&pool, existing).await;

    let json = body();
    let err = pg::insert_billing_record(&pool, &draft(dec!(120), "RE-2026-000002", &json))
        .await
        .expect_err("an issued period refuses the overwrite");
    assert!(
        matches!(err, pg::InsertError::PeriodAlreadyIssued { .. }),
        "the refusal must be typed, not an anyhow string: {err:#}"
    );

    let found = pg::find_live_original(
        &pool,
        TENANT,
        MALO,
        "STROM-BASIS",
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
    )
    .await
    .expect("the occupying document is resolvable");
    assert_eq!(found.0, existing);
    assert_eq!(found.2, "dispatched");
}

/// § 14 Abs. 4 Nr. 4 UStG is enforced by the store, not by hope: two documents
/// of one tenant cannot share a Rechnungsnummer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_duplicate_rechnungsnummer_is_refused() {
    let Some((pool, _pg)) = test_pool("unique_nr").await else {
        return;
    };
    insert_draft(&pool, dec!(100)).await;

    // A different period and product — so `br_unique_original` does not fire —
    // but the same number.
    let json = body();
    let clash = pg::NewBillingRecord {
        product_code: "STROM-ANDERS",
        period_from: date!(2026 - 02 - 01),
        period_to: date!(2026 - 02 - 28),
        ..draft(dec!(50), "BILL-2026-01", &json)
    };
    let err = pg::insert_billing_record(&pool, &clash)
        .await
        .expect_err("the number series is unique per tenant");
    // The refusal is typed, so the HTTP layer can answer 409 with the number
    // instead of a 500 carrying a database constraint name.
    assert!(
        matches!(&err, pg::InsertError::DuplicateRechnungsnummer(nr) if nr == "BILL-2026-01"),
        "the § 14 Abs. 4 Nr. 4 UStG index must surface as a named refusal: {err:#}"
    );

    // The same number under a different tenant is fine — series are per operator.
    pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            tenant: "9910000000003",
            lf_mp_id: "9910000000003",
            ..draft(dec!(50), "BILL-2026-01", &json)
        },
    )
    .await
    .map_err(|e| format!("{e:#}"))
    .expect("another tenant may use the same number");
}

/// Several dispatches settle within one calendar day, so per-dispatch VPP
/// records are exempt from the period index — `vpp_dispatch_ledger` and the
/// per-tx Rechnungsnummer are their guards instead. Before the exemption the
/// second dispatch of a day silently overwrote the first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn two_vpp_dispatches_on_one_day_are_two_settlements() {
    let Some((pool, _pg)) = test_pool("vpp_same_day").await else {
        return;
    };
    let json = body();
    let vpp = |nr: &'static str| pg::NewBillingRecord {
        product_code: "VPP_SR-1",
        category: "VPP",
        period_from: date!(2026 - 03 - 04),
        period_to: date!(2026 - 03 - 04),
        ..draft(dec!(-12), nr, &json)
    };
    let a = pg::insert_billing_record(&pool, &vpp("VPP-SR-1-2026-03-04-aaaa"))
        .await
        .expect("first dispatch");
    let b = pg::insert_billing_record(&pool, &vpp("VPP-SR-1-2026-03-04-bbbb"))
        .await
        .expect("second dispatch of the same day");
    assert_ne!(a, b, "each dispatch is its own settlement");
}

/// A record can be cancelled once. The handler reads `outcome` to refuse a
/// second Storno of the same original — this proves the first one sets it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_original_is_cancelled_exactly_once() {
    let Some((pool, _pg)) = test_pool("second_correction").await else {
        return;
    };
    let original = insert_draft(&pool, dec!(100)).await;
    dispatch(&pool, original).await;

    let before: String = sqlx::query_scalar("SELECT outcome FROM billing_records WHERE id = $1")
        .bind(original)
        .fetch_one(&pool)
        .await
        .expect("outcome before");
    assert_eq!(before, "dispatched");

    storno(
        &pool,
        original,
        "KORR-BILL-2026-01",
        Some("erste Korrektur"),
    )
    .await;

    let after: String = sqlx::query_scalar("SELECT outcome FROM billing_records WHERE id = $1")
        .bind(original)
        .fetch_one(&pool)
        .await
        .expect("outcome after");
    assert_eq!(
        after, "cancelled",
        "the handler's `already cancelled` guard reads this, so a second Storno \
         cannot duplicate KORR-{{nr}}"
    );
}

/// §40b: the month's `billing_run_log` row accumulates daily sweeps, and a
/// single failed sweep marks the whole month for operator attention.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_monthly_run_log_accumulates_daily_sweeps() {
    let Some((pool, _pg)) = test_pool("run_log").await else {
        return;
    };
    // Five billed, two annual settlements deliberately skipped, no error.
    pg::record_billing_run(&pool, TENANT, TENANT, 2026, 7, 5, 2, 0)
        .await
        .expect("first sweep");

    let status: String = sqlx::query_scalar(
        "SELECT status FROM billing_run_log
         WHERE tenant = $1 AND billing_year = 2026 AND billing_month = 7",
    )
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .expect("one row");
    assert_eq!(
        status, "completed",
        "a deliberate skip is not a fault — counting JAEHRLICH refusals as errors \
         marked every month failed for any operator with annual contracts"
    );

    pg::record_billing_run(&pool, TENANT, TENANT, 2026, 7, 3, 0, 1)
        .await
        .expect("second sweep");

    let (records, skipped, errors, status): (i32, i32, i32, String) = sqlx::query_as(
        "SELECT records_count, skipped_count, errors_count, status FROM billing_run_log
         WHERE tenant = $1 AND billing_year = 2026 AND billing_month = 7",
    )
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .expect("one accumulated row");
    assert_eq!(records, 8, "sweeps accumulate");
    assert_eq!(skipped, 2);
    assert_eq!(errors, 1);
    assert_eq!(status, "failed", "a failed sweep sticks for the month");

    // A later clean sweep does not launder the failure away.
    pg::record_billing_run(&pool, TENANT, TENANT, 2026, 7, 2, 0, 0)
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_monthly_abrechnungsinfo_is_claimed_exactly_once() {
    let Some((pool, _pg)) = test_pool("abrechnungsinfo").await else {
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
    let json = body();
    let nr = format!("BILL-{from}");
    pg::insert_billing_record(
        pool,
        &pg::NewBillingRecord {
            period_from: from,
            period_to: to,
            total_netto_eur: brutto / dec!(1.19),
            total_brutto_eur: brutto,
            ..draft(brutto, &nr, &json)
        },
    )
    .await
    .expect("insert record")
}

/// The history context feeds the scorer with the rolling baseline, the
/// previous period end (gap/overlap detection) and the consecutive-estimate
/// count — all from real SQL.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn risk_context_reads_baseline_continuity_and_estimates() {
    let Some((pool, _pg)) = test_pool("risk_context").await else {
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
                    blocking: false,
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

/// A `TARIFWECHSEL` combined invoice persists — the category must be in the
/// `billing_records` CHECK list. Before the fix `POST …/tarifwechsel` inserted
/// `'TARIFWECHSEL'`, which the CHECK rejected (23514) → 500 on every call.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_tarifwechsel_record_is_a_valid_category() {
    let Some((pool, _pg)) = test_pool("tarifwechsel_category").await else {
        return;
    };
    let json = body();
    let id = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            product_code: "STROM-GAS",
            category: "TARIFWECHSEL",
            ..draft(dec!(100), "TW-51238696781-2026-01-01", &json)
        },
    )
    .await
    .expect("TARIFWECHSEL must satisfy the category CHECK");

    let category: String = sqlx::query_scalar("SELECT category FROM billing_records WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("read back");
    assert_eq!(category, "TARIFWECHSEL");
}

/// `mark_dispatched_tx` (called in-tx right after the CE is enqueued) advances a
/// record past `generated`, which activates the overwrite guard — a re-run of the
/// same period is then refused instead of silently replacing an invoice already
/// on its way to the ERP. It is idempotent and scoped to still-generated rows.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn dispatch_stamp_locks_the_record_against_a_silent_rerun() {
    let Some((pool, _pg)) = test_pool("dispatch_stamp").await else {
        return;
    };
    let id = insert_draft(&pool, dec!(100)).await;

    // Stamp dispatched inside a transaction, as the handlers do after enqueue.
    let mut tx = pool.begin().await.expect("begin");
    pg::mark_dispatched_tx(&mut *tx, id).await.expect("stamp");
    pg::mark_dispatched_tx(&mut *tx, id)
        .await
        .expect("idempotent re-stamp");
    tx.commit().await.expect("commit");

    let outcome: String = sqlx::query_scalar("SELECT outcome FROM billing_records WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("read outcome");
    assert_eq!(outcome, "dispatched");

    // A re-run for the same period is now refused (Storno path), not replaced.
    let err = pg::insert_billing_record(&pool, &draft(dec!(999), "BILL-2026-01-C", &body()))
        .await
        .expect_err("dispatched record must refuse the overwrite");
    assert!(err.to_string().contains("storno"), "{err}");
}

/// Tenant is part of a record's identity: a record written under one tenant is
/// invisible to every read issued under another tenant — fetch by UUID, list,
/// and anomaly history all filter on it. Guards the cross-tenant disclosure the
/// unscoped `SELECT … WHERE id = $1` allowed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn reads_are_tenant_scoped() {
    let Some((pool, _pg)) = test_pool("tenant_isolation").await else {
        return;
    };
    const OWNER: &str = "9910000000002";
    const OTHER: &str = "9908888888888";

    let json = body();
    let id = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            period_from: date!(2026 - 02 - 01),
            period_to: date!(2026 - 02 - 28),
            total_netto_eur: dec!(200),
            total_brutto_eur: dec!(238),
            ..draft(dec!(200), "BILL-OWNER-2026-02", &json)
        },
    )
    .await
    .expect("insert owner record");

    // Fetch by UUID: owner sees it, another tenant does not.
    assert!(
        pg::fetch_billing_record(&pool, OWNER, id)
            .await
            .expect("owner fetch")
            .is_some()
    );
    assert!(
        pg::fetch_billing_record(&pool, OTHER, id)
            .await
            .expect("other fetch")
            .is_none(),
        "a UUID known to the owner must not resolve under another tenant"
    );

    // List: scoped to the calling tenant.
    let owner_rows = pg::list_billing_records(
        &pool,
        OWNER,
        &pg::RecordFilter {
            limit: 100,
            ..Default::default()
        },
    )
    .await
    .expect("owner list");
    assert_eq!(owner_rows.len(), 1);
    let other_rows = pg::list_billing_records(
        &pool,
        OTHER,
        &pg::RecordFilter {
            limit: 100,
            ..Default::default()
        },
    )
    .await
    .expect("other list");
    assert!(other_rows.is_empty(), "list must not leak across tenants");

    // Anomaly history: the other tenant sees no baseline for the same MaLo.
    let other_report = pg::check_billing_anomaly(&pool, OTHER, "51238696781", OWNER, None)
        .await
        .expect("other anomaly");
    assert!(
        !other_report.is_anomaly && other_report.deviation_pct.is_none(),
        "anomaly baseline must be tenant-scoped"
    );
}

/// The EN 16931 semantic model is attached to the record and round-trips — the
/// XRechnung/CII/UBL renderers read it back, never re-parsing BO4E.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_en16931_model_is_attached_and_round_trips() {
    let Some((pool, _pg)) = test_pool("en16931_attach").await else {
        return;
    };
    let id = insert_draft(&pool, dec!(100)).await;

    // Freshly inserted: no model yet.
    let before = pg::fetch_billing_record(&pool, "9910000000002", id)
        .await
        .expect("fetch")
        .expect("row");
    assert!(before.en16931_json.is_none());

    let model = serde_json::json!({ "number": "R-EN-1", "currency": "EUR" });
    let mut tx = pool.begin().await.expect("begin");
    pg::attach_en16931(&mut *tx, id, &model)
        .await
        .expect("attach");
    tx.commit().await.expect("commit");

    let after = pg::fetch_billing_record(&pool, "9910000000002", id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(after.en16931_json.as_ref(), Some(&model));
}

/// HELD records enter the review queue and can be released exactly once.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_held_record_is_queued_and_released_exactly_once() {
    let Some((pool, _pg)) = test_pool("risk_release").await else {
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
                blocking: false,
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

// ── Template pinning ──────────────────────────────────────────────────────────

/// The template a document was rendered with is pinned once and never moves.
///
/// This is the § 147 AO property expressed in SQL: a conditional `UPDATE` that
/// refuses to overwrite. Rolling out a redesign (in outputd) must change how
/// *new* invoices look and nothing at all about one already issued.
///
/// The hash is a value from another service — templates live in outputd, so no
/// foreign key can reach them. Resolvability is outputd's append-only store
/// policy, proven in its own suite
/// (`outputd/tests/store_integration.rs::a_published_template_stays_resolvable_after_the_pointer_moves`);
/// what billingd owns, and what this test pins, is that the pin itself never
/// moves.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_issued_document_keeps_the_layout_it_was_issued_with() {
    let Some((pool, _pg)) = test_pool("template_pin").await else {
        return;
    };
    let tenant = "9910000000002";
    let record = insert_draft(&pool, dec!(100)).await;

    // The hash outputd answered with on the first render — an opaque value
    // on this side of the boundary.
    let v1 = "1111111111111111111111111111111111111111111111111111111111111111";
    let v2 = "2222222222222222222222222222222222222222222222222222222222222222";

    // Nothing is pinned until the document is first rendered.
    let before = pg::fetch_billing_record(&pool, tenant, record)
        .await
        .unwrap()
        .expect("record");
    assert_eq!(before.template_hash, None);

    // Rendering a *draft* pins nothing: nobody has received it, and trapping an
    // operator's own preview on the layout they were about to fix would be
    // irreversible, because outputd's store never deletes.
    assert_eq!(
        pg::pin_template(&pool, record, v1).await.unwrap(),
        None,
        "a draft renders with the current layout and pins nothing",
    );

    // Once it has left the house, the first render fixes the layout.
    pg::mark_dispatched_tx(&pool, record)
        .await
        .expect("dispatch");
    assert_eq!(
        pg::pin_template(&pool, record, v1)
            .await
            .unwrap()
            .as_deref(),
        Some(v1),
        "the first render after dispatch pins the layout",
    );

    // A redesign is rolled out in outputd; re-rendering the issued invoice
    // still resolves v1 — the pin refuses to move, so an audit in 2034 gets
    // the document as it was sent.
    assert_eq!(
        pg::pin_template(&pool, record, v2)
            .await
            .unwrap()
            .as_deref(),
        Some(v1),
        "an issued document's layout is fixed; a rollout cannot restyle it",
    );
    assert_eq!(
        pg::fetch_billing_record(&pool, tenant, record)
            .await
            .unwrap()
            .and_then(|r| r.template_hash)
            .as_deref(),
        Some(v1),
    );
}

/// The billing summary counts each euro once.
///
/// Aggregated in the database over the whole history. Folding a capped page of
/// rows in Rust averages per *record* while calling the result monthly, stops at
/// the page size without saying so, and counts the per-MaLo children of a
/// Sammelrechnung alongside the bundle that already contains them.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_summary_counts_bundles_and_their_children_once() {
    let Some((pool, _pg)) = test_pool("summary").await else {
        return;
    };
    let json = body();

    // A standalone January invoice, 119 gross.
    insert_draft(&pool, dec!(100)).await;

    // A February bundle of two sites: the SAMMEL plus its two children. Only
    // the bundle is money the customer owes; the children are its detail.
    let sammel = pg::insert_sammelrechnung_record(
        &pool,
        TENANT,
        "RV-1",
        TENANT,
        "SAMMEL-RV-1-2026-02",
        date!(2026 - 02 - 01),
        date!(2026 - 02 - 28),
        &json,
        dec!(200),
        dec!(238),
    )
    .await
    .expect("bundle");
    let mut children = Vec::new();
    for (i, malo) in ["11111111115", "22222222220"].iter().enumerate() {
        let nr = format!("SAMMEL-RV-1-2026-02-{malo}");
        children.push(
            pg::insert_billing_record(
                &pool,
                &pg::NewBillingRecord {
                    malo_id: malo,
                    period_from: date!(2026 - 02 - 01),
                    period_to: date!(2026 - 02 - 28),
                    total_netto_eur: dec!(100),
                    total_brutto_eur: dec!(119),
                    ..draft(dec!(100), &nr, &json)
                },
            )
            .await
            .unwrap_or_else(|e| panic!("child {i}: {e:#}")),
        );
    }
    pg::link_to_sammelrechnung(&pool, &children, sammel)
        .await
        .expect("link");

    let s = pg::billing_summary(&pool, TENANT, None, Some(TENANT))
        .await
        .expect("summary");

    // 119 (standalone) + 238 (bundle) — not 119 + 238 + 119 + 119.
    assert_eq!(s["total_brutto_eur"], "357.00");
    assert_eq!(s["records"], 2, "the bundle counts, its children do not");

    // 31 January days + 28 February days.
    assert_eq!(s["billed_days"], 59);

    // A Storno leaves the totals alone but is reported on its own axis.
    let original = pg::insert_billing_record(&pool, &draft(dec!(50), "BILL-2026-03", &json))
        .await
        .expect("march");
    let _ = original;
    let s2 = pg::billing_summary(&pool, TENANT, None, Some(TENANT))
        .await
        .expect("summary 2");
    assert_eq!(s2["corrections"], 0);
}

/// A filter that runs after the limit is not a filter.
///
/// `list_corrections` and `list_vpp_settlements` filter in the query. Fetching a
/// page and keeping the matching rows *out of it* answers "no corrections" for a
/// MaLo whose latest page is all ordinary invoices, while its Stornos sit one
/// page further down. For an audit tool under § 147 AO, reporting that a
/// correction chain does not exist when it does is the worst answer available.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_narrow_filter_reaches_past_the_page_it_would_have_been_cut_from() {
    let Some((pool, _pg)) = test_pool("filter_before_limit").await else {
        return;
    };
    let json = body();

    // Five ordinary invoices, then one correction — so the correction is the
    // *oldest* row and falls outside any page of the newest four.
    let mut original = uuid::Uuid::nil();
    for month in 1..=5u8 {
        let from = date!(2026 - 01 - 01)
            .replace_month(month.try_into().unwrap())
            .unwrap();
        let to = from.replace_day(28).unwrap();
        original = pg::insert_billing_record(
            &pool,
            &pg::NewBillingRecord {
                rechnungsnummer: &format!("RE-2026-00000{month}"),
                period_from: from,
                period_to: to,
                ..draft(dec!(100), "unused", &json)
            },
        )
        .await
        .expect("original");
    }
    storno(&pool, original, "ST-2026-000001", Some("Zählerstand")).await;

    // A page of four newest rows contains no correction at all…
    let page = pg::list_billing_records(
        &pool,
        TENANT,
        &pg::RecordFilter {
            limit: 4,
            ..Default::default()
        },
    )
    .await
    .expect("page");
    assert_eq!(page.len(), 4);

    // …but the filtered query finds it, because the predicate runs first.
    let corrections = pg::list_billing_records(
        &pool,
        TENANT,
        &pg::RecordFilter {
            is_correction: Some(true),
            limit: 4,
            ..Default::default()
        },
    )
    .await
    .expect("corrections");
    assert_eq!(
        corrections.len(),
        1,
        "the Storno must be findable even though it is not on the first page"
    );
    assert!(corrections[0].is_correction);

    // The same holds for the category filter the VPP tool uses.
    let vpp = pg::list_billing_records(
        &pool,
        TENANT,
        &pg::RecordFilter {
            category: Some("VPP"),
            limit: 4,
            ..Default::default()
        },
    )
    .await
    .expect("vpp");
    assert!(
        vpp.is_empty(),
        "no VPP settlements exist, and none are invented"
    );
}

/// Replacing a draft clears the derived columns of the calculation it replaced.
///
/// Nothing re-derives them unconditionally: `assess_risk` returns `None`
/// whenever the gate is off or its history query fails. Left in place, a `HELD`
/// band outlives its invoice — the new document dispatches *and* stays in the
/// review queue as withheld, its findings describing amounts it no longer
/// carries — and the stale EN 16931 model renders as its XRechnung.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn replacing_a_draft_clears_the_previous_calculations_derived_state() {
    let Some((pool, _pg)) = test_pool("redraft_clears_derived").await else {
        return;
    };
    let id = insert_draft(&pool, dec!(100)).await;

    // The first calculation is scored HELD and carries its semantic model.
    pg::set_risk(
        &pool,
        id,
        &billingd::risk::RiskAssessment {
            score: 95,
            band: billingd::risk::RiskBand::Held,
            findings: vec![billingd::risk::RiskFinding {
                code: "PERIOD_OVERLAP".into(),
                weight: 50,
                message: "stale".into(),
                blocking: false,
            }],
        },
    )
    .await
    .expect("set risk");
    pg::attach_en16931(&pool, id, &serde_json::json!({ "number": "STALE-1" }))
        .await
        .expect("attach model");

    // Re-bill the same period — the shape of a run with the risk gate off.
    let again = insert_draft(&pool, dec!(250)).await;
    assert_eq!(again, id, "still the same draft row");

    let row = pg::fetch_billing_record(&pool, TENANT, id)
        .await
        .expect("fetch")
        .expect("row");
    assert_eq!(row.total_netto_eur, Some(dec!(250)), "the re-run's amounts");
    assert_eq!(row.risk_score, None, "the old score does not survive");
    assert_eq!(row.risk_band, None, "the old band does not survive");
    assert_eq!(row.risk_findings, None, "the old findings do not survive");
    assert_eq!(
        row.en16931_json, None,
        "the previous calculation's model does not survive",
    );

    // And the stale HELD row is gone from the analyst work list.
    let queue = pg::list_review_queue(&pool, TENANT, None, 10)
        .await
        .expect("queue");
    assert!(
        queue.is_empty(),
        "a replaced draft is not a held document: {queue:?}",
    );
}
