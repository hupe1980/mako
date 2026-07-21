//! Iceberg archive tier — write-then-read-back, concurrent commit, and
//! crash-recovery idempotency against a real PostgreSQL SQL catalog and a
//! local `file://` warehouse.
//!
//! The defects these guard against live in the interaction of three systems
//! (PostgreSQL hot tier, Iceberg SQL catalog, Parquet object storage): a
//! stale-snapshot commit that livelocks, a restart that re-commits already
//! archived paths, a read path that cannot see what the writer wrote. None of
//! that is testable against mocks.
//!
//! ```bash
//! docker run -d --name edmd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=edmd -p 55432:5432 postgres:17-alpine
//! export EDMD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55432/edmd"
//! cargo test -p edmd --test iceberg_archive -- --include-ignored
//! ```

use edmd::config::ArchiveConfig;
use edmd::iceberg::worker::{ArchiveWorker, load_table_for_olap};
use mako_edm::domain::{IngestionSource, MeterRead, QualityFlag, Sparte};
use mako_edm::repository::TimeSeriesRepository;
use rust_decimal::dec;
use sqlx::PgPool;
use time::macros::datetime;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

/// Connect and provision a fresh schema; also returns a URL whose
/// `search_path` points at that schema, so the Iceberg SQL catalog's own
/// tables are isolated per test exactly like the hot tier.
async fn test_db(test_name: &str) -> Option<(PgPool, String)> {
    let base = std::env::var("EDMD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;

    let schema = format!("ice_{test_name}");
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

    let sep = if base.contains('?') { '&' } else { '?' };
    let url = format!("{base}{sep}options=-csearch_path%3D{schema}");
    Some((pool, url))
}

/// Split the DDL on `;` at statement level, keeping `$$`-quoted bodies intact.
fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_dollar = false;
    for line in sql.lines() {
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

/// An archive-eligible reading: 2026-04 is far past a 1-month retention
/// window and safely inside the 24-month partition backlog the DDL creates.
fn old_read(malo: &str, quarter: i64, kwh: rust_decimal::Decimal) -> MeterRead {
    let from = datetime!(2026-04-01 00:00 UTC) + time::Duration::minutes(15 * quarter);
    MeterRead {
        malo_id: malo.to_owned(),
        melo_id: None,
        dtm_from: from,
        dtm_to: from + time::Duration::minutes(15),
        quantity_kwh: kwh,
        quality: QualityFlag::Measured,
        pid: 13017,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        tenant: "T1".to_owned(),
        source: IngestionSource::Mscons,
        push_session: None,
        quality_warnings: None,
        sender_mp_id: None,
        allocation_version: "INITIAL".to_owned(),
        valid_from_tx: None,
    }
}

fn archive_cfg(catalog_name: &str, warehouse: &std::path::Path) -> ArchiveConfig {
    ArchiveConfig {
        enabled: true,
        storage_uri: format!("file://{}", warehouse.display()),
        retention_months: 1,
        batch_size: 10_000,
        interval_secs: 3_600,
        iceberg_catalog_schema: "public".to_owned(),
        iceberg_catalog_name: catalog_name.to_owned(),
        access_key_id: None,
        secret_access_key: None,
        region: "eu-central-1".to_owned(),
        endpoint_url: None,
    }
}

fn worker(pool: &PgPool, cfg: &ArchiveConfig, url: &str) -> ArchiveWorker {
    let file_io = edmd::iceberg::build_file_io(cfg).expect("file io");
    ArchiveWorker::new(pool.clone(), cfg.clone(), file_io, url.to_owned())
}

// ── Write → commit → read back ────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn archived_rows_are_readable_from_the_cold_tier() {
    let Some((pool, url)) = test_db("roundtrip").await else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = archive_cfg("cat_roundtrip", tmp.path());
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    repo.store_reads(&[
        old_read("51238696781", 0, dec!(1.25)),
        old_read("51238696781", 1, dec!(2.5)),
        old_read("98765432109", 0, dec!(7.0)),
    ])
    .await
    .expect("store");

    let archived = worker(&pool, &cfg, &url)
        .run_once()
        .await
        .expect("archival pass");
    assert_eq!(archived, 3, "all three eligible rows archived");

    let unarchived: i64 =
        sqlx::query_scalar("SELECT count(*) FROM meter_reads WHERE archived = false")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unarchived, 0, "every row marked durable");

    let (status, files): (String, i32) =
        sqlx::query_as("SELECT status, file_count FROM archive_batches")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "committed");
    assert!(files > 0, "at least one Parquet file written");

    // Read back through the DataFusion OLAP engine — the same path the
    // /api/v1/archive endpoints use.
    let engine = load_table_for_olap(&cfg, &url, pool.clone(), "T1".to_owned())
        .await
        .expect("olap engine");
    let rows = engine
        .time_series(
            "51238696781",
            datetime!(2026-04-01 00:00 UTC),
            datetime!(2026-04-02 00:00 UTC),
            100,
        )
        .await
        .expect("cold-tier query");
    assert_eq!(rows.len(), 2, "both quarters of the MaLo are in Iceberg");
    let kwh: Vec<&str> = rows.iter().map(|r| r.quantity_kwh.as_str()).collect();
    assert!(
        kwh.contains(&"1.25000") && kwh.contains(&"2.50000"),
        "NUMERIC(18,5) values survive the Parquet round-trip: {kwh:?}"
    );

    // GDPR read-time exclusion: an erased MaLo disappears from every query.
    sqlx::query(
        "INSERT INTO gdpr_deletions (malo_id, tenant, reason, authorized_by)
         VALUES ('98765432109', 'T1', 'DSGVO Art. 17', 'test')",
    )
    .execute(&pool)
    .await
    .expect("erasure request");
    let erased = engine
        .time_series(
            "98765432109",
            datetime!(2026-04-01 00:00 UTC),
            datetime!(2026-04-02 00:00 UTC),
            100,
        )
        .await
        .expect("query after erasure");
    assert!(erased.is_empty(), "erased MaLo must not be readable");
}

