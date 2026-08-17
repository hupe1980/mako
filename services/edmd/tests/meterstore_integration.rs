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
//! - **§ 60 Abs. 2 MsbG Ersatzwertbildung** — a substitute reproduces the same
//!   quarter-hour one week earlier (not a degraded fallback), never overwrites a
//!   real measurement, and leaves one § 60 Abs. 6 audit row per substituted slot.
//! - **A FAULTY slot is a gap** — the case § 60 Abs. 2 exists for. The
//!   substitute displaces the faulty reading rather than coexisting with it,
//!   which only holds while the Ersatzwert inherits the reporting operator that
//!   keys its meterstore version scope.
//! - **Periods are Berlin calendar periods** — German July runs
//!   2026-06-30T22:00Z → 2026-07-31T22:00Z, and a period's quality is its worst
//!   contributor by `severity_rank`, not by discriminant order.

use edmd::domain::validation::ValidatedReads;
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

/// `store_reads` only accepts a batch that has been through V01–V10, so these
/// tests run the same validation the handlers do. Validation annotates and never
/// rejects, so a deliberately faulty fixture still reaches the store.
fn validated(reads: Vec<MeterRead>) -> ValidatedReads {
    let malo = reads
        .first()
        .map_or("51238696012", |r| r.malo_id.as_str())
        .to_owned();
    ValidatedReads::validate(reads, "TEST", &malo).0
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
        mscons_version: None,
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
         VALUES ('51238696012','2026-01-01','2026-01-31', 1, 'BOGUS', 't')",
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
         VALUES ('51238696012','2026-02-01','2026-01-01', 1, 'MEASURED', 't')",
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
         VALUES ('51238696012', now(), now() - interval '1 hour',
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
         VALUES ('51238696012','2026-01-01','2026-01-31', 1, 'SUBSTITUTED', 't')",
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
        .store_reads(validated(vec![read(
            "51238696012",
            t,
            "3.5",
            Sparte::Strom,
            "1-0:1.8.0",
        )]))
        .await
        .expect("store authoritative read");
    typ2.store_typ2_reads(&[Typ2Read {
        malo_id: "51238696012".to_owned(),
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
        .query(&window("51238696012", t))
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
        .query_typ2(&window("51238696012", t))
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

    repo.store_reads(validated(vec![read(
        "51238696012",
        t,
        "3.5",
        Sparte::Gas,
        "7-1:3.0.0",
    )]))
    .await
    .expect("store gas read");

    let reads = repo.query(&window("51238696012", t)).await.expect("query");
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

    let mut r = read("51238696012", t, "7.0", Sparte::Strom, "1-0:1.8.0");
    r.source = IngestionSource::DirectPush;
    r.sender_mp_id = Some("9988888888888".to_owned());
    r.allocation_version = "ESA-42".to_owned();
    repo.store_reads(validated(vec![r]))
        .await
        .expect("store read");

    let reads = repo.query(&window("51238696012", t)).await.expect("query");
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

    let good = read("51238696012", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    let mut bad = read(
        "51238696012",
        t + Duration::minutes(15),
        "9.0",
        Sparte::Strom,
        "1-0:1.8.0",
    );
    bad.quality = QualityFlag::Faulty;
    repo.store_reads(validated(vec![good, bad]))
        .await
        .expect("store reads");

    // The report's period is a Berlin calendar day, so the day the interval
    // belongs to is its *local* one — `t.date()` disagrees for anything in the
    // last one or two UTC hours of a day and would make this test's outcome
    // depend on the hour it runs at.
    let day = metering::calendar::local_day(t);
    let report = repo
        .imbalance("51238696012", day, day, "9910000000001", Sparte::Strom)
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
    let mut original = read("51238696012", interval, "3.5", Sparte::Strom, "1-0:1.8.0");
    original.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(3));
    repo.store_reads(validated(vec![original]))
        .await
        .expect("store original");

    // Correction delivered 1h ago (supersedes on current read).
    let mut corrected = read("51238696012", interval, "4.0", Sparte::Strom, "1-0:1.8.0");
    corrected.quality = QualityFlag::Corrected;
    corrected.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(1));
    repo.store_reads(validated(vec![corrected]))
        .await
        .expect("store correction");

    let q = window("51238696012", interval);

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

    let mut early = read("51238696012", interval, "3.5", Sparte::Strom, "1-0:1.8.0");
    early.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(3));
    repo.store_reads(validated(vec![early]))
        .await
        .expect("store early");

    // A second interval, first stored 1h ago.
    let later_interval = interval + Duration::minutes(15);
    let mut late = read(
        "51238696012",
        later_interval,
        "9.0",
        Sparte::Strom,
        "1-0:1.8.0",
    );
    late.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(1));
    repo.store_reads(validated(vec![late]))
        .await
        .expect("store late");

    let q = window("51238696012", interval);
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

    repo.store_reads(validated(vec![
        read("51238696012", earlier, "1.0", Sparte::Strom, "1-0:1.8.0"),
        read("51238696012", later, "2.0", Sparte::Strom, "1-0:1.8.0"),
    ]))
    .await
    .expect("store reads");

    let latest = repo
        .latest_read("51238696012", "9910000000001")
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
    let mut original = read("51238696012", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    original.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(2));
    repo.store_reads(validated(vec![original]))
        .await
        .expect("store original");

    let ids = repo
        .store_corrections(&[CorrectionRecord {
            malo_id: "51238696012".to_owned(),
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
            .bind("51238696012")
            .fetch_one(&pool)
            .await
            .expect("count audit rows");
    assert_eq!(audit_rows, 1, "an immutable correction audit row exists");

    // The corrected value now wins on read (latest-version-wins).
    let reads = repo.query(&window("51238696012", t)).await.expect("query");
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

    let mut first = read("51238696012", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    first.valid_from_tx = Some(OffsetDateTime::now_utc() - Duration::hours(2));
    repo.store_reads(validated(vec![first]))
        .await
        .expect("store first");

    // Same interval, newer transaction time, different value → overwrites.
    let mut second = read("51238696012", t, "9.0", Sparte::Strom, "1-0:1.8.0");
    second.valid_from_tx = Some(OffsetDateTime::now_utc());
    repo.store_reads(validated(vec![second]))
        .await
        .expect("store overwrite");

    let (dtm_from, dtm_to): (OffsetDateTime, OffsetDateTime) = sqlx::query_as(
        "SELECT dtm_from, dtm_to FROM meter_read_corrections WHERE malo_id = $1 AND source = 'MSCONS_UPDATE'",
    )
    .bind("51238696012")
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

    repo.store_reads(validated(vec![read(
        "51238696012",
        t,
        "3.5",
        Sparte::Strom,
        "1-0:1.8.0",
    )]))
    .await
    .expect("store read");

    let registry = repo
        .store()
        .subject_registry()
        .expect("authoritative store has a subject registry");

    // The subject is qualified by tenant, so one tenant's erasure cannot unlink
    // another tenant's reading of the same MaLo.
    let natural = edmd::store::subject_natural_id("9910000000001", "51238696012");

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
    let mut alpha = read("51238696012", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    alpha.tenant = "9910000000001".to_owned();
    let mut beta = read("51238696012", t, "9.9", Sparte::Strom, "1-0:1.8.0");
    beta.tenant = "9920000000002".to_owned();
    repo.store_reads(validated(vec![alpha]))
        .await
        .expect("store alpha");
    repo.store_reads(validated(vec![beta]))
        .await
        .expect("store beta");

    let q = |tenant: &str| TimeSeriesQuery {
        malo_id: "51238696012".to_owned(),
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

/// § 60 Abs. 2 MsbG Ersatzwertbildung, end to end against a real database.
///
/// The regulated artefact is the *number* a substitute carries, and the method
/// that has to produce it — Vergleichstag, the same slot one week earlier — is
/// the same shape as the seasonal-forecast defect this service already hit
/// once, where the handler passed no reference window and every result
/// silently degraded to a naive fallback. A degraded substitute is
/// indistinguishable from a correct one in the response body, so the assertion
/// has to be on the value.
///
/// The fixture gives every quarter-hour of the prior week a distinct value, so
/// "same slot one week earlier" is the only rule that reproduces it: an average,
/// a carry-forward and a zero-fill all land somewhere else.
///
/// Both missing runs are deliberately **longer than the short-gap threshold**
/// (3 intervals): runs of at most three slots are linearly interpolated
/// whatever method was requested — the VDE-AR-N 4400 hierarchy, pinned by
/// `a_short_gap_interpolates_between_its_real_brackets` below — so a fixture
/// with short runs would (correctly) never see the reference week at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_substitute_reproduces_the_same_slot_one_week_earlier() {
    use edmd::server::{SubstituteRequest, run_substitute_values};
    use time::format_description::well_known::Rfc3339;

    let (repo, pool, _pg, _wh) = setup().await;
    let malo = "51238696012";
    let obis = "1-0:1.8.0";

    // A gap well in the past, so V08 (future timestamp) cannot fire on the
    // reference data, and slot-aligned so the week-earlier slot exists.
    let gap_from = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");
    // Ten quarter-hours: with the measurement at slot 4, the missing runs are
    // 4 and 5 slots — both past the short-gap threshold, so the requested
    // Vergleichstag method is what runs.
    let gap_to = gap_from + Duration::minutes(150);
    let week = Duration::days(7);

    // 672 quarter-hours covering exactly [gap_from - 7d, gap_from), each with a
    // unique value derived from its index.
    let prior: Vec<MeterRead> = (0..672)
        .map(|i| {
            let start = gap_from - week + Duration::minutes(15 * i);
            read(malo, start, &format!("{}.5", i + 1), Sparte::Strom, obis)
        })
        .collect();
    repo.store_reads(validated(prior))
        .await
        .expect("store prior week");

    // One real measurement inside the gap: § 60 Abs. 2 authorises a substitute
    // only where no measurement exists, so this slot must survive untouched.
    let measured_slot = gap_from + Duration::minutes(60);
    repo.store_reads(validated(vec![read(
        malo,
        measured_slot,
        "9999.0",
        Sparte::Strom,
        obis,
    )]))
    .await
    .expect("store the measured slot inside the gap");

    let req = SubstituteRequest {
        gap_from: gap_from.format(&Rfc3339).expect("rfc3339"),
        gap_to: gap_to.format(&Rfc3339).expect("rfc3339"),
        interval_secs: Some(900),
        method: Some("PriorPeriodAverage".to_owned()),
        prior_days: Some(7),
        operator_id: Some("TEST-OPERATOR".to_owned()),
        sparte: Some("STROM".to_owned()),
        reason: Some("NoMeasurementAvailable".to_owned()),
        obis_code: Some(obis.to_owned()),
    };
    let status = run_substitute_values(&repo, "9910000000001", malo, &req)
        .await
        .status();
    assert_eq!(
        status.as_u16(),
        201,
        "the substitute run must succeed with a full reference week"
    );

    let stored = repo
        .query(&TimeSeriesQuery {
            malo_id: malo.to_owned(),
            from: gap_from,
            to: gap_to,
            sparte: None,
            tenant: "9910000000001".to_owned(),
        })
        .await
        .expect("read the gap window back");

    // Gap slot `n` starts at `gap_from + 15n min`; one week earlier that is
    // `gap_from - 7d + 15n min`, which is prior index `n` — value `n + 1` .5.
    for (n, slot) in (0..10).map(|n| (n, gap_from + Duration::minutes(15 * n))) {
        let row = stored
            .iter()
            .find(|r| r.dtm_from == slot)
            .unwrap_or_else(|| panic!("no reading at gap slot {n}"));

        if slot == measured_slot {
            assert_eq!(
                row.quantity_kwh,
                kwh("9999.0"),
                "a slot that already carried a measurement must not be substituted"
            );
            assert_eq!(
                row.quality,
                QualityFlag::Measured,
                "the surviving measurement must keep its quality"
            );
            continue;
        }

        let expected = kwh(&format!("{}.5", n + 1));
        assert_eq!(
            row.quantity_kwh, expected,
            "gap slot {n} must reproduce the value from the same slot one week \
             earlier ({expected}), not a degraded fallback"
        );
        assert_eq!(
            row.quality,
            QualityFlag::Substituted,
            "every § 60 Abs. 2 Ersatzwert must be flagged Substituted, or it is \
             indistinguishable from a measurement"
        );
    }

    // § 60 Abs. 6 MsbG: one audit row per substituted interval, and none for the
    // slot that was left alone.
    let logged: Vec<(OffsetDateTime, String)> = sqlx::query_as(
        "SELECT dtm_from, method FROM substitute_value_log
          WHERE malo_id = $1 AND tenant = $2 ORDER BY dtm_from",
    )
    .bind(malo)
    .bind("9910000000001")
    .fetch_all(&pool)
    .await
    .expect("read the substitute audit log");

    assert_eq!(
        logged.len(),
        9,
        "nine substituted slots must each leave an audit row, and the measured \
         slot none — got {logged:?}"
    );
    assert!(
        logged.iter().all(|(_, m)| m == "PriorPeriodAverage"),
        "the audit row must name the method that actually ran: {logged:?}"
    );
    assert!(
        logged.iter().all(|(t, _)| *t != measured_slot),
        "the untouched measured slot must not appear in the audit log"
    );
}

/// A run of at most three missing slots is linearly interpolated between its
/// real neighbours — the VDE-AR-N 4400 short-gap rule — even when those
/// neighbours lie *outside* the requested gap window.
///
/// This pins the bracket plumbing: the engine derives a gap's opening bracket
/// only from slots its walk has visited, so the handler must start the fill at
/// the last billable reading before the gap. Before it did, a leading short
/// gap had no left bracket and "interpolation" silently produced a flat copy
/// of the closing value — a degraded number indistinguishable from a correct
/// one in the response body. The audit row must also name what actually ran
/// (`LinearInterpolation`), not what was requested.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_short_gap_interpolates_between_its_real_brackets() {
    use edmd::server::{SubstituteRequest, run_substitute_values};
    use time::format_description::well_known::Rfc3339;

    let (repo, pool, _pg, _wh) = setup().await;
    let malo = "51238696012";
    let obis = "1-0:1.8.0";

    // Brackets at T0 and T0+45min; the two slots between them are missing.
    // Both brackets are outside the requested window [T0+15, T0+45).
    let t0 = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");
    repo.store_reads(validated(vec![
        read(malo, t0, "100.0", Sparte::Strom, obis),
        read(
            malo,
            t0 + Duration::minutes(45),
            "400.0",
            Sparte::Strom,
            obis,
        ),
    ]))
    .await
    .expect("store the bracketing measurements");

    let gap_from = t0 + Duration::minutes(15);
    let gap_to = t0 + Duration::minutes(45);
    let req = SubstituteRequest {
        gap_from: gap_from.format(&Rfc3339).expect("rfc3339"),
        gap_to: gap_to.format(&Rfc3339).expect("rfc3339"),
        interval_secs: Some(900),
        // Deliberately *not* interpolation: the short-gap rule must override
        // the requested method, and the audit trail must say so.
        method: Some("PriorPeriodAverage".to_owned()),
        prior_days: Some(7),
        operator_id: Some("TEST-OPERATOR".to_owned()),
        sparte: Some("STROM".to_owned()),
        reason: Some("NoMeasurementAvailable".to_owned()),
        obis_code: Some(obis.to_owned()),
    };
    let status = run_substitute_values(&repo, "9910000000001", malo, &req)
        .await
        .status();
    assert_eq!(status.as_u16(), 201, "the substitute run must succeed");

    let stored = repo
        .query(&TimeSeriesQuery {
            malo_id: malo.to_owned(),
            from: gap_from,
            to: gap_to,
            sparte: None,
            tenant: "9910000000001".to_owned(),
        })
        .await
        .expect("read the gap window back");

    // Two unknowns between 100 and 400 sit at thirds: 200 and 300. A flat
    // copy of either bracket is exactly the degradation this test exists to
    // refuse.
    for (slot, expected) in [
        (gap_from, kwh("200.0")),
        (gap_from + Duration::minutes(15), kwh("300.0")),
    ] {
        let row = stored
            .iter()
            .find(|r| r.dtm_from == slot)
            .unwrap_or_else(|| panic!("no reading at {slot}"));
        assert_eq!(
            row.quantity_kwh, expected,
            "a short gap must interpolate between its real brackets \
             (100 → 400 at thirds), not copy one of them"
        );
        assert_eq!(row.quality, QualityFlag::Substituted);
    }

    // Nothing outside the requested window was written: the run-up slot the
    // handler walks for its opening bracket is bracket, not product.
    let logged: Vec<(OffsetDateTime, String)> = sqlx::query_as(
        "SELECT dtm_from, method FROM substitute_value_log
          WHERE malo_id = $1 AND tenant = $2 ORDER BY dtm_from",
    )
    .bind(malo)
    .bind("9910000000001")
    .fetch_all(&pool)
    .await
    .expect("read the substitute audit log");
    assert_eq!(
        logged.len(),
        2,
        "exactly the two requested slots leave audit rows — got {logged:?}"
    );
    assert!(
        logged.iter().all(|(_, m)| m == "LinearInterpolation"),
        "the audit row must name the method that actually ran, not the one \
         requested: {logged:?}"
    );
}

/// A Liefermonat is a **Berlin** calendar period, not a UTC one.
///
/// German July 2026 runs from 2026-06-30T22:00Z to 2026-07-31T22:00Z. Deriving
/// the window from naive UTC midnight instead misses the first two hours of the
/// month and reaches two hours into August — an off-by-one-to-two-hour error at
/// every month edge, and the wrong length across a DST transition. The fixture
/// puts one quarter-hour on each side of both boundaries, so a UTC window and a
/// Berlin one cannot produce the same total.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_billing_period_is_a_berlin_month_not_a_utc_one() {
    use edmd::domain::BillingPeriodQuery;
    use time::macros::{date, datetime};

    let (repo, _pool, _pg, _wh) = setup().await;
    let malo = "51238696012";
    let obis = "1-0:1.8.0";

    let reads = vec![
        // Still June in Berlin (23:45 CEST on the 30th).
        read(
            malo,
            datetime!(2026-06-30 21:45 UTC),
            "100",
            Sparte::Strom,
            obis,
        ),
        // The first quarter-hour of German July.
        read(
            malo,
            datetime!(2026-06-30 22:00 UTC),
            "1",
            Sparte::Strom,
            obis,
        ),
        // The last quarter-hour of German July (23:45 CEST on the 31st).
        read(
            malo,
            datetime!(2026-07-31 21:45 UTC),
            "2",
            Sparte::Strom,
            obis,
        ),
        // Already August in Berlin.
        read(
            malo,
            datetime!(2026-07-31 22:00 UTC),
            "500",
            Sparte::Strom,
            obis,
        ),
    ];
    repo.store_reads(validated(reads))
        .await
        .expect("store the month-boundary fixture");

    let period = repo
        .billing_period(&BillingPeriodQuery {
            sparte: Sparte::Strom,
            malo_id: malo.to_owned(),
            period_from: date!(2026 - 07 - 01),
            period_to: date!(2026 - 07 - 31),
            tenant: "9910000000001".to_owned(),
        })
        .await
        .expect("billing period")
        .expect("the period has readings");

    assert_eq!(
        period.arbeitsmenge_kwh,
        kwh("3"),
        "German July is [2026-06-30T22:00Z, 2026-07-31T22:00Z): it holds both \
         boundary quarter-hours and neither neighbour"
    );
}

/// The quality a period reports is its worst contributor, ranked the way the
/// domain ranks it.
///
/// `QualityFlag`'s declaration order is not the severity order —
/// `severity_rank` puts Corrected/Substituted at 2 and Estimated at 3 — so
/// picking the maximum discriminant reports a period of [Estimated, Corrected]
/// as CORRECTED, which understates it, and caches that answer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_periods_quality_is_its_worst_contributor_by_severity_rank() {
    use edmd::domain::BillingPeriodQuery;
    use time::macros::{date, datetime};

    let (repo, _pool, _pg, _wh) = setup().await;
    let malo = "51238696781";
    let obis = "1-0:1.8.0";

    let mut estimated = read(
        malo,
        datetime!(2026-07-10 10:00 UTC),
        "4",
        Sparte::Strom,
        obis,
    );
    estimated.quality = QualityFlag::Estimated;
    let mut corrected = read(
        malo,
        datetime!(2026-07-10 10:15 UTC),
        "6",
        Sparte::Strom,
        obis,
    );
    corrected.quality = QualityFlag::Corrected;
    repo.store_reads(validated(vec![estimated, corrected]))
        .await
        .expect("store the mixed-quality period");

    let period = repo
        .billing_period(&BillingPeriodQuery {
            sparte: Sparte::Strom,
            malo_id: malo.to_owned(),
            period_from: date!(2026 - 07 - 01),
            period_to: date!(2026 - 07 - 31),
            tenant: "9910000000001".to_owned(),
        })
        .await
        .expect("billing period")
        .expect("the period has readings");

    assert_eq!(
        period.quality,
        QualityFlag::Estimated,
        "Estimated outranks Corrected on the canonical severity ranking, so a \
         period holding both is Estimated"
    );
}

/// § 60 Abs. 2 MsbG exists for the meter that reported something wrong, not
/// only for the one that reported nothing.
///
/// A FAULTY interval is deliberately stored — ingest annotates and never
/// rejects — so a substitute flow that treats "a row exists" as "a measurement
/// exists" generates nothing for exactly the case the paragraph is about, and
/// answers `generated_count: 0`. The reference series must therefore be
/// filtered to billable qualities: a FAULTY slot is a gap.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_faulty_slot_is_a_gap_a_substitute_may_fill() {
    use edmd::server::{SubstituteRequest, run_substitute_values};
    use time::format_description::well_known::Rfc3339;

    let (repo, pool, _pg, _wh) = setup().await;
    let malo = "51238696129";
    let obis = "1-0:1.8.0";

    let gap_from = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");
    let gap_to = gap_from + Duration::minutes(150);
    let week = Duration::days(7);

    // A full, distinct reference week, so the Vergleichstag value is checkable.
    let prior: Vec<MeterRead> = (0..672)
        .map(|i| {
            let start = gap_from - week + Duration::minutes(15 * i);
            read(malo, start, &format!("{}.5", i + 1), Sparte::Strom, obis)
        })
        .collect();
    repo.store_reads(validated(prior))
        .await
        .expect("store prior week");

    // The window is not empty: every slot in it carries a FAULTY row. Nothing
    // here is a measurement, so every slot is substitutable.
    let faulty: Vec<MeterRead> = (0..10)
        .map(|n| {
            let mut r = read(
                malo,
                gap_from + Duration::minutes(15 * n),
                "0.0",
                Sparte::Strom,
                obis,
            );
            r.quality = QualityFlag::Faulty;
            r
        })
        .collect();
    repo.store_reads(validated(faulty))
        .await
        .expect("store the faulty window");

    let req = SubstituteRequest {
        gap_from: gap_from.format(&Rfc3339).expect("rfc3339"),
        gap_to: gap_to.format(&Rfc3339).expect("rfc3339"),
        interval_secs: Some(900),
        method: Some("PriorPeriodAverage".to_owned()),
        prior_days: Some(7),
        operator_id: Some("TEST-OPERATOR".to_owned()),
        sparte: None,
        reason: Some("MeterFault".to_owned()),
        obis_code: Some(obis.to_owned()),
    };
    let status = run_substitute_values(&repo, "9910000000001", malo, &req)
        .await
        .status();
    assert_eq!(
        status.as_u16(),
        201,
        "a window full of FAULTY readings is exactly what § 60 Abs. 2 authorises \
         a substitute for — it must not answer `generated_count: 0`"
    );

    let stored = repo
        .query(&TimeSeriesQuery {
            malo_id: malo.to_owned(),
            from: gap_from,
            to: gap_to,
            sparte: None,
            tenant: "9910000000001".to_owned(),
        })
        .await
        .expect("read the gap window back");

    // Exactly one row per slot. The Ersatzwert has to *displace* the faulty
    // reading, not sit beside it — meterstore versions only supersede within a
    // version scope, and that scope is keyed on the reporting operator, so a
    // substitute filed under the tenant would leave both rows standing and
    // double-count every slot in every aggregate.
    assert_eq!(
        stored.len(),
        10,
        "the substitute must supersede the faulty reading, not coexist with it: \
         {stored:#?}"
    );

    for n in 0..10i64 {
        let slot = gap_from + Duration::minutes(15 * n);
        let row = stored
            .iter()
            .find(|r| r.dtm_from == slot)
            .unwrap_or_else(|| panic!("no reading at slot {n}"));
        assert_eq!(
            row.quality,
            QualityFlag::Substituted,
            "slot {n} must have been replaced by an Ersatzwert, not left FAULTY"
        );
        assert_eq!(
            row.quantity_kwh,
            kwh(&format!("{}.5", n + 1)),
            "the Ersatzwert must be the same slot one week earlier, and the \
             FAULTY 0.0 must not have been used as reference data"
        );
    }

    // The Sparte comes from the resolved series, not the (omitted) request
    // field, so a Strom gap stays Strom rather than defaulting by luck.
    assert!(
        stored.iter().all(|r| r.sparte == Sparte::Strom),
        "the substitute must inherit the stored series' Sparte"
    );

    let logged: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM substitute_value_log WHERE malo_id = $1 AND tenant = $2",
    )
    .bind(malo)
    .bind("9910000000001")
    .fetch_one(&pool)
    .await
    .expect("read the substitute audit log");
    assert_eq!(logged, 10, "one § 60 Abs. 6 audit row per substituted slot");
}
