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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_references_its_untouched_original() {
    let Some((pool, _pg)) = test_pool("correction_chain").await else {
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
        &serde_json::json!({
            "_typ": "RECHNUNG",
            "istOriginal": false,
            "zusatzAttribute": [{ "name": "rechnungsart", "wert": "KORREKTURRECHNUNG" }]
        }),
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_second_correction_of_the_same_original_is_detected() {
    let Some((pool, _pg)) = test_pool("second_correction").await else {
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
        &serde_json::json!({
            "_typ": "RECHNUNG",
            "istOriginal": false,
            "zusatzAttribute": [{ "name": "rechnungsart", "wert": "KORREKTURRECHNUNG" }]
        }),
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_monthly_run_log_accumulates_daily_sweeps() {
    let Some((pool, _pg)) = test_pool("run_log").await else {
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
    let id = pg::insert_billing_record(
        &pool,
        "9910000000002",
        "51238696781",
        "9910000000002",
        "STROM-GAS",
        "TARIFWECHSEL",
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
        &serde_json::json!({ "_typ": "RECHNUNG" }),
        dec!(100),
        dec!(119),
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

    // A re-run for the same period is now refused (correction path), not replaced.
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
    .expect_err("dispatched record must refuse the overwrite");
    assert!(err.to_string().contains("correction"), "{err}");
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

    let id = pg::insert_billing_record(
        &pool,
        OWNER,
        "51238696781",
        OWNER,
        "STROM-BASIS",
        "STROM",
        date!(2026 - 02 - 01),
        date!(2026 - 02 - 28),
        &serde_json::json!({ "_typ": "RECHNUNG" }),
        dec!(200),
        dec!(238),
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
    let owner_rows = pg::list_billing_records(&pool, OWNER, None, None, None, 100)
        .await
        .expect("owner list");
    assert_eq!(owner_rows.len(), 1);
    let other_rows = pg::list_billing_records(&pool, OTHER, None, None, None, 100)
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

// ── Document template store ───────────────────────────────────────────────────

/// Publishing is content-addressed and idempotent; the pointer moves, the
/// templates it points at do not.
///
/// All of this is SQL — a primary key on the hash, a foreign key from the
/// pointer, a foreign key from the issued invoice — so only a real database
/// proves it. The property that matters is the last one: a template an invoice
/// was rendered with must stay resolvable, because § 147 AO / GoBD keep that
/// invoice for 8 years and its appearance has to remain explicable.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_published_template_stays_resolvable_after_the_pointer_moves() {
    use billingd::document::gate::Proof;
    use billingd::template_store::{self, TemplateKind};

    let Some((pool, _pg)) = test_pool("template_store").await else {
        return;
    };
    let tenant = "9900000000001";

    let v1 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v1",
        Some("a-3b"),
        Proof::RenderedPdfa,
        Some("ops@example"),
    )
    .await
    .expect("publish v1");

    // Same source → same identity, and no duplicate row.
    let again = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v1",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("re-publish is a no-op");
    assert_eq!(
        again, v1,
        "content-addressed: identical source, identical hash"
    );
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM document_templates")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1, "re-publishing must not duplicate");

    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v1)
        .await
        .expect("roll out v1");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v1.clone()),
    );

    // Publish a redesign and roll it out.
    let v2 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#set page(paper: \"a4\")\n= Rechnung v2 (neues Logo)",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("publish v2");
    assert_ne!(v2, v1);
    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v2)
        .await
        .expect("roll out v2");

    // The point of the whole design: v1 is still there, unchanged, and an
    // invoice rendered with it can still explain how it looked.
    let old = template_store::by_hash(&pool, &v1)
        .await
        .unwrap()
        .expect("v1 survives the rollout");
    assert!(old.source.contains("v1"), "the old source is intact");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v2),
        "only the pointer moved",
    );

    // A pointer into nothing is refused — that is why it is a table with a
    // foreign key rather than a free-text column.
    assert!(
        template_store::set_current(&pool, tenant, TemplateKind::Invoice, "deadbeef")
            .await
            .is_err(),
        "cannot point at an unpublished template",
    );

    // Textform kinds share the store, and their pointers are independent.
    let mahnung = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        "= Zahlungserinnerung",
        None,
        Proof::Parsed,
        None,
    )
    .await
    .expect("publish Mahnung");
    template_store::set_current(&pool, tenant, TemplateKind::Mahnung, &mahnung)
        .await
        .unwrap();
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.kind),
        Some("INVOICE".to_owned()),
        "rolling out a Mahnung must not disturb the invoice pointer",
    );
}

