//! meterstore-backed storage integration tests for edmd.
//!
//! Recreates the storage-layer coverage that moved out of edmd when `meter_reads`
//! became a `meterstore` table. Each test spins a real PostgreSQL via
//! testcontainers and provisions meterstore's hot + cold tiers over a throwaway
//! filesystem Iceberg warehouse, so they are `#[ignore]`d by default and run
//! explicitly (Docker required):
//!
//! ```bash
//! just test-edmd-db
//! # or
//! cargo test -p edmd --test meterstore_integration -- --include-ignored --test-threads=1
//! ```
//!
//! What they pin:
//! - **Sparte survives the read-back** — the exact defect where a Gas MaLo came
//!   back labelled `Strom` because `MeasurementSeries` carried no Sparte.
//! - **Latest reading** resolves to the newest interval.
//! - **Corrections** supersede the value on read *and* leave an immutable
//!   `meter_read_corrections` audit row (§ 60 Abs. 6 MsbG).
//! - **GDPR Art. 17** — a MaLo is registered as an erasure subject at ingest, and
//!   erasing the mapping unlinks it (pseudonymisation).
//! - **Tenant scoping** — the same MaLo under two tenants stays isolated on read,
//!   never merged.

use edmd::domain::{
    CorrectionRecord, CorrectionSource, IngestionSource, MeterRead, QualityFlag, Sparte,
    TimeSeriesQuery, TimeSeriesRepository, Typ2DeliveryPath, Typ2Read, Typ2Repository,
};
use edmd::store::{
    MeterStoreTimeSeriesRepository, MeterStoreTyp2Repository, TieringConfig, WarehouseAuth,
    build_stores,
};
use rust_decimal::Decimal;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use time::{Duration, OffsetDateTime};

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// PostgreSQL + edmd migrations + a throwaway filesystem warehouse.
///
/// Returns `(pool, database_url, warehouse_uri, container_guard, warehouse_guard)`
/// — the caller **must** hold the last two: dropping them removes the container
/// and the temp warehouse (no leak).
async fn boot() -> (sqlx::PgPool, String, String, PgContainer, tempfile::TempDir) {
    use testcontainers::ImageExt;
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

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await
        .expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("edmd migrations");

    let warehouse = tempfile::tempdir().expect("warehouse tempdir");
    let warehouse_uri = format!("file://{}", warehouse.path().display());

    (pool, url, warehouse_uri, container, warehouse)
}

