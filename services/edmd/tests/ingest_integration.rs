//! Handler-level tests for the `edmd` ingest and correction paths.
//!
//! These exercise the real router against a real PostgreSQL, because the
//! defects they guard against live in SQL, not in Rust: a primary key that
//! silently splits one register into two rows, a conflict action that overwrites
//! a measured reading, a cached aggregate that is never invalidated. A test
//! against a repository double would pass while every one of those was broken.
//!
//! ```bash
//! docker run -d --name edmd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=edmd -p 55432:5432 postgres:17-alpine
//! export EDMD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55432/edmd"
//! cargo test -p edmd --test ingest_integration -- --include-ignored
//! ```
//!
//! Every test provisions its own schema-isolated database, so they may run
//! concurrently and leave nothing behind.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use mako_edm::domain::{IngestionSource, MeterRead, QualityFlag, Sparte};
use mako_edm::repository::TimeSeriesRepository;
use rust_decimal::dec;
use sqlx::PgPool;
use time::macros::datetime;
use tower::ServiceExt as _;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Connect and provision a fresh schema, or skip when no database is configured.
///
/// Returns `None` rather than failing so the suite stays runnable without
/// Docker; the `#[ignore]` attribute is what keeps these out of the default run.
async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("EDMD_TEST_DATABASE_URL").ok()?;
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

    // The DDL creates partitions via plpgsql, which resolves `meter_reads`
    // through the search_path set above.
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
        // `$$` opens and closes a function body; `$re$` is a named dollar quote.
        let dollars = line.matches("$$").count() + line.matches("$re$").count();
        if dollars % 2 == 1 {
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

fn read(obis: &str, kwh: rust_decimal::Decimal, quality: QualityFlag) -> MeterRead {
    MeterRead {
        malo_id: "51238696781".to_owned(),
        melo_id: None,
        dtm_from: datetime!(2026-07-01 00:00 UTC),
        dtm_to: datetime!(2026-07-01 00:15 UTC),
        quantity_kwh: kwh,
        quality,
        pid: 0,
        sparte: Sparte::Strom,
        obis_code: Some(obis.to_owned()),
        tenant: "T1".to_owned(),
        source: IngestionSource::ApiImport,
        push_session: None,
        quality_warnings: None,
        sender_mp_id: None,
        allocation_version: "INITIAL".to_owned(),
        valid_from_tx: None,
    }
}

// ── Ingest identity ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn two_obis_registers_at_one_instant_are_two_readings() {
    let Some(pool) = test_pool("two_registers").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    // Import (1.8.0) and export (2.8.0) at the same quarter-hour.
    repo.store_reads(&[
        read("1-0:1.8.0", dec!(10), QualityFlag::Measured),
        read("1-0:2.8.0", dec!(4), QualityFlag::Measured),
    ])
    .await
    .expect("store");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM meter_reads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "each register is its own reading");
}

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn the_same_register_spelled_two_ways_is_one_reading() {
    let Some(pool) = test_pool("obis_norm").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    // `*255` is the BDEW wildcard suffix for the same register.
    repo.store_reads(&[read("1-0:1.8.0", dec!(10), QualityFlag::Measured)])
        .await
        .expect("first");
    repo.store_reads(&[read("1-0:1.8.0*255", dec!(12), QualityFlag::Measured)])
        .await
        .expect("second");

    let (count, kwh): (i64, rust_decimal::Decimal) =
        sqlx::query_as("SELECT count(*), max(quantity_kwh) FROM meter_reads")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 1,
        "two spellings of one register must not become two rows"
    );
    assert_eq!(kwh, dec!(12), "the later value wins");
}

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn a_value_changing_redelivery_leaves_an_audit_row() {
    let Some(pool) = test_pool("overwrite_audit").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    repo.store_reads(&[read("1-0:1.8.0", dec!(10), QualityFlag::Measured)])
        .await
        .expect("first delivery");
    // Same interval redelivered with a different value — §22 MessZV requires
    // the displaced value to survive in `meter_read_corrections`.
    repo.store_reads(&[read("1-0:1.8.0", dec!(12), QualityFlag::Measured)])
        .await
        .expect("redelivery");
    // An identical redelivery must NOT add a second audit row.
    repo.store_reads(&[read("1-0:1.8.0", dec!(12), QualityFlag::Measured)])
        .await
        .expect("identical redelivery");

    let (count, original, corrected): (i64, rust_decimal::Decimal, rust_decimal::Decimal) =
        sqlx::query_as(
            "SELECT count(*), max(original_kwh), max(corrected_kwh)
             FROM meter_read_corrections",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "exactly one audit row: value-changing overwrites are recorded, identical ones are not"
    );
    assert_eq!(original, dec!(10), "the displaced value is preserved");
    assert_eq!(corrected, dec!(12), "the overwriting value is recorded");

    let source: String = sqlx::query_scalar("SELECT source FROM meter_read_corrections")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(source, "OTHER", "API_IMPORT maps to the OTHER audit source");
}

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn two_tenants_may_hold_the_same_malo_id() {
    let Some(pool) = test_pool("tenant_isolation").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    let mut theirs = read("1-0:1.8.0", dec!(999), QualityFlag::Measured);
    theirs.tenant = "T2".to_owned();

    repo.store_reads(&[read("1-0:1.8.0", dec!(10), QualityFlag::Measured), theirs])
        .await
        .expect("store");

    let ours: rust_decimal::Decimal =
        sqlx::query_scalar("SELECT quantity_kwh FROM meter_reads WHERE tenant = 'T1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ours,
        dec!(10),
        "one tenant's reading must not overwrite another's for the same MaLo-ID"
    );
}