/// The template a document was rendered with is pinned once and never moves.
///
/// This is the § 147 AO property expressed in SQL, and only a real database can
/// show it: a conditional `UPDATE`, a foreign key into an append-only table, and
/// a pointer that other rows share. Rolling out a redesign must change how *new*
/// invoices look and nothing at all about one already issued.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_issued_document_keeps_the_layout_it_was_issued_with() {
    use billingd::document::gate::Proof;
    use billingd::template_store::{self, TemplateKind};

    let Some((pool, _pg)) = test_pool("template_pin").await else {
        return;
    };
    let tenant = "9910000000002";
    let record = insert_draft(&pool, dec!(100)).await;

    let v1 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [Rechnung v1]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        Some("ops@example"),
    )
    .await
    .expect("publish v1");

    // Nothing is pinned until the document is first rendered.
    let before = pg::fetch_billing_record(&pool, tenant, record)
        .await
        .unwrap()
        .expect("record");
    assert_eq!(before.template_hash, None);

    // Rendering a *draft* pins nothing: nobody has received it, and trapping an
    // operator's own preview on the layout they were about to fix would be
    // irreversible in an append-only store.
    assert_eq!(
        pg::pin_template(&pool, record, &v1).await.unwrap(),
        None,
        "a draft renders with the current layout and pins nothing",
    );

    // Once it has left the house, the first render fixes the layout.
    pg::mark_dispatched_tx(&pool, record)
        .await
        .expect("dispatch");
    assert_eq!(
        pg::pin_template(&pool, record, &v1).await.unwrap(),
        Some(v1.clone()),
        "the first render after dispatch pins the layout",
    );

    // A redesign is published and rolled out.
    let v2 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [Rechnung v2]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("publish v2");
    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v2)
        .await
        .expect("roll out v2");

    // Re-rendering the issued invoice still resolves v1 — the pin refuses to
    // move, so an audit in 2034 gets the document as it was sent.
    assert_eq!(
        pg::pin_template(&pool, record, &v2).await.unwrap(),
        Some(v1.clone()),
        "an issued document's layout is fixed; a rollout cannot restyle it",
    );
    assert_eq!(
        pg::fetch_billing_record(&pool, tenant, record)
            .await
            .unwrap()
            .and_then(|r| r.template_hash),
        Some(v1),
    );
}

/// The schema refuses an invoice template that was not fully proven.
///
/// The gate is a code path and code paths can be bypassed — by a future caller,
/// by a migration script, by a hand-written `INSERT`. The constraint is what
/// makes "an INVOICE row is always a rendered, conformant carrier" a property of
/// the data rather than of the current call graph.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_database_refuses_an_unproven_invoice_template() {
    use billingd::document::gate::Proof;
    use billingd::template_store::{self, TemplateKind};

    let Some((pool, _pg)) = test_pool("template_proof").await else {
        return;
    };
    let tenant = "9910000000002";

    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Invoice,
            "#let render(i) = []",
            Some("a-3b"),
            Proof::Parsed,
            None,
        )
        .await
        .is_err(),
        "an invoice template may not be stored on the weaker proof",
    );

    assert!(
        template_store::publish(
            &pool,
            tenant,
            TemplateKind::Invoice,
            "#let render(i) = []",
            None,
            Proof::RenderedPdfa,
            None,
        )
        .await
        .is_err(),
        "an invoice template must record the PDF/A level it met",
    );

    // The Textform kinds are stored on the proof they can actually offer.
    let hash = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        "#let render(i) = [Zahlungserinnerung]",
        None,
        Proof::Parsed,
        None,
    )
    .await
    .expect("a Textform template stores on the parse proof");
    assert_eq!(
        template_store::by_hash(&pool, &hash)
            .await
            .unwrap()
            .map(|t| t.proof),
        Some("PARSED".to_owned()),
        "the store records which proof was obtained",
    );
}

/// An operator can find the hash to roll back to.
///
/// The store never deletes so a previous layout stays restorable, and the API
/// documents rollback as "PUT the previous hash" — which is not a performable
/// instruction unless something says what the previous hash was. `current`
/// names exactly one template and `by_hash` needs the answer already, so
/// without a listing the documented recovery path could not be walked.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rollback_can_discover_the_hash_to_roll_back_to() {
    use billingd::document::gate::Proof;
    use billingd::template_store::{self, TemplateKind};

    let Some((pool, _pg)) = test_pool("template_list").await else {
        return;
    };
    let tenant = "9910000000002";

    let v1 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [v1]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        Some("ops@example"),
    )
    .await
    .expect("publish v1");
    let v2 = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Invoice,
        "#let render(i) = [v2]",
        Some("a-3b"),
        Proof::RenderedPdfa,
        None,
    )
    .await
    .expect("publish v2");
    let mahnung = template_store::publish(
        &pool,
        tenant,
        TemplateKind::Mahnung,
        "#let render(i) = [Mahnung]",
        None,
        Proof::Parsed,
        None,
    )
    .await
    .expect("publish Mahnung");
    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &v2)
        .await
        .expect("roll out v2");

    let all = template_store::list(&pool, tenant, None, 100)
        .await
        .expect("list");
    assert_eq!(all.len(), 3, "every kind, newest first");
    assert_eq!(all[0].hash, mahnung, "ordered by publication, newest first");

    // Exactly one INVOICE row is current, and it is the one rolled out.
    let invoices = template_store::list(&pool, tenant, Some(TemplateKind::Invoice), 100)
        .await
        .expect("list invoices");
    assert_eq!(invoices.len(), 2, "the kind filter applies");
    let current: Vec<&String> = invoices
        .iter()
        .filter(|t| t.is_current)
        .map(|t| &t.hash)
        .collect();
    assert_eq!(current, vec![&v2]);

    // And the previous one is right there, which is the whole point.
    let previous = invoices
        .iter()
        .find(|t| !t.is_current)
        .expect("the layout to roll back to");
    assert_eq!(previous.hash, v1);
    assert_eq!(previous.proof, "RENDERED_PDFA");
    assert_eq!(previous.pdf_standard.as_deref(), Some("a-3b"));
    assert_eq!(previous.published_by.as_deref(), Some("ops@example"));

    template_store::set_current(&pool, tenant, TemplateKind::Invoice, &previous.hash)
        .await
        .expect("roll back");
    assert_eq!(
        template_store::current(&pool, tenant, TemplateKind::Invoice)
            .await
            .unwrap()
            .map(|t| t.hash),
        Some(v1),
        "the rollback the listing made discoverable actually works",
    );

    // A tenant sees only its own templates.
    assert!(
        template_store::list(&pool, "9900000000004", None, 100)
            .await
            .expect("list")
            .is_empty(),
        "listings are tenant-scoped",
    );
}
