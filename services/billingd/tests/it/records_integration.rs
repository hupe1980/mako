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
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use billingd::pg;
use rust_decimal::dec;
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../../migrations/0001_schema.sql");

/// Connect and provision a fresh schema, or skip when no database is configured.
async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    // Issuing a document enqueues its event, as `Daemon::migrate` provides.
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
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
        steuerzeilen: &[],
        due_on: None,
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
            steuerzeilen: &[],
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
/// corrected amounts can be billed as a fresh original — the flow the
/// correction endpoint tells callers to follow, so the schema must admit it.
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
        TENANT,
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

/// § 40b Abs. 3: the monthly Abrechnungsinformation is claimed exactly once
/// for each issuer, MaLo and month. The outbox and the claim commit together.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn monthly_information_is_queued_once_per_issuer() {
    let Some((pool, _pg)) = test_pool("abrechnungsinfo").await else {
        return;
    };
    let send = async |issuer, month| {
        let event = mako_service::CloudEvent::new(
            mako_service::source("billingd", TENANT),
            mako_events::billing::ABRECHNUNGSINFORMATION_MONATLICH,
            MALO,
            serde_json::json!({"lf_mp_id":issuer}),
        );
        pg::enqueue_abrechnungsinfo(&pool, TENANT, issuer, MALO, 2026, month, &event).await
    };
    sqlx::query("ALTER TABLE event_outbox ADD CONSTRAINT reject_delivery CHECK (false)")
        .execute(&pool)
        .await
        .unwrap();
    assert!(send(TENANT, 6).await.is_err());
    assert!(
        !pg::abrechnungsinfo_queued(&pool, TENANT, TENANT, MALO, 2026, 6)
            .await
            .unwrap()
    );
    sqlx::query("ALTER TABLE event_outbox DROP CONSTRAINT reject_delivery")
        .execute(&pool)
        .await
        .unwrap();
    let (first, second) = tokio::join!(send(TENANT, 6), send(TENANT, 6));
    assert_ne!(first.unwrap(), second.unwrap());
    assert!(send("9900357000028", 6).await.unwrap());
    assert!(send(TENANT, 7).await.unwrap());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM event_outbox")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
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
            steuerzeilen: &[],
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
/// `billing_records` CHECK list, or `POST …/tarifwechsel` is a 23514 → 500 on
/// every call.
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

    // A re-run for the same period is refused (Storno path), not replaced.
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
            steuerzeilen: &[],
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
/// (`outputd/tests/it/store_integration.rs::a_published_template_stays_resolvable_after_the_pointer_moves`);
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
                    steuerzeilen: &[],
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
    assert_eq!(row.total_netto_eur, dec!(250), "the re-run's amounts");
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

/// A document with no EN 16931 representation is offered on paper only where
/// § 27 Abs. 38 UStG still admits it. A B2B supply in 2028 has no grace left,
/// so the paper answer is refused naming the provision; the same document to a
/// household customer stays on paper, and one whose recipient is unrecorded is
/// refused rather than assumed either way.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_paper_fallback_follows_section_27_abs_38_ustg() {
    let Some((pool, _pg)) = test_pool("paper_fallback").await else {
        return;
    };
    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": TENANT,
        "productd_url": "http://localhost:9080",
        "edmd_url": "http://localhost:8380",
        "marktd_url": "http://localhost:8080"
    }))
    .expect("config parses");

    let code_for = async |nr: &str, unternehmer: Option<bool>| -> &'static str {
        let json = body();
        let mut rec = draft(dec!(100), nr, &json);
        rec.product_code = nr;
        rec.period_from = date!(2028 - 01 - 01);
        rec.period_to = date!(2028 - 01 - 31);
        let id = pg::insert_billing_record(&pool, &rec)
            .await
            .expect("insert");
        pg::attach_en16931_blocked(&pool, id, "BR-O-11: category O beside S")
            .await
            .expect("block");
        pg::attach_empfaenger_unternehmer(&pool, id, unternehmer)
            .await
            .expect("classify");
        match billingd::handlers::record_with_model_am(&pool, &cfg, id, date!(2028 - 02 - 15)).await
        {
            Err(billingd::error::BillingError::Unprocessable { code, .. }) => code,
            other => panic!("expected a 422, got {other:?}"),
        }
    };

    assert_eq!(code_for("B2B-2028", Some(true)).await, "PAPIER_UNZULAESSIG");
    assert_eq!(
        code_for("B2C-2028", Some(false)).await,
        "MODEL_NOT_REPRESENTABLE"
    );
    assert_eq!(code_for("UNKNOWN-2028", None).await, "PAPIER_UNZULAESSIG");
}

/// § 27 Abs. 38 Nr. 2 UStG reads the Gesamtumsatz of the record's own
/// Rechnungsaussteller: a 2027 supply by an issuer whose tax identity carries
/// a 2026 figure under 800 000 EUR stays on paper, and the same document by an
/// issuer with no entry — or with no recorded identity — is refused rather
/// than admitted on another issuer's turnover.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_nr2_turnover_is_the_record_issuers_own() {
    let Some((pool, _pg)) = test_pool("paper_fallback_issuer").await else {
        return;
    };
    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": TENANT,
        "productd_url": "http://localhost:9080",
        "edmd_url": "http://localhost:8380",
        "marktd_url": "http://localhost:8080",
        "e_rechnung": { "gesamtumsatz": [
            { "ust_id": "DE123456789", "jahr": 2026, "eur": "500000" }
        ]}
    }))
    .expect("config parses");

    let code_for = async |nr: &str, ust_id: Option<&str>| -> &'static str {
        let json = body();
        let mut rec = draft(dec!(100), nr, &json);
        rec.product_code = nr;
        rec.period_from = date!(2027 - 06 - 01);
        rec.period_to = date!(2027 - 06 - 30);
        let id = pg::insert_billing_record(&pool, &rec)
            .await
            .expect("insert");
        pg::attach_en16931_blocked(&pool, id, "BR-O-11: category O beside S")
            .await
            .expect("block");
        pg::attach_empfaenger_unternehmer(&pool, id, Some(true))
            .await
            .expect("classify");
        pg::attach_aussteller(&pool, id, ust_id, None)
            .await
            .expect("issuer");
        match billingd::handlers::record_with_model_am(&pool, &cfg, id, date!(2027 - 07 - 15)).await
        {
            Err(billingd::error::BillingError::Unprocessable { code, .. }) => code,
            other => panic!("expected a 422, got {other:?}"),
        }
    };

    assert_eq!(
        code_for("NR2-OWN", Some("DE123456789")).await,
        "MODEL_NOT_REPRESENTABLE"
    );
    assert_eq!(
        code_for("NR2-OTHER", Some("DE999999998")).await,
        "PAPIER_UNZULAESSIG"
    );
    assert_eq!(code_for("NR2-NONE", None).await, "PAPIER_UNZULAESSIG");
}