// ── Retention ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn a_partition_holding_unexported_rows_is_not_released() {
    let Some(pool) = test_pool("retention_guard").await else {
        return;
    };

    sqlx::query("SELECT ensure_meter_reads_partition('2019-03-01')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO meter_reads
            (malo_id, dtm_from, dtm_to, quantity_kwh, pid, tenant, quality, obis_code_norm, archived)
         VALUES
            ('511','2019-03-05 00:00Z','2019-03-05 00:15Z',1,0,'T1','MEASURED','x',true),
            ('511','2019-03-05 00:15Z','2019-03-05 00:30Z',2,0,'T1','MEASURED','x',false)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let dropped: Vec<(String, i64)> =
        sqlx::query_as("SELECT * FROM drop_archived_meter_reads_partitions(now())")
            .fetch_all(&pool)
            .await
            .unwrap();

    assert!(
        !dropped
            .iter()
            .any(|(name, _)| name == "meter_reads_p201903"),
        "a partition with an unexported row must survive; dropped: {dropped:?}"
    );
}

// ── Substitution (§17 MessZV) ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn a_substitute_does_not_replace_a_measured_reading() {
    let Some(pool) = test_pool("substitute_guard").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());
    repo.store_reads(&[read("1-0:1.8.0", dec!(100), QualityFlag::Measured)])
        .await
        .expect("store");

    // The normalised key as ingest actually stored it. Hardcoding the raw
    // spelling here would target a different key and the conflict would never
    // fire, so the test would pass while proving nothing.
    let stored_norm: String = sqlx::query_scalar("SELECT obis_code_norm FROM meter_reads")
        .fetch_one(&pool)
        .await
        .unwrap();

    // The conflict action the substitute path uses.
    let affected = sqlx::query(
        "INSERT INTO meter_reads
             (malo_id, dtm_from, dtm_to, quantity_kwh, quality, pid, sparte, unit,
              obis_code, obis_code_norm, source, tenant)
         VALUES ('51238696781','2026-07-01 00:00Z','2026-07-01 00:15Z',999,'SUBSTITUTED',
                 0,'STROM','KWH','1-0:1.8.0',$1,'AUTO_SUBSTITUTE','T1')
         ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO UPDATE
             SET quantity_kwh = EXCLUDED.quantity_kwh
             WHERE meter_reads.quality IN ('FAULTY','UNKNOWN')",
    )
    .bind(&stored_norm)
    .execute(&pool)
    .await
    .unwrap()
    .rows_affected();

    assert_eq!(affected, 0, "the conflict action must decline");

    let (count, kwh): (i64, rust_decimal::Decimal) =
        sqlx::query_as("SELECT count(*), max(quantity_kwh) FROM meter_reads")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "no second row on the empty-string register");
    assert_eq!(kwh, dec!(100), "the measured value survives");
}

// ── Billing aggregate cache ───────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn ingest_invalidates_a_cached_billing_period() {
    let Some(pool) = test_pool("cache_invalidation").await else {
        return;
    };
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    sqlx::query(
        "INSERT INTO meter_billing_periods
             (malo_id, period_from, period_to, messtyp, sparte, arbeitsmenge_kwh, quality, tenant)
         VALUES ('51238696781','2026-07-01','2026-07-31','RLM','STROM',5,'VALID','T1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    // A reading inside the cached period must drop the stale aggregate.
    repo.store_reads(&[read("1-0:1.8.0", dec!(10), QualityFlag::Measured)])
        .await
        .expect("store");

    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM meter_billing_periods")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        remaining, 0,
        "a mid-period ingest must invalidate the cached aggregate, or the partial \
         sum is served forever"
    );
}

// ── Router wiring ─────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn the_readiness_probe_reports_a_live_database() {
    let Some(pool) = test_pool("health").await else {
        return;
    };
    let state = edmd::handler::HandlerState {
        repo: edmd::pg::PgTimeSeriesRepository::new(pool),
        inbound_secret: std::sync::Arc::new(None),
        tenant: "T1".to_owned(),
        olap_engine: None,
        marktd_url: String::new(),
        marktd_api_key: secrecy::SecretString::from(String::new()),
        erp_webhook_url: None,
    };

    let response = edmd::server::router(state)
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