// ── Concurrent writers ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn concurrent_archival_passes_both_commit() {
    let Some((pool, url)) = test_db("concurrent").await else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = archive_cfg("cat_concurrent", tmp.path());
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    // First pass creates namespace + table so the concurrent passes contend
    // on the snapshot commit, not on CREATE TABLE.
    repo.store_reads(&[old_read("11111111111", 0, dec!(1))])
        .await
        .expect("seed");
    assert_eq!(
        worker(&pool, &cfg, &url)
            .run_once()
            .await
            .expect("seed pass"),
        1
    );

    repo.store_reads(&[
        old_read("22222222222", 0, dec!(2)),
        old_read("33333333333", 0, dec!(3)),
    ])
    .await
    .expect("store");

    // Two workers race the same catalog + table. Whichever commits second
    // must take the CatalogCommitConflicts retry path (reload table, rebuild
    // transaction) rather than failing or livelocking on its stale snapshot.
    let w1 = worker(&pool, &cfg, &url);
    let w2 = worker(&pool, &cfg, &url);
    let (r1, r2) = tokio::join!(w1.run_once(), w2.run_once());
    let n1 = r1.expect("worker 1 commits");
    let n2 = r2.expect("worker 2 commits");
    assert!(
        n1 > 0 || n2 > 0,
        "at least one pass archived the batch (n1={n1}, n2={n2})"
    );

    let unarchived: i64 =
        sqlx::query_scalar("SELECT count(*) FROM meter_reads WHERE archived = false")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unarchived, 0, "no row left behind by the race");

    // Every archived MaLo is readable — however the race interleaved.
    let engine = load_table_for_olap(&cfg, &url, pool.clone(), "T1".to_owned())
        .await
        .expect("olap engine");
    for malo in ["22222222222", "33333333333"] {
        let rows = engine
            .time_series(
                malo,
                datetime!(2026-04-01 00:00 UTC),
                datetime!(2026-04-02 00:00 UTC),
                100,
            )
            .await
            .expect("cold-tier query");
        assert!(!rows.is_empty(), "MaLo {malo} must be durable in Iceberg");
    }
}

// ── Crash recovery ────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires EDMD_TEST_DATABASE_URL"]
async fn recovery_after_crash_between_commit_and_mark_is_idempotent() {
    let Some((pool, url)) = test_db("crash").await else {
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = archive_cfg("cat_crash", tmp.path());
    let repo = edmd::pg::PgTimeSeriesRepository::new(pool.clone());

    repo.store_reads(&[
        old_read("51238696781", 0, dec!(4)),
        old_read("51238696781", 1, dec!(5)),
    ])
    .await
    .expect("store");

    assert_eq!(
        worker(&pool, &cfg, &url).run_once().await.expect("pass 1"),
        2
    );

    // Simulate the documented crash window: the Iceberg snapshot committed,
    // but the process died before step 5 marked the rows archived. On restart
    // the rows are re-selected.
    sqlx::query("UPDATE meter_reads SET archived = false")
        .execute(&pool)
        .await
        .unwrap();

    let recovered = worker(&pool, &cfg, &url)
        .run_once()
        .await
        .expect("recovery pass must not fail");
    assert_eq!(recovered, 2, "recovery re-archives the unmarked rows");

    let unarchived: i64 =
        sqlx::query_scalar("SELECT count(*) FROM meter_reads WHERE archived = false")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unarchived, 0, "rows durable after recovery");

    let committed: i64 =
        sqlx::query_scalar("SELECT count(*) FROM archive_batches WHERE status = 'committed'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(committed, 2, "each pass is its own committed batch");

    // The recovery pass re-wrote the same rows under a fresh UUID file prefix
    // — duplicate data files by design, cleaned by a later expire_snapshots.
    // What matters: the query tier still serves the readings.
    let engine = load_table_for_olap(&cfg, &url, pool.clone(), "T1".to_owned())
        .await
        .expect("olap engine");
    let rows = engine
        .time_series(
            "51238696781",
            datetime!(2026-04-01 00:00 UTC),
            datetime!(2026-04-02 00:00 UTC),
            100,
        )
        .await
        .expect("cold-tier query");
    assert!(
        rows.len() >= 2,
        "both readings remain readable after recovery (duplicates permitted \
         until expire_snapshots): {}",
        rows.len()
    );
}