/// The analyst work list holds what is still undecided: a reviewed REVIEW
/// record and a released HELD record have left it, so a Heute fed by it does
/// not keep offering a decision somebody already took. The review is the
/// REVIEW band's alone and names its reviewer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_decided_record_leaves_the_review_queue() {
    let Some((pool, _pg)) = test_pool("review_queue_decided").await else {
        return;
    };
    let json = body();
    let review = pg::insert_billing_record(&pool, &draft(dec!(100), "BILL-R", &json))
        .await
        .expect("insert review");
    let mut held_rec = draft(dec!(200), "BILL-H", &json);
    held_rec.period_from = date!(2026 - 02 - 01);
    held_rec.period_to = date!(2026 - 02 - 28);
    let held = pg::insert_billing_record(&pool, &held_rec)
        .await
        .expect("insert held");
    for (id, band) in [(review, "REVIEW"), (held, "HELD")] {
        sqlx::query("UPDATE billing_records SET risk_band = $2, risk_score = 50 WHERE id = $1")
            .bind(id)
            .bind(band)
            .execute(&pool)
            .await
            .expect("band");
    }
    let queued = |pool: PgPool| async move {
        let mut ids: Vec<Uuid> = pg::list_review_queue(&pool, TENANT, None, 50)
            .await
            .expect("queue")
            .into_iter()
            .map(|r| r.id)
            .collect();
        ids.sort();
        ids
    };
    let mut both = vec![review, held];
    both.sort();
    assert_eq!(queued(pool.clone()).await, both);

    // A HELD record is released, not reviewed.
    assert_eq!(
        pg::mark_reviewed(&pool, TENANT, held, "anna")
            .await
            .expect("review"),
        None
    );
    assert_eq!(
        pg::mark_reviewed(&pool, TENANT, review, "anna")
            .await
            .expect("review"),
        Some(review)
    );
    assert_eq!(
        pg::mark_reviewed(&pool, TENANT, review, "bert")
            .await
            .expect("again"),
        None,
        "a review is recorded once"
    );
    pg::release_held_record(&pool, TENANT, held, "carla")
        .await
        .expect("release")
        .expect("held");
    assert!(queued(pool.clone()).await.is_empty());

    let by: Option<String> =
        sqlx::query_scalar("SELECT reviewed_by FROM billing_records WHERE id = $1")
            .bind(review)
            .fetch_one(&pool)
            .await
            .expect("reviewer");
    assert_eq!(by.as_deref(), Some("anna"));
}

/// **Invariant: an invoice's Strom- and Energiesteuer lines are stored typed by
/// the statement that stores the invoice, and a Storno nets them to zero.**
///
/// The Steueranmeldung (§ 8 StromStG) is a sum over these rows, grouped by
/// ground. A line left inside the invoice JSON is a line no declaration
/// counts; a Storno that carried none would leave the cancelled supply
/// declared; a re-run of a draft that kept the old lines beside the new
/// would declare the supply twice.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn tax_lines_are_stored_typed_and_a_storno_nets_them_to_zero() {
    use billingd::steuerzeilen::{Art, Steuer, Steuerzeile};
    let Some((pool, _pg)) = test_pool("steuerzeilen").await else {
        return;
    };
    let zeilen = [
        Steuerzeile {
            steuer: Steuer::Stromsteuer,
            art: Art::Regelsatz,
            rechtsgrundlage: "§ 3 StromStG".to_owned(),
            menge: dec!(1000),
            einheit: "kWh".to_owned(),
            satz_ct: dec!(2.05),
            betrag_eur: dec!(20.50),
        },
        Steuerzeile {
            steuer: Steuer::Stromsteuer,
            art: Art::Ermaessigung,
            rechtsgrundlage: "§ 9 Abs. 2 Nr. 1 StromStG".to_owned(),
            menge: dec!(200),
            einheit: "kWh".to_owned(),
            satz_ct: dec!(1.142),
            betrag_eur: dec!(2.284),
        },
    ];
    let json = body();
    let record = |zeilen| pg::NewBillingRecord {
        steuerzeilen: zeilen,
        ..draft(dec!(100), "BILL-2026-01", &json)
    };

    // A re-run of the draft replaces its lines rather than adding to them.
    pg::insert_billing_record(&pool, &record(&zeilen[..1]))
        .await
        .expect("first draft");
    let original = pg::insert_billing_record(&pool, &record(&zeilen))
        .await
        .expect("re-run");
    let stored = pg::steuerzeilen_of(&pool, TENANT, original)
        .await
        .expect("read back");
    assert_eq!(stored.len(), 2, "two grounds, two rows: {stored:?}");
    assert_eq!(
        (
            stored[1].art.as_str(),
            stored[1].rechtsgrundlage.as_str(),
            stored[1].menge
        ),
        ("ERMAESSIGUNG", "§ 9 Abs. 2 Nr. 1 StromStG", dec!(200))
    );
    assert!(stored.iter().all(|z| z.period_from == date!(2026 - 01 - 01)
        && z.period_to == date!(2026 - 01 - 31)));

    dispatch(&pool, original).await;
    let st = storno(&pool, original, "ST-2026-000001", Some("Ablesefehler")).await;
    let negated = pg::steuerzeilen_of(&pool, TENANT, st).await.expect("read");
    assert_eq!(negated.len(), 2, "{negated:?}");

    let netted: Vec<(String, String, rust_decimal::Decimal, rust_decimal::Decimal)> =
        sqlx::query_as(
            "SELECT steuer, art, sum(menge), sum(betrag_eur) FROM steuerzeilen
             GROUP BY steuer, art ORDER BY art",
        )
        .fetch_all(&pool)
        .await
        .expect("net");
    assert_eq!(netted.len(), 2);
    for (steuer, art, menge, betrag) in netted {
        assert_eq!(steuer, "STROMSTEUER");
        assert!(
            menge.is_zero() && betrag.is_zero(),
            "{art}: the Storno must take back what the original declared ({menge} kWh, {betrag} EUR)"
        );
    }
}