// ── §40 Abs. 2 EnWG compliance ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn only_executed_readings_count_toward_the_ablesequote() {
    let Some(pool) = test_pool("compliance").await else {
        return;
    };

    // One of each terminal outcome, all in campaign year 2026.
    sqlx::query(
        "INSERT INTO ablese_auftraege
             (malo_id, anlass, auftraggeber_rolle, geplant_am, ausfuehrt_bis, status,
              fehlschlag_grund, tenant)
         VALUES
             ('1','JAHRESABLESUNG','NB','2026-12-31','2027-01-31','AUSGEFUEHRT',NULL,'T1'),
             ('2','JAHRESABLESUNG','NB','2026-12-31','2027-01-31','STORNIERT',NULL,'T1'),
             ('3','JAHRESABLESUNG','NB','2026-12-31','2026-01-31','FEHLGESCHLAGEN','KEIN_ZUTRITT','T1'),
             ('4','JAHRESABLESUNG','NB','2026-12-31','2026-01-31','OFFEN',NULL,'T1')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT status, count(*) AS orders,
                count(*) FILTER (WHERE ausfuehrt_bis < CURRENT_DATE
                                   AND status <> 'AUSGEFUEHRT') AS overdue
           FROM ablese_auftraege
          WHERE tenant = 'T1' AND anlass = 'JAHRESABLESUNG'
            AND extract(year FROM geplant_am) = 2026
          GROUP BY status",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    let total: i64 = rows.iter().map(|(_, n, _)| n).sum();
    let ausgefuehrt: i64 = rows
        .iter()
        .find(|(s, _, _)| s == "AUSGEFUEHRT")
        .map_or(0, |(_, n, _)| *n);
    let overdue: i64 = rows.iter().map(|(_, _, o)| o).sum();

    assert_eq!(total, 4);
    assert_eq!(
        ausgefuehrt, 1,
        "only AUSGEFUEHRT discharges the §40 Abs. 2 EnWG obligation"
    );
    assert_eq!(
        overdue, 2,
        "FEHLGESCHLAGEN and OFFEN past their deadline are both still owed; \
         STORNIERT is not, and AUSGEFUEHRT is done"
    );
}

// ── Quality history ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn the_quality_reader_selects_columns_that_exist() {
    let Some(pool) = test_pool("quality_read").await else {
        return;
    };

    sqlx::query(
        "INSERT INTO quality_assessments
             (malo_id, period_from, period_to, grade, interval_count, expected_count,
              gaps_detected, zero_run, outlier_count, coverage_pct, billing_blocked,
              issues_json, pid, source, tenant)
         VALUES ('511','2026-07-01 00:00Z','2026-07-31 00:00Z','C',2800,2880,
                 80,0,3,97.22,false,'{\"V02\":80}',13005,'MSCONS','T1')",
    )
    .execute(&pool)
    .await
    .expect("insert");

    // Exactly the projection `list_quality_assessments` issues. Every column it
    // names must exist, or the endpoint 500s on every call.
    let row: (String, i32, Option<i32>, Option<i32>) = sqlx::query_as(
        "SELECT grade, interval_count, expected_count, pid
           FROM quality_assessments
          WHERE malo_id = $1 AND tenant = $2",
    )
    .bind("511")
    .bind("T1")
    .fetch_one(&pool)
    .await
    .expect("the reader's projection must resolve against the schema");

    assert_eq!(row.0, "C");
    assert_eq!(row.1, 2800);
    assert_eq!(row.2, Some(2880));
    assert_eq!(row.3, Some(13005));
}

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn rescoring_a_window_supersedes_rather_than_duplicates() {
    let Some(pool) = test_pool("quality_upsert").await else {
        return;
    };

    for grade in ["F", "B"] {
        sqlx::query(
            "INSERT INTO quality_assessments
                 (malo_id, period_from, period_to, grade, billing_blocked, source, tenant)
             VALUES ('511','2026-07-01 00:00Z','2026-07-31 00:00Z',$1,$2,'BATCH_RESCORE','T1')
             ON CONFLICT (tenant, malo_id, period_from, period_to, source) DO UPDATE
                 SET grade = EXCLUDED.grade, billing_blocked = EXCLUDED.billing_blocked",
        )
        .bind(grade)
        .bind(grade == "F")
        .execute(&pool)
        .await
        .expect("upsert");
    }

    let (count, grade, blocked): (i64, String, bool) = sqlx::query_as(
        "SELECT count(*), max(grade), bool_or(billing_blocked) FROM quality_assessments",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(count, 1, "re-scoring must supersede, not append");
    assert_eq!(grade, "B", "the later verdict wins");
    assert!(
        !blocked,
        "clearing the block must persist with the new grade"
    );
}