async fn setup() -> (
    MeterStoreTimeSeriesRepository,
    sqlx::PgPool,
    PgContainer,
    tempfile::TempDir,
) {
    let (pool, url, warehouse_uri, container, warehouse) = boot().await;
    // Builds both tables (reads + Typ-2) over one shared catalog/session; this
    // helper exercises the authoritative reads store.
    let (reads_store, _typ2_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");

    (
        MeterStoreTimeSeriesRepository::new(reads_store, pool.clone()),
        pool,
        container,
        warehouse,
    )
}

fn kwh(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

/// One 15-minute interval for `malo` starting at `from`.
fn read(malo: &str, from: OffsetDateTime, value: &str, sparte: Sparte, obis: &str) -> MeterRead {
    MeterRead {
        malo_id: malo.to_owned(),
        melo_id: None,
        dtm_from: from,
        dtm_to: from + Duration::minutes(15),
        quantity_kwh: kwh(value),
        quality: QualityFlag::Measured,
        pid: 13025,
        sparte,
        obis_code: Some(obis.to_owned()),
        tenant: "9910000000001".to_owned(),
        source: IngestionSource::Mscons,
        push_session: None,
        quality_warnings: None,
        sender_mp_id: Some("9900000000001".to_owned()),
        allocation_version: "INITIAL".to_owned(),
        valid_from_tx: None,
    }
}

fn window(malo: &str, around: OffsetDateTime) -> TimeSeriesQuery {
    TimeSeriesQuery {
        malo_id: malo.to_owned(),
        from: around - Duration::hours(1),
        to: around + Duration::hours(1),
        sparte: None,
        tenant: "9910000000001".to_owned(),
    }
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn schema_check_constraints_reject_bad_data() {
    // Proves the DB-layer CHECKs (quality vocabulary + forward interval/period)
    // are applied by the migration and actually enforced, not just valid syntax.
    let (pool, _url, _wh_uri, _pg, _wh) = boot().await;

    // Off-vocabulary quality → rejected by the quality CHECK.
    let bad_quality = sqlx::query(
        "INSERT INTO meter_billing_periods
           (malo_id, period_from, period_to, arbeitsmenge_kwh, quality, tenant)
         VALUES ('51238696780','2026-01-01','2026-01-31', 1, 'BOGUS', 't')",
    )
    .execute(&pool)
    .await;
    assert!(
        bad_quality.is_err(),
        "an off-vocabulary quality must be rejected by the CHECK"
    );

    // period_to < period_from → rejected by mbp_period_forward.
    let bad_period = sqlx::query(
        "INSERT INTO meter_billing_periods
           (malo_id, period_from, period_to, arbeitsmenge_kwh, quality, tenant)
         VALUES ('51238696780','2026-02-01','2026-01-01', 1, 'MEASURED', 't')",
    )
    .execute(&pool)
    .await;
    assert!(
        bad_period.is_err(),
        "period_to < period_from must be rejected"
    );

    // Reversed audit interval → rejected by mrc_interval_forward.
    let bad_interval = sqlx::query(
        "INSERT INTO meter_read_corrections
           (malo_id, dtm_from, dtm_to, original_kwh, original_quality,
            corrected_kwh, corrected_quality, reason, source, tenant)
         VALUES ('51238696780', now(), now() - interval '1 hour',
                 1, 'MEASURED', 2, 'CORRECTED', 'r', 'OPERATOR', 't')",
    )
    .execute(&pool)
    .await;
    assert!(
        bad_interval.is_err(),
        "a reversed correction interval must be rejected"
    );

    // A well-formed row of each still inserts.
    sqlx::query(
        "INSERT INTO meter_billing_periods
           (malo_id, period_from, period_to, arbeitsmenge_kwh, quality, tenant)
         VALUES ('51238696780','2026-01-01','2026-01-31', 1, 'SUBSTITUTED', 't')",
    )
    .execute(&pool)
    .await
    .expect("a valid billing period must insert");
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn reads_and_typ2_share_a_catalog_but_stay_isolated() {
    // The authoritative reads store and the non-authoritative ESA Typ-2 store are
    // built as two tables over ONE shared Iceberg catalog and DataFusion session.
    // The point of two tables is isolation: an ESA Typ-2 value must never surface
    // in a billing read, and vice versa.
    let (pool, url, warehouse_uri, _pg, _wh) = boot().await;
    let (reads_store, typ2_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");
    let reads = MeterStoreTimeSeriesRepository::new(reads_store, pool.clone());
    let typ2 = MeterStoreTyp2Repository::new(typ2_store);

    let t = OffsetDateTime::now_utc() - Duration::days(1);
    reads
        .store_reads(&[read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0")])
        .await
        .expect("store authoritative read");
    typ2.store_typ2_reads(&[Typ2Read {
        malo_id: "51238696780".to_owned(),
        melo_id: None,
        dtm_from: t,
        dtm_to: t + Duration::minutes(15),
        quantity_kwh: kwh("9.0"),
        quality: QualityFlag::Measured,
        pid: 13027,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        tenant: "9910000000001".to_owned(),
        delivery_path: Typ2DeliveryPath::default(),
        sender_mp_id: None,
        received_at: None,
    }])
    .await
    .expect("store typ2 read");

    // The billing read sees only the authoritative value.
    let billing = reads
        .query(&window("51238696780", t))
        .await
        .expect("query reads");
    assert_eq!(billing.len(), 1);
    assert_eq!(
        billing[0].quantity_kwh,
        kwh("3.5"),
        "the ESA Typ-2 value must never reach a billing read"
    );

    // The Typ-2 read sees only the Typ-2 value.
    let esa = typ2
        .query_typ2(&window("51238696780", t))
        .await
        .expect("query typ2");
    assert_eq!(esa.len(), 1);
    assert_eq!(esa[0].quantity_kwh, kwh("9.0"));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn ingest_roundtrip_preserves_gas_sparte() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    repo.store_reads(&[read("51238696780", t, "3.5", Sparte::Gas, "7-1:3.0.0")])
        .await
        .expect("store gas read");

    let reads = repo.query(&window("51238696780", t)).await.expect("query");
    assert_eq!(reads.len(), 1, "the stored interval reads back");
    assert_eq!(
        reads[0].sparte,
        Sparte::Gas,
        "Sparte must survive the read-back, not default to Strom"
    );
    assert_eq!(reads[0].quantity_kwh, kwh("3.5"));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn ingest_roundtrip_preserves_provenance() {
    // The ingestion source, reporting operator MP-ID and allocation version used
    // to be dropped on the store round-trip and reconstructed with defaults
    // (MSCONS / None / INITIAL) on read-back. They are now declared attribute
    // columns, folded from the newest delivery, so a read-back MeterRead names its
    // true provenance rather than a guess.
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    let mut r = read("51238696780", t, "7.0", Sparte::Strom, "1-0:1.8.0");
    r.source = IngestionSource::DirectPush;
    r.sender_mp_id = Some("9988888888888".to_owned());
    r.allocation_version = "ESA-42".to_owned();
    repo.store_reads(&[r]).await.expect("store read");

    let reads = repo.query(&window("51238696780", t)).await.expect("query");
    assert_eq!(reads.len(), 1);
    assert_eq!(
        reads[0].source,
        IngestionSource::DirectPush,
        "the ingestion source survives the round-trip"
    );
    assert_eq!(
        reads[0].sender_mp_id.as_deref(),
        Some("9988888888888"),
        "the reporting operator MP-ID survives the round-trip"
    );
    assert_eq!(
        reads[0].allocation_version, "ESA-42",
        "the allocation version survives the round-trip"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn billable_filter_excludes_faulty_from_aggregates() {
    // The §60 Abs. 2 billable filter is pushed into the scan (`quality_in`), so a
    // FAULTY interval never reaches the MMM saldo — it is dropped at the storage
    // layer, not re-filtered in memory at each call site.
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    let good = read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    let mut bad = read(
        "51238696780",
        t + Duration::minutes(15),
        "9.0",
        Sparte::Strom,
        "1-0:1.8.0",
    );
    bad.quality = QualityFlag::Faulty;
    repo.store_reads(&[good, bad]).await.expect("store reads");

    let day = t.date();
    let report = repo
        .imbalance("51238696780", day, day, "9910000000001")
        .await
        .expect("imbalance");
    assert_eq!(
        report.lf_quantity_kwh,
        kwh("3.5"),
        "the FAULTY interval is excluded from the billable saldo"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn query_as_of_reconstructs_the_value_in_force() {
    // § 60 Abs. 6 MsbG point-in-time reconstruction through meterstore's
    // transaction-time axis: a correction delivered after the as-of instant is
    // invisible, so the value returned is the one that was in force then.
    let (repo, _pool, _pg, _wh) = setup().await;
    let interval = OffsetDateTime::now_utc() - Duration::days(1);

    // Original delivered 3h ago.
    let mut original = read("51238696780", interval, "3.5", Sparte::Strom, "1-0:1.8.0");
    original.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(3));
    repo.store_reads(&[original]).await.expect("store original");

    // Correction delivered 1h ago (supersedes on current read).
    let mut corrected = read("51238696780", interval, "4.0", Sparte::Strom, "1-0:1.8.0");
    corrected.quality = QualityFlag::Corrected;
    corrected.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(1));
    repo.store_reads(&[corrected])
        .await
        .expect("store correction");

    let q = window("51238696780", interval);

    // Current knowledge: the correction.
    let now_reads = repo.query(&q).await.expect("query");
    assert_eq!(now_reads.len(), 1);
    assert_eq!(now_reads[0].quantity_kwh, kwh("4.0"));

    // As of 2h ago — after the original, before the correction: the original.
    let mid = OffsetDateTime::now_utc() - Duration::hours(2);
    let as_of_reads = repo.query_as_of(&q, mid).await.expect("query_as_of");
    assert_eq!(as_of_reads.len(), 1, "the interval was already known");
    assert_eq!(
        as_of_reads[0].quantity_kwh,
        kwh("3.5"),
        "the correction was not yet known 2h ago"
    );

    // As of now: the correction is visible again.
    let latest = repo
        .query_as_of(&q, OffsetDateTime::now_utc())
        .await
        .expect("query_as_of");
    assert_eq!(latest[0].quantity_kwh, kwh("4.0"));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn query_as_of_hides_an_interval_first_stored_later() {
    // The set-membership property the old audit-table overlay could not give:
    // an interval first stored after the as-of instant is absent, not merely
    // unchanged.
    let (repo, _pool, _pg, _wh) = setup().await;
    let interval = OffsetDateTime::now_utc() - Duration::days(1);

    let mut early = read("51238696780", interval, "3.5", Sparte::Strom, "1-0:1.8.0");
    early.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(3));
    repo.store_reads(&[early]).await.expect("store early");

    // A second interval, first stored 1h ago.
    let later_interval = interval + Duration::minutes(15);
    let mut late = read(
        "51238696780",
        later_interval,
        "9.0",
        Sparte::Strom,
        "1-0:1.8.0",
    );
    late.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(1));
    repo.store_reads(&[late]).await.expect("store late");

    let q = window("51238696780", interval);
    let mid = OffsetDateTime::now_utc() - Duration::hours(2);
    let as_of_reads = repo.query_as_of(&q, mid).await.expect("query_as_of");
    assert_eq!(
        as_of_reads.len(),
        1,
        "only the interval known 2h ago is present"
    );
    assert_eq!(as_of_reads[0].dtm_from, interval);

    // Current knowledge holds both.
    let now_reads = repo.query(&q).await.expect("query");
    assert_eq!(now_reads.len(), 2);
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn latest_read_returns_the_newest_interval() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let base = OffsetDateTime::now_utc() - Duration::days(1);
    let earlier = base;
    let later = base + Duration::minutes(15);

    repo.store_reads(&[
        read("51238696780", earlier, "1.0", Sparte::Strom, "1-0:1.8.0"),
        read("51238696780", later, "2.0", Sparte::Strom, "1-0:1.8.0"),
    ])
    .await
    .expect("store reads");

    let latest = repo
        .latest_read("51238696780", "9910000000001")
        .await
        .expect("latest_read")
        .expect("a reading exists");
    assert_eq!(latest.dtm_from, later, "the newest interval wins");
    assert_eq!(latest.quantity_kwh, kwh("2.0"));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn correction_supersedes_value_and_writes_audit_row() {
    let (repo, pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    // Store the original two hours "ago" (transaction time) so the correction,
    // stored now, is unambiguously the newer version on resolution.
    let mut original = read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    original.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(2));
    repo.store_reads(&[original]).await.expect("store original");

    let ids = repo
        .store_corrections(&[CorrectionRecord {
            malo_id: "51238696780".to_owned(),
            obis_code: Some("1-0:1.8.0".to_owned()),
            dtm_from: t,
            dtm_to: t + Duration::minutes(15),
            original_kwh: kwh("3.5"),
            original_quality: QualityFlag::Measured,
            corrected_kwh: kwh("4.0"),
            corrected_quality: QualityFlag::Corrected,
            reason: "meter re-read".to_owned(),
            source: CorrectionSource::Operator,
            corrected_by: Some("ops-1".to_owned()),
            process_id: None,
            pid: None,
            tenant: "9910000000001".to_owned(),
        }])
        .await
        .expect("store correction");
    assert_eq!(ids.len(), 1, "one correction id returned");

    // The § 60 Abs. 6 immutable audit row was written.
    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM meter_read_corrections WHERE malo_id = $1")
            .bind("51238696780")
            .fetch_one(&pool)
            .await
            .expect("count audit rows");
    assert_eq!(audit_rows, 1, "an immutable correction audit row exists");

    // The corrected value now wins on read (latest-version-wins).
    let reads = repo.query(&window("51238696780", t)).await.expect("query");
    assert_eq!(reads.len(), 1);
    assert_eq!(
        reads[0].quantity_kwh,
        kwh("4.0"),
        "corrected value supersedes"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn ingest_overwrite_audit_row_covers_the_full_interval() {
    // A later delivery overwriting a stored interval opens a § 60 Abs. 6 audit
    // row driven by the store's displacement report. The displacement carries the
    // interval end, so the audit row spans `[dtm_from, dtm_to)` rather than
    // collapsing to a zero-width `[dtm_from, dtm_from)`.
    let (repo, pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    let mut first = read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    first.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(2));
    repo.store_reads(&[first]).await.expect("store first");

    // Same interval, newer transaction time, different value → overwrites.
    let mut second = read("51238696780", t, "9.0", Sparte::Strom, "1-0:1.8.0");
    second.valid_from_tx = Some(OffsetDateTime::now_utc());
    repo.store_reads(&[second]).await.expect("store overwrite");

    let (dtm_from, dtm_to): (OffsetDateTime, OffsetDateTime) = sqlx::query_as(
        "SELECT dtm_from, dtm_to FROM meter_read_corrections WHERE malo_id = $1 AND source = 'MSCONS_UPDATE'",
    )
    .bind("51238696780")
    .fetch_one(&pool)
    .await
    .expect("the overwrite wrote a § 60 Abs. 6 audit row");
    assert_eq!(dtm_from, t);
    assert_eq!(
        dtm_to,
        t + Duration::minutes(15),
        "the audit row covers the interval, not a zero-width collapse to its start"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn gdpr_erasure_unlinks_the_ingest_registered_subject() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    repo.store_reads(&[read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0")])
        .await
        .expect("store read");

    let registry = repo
        .store()
        .subject_registry()
        .expect("authoritative store has a subject registry");

    // The subject is qualified by tenant, so one tenant's erasure cannot unlink
    // another tenant's reading of the same MaLo.
    let natural = edmd::store::subject_natural_id("9910000000001", "51238696780");

    // Ingest registered the MaLo as an erasure subject.
    let subject = registry
        .lookup(&natural)
        .await
        .expect("lookup")
        .expect("ingest registers the MaLo as a subject");

    // Art. 17: destroying the mapping leaves the readings unattributable.
    repo.store()
        .erase_subject(
            &subject,
            "Art. 17 request",
            "dpo",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("erase subject");

    assert!(
        registry
            .lookup(&natural)
            .await
            .expect("lookup after erasure")
            .is_none(),
        "the subject mapping is gone after erasure"
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn reads_are_scoped_to_their_tenant() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    // The SAME MaLo under two tenants, one interval each.
    let mut alpha = read("51238696780", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    alpha.tenant = "9910000000001".to_owned();
    let mut beta = read("51238696780", t, "9.9", Sparte::Strom, "1-0:1.8.0");
    beta.tenant = "9920000000002".to_owned();
    repo.store_reads(&[alpha]).await.expect("store alpha");
    repo.store_reads(&[beta]).await.expect("store beta");

    let q = |tenant: &str| TimeSeriesQuery {
        malo_id: "51238696780".to_owned(),
        from: t - Duration::hours(1),
        to: t + Duration::hours(1),
        sparte: None,
        tenant: tenant.to_owned(),
    };

    let a = repo.query(&q("9910000000001")).await.expect("query alpha");
    assert_eq!(
        a.len(),
        1,
        "alpha sees exactly its own interval, not beta's"
    );
    assert_eq!(
        a[0].quantity_kwh,
        kwh("3.5"),
        "no cross-tenant merge (would be 3.5+9.9) and not beta's 9.9"
    );

    let b = repo.query(&q("9920000000002")).await.expect("query beta");
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].quantity_kwh, kwh("9.9"));
}