/// The `steuerzeilen` CHECK lists and the Rust enums that write them agree in
/// both directions: a variant the CHECK omits fails every insert carrying it,
/// and a CHECK value no variant writes is a ground no line can have.
#[test]
fn the_tax_line_checks_match_the_enums() {
    use billingd::steuerzeilen::{Art, Steuer};
    let schema = SCHEMA.split_whitespace().collect::<Vec<_>>().join(" ");
    let check = |column: &str| -> Vec<String> {
        let start = schema
            .find(&format!("{column} TEXT NOT NULL CHECK ({column} IN ("))
            .unwrap_or_else(|| panic!("steuerzeilen.{column} carries a CHECK list"));
        let list = &schema[start..];
        let list = &list[list.find("IN (").expect("IN") + 4..list.find("))").expect("))")];
        list.split(',')
            .map(|v| v.trim().trim_matches('\'').to_owned())
            .collect()
    };
    assert_eq!(
        check("steuer"),
        Steuer::ALL.map(|s| s.as_str().to_owned()).to_vec()
    );
    assert_eq!(
        check("art"),
        Art::ALL.map(|a| a.as_str().to_owned()).to_vec()
    );
}

/// **Invariant: a period's Gebühr is stored as a Gebührenbescheid record beside
/// its supply invoice, under one run.** The Bescheid carries its view and no
/// EN 16931 model, occupies its own slot of `br_unique_original`, and the
/// schema refuses a Bescheid without a view.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_gebuehr_is_stored_as_a_bescheid_beside_its_invoice() {
    use energy_billing::{
        BillingContext, BillingPeriod, GridInput, InvoiceType, Product, Quantities,
        Rechnungsempfaenger, RegulatoryRates, WasserMeterInput,
    };
    let Some((pool, _pg)) = test_pool("gebuehrenbescheid").await else {
        return;
    };
    let tariff: Product = serde_json::from_str(
        r#"{"category":"WASSER","wasser_mengenpreis_eur_per_m3":"2.10",
             "schmutzwasser_eur_per_m3":"2.60","abwasser_regime":"PUBLIC_LAW_FEE"}"#,
    )
    .expect("a WASSER product");
    let rates = RegulatoryRates::default();
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            BillingContext {
                malo_id: MALO.to_owned(),
                lf_mp_id: TENANT.to_owned(),
                rechnungsnummer: "RE-2026-000001".to_owned(),
                period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                    .expect("a period"),
                invoice_type: InvoiceType::Initial,
                regulatory_rates: rates.clone(),
                rechnungsempfaenger: Some(Rechnungsempfaenger {
                    name: Some("Erika Mustermann".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            &Quantities {
                wasser: Some(WasserMeterInput {
                    frischwasser_m3: dec!(120),
                    months: Some(dec!(1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .expect("bills");
    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": TENANT,
        "productd_url": "http://localhost:9080",
        "edmd_url": "http://localhost:8380",
        "marktd_url": "http://localhost:8080",
        "gebuehrenbescheid": {
            "rechtsbehelf": "Widerspruch",
            "frist": "innerhalb eines Monats nach Bekanntgabe",
            "behoerde": "Stadtwerke Musterstadt",
            "zahlungsfrist_tage": 30,
        },
    }))
    .expect("config");

    let mut belege = billingd::gebuehrenbescheid::trennen(invoice);
    billingd::gebuehrenbescheid::eine_lauf_id(&mut belege);
    let lieferung = belege.lieferung.expect("supply");
    let mut gebuehr = belege.gebuehr.expect("Gebühr");
    gebuehr.context.rechnungsnummer = "GB-2026-000001".to_owned();
    let view = billingd::gebuehrenbescheid::view(&gebuehr, &cfg).expect("view");

    let mut tx = pool.begin().await.expect("begin");
    let json = lieferung.to_rechnung_json();
    let supply = pg::insert_billing_record(
        &mut *tx,
        &pg::NewBillingRecord {
            product_code: "WASSER-BASIS",
            category: "WASSER",
            rechnungsnummer: "RE-2026-000001",
            total_netto_eur: lieferung.netto_eur,
            total_brutto_eur: lieferung.brutto_eur,
            ..draft(dec!(0), "RE-2026-000001", &json)
        },
    )
    .await
    .expect("supply invoice");
    let bescheid = billingd::gebuehrenbescheid::speichern(
        &mut tx,
        &cfg,
        &gebuehr,
        &view,
        "WASSER-BASIS",
        "WASSER",
        false,
    )
    .await
    .expect("Bescheid beside it");
    tx.commit().await.expect("commit");

    let b = pg::fetch_billing_record(&pool, TENANT, bescheid)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(b.dokumentart, "GEBUEHRENBESCHEID");
    assert_eq!(b.product_code, "WASSER-BASIS:GEBUEHR");
    assert!(b.en16931_json.is_none() && b.en16931_blocked.is_some());
    assert_eq!(b.bescheid_json.as_ref(), Some(&view));
    assert_eq!(b.outcome, "dispatched");
    let s = pg::fetch_billing_record(&pool, TENANT, supply)
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(s.dokumentart, "RECHNUNG");
    let run = |r: &serde_json::Value| {
        r["zusatzAttribute"]
            .as_array()
            .and_then(|a| a.iter().find(|z| z["name"] == "mako:billing_run_id"))
            .map(|z| z["wert"].clone())
    };
    assert!(run(&s.rechnung_json).is_some());
    assert_eq!(run(&s.rechnung_json), run(&b.rechnung_json), "one run");

    let refused =
        sqlx::query("UPDATE billing_records SET dokumentart = 'GEBUEHRENBESCHEID' WHERE id = $1")
            .bind(supply)
            .execute(&pool)
            .await;
    assert!(refused.is_err(), "a Bescheid without its view is refused");
}

/// **Invariant: the § 40b run sends a period's Gebührenbescheid exactly as it
/// sends the invoice beside it** — through outputd's document endpoint, on the
/// same channels to the same recipient. A Bescheid stored and never sent
/// starts no Rechtsbehelfsfrist.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_mixed_periods_run_delivers_both_documents() {
    use energy_billing::{
        BillingContext, BillingPeriod, GridInput, InvoiceType, Product, Quantities,
        Rechnungsempfaenger, RegulatoryRates, WasserMeterInput,
    };
    use std::sync::{Arc, Mutex};
    let Some((pool, _pg)) = test_pool("bescheid_versand").await else {
        return;
    };

    // An outputd that records every document it is asked to issue.
    let issued: Arc<Mutex<Vec<(String, serde_json::Value)>>> = Arc::default();
    let seen = Arc::clone(&issued);
    let app = axum::Router::new().route(
        "/api/v1/documents/issue/{kind}",
        axum::routing::post(
            move |axum::extract::Path(kind): axum::extract::Path<String>,
                  axum::Json(body): axum::Json<serde_json::Value>| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.lock().expect("lock").push((kind, body));
                    axum::Json(serde_json::json!({
                        "document_id": uuid::Uuid::new_v4(),
                        "template_hash": "a".repeat(64),
                    }))
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let outputd = format!("http://{}", listener.local_addr().expect("addr"));
    tokio::spawn(async move { axum::serve(listener, app).await });

    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": TENANT,
        "productd_url": "http://127.0.0.1:9",
        "edmd_url": "http://127.0.0.1:9",
        "marktd_url": "http://127.0.0.1:9",
        "vertragd_url": "http://127.0.0.1:9",
        "outputd_url": outputd,
        "seller_name": "Stadtwerke Musterstadt",
        "seller_vat_id": "DE123456789",
        "gebuehrenbescheid": {
            "rechtsbehelf": "Widerspruch",
            "frist": "innerhalb eines Monats nach Bekanntgabe",
            "behoerde": "Stadtwerke Musterstadt",
            "zahlungsfrist_tage": 30,
        },
    }))
    .expect("config");
    let tariff: Product = serde_json::from_str(
        r#"{"category":"WASSER","wasser_mengenpreis_eur_per_m3":"2.10",
             "schmutzwasser_eur_per_m3":"2.60","abwasser_regime":"PUBLIC_LAW_FEE"}"#,
    )
    .expect("a WASSER product");
    let rates = RegulatoryRates::default();
    let invoice = tariff
        .build_engine(&GridInput::default(), &rates)
        .bill(
            BillingContext {
                malo_id: MALO.to_owned(),
                lf_mp_id: TENANT.to_owned(),
                rechnungsnummer: "RE-2026-000001".to_owned(),
                period: BillingPeriod::new(date!(2026 - 01 - 01), date!(2026 - 01 - 31))
                    .expect("a period"),
                invoice_type: InvoiceType::Initial,
                regulatory_rates: rates.clone(),
                rechnungsempfaenger: Some(Rechnungsempfaenger {
                    name: Some("Erika Mustermann".to_owned()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            &Quantities {
                wasser: Some(WasserMeterInput {
                    frischwasser_m3: dec!(120),
                    months: Some(dec!(1)),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .expect("bills");
    let mut belege = billingd::gebuehrenbescheid::trennen(invoice);
    billingd::gebuehrenbescheid::eine_lauf_id(&mut belege);
    let lieferung = belege.lieferung.expect("supply");
    let mut gebuehr = belege.gebuehr.expect("Gebühr");
    gebuehr.context.rechnungsnummer = "GB-2026-000001".to_owned();
    let view = billingd::gebuehrenbescheid::view(&gebuehr, &cfg).expect("view");

    // The run's write: the invoice issued with its e-invoice model, the
    // Bescheid beside it.
    let mut tx = pool.begin().await.expect("begin");
    let json = lieferung.to_rechnung_json();
    let supply = pg::insert_billing_record(
        &mut *tx,
        &pg::NewBillingRecord {
            product_code: "WASSER-BASIS",
            category: "WASSER",
            rechnungsnummer: "RE-2026-000001",
            total_netto_eur: lieferung.netto_eur,
            total_brutto_eur: lieferung.brutto_eur,
            ..draft(dec!(0), "RE-2026-000001", &json)
        },
    )
    .await
    .expect("supply invoice");
    pg::mark_dispatched_tx(&mut *tx, supply)
        .await
        .expect("issued");
    billingd::einvoice::store(&mut tx, supply, &lieferung, &cfg, MALO)
        .await
        .expect("the supply invoice has its e-invoice");
    let bescheid = billingd::gebuehrenbescheid::speichern(
        &mut tx,
        &cfg,
        &gebuehr,
        &view,
        "WASSER-BASIS",
        "WASSER",
        false,
    )
    .await
    .expect("Bescheid");
    tx.commit().await.expect("commit");

    let deps = Arc::new(billingd::clients::BillingDeps {
        cfg: Arc::new(cfg),
        productd: Arc::new(billingd::clients::ProductdClient::new(
            "http://127.0.0.1:9",
            None,
        )),
        edmd: Arc::new(billingd::clients::EdmdClient::new(
            "http://127.0.0.1:9",
            None,
        )),
        marktd: Arc::new(mako_markt::marktd_client::MarktdClient::new(
            "http://127.0.0.1:9",
            secrecy::SecretString::from(String::new()),
            reqwest::Client::new(),
        )),
        vertragd: Arc::new(billingd::clients::VertragdClient::new(
            "http://127.0.0.1:9",
            None,
        )),
        outputd: Arc::new(billingd::clients::OutputdClient::new(&outputd, None)),
        accountingd: None,
    });
    billingd::billing_runs::versenden(&pool, &deps, MALO, &[supply, bescheid]).await;

    let issued = issued.lock().expect("lock").clone();
    let kinds: Vec<&str> = issued.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(kinds, vec!["INVOICE", "GEBUEHRENBESCHEID"], "both are sent");
    let (_, rechnung) = &issued[0];
    let (_, bescheid_body) = &issued[1];
    assert_eq!(bescheid_body["subject_ref"], "GB-2026-000001");
    assert_eq!(bescheid_body["view"], view);
    for key in ["channels", "recipient", "malo_id"] {
        assert_eq!(
            rechnung[key], bescheid_body[key],
            "same {key} as the invoice"
        );
    }
    for id in [supply, bescheid] {
        let row = pg::fetch_billing_record(&pool, TENANT, id)
            .await
            .expect("read")
            .expect("stored");
        assert!(row.template_hash.is_some(), "the sent layout is pinned");
    }
}

// ── § 40c Abs. 2 EnWG watch ─────────────────────────────────────────────

fn unbilled<'a>(reason: &'a str) -> pg::NewUnbilledPeriod<'a> {
    pg::NewUnbilledPeriod {
        tenant: TENANT,
        malo_id: MALO,
        lf_mp_id: TENANT,
        period_from: date!(2026 - 01 - 01),
        period_to: date!(2026 - 01 - 31),
        final_invoice: false,
        reason_code: reason,
        due_on: date!(2026 - 03 - 14),
    }
}

/// A period the run could not bill is one row with its refusal and its
/// § 40c date; the invoice written for it later closes the row, and a Storno of
/// that invoice opens it again.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unbilled_period_closes_on_its_invoice_and_reopens_on_storno() {
    let Some((pool, _pg)) = test_pool("unbilled_watch").await else {
        return;
    };
    pg::record_unbilled_period(&pool, &unbilled("RATE_REFUSED"))
        .await
        .expect("record");
    pg::record_unbilled_period(&pool, &unbilled("RATE_REFUSED"))
        .await
        .expect("a second failed run refreshes");
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .expect("list");
    assert_eq!(open.len(), 1, "one row per period, however often it fails");
    assert_eq!(open[0].reason_code, "RATE_REFUSED");
    assert_eq!(open[0].due_on, date!(2026 - 03 - 14));
    assert_eq!(open[0].attempts, 2);

    // The rate is loaded; the next run bills the period.
    let json = body();
    let id = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            due_on: Some(date!(2026 - 03 - 14)),
            ..draft(dec!(100), "RE-2026-000001", &json)
        },
    )
    .await
    .expect("bill");
    assert_eq!(
        pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .expect("list")
            .len(),
        1,
        "a record the risk gate holds has reached nobody — the case stays open"
    );
    dispatch(&pool, id).await;
    assert!(
        pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .expect("list")
            .is_empty(),
        "the invoice closes the case"
    );
    let all = pg::list_unbilled_periods(&pool, TENANT, false, 50)
        .await
        .expect("list");
    assert_eq!(
        all[0].settled_by,
        Some(id),
        "with that invoice as its outcome"
    );

    storno(&pool, id, "ST-2026-000001", Some("Zählerstand falsch")).await;
    let reopened = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .expect("list");
    assert_eq!(reopened.len(), 1, "a Storno re-opens the period's watch");
    assert_eq!(reopened[0].reason_code, "STORNIERT");
    assert_eq!(reopened[0].due_on, date!(2026 - 03 - 14));
}

async fn covers_period(pool: &PgPool, p: &pg::NewUnbilledPeriod<'_>) -> bool {
    pg::billing_record_exists_for_period(
        pool,
        p.tenant,
        p.malo_id,
        p.lf_mp_id,
        p.period_from,
        p.period_to,
    )
    .await
    .expect("sweep idempotency probe")
}

/// Each issuer owns its refusal, issued original and replacement even when another
/// issuer bills the same MaLo and dates. nothing-unbilled/SR-111.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn supply_periods_are_isolated_by_issuer() {
    let Some((pool, _pg)) = test_pool("supply_period_issuers").await else {
        return;
    };
    let a = unbilled("NO_METER_DATA");
    let b = pg::NewUnbilledPeriod {
        lf_mp_id: "9900357000028",
        reason_code: "RATE_REFUSED",
        ..a
    };
    pg::record_unbilled_period(&pool, &a).await.unwrap();
    pg::record_unbilled_period(&pool, &b).await.unwrap();
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 2);
    assert!(open.iter().all(|p| p.attempts == 1));

    let json = body();
    let a_record = pg::NewBillingRecord {
        due_on: Some(a.due_on),
        ..draft(dec!(100), "ISSUER-A", &json)
    };
    let id_a = pg::insert_billing_record(&pool, &a_record).await.unwrap();
    assert!(
        covers_period(&pool, &a).await,
        "the held original reserves A's period"
    );
    assert!(
        !covers_period(&pool, &b).await,
        "B still needs its own original"
    );
    assert_eq!(
        pg::latest_billed_to(&pool, TENANT, MALO, b.lf_mp_id)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .unwrap()
            .len(),
        2
    );
    dispatch(&pool, id_a).await;
    pg::record_unbilled_period(&pool, &a).await.unwrap();
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].lf_mp_id, b.lf_mp_id);
    assert_eq!(open[0].reason_code, "RATE_REFUSED");

    let id_b = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            lf_mp_id: b.lf_mp_id,
            rechnungsnummer: "ISSUER-B",
            ..a_record
        },
    )
    .await
    .unwrap();
    dispatch(&pool, id_b).await;
    for (issuer, expected) in [(a.lf_mp_id, id_a), (b.lf_mp_id, id_b)] {
        let found = pg::find_live_original(
            &pool,
            TENANT,
            MALO,
            issuer,
            "STROM-BASIS",
            a.period_from,
            a.period_to,
        )
        .await
        .unwrap();
        assert_eq!(
            found.0, expected,
            "a retry receives its issuer's occupying record"
        );
    }
    storno(&pool, id_a, "ST-A", None).await;
    assert!(!covers_period(&pool, &a).await);
    assert!(covers_period(&pool, &b).await);
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(
        (&*open[0].lf_mp_id, &*open[0].reason_code),
        (a.lf_mp_id, "STORNIERT")
    );

    let replacement = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            rechnungsnummer: "ISSUER-A2",
            ..a_record
        },
    )
    .await
    .unwrap();
    dispatch(&pool, replacement).await;
    let all = pg::list_unbilled_periods(&pool, TENANT, false, 50)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert!(
        all.iter()
            .any(|p| p.lf_mp_id == a.lf_mp_id && p.settled_by == Some(replacement))
    );
    assert!(
        all.iter()
            .any(|p| p.lf_mp_id == b.lf_mp_id && p.settled_by == Some(id_b))
    );
}

/// A refusal can arrive while the issuing transaction still holds its invoice.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_concurrent_first_refusal_observes_the_committed_invoice() {
    let Some((pool, _pg)) = test_pool("supply_period_concurrent").await else {
        return;
    };
    let p = unbilled("RATE_REFUSED");
    let json = serde_json::json!({"rechnungstyp": "ABSCHLUSSRECHNUNG"});
    let id = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            due_on: Some(p.due_on),
            ..draft(dec!(100), "CONCURRENT-1", &json)
        },
    )
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    pg::mark_dispatched_tx(&mut *tx, id).await.unwrap();
    pg::record_unbilled_period(&pool, &p).await.unwrap();
    assert_eq!(
        pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .unwrap()
            .len(),
        1
    );
    tx.commit().await.unwrap();
    assert!(
        pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .unwrap()
            .is_empty()
    );
    storno(&pool, id, "ST-CONCURRENT", None).await;
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
    assert!(
        open[0].final_invoice,
        "the reversed invoice determines the kind"
    );
}

/// Operational refusal history expires only after the newest live issuance.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn settled_watch_cleanup_preserves_open_obligations_and_originals() {
    let Some((pool, _pg)) = test_pool("watch_retention").await else {
        return;
    };
    let p = unbilled("RATE_REFUSED");
    pg::record_unbilled_period(&pool, &p).await.unwrap();
    let invalid = pg::NewUnbilledPeriod {
        reason_code: "customer details",
        ..p
    };
    assert!(pg::record_unbilled_period(&pool, &invalid).await.is_err());
    let other = pg::NewUnbilledPeriod {
        lf_mp_id: "9900357000028",
        ..p
    };
    pg::record_unbilled_period(&pool, &other).await.unwrap();
    let json = body();
    let record = pg::NewBillingRecord {
        due_on: Some(p.due_on),
        ..draft(dec!(100), "RET-1", &json)
    };
    let older = pg::insert_billing_record(&pool, &record).await.unwrap();
    dispatch(&pool, older).await;
    sqlx::query(
        "UPDATE billing_records SET dispatched_at = now() - interval '90 days' WHERE id = $1",
    )
    .bind(older)
    .execute(&pool)
    .await
    .unwrap();
    let newer = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            product_code: "SECOND",
            rechnungsnummer: "RET-2",
            ..record
        },
    )
    .await
    .unwrap();
    dispatch(&pool, newer).await;
    assert_eq!(pg::purge_settled_unbilled(&pool, TENANT).await.unwrap(), 0);
    storno(&pool, newer, "ST-RET-2", None).await;
    assert_eq!(
        pg::purge_settled_unbilled(&pool, "another-tenant")
            .await
            .unwrap(),
        0
    );

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM billing_records WHERE id = $1 FOR UPDATE")
        .bind(older)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        pg::purge_settled_unbilled(&pool, TENANT).await.unwrap(),
        0,
        "skip a busy invoice"
    );
    tx.commit().await.unwrap();
    assert_eq!(pg::purge_settled_unbilled(&pool, TENANT).await.unwrap(), 1);
    assert!(
        pg::fetch_billing_record(&pool, TENANT, older)
            .await
            .unwrap()
            .is_some()
    );
    let retained = pg::list_unbilled_periods(&pool, TENANT, false, 50)
        .await
        .unwrap();
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].lf_mp_id, other.lf_mp_id);
    storno(&pool, older, "ST-RET-1", None).await;
    let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
        .await
        .unwrap();
    assert_eq!(open.len(), 2);
    assert!(
        open.iter()
            .any(|r| r.lf_mp_id == p.lf_mp_id && r.reason_code == "STORNIERT")
    );
}

/// A settlement, bundle, child or Bescheid cannot discharge a supply invoice.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn only_a_supply_original_can_cover_the_period() {
    let Some((pool, _pg)) = test_pool("supply_period_documents").await else {
        return;
    };
    let p = unbilled("NO_METER_DATA");
    pg::record_unbilled_period(&pool, &p).await.unwrap();
    let json = body();
    for (kind, category) in [
        ("virtual", "VPP"),
        ("bundle", "SAMMEL"),
        ("child", "STROM"),
        ("notice", "WASSER"),
        ("no-due-date", "STROM"),
    ] {
        let id = pg::insert_billing_record(
            &pool,
            &pg::NewBillingRecord {
                category,
                product_code: kind,
                due_on: (kind != "no-due-date").then_some(p.due_on),
                ..draft(dec!(100), kind, &json)
            },
        )
        .await
        .unwrap();
        if kind == "child" {
            sqlx::query("UPDATE billing_records SET sammelrechnung_id = (SELECT id FROM billing_records WHERE tenant = $2 AND rechnungsnummer = 'bundle') WHERE id = $1")
                .bind(id).bind(TENANT).execute(&pool).await.unwrap();
        } else if kind == "notice" {
            sqlx::query("UPDATE billing_records SET dokumentart = 'GEBUEHRENBESCHEID', bescheid_json = '{}'::jsonb WHERE id = $1")
                .bind(id).execute(&pool).await.unwrap();
        }
        dispatch(&pool, id).await;
        assert!(
            !covers_period(&pool, &p).await,
            "{kind} is not a supply original"
        );
        assert_eq!(
            pg::latest_billed_to(&pool, TENANT, MALO, p.lf_mp_id)
                .await
                .unwrap(),
            None
        );
        let open = pg::list_unbilled_periods(&pool, TENANT, true, 50)
            .await
            .unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].reason_code, "NO_METER_DATA");
    }
    let summary = pg::deadline_summary(&pool, TENANT).await.unwrap();
    assert_eq!(summary.len(), 1);
    assert_eq!((summary[0].issued_late, summary[0].issued_on_time), (0, 0));
}

/// Per Lieferant, invoices owed, issued on time, issued late, overdue.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_deadline_summary_counts_each_state() {
    let Some((pool, _pg)) = test_pool("deadline_summary").await else {
        return;
    };
    let json = body();
    // Written today, due long ago → late; due far ahead → on time.
    let late = pg::NewBillingRecord {
        due_on: Some(date!(2020 - 01 - 01)),
        ..draft(dec!(100), "RE-2026-000001", &json)
    };
    let late_id = pg::insert_billing_record(&pool, &late).await.expect("late");
    dispatch(&pool, late_id).await;
    let on_time = pg::NewBillingRecord {
        period_from: date!(2026 - 02 - 01),
        period_to: date!(2026 - 02 - 28),
        due_on: Some(date!(2099 - 01 - 01)),
        ..draft(dec!(100), "RE-2026-000002", &json)
    };
    let on_time_id = pg::insert_billing_record(&pool, &on_time)
        .await
        .expect("on time");
    dispatch(&pool, on_time_id).await;
    // One open period overdue, one still owed.
    pg::record_unbilled_period(
        &pool,
        &pg::NewUnbilledPeriod {
            period_from: date!(2025 - 01 - 01),
            period_to: date!(2025 - 01 - 31),
            due_on: date!(2025 - 03 - 14),
            ..unbilled("NO_METER_DATA")
        },
    )
    .await
    .expect("overdue");
    pg::record_unbilled_period(
        &pool,
        &pg::NewUnbilledPeriod {
            period_from: date!(2099 - 01 - 01),
            period_to: date!(2099 - 01 - 31),
            due_on: date!(2099 - 03 - 14),
            ..unbilled("NO_METER_DATA")
        },
    )
    .await
    .expect("owed");

    let rows = pg::deadline_summary(&pool, TENANT).await.expect("summary");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(
        (r.owed, r.issued_on_time, r.issued_late, r.overdue),
        (1, 1, 1, 1)
    );
}

// ── Rechnung nachrechnen ────────────────────────────────────────────────

/// An issued record carries its seal and its token's hash; the
/// token finds the record, an altered `rechnung_json` breaks the seal, and a
/// revoked token finds nothing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_issued_record_is_sealed_and_reachable_by_its_token() {
    let Some((pool, _pg)) = test_pool("nachrechnen_seal").await else {
        return;
    };
    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://localhost/x" },
        "tenant": TENANT,
        "productd_url": "http://localhost:9080",
        "edmd_url": "http://localhost:8380",
        "marktd_url": "http://localhost:8080",
        "nachrechnen": {
            "token_key": "0123456789abcdef0123456789abcdef",
            "public_url": "https://portal.example.de/nachrechnen"
        }
    }))
    .expect("config parses");
    let id = insert_draft(&pool, dec!(100)).await;
    let mut tx = pool.begin().await.expect("begin");
    pg::mark_dispatched_tx(&mut *tx, id).await.expect("issue");
    billingd::nachrechnen::seal_record(&mut tx, &cfg, id)
        .await
        .expect("seal");
    tx.commit().await.expect("commit");

    let row = pg::fetch_billing_record(&pool, TENANT, id)
        .await
        .expect("read")
        .expect("stored");
    assert!(row.seal_sha256.is_some(), "the record is sealed at issue");
    let link = billingd::nachrechnen::public_link(&cfg, id).expect("a link");
    let token = link
        .rsplit('/')
        .next()
        .expect("the token ends the link")
        .to_owned();
    let found =
        pg::fetch_by_verify_token(&pool, TENANT, &billingd::nachrechnen::token_hash(&token))
            .await
            .expect("lookup")
            .expect("the token finds its record");
    assert_eq!(found.id, id);
    assert_eq!(
        billingd::nachrechnen::nachrechnen(&found)
            .expect("recompute")
            .verdict
            .seal_holds,
        Some(true)
    );

    // An issued document refuses the change through every ordinary path…
    sqlx::query(
        "UPDATE billing_records SET rechnung_json = rechnung_json || '{\"x\": 1}' WHERE id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect_err("an issued record's document is immutable");
    // …so a change only arrives behind the trigger's back, and the seal sees it.
    for sql in [
        "ALTER TABLE billing_records DISABLE TRIGGER billing_records_immutable_after_issue",
        "UPDATE billing_records SET rechnung_json = rechnung_json || '{\"x\": 1}' WHERE id = $1",
        "ALTER TABLE billing_records ENABLE TRIGGER billing_records_immutable_after_issue",
    ] {
        let q = sqlx::query(sql);
        let q = if sql.contains("$1") { q.bind(id) } else { q };
        q.execute(&pool).await.expect("tamper as the table owner");
    }
    let tampered = pg::fetch_billing_record(&pool, TENANT, id)
        .await
        .expect("read")
        .expect("stored");
    let verdict = billingd::nachrechnen::nachrechnen(&tampered)
        .expect("recompute")
        .verdict;
    assert_eq!(
        verdict.seal_holds,
        Some(false),
        "an altered document breaks the seal"
    );
    assert!(!verdict.matches);

    assert!(
        pg::revoke_verify_token(&pool, TENANT, id, "sachbearbeiter@stadtwerk")
            .await
            .expect("revoke")
    );
    let by: Option<String> =
        sqlx::query_scalar("SELECT verify_revoked_by FROM billing_records WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read");
    assert_eq!(
        by.as_deref(),
        Some("sachbearbeiter@stadtwerk"),
        "the revocation names its principal"
    );
    assert!(
        pg::fetch_by_verify_token(&pool, TENANT, &billingd::nachrechnen::token_hash(&token))
            .await
            .expect("lookup")
            .is_none(),
        "a revoked token answers like an unknown one"
    );
}

/// No two live originals of one MaLo, Lieferant and product may bill the same
/// day: a Schlussrechnung over days already billed is refused.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_overlapping_original_is_refused() {
    let Some((pool, _pg)) = test_pool("overlap").await else {
        return;
    };
    let json = body();
    insert_draft(&pool, dec!(100)).await;
    let err = pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            period_from: date!(2026 - 01 - 15),
            period_to: date!(2026 - 02 - 10),
            ..draft(dec!(50), "RE-2026-000002", &json)
        },
    )
    .await
    .expect_err("the days 15.–31.01. are billed already");
    assert!(
        matches!(err, pg::InsertError::PeriodOverlaps { .. }),
        "{err}"
    );
    pg::insert_billing_record(
        &pool,
        &pg::NewBillingRecord {
            period_from: date!(2026 - 02 - 01),
            period_to: date!(2026 - 02 - 10),
            ..draft(dec!(50), "RE-2026-000003", &json)
        },
    )
    .await
    .expect("the days after the last billed one are free");
}

// ── Webhook delivery ──────────────────────────────────────────────────────────

/// The configured `erp_webhook_url` is the webhook endpoint `default`: an
/// event committed to the outbox reaches it signed with `erp_hmac_secret`, the
/// secret the ERP verifies with.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_erp_webhook_receives_an_issued_event_signed_with_its_secret() {
    use mako_service::Daemon as _;
    let Some((url, _pg)) = pg_container().await else {
        return;
    };
    let pool = PgPool::connect(&url).await.expect("connect");

    let (tx, mut rx) = tokio::sync::mpsc::channel::<(axum::http::HeaderMap, axum::body::Bytes)>(4);
    let receiver = axum::Router::new().route(
        "/erp",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::body::Bytes| {
                let tx = tx.clone();
                async move {
                    let _ = tx.send((headers, body)).await;
                    axum::http::StatusCode::NO_CONTENT
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, receiver).await });

    let cfg: billingd::config::BillingdConfig = serde_json::from_value(serde_json::json!({
        "database": {"url": url},
        "tenant": "stadtwerke-musterstadt",
        "productd_url": "http://127.0.0.1:1",
        "edmd_url": "http://127.0.0.1:1",
        "marktd_url": "http://127.0.0.1:1",
        "erp_webhook_url": format!("http://{addr}/erp"),
        "erp_hmac_secret": "erp-secret",
        "allow_insecure_no_auth": true,
    }))
    .expect("the configuration parses");
    billingd::daemon::Billingd::migrate(&cfg, &pool)
        .await
        .expect("migrate");

    let subs = billingd::daemon::subscriptions(&cfg, pool.clone());
    let shutdown = tokio_util::sync::CancellationToken::new();
    subs.start(shutdown.clone())
        .await
        .expect("the module starts");

    let ce = mako_service::CloudEvent::new(
        mako_service::source("billingd", "stadtwerke-musterstadt"),
        mako_events::billing::RECHNUNG_ERSTELLT,
        MALO.to_owned(),
        serde_json::json!({ "malo_id": MALO, "lf_mp_id": "9900357000028" }),
    );
    let mut tx = pool.begin().await.expect("begin");
    mako_service::outbox::enqueue(&mut tx, &ce)
        .await
        .expect("enqueue");
    tx.commit().await.expect("commit");

    let (headers, body) = tokio::time::timeout(std::time::Duration::from_secs(20), rx.recv())
        .await
        .expect("the delivery arrives")
        .expect("a delivery");
    let id = mako_service::webhook::verify_request(Some(b"erp-secret"), &headers, &body)
        .expect("signed with erp_hmac_secret")
        .expect("a webhook-id");
    assert_eq!(id.0, ce.id, "webhook-id is the CloudEvent id");
    let received: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(received["source"], ce.source);
    assert_eq!(received["data"]["lf_mp_id"], "9900357000028");
    shutdown.cancel();
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_lf_watch_checks_the_same_role_in_rest_and_mcp() {
    let (url, _db) = pg_container().await.expect("test PostgreSQL starts");
    let pool = PgPool::connect(&url).await.unwrap();
    super::mcp_round_trips::check_lf_watch(pool).await;
}

mod discovery;
