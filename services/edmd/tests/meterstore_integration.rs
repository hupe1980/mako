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
//!   `meter_read_corrections` audit row (§ 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD)).
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
    BillingPeriodQuery, CorrectionRecord, CorrectionSource, IngestionSource, MeterRead,
    QualityFlag, Sparte, TimeSeriesQuery, TimeSeriesRepository, Typ2DeliveryPath, Typ2Read,
    Typ2Repository,
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
    let (reads_store, _typ2_store, zsg_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");

    (
        MeterStoreTimeSeriesRepository::new(reads_store, zsg_store, pool.clone()),
        pool,
        container,
        warehouse,
    )
}

/// A 33-character Zählpunktbezeichnung, for the ZSG point table whose merge key
/// includes the Messlokation.
const TEST_MELO: &str = "DE0001234567890123456789012345678";

fn kwh(s: &str) -> Decimal {
    s.parse().expect("decimal")
}

/// `store_reads` only accepts a batch that has been through the V-rules, so these
/// tests run the same validation the handlers do. Validation annotates and never
/// rejects, so a deliberately faulty fixture still reaches the store.
fn validated(reads: Vec<MeterRead>) -> ValidatedReads {
    let malo = reads
        .first()
        .map_or("51238696012", |r| r.malo_id.as_str())
        .to_owned();
    ValidatedReads::validate(reads, edmd::domain::IngestContext::new("TEST", &malo)).0
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
    let (reads_store, typ2_store, zsg_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");
    let reads = MeterStoreTimeSeriesRepository::new(reads_store, zsg_store, pool.clone());
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
        bestellung_ref: Some("ESABE0000000001".to_owned()),
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
    // `SG1 RFF+AGI` — which subscription these values are. A Meldepunkt may
    // carry several (a subscription is the (Meldepunkt, Messprodukt) pair), so
    // without it the delivery-surveillance sweep can only report that *some*
    // register went quiet, never which subscription stopped.
    assert_eq!(
        esa[0].bestellung_ref.as_deref(),
        Some("ESABE0000000001"),
        "the ordering ORDERS' Belegnummer must survive the store round trip"
    );
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
    // 4.0 kWh was allocated to the Bilanzkreis from the profile; 3.5 kWh was
    // actually measured (the FAULTY 9.0 does not count). Under-consumption
    // against the profile is a **Mehr**menge, which the network operator
    // credits — the sign convention is named from the NB's side (GPKE Teil 1
    // Kap. 8.4 Nr. 3).
    let report = repo
        .imbalance(
            "51238696012",
            day,
            day,
            "9910000000001",
            Sparte::Strom,
            kwh("4.0"),
        )
        .await
        .expect("imbalance");
    assert_eq!(
        report.gemessen_kwh,
        kwh("3.5"),
        "the FAULTY interval is excluded from the billable saldo"
    );
    assert_eq!(report.bilanziert_kwh, kwh("4.0"));
    assert_eq!(
        report.mehrmenge_kwh,
        kwh("0.5"),
        "consuming under the profile leaves a Mehrmenge the NB credits"
    );
    assert_eq!(report.mindermenge_kwh, kwh("0"));
    assert_eq!(report.delta_kwh, kwh("-0.5"));
    assert_eq!(
        report.quality,
        QualityFlag::Measured,
        "only billable reads contributed, so the worst quality is MEASURED"
    );
    assert_eq!(report.interval_count, 1);

    // The other direction, so a zero delta can only come from a real balance.
    let over = repo
        .imbalance(
            "51238696012",
            day,
            day,
            "9910000000001",
            Sparte::Strom,
            kwh("3.0"),
        )
        .await
        .expect("imbalance");
    assert_eq!(over.mindermenge_kwh, kwh("0.5"));
    assert_eq!(over.mehrmenge_kwh, kwh("0"));
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn query_as_of_reconstructs_the_value_in_force() {
    // § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD) point-in-time reconstruction through meterstore's
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

    // § 147 Abs. 1 AO / § 146 Abs. 4 AO (GoBD): one audit row per substituted interval, and none for the
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

// ── Fixes pinned against a real database ─────────────────────────────────────

/// A correction invalidates the cached billing-period aggregate.
///
/// `billing_period` is read-through: it caches what it computes and returns the
/// cached row on the next call. The ingest path has always dropped the cache for
/// the window it touched; the correction path did not — so an explicit § 147 AO
/// correction was the one write that could never reach an invoice. `invoicd` and
/// `netzbilanzd` kept billing the pre-correction total indefinitely.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_correction_invalidates_the_cached_billing_period() {
    use edmd::domain::BillingPeriodQuery;

    let (repo, pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(2);
    let day = metering::calendar::local_day(t);

    repo.store_reads(validated(vec![read(
        "51238696012",
        t,
        "10.0",
        Sparte::Strom,
        "1-0:1.8.0",
    )]))
    .await
    .expect("store reads");

    let q = BillingPeriodQuery {
        malo_id: "51238696012".to_owned(),
        period_from: day,
        period_to: day,
        tenant: "9910000000001".to_owned(),
        sparte: Sparte::Strom,
    };

    // First read computes and caches.
    let before = repo.billing_period(&q).await.expect("aggregate").unwrap();
    assert_eq!(before.arbeitsmenge_kwh, kwh("10.0"));
    let cached: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods WHERE malo_id = $1")
            .bind("51238696012")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(cached, 1, "the read-through path caches its result");

    repo.store_corrections(&[CorrectionRecord {
        malo_id: "51238696012".to_owned(),
        obis_code: Some("1-0:1.8.0".to_owned()),
        dtm_from: t,
        dtm_to: t + Duration::minutes(15),
        original_kwh: kwh("10.0"),
        original_quality: QualityFlag::Measured,
        corrected_kwh: kwh("4.0"),
        corrected_quality: QualityFlag::Corrected,
        reason: "Zählerfehlstand".to_owned(),
        source: CorrectionSource::Operator,
        corrected_by: Some("dispatcher@nb.example".to_owned()),
        process_id: None,
        pid: None,
        tenant: "9910000000001".to_owned(),
    }])
    .await
    .expect("store correction");

    let stale: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM meter_billing_periods WHERE malo_id = $1")
            .bind("51238696012")
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(stale, 0, "the correction must drop the cached aggregate");

    let after = repo.billing_period(&q).await.expect("aggregate").unwrap();
    assert_eq!(
        after.arbeitsmenge_kwh,
        kwh("4.0"),
        "the next billing read must see the corrected value"
    );
}

/// A Gasbeschaffenheit delivery is recorded and reads back.
///
/// PID 13007 used to patch `meter_billing_periods` and nothing else:
/// `gas_quality_data` was declared, read by two endpoints, and never written, so
/// `GET /api/v1/gas-quality/{malo_id}` could only ever be empty — and in fact
/// errored, because it selected a column named `pid` from a table whose column
/// is `source_pid`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_gas_quality_delivery_is_recorded_and_reads_back() {
    use edmd::domain::GasQualityRecord;
    use time::macros::date;

    let (repo, pool, _pg, _wh) = setup().await;

    repo.record_gas_quality(&GasQualityRecord {
        tenant: "9910000000001".to_owned(),
        malo_id: "51238696012".to_owned(),
        period_from: date!(2026 - 07 - 01),
        period_to: date!(2026 - 07 - 31),
        brennwert_kwh_per_m3: Some(kwh("11.2340")),
        zustandszahl: Some(kwh("0.9563")),
        source_pid: Some(13007),
    })
    .await
    .expect("record gas quality");

    // The exact SELECT the endpoint and the MCP tool run.
    let row = sqlx::query_as::<_, (Option<Decimal>, Option<Decimal>, Option<i32>)>(
        "SELECT brennwert_kwh_per_m3, zustandszahl, source_pid
           FROM gas_quality_data WHERE malo_id = $1 AND tenant = $2",
    )
    .bind("51238696012")
    .bind("9910000000001")
    .fetch_one(&pool)
    .await
    .expect("gas quality reads back");
    assert_eq!(row.0, Some(kwh("11.2340")));
    assert_eq!(row.1, Some(kwh("0.9563")));
    assert_eq!(row.2, Some(13007));

    // A re-delivery for the same period supersedes rather than duplicating.
    repo.record_gas_quality(&GasQualityRecord {
        tenant: "9910000000001".to_owned(),
        malo_id: "51238696012".to_owned(),
        period_from: date!(2026 - 07 - 01),
        period_to: date!(2026 - 07 - 31),
        brennwert_kwh_per_m3: Some(kwh("11.4000")),
        zustandszahl: None,
        source_pid: Some(13007),
    })
    .await
    .expect("re-record");

    let (count, brennwert, zustandszahl) =
        sqlx::query_as::<_, (i64, Option<Decimal>, Option<Decimal>)>(
            "SELECT COUNT(*) OVER (), brennwert_kwh_per_m3, zustandszahl
           FROM gas_quality_data WHERE malo_id = $1",
        )
        .bind("51238696012")
        .fetch_one(&pool)
        .await
        .expect("one row");
    assert_eq!(count, 1, "a re-delivery supersedes the period, not appends");
    assert_eq!(brennwert, Some(kwh("11.4000")));
    assert_eq!(
        zustandszahl,
        Some(kwh("0.9563")),
        "a partial delivery keeps the factor it did not carry"
    );
}

/// Two tenants may mint the same session id; neither may overwrite the other.
///
/// Every read of `direct_push_sessions` was tenant-scoped, but the table was
/// keyed on `session_id` alone — so the ingest upsert's `ON CONFLICT` landed on
/// whichever tenant got there first and rewrote its status and quality summary.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_push_session_id_is_private_to_its_tenant() {
    let (_repo, pool, _pg, _wh) = setup().await;

    for (tenant, malo, count) in [
        ("9910000000001", "51238696012", 96),
        ("9910000000002", "51238696781", 42),
    ] {
        sqlx::query(
            "INSERT INTO direct_push_sessions
                 (session_id, malo_id, interval_count, status, tenant)
             VALUES ('SMGW-SN1234-2026-07-12', $1, $2, 'committed', $3)
             ON CONFLICT (tenant, session_id) DO UPDATE SET status = 'committed'",
        )
        .bind(malo)
        .bind(count)
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("both tenants may use the same session id");
    }

    let rows: Vec<(String, String, i32)> = sqlx::query_as(
        "SELECT tenant, malo_id, interval_count FROM direct_push_sessions
          WHERE session_id = 'SMGW-SN1234-2026-07-12' ORDER BY tenant",
    )
    .fetch_all(&pool)
    .await
    .expect("query");
    assert_eq!(rows.len(), 2, "one row per tenant, not one shared row");
    assert_eq!(rows[0], ("9910000000001".into(), "51238696012".into(), 96));
    assert_eq!(rows[1], ("9910000000002".into(), "51238696781".into(), 42));
}

/// A Gas reading is stored — and labelled — in kWh.
///
/// Every ingest door converts m³ → kWh_Hs before the value reaches the store
/// (§ 25 Nr. 4 MessEV), but the stored unit was the Sparte's *measured* unit, so
/// gas was tagged `m³`. Anything trusting the unit — the BO4E `Mengeneinheit`,
/// an external engine reading the cold tier — saw a tenth of the real quantity.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_gas_reading_is_stored_in_its_billing_unit() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    repo.store_reads(validated(vec![read(
        "51238696012",
        t,
        "112.34",
        Sparte::Gas,
        "7-1:99.33.17",
    )]))
    .await
    .expect("store gas reads");

    let unit: Option<String> = sqlx::query_scalar(
        r#"SELECT "unit" FROM meter_reads_versions WHERE "malo_id" = $1 LIMIT 1"#,
    )
    .bind("51238696012")
    .fetch_optional(repo.pool())
    .await
    .expect("query the raw versioned table")
    .flatten();

    assert_eq!(
        unit.as_deref(),
        Some(metering::Sparte::Gas.billing_unit().as_str()),
        "gas is converted at ingest, so it is stored — and must be labelled — in kWh"
    );

    // The value itself survives untouched either way.
    let reads = repo.query(&window("51238696012", t)).await.expect("query");
    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].quantity_kwh, kwh("112.34"));
    assert_eq!(reads[0].sparte, Sparte::Gas);
}

// ── Delivery surveillance ─────────────────────────────────────────────────────

/// A measuring point that stops delivering is found, reported once, and closed
/// when it comes back.
///
/// This is the failure every other quality mechanism misses. The V-rules run on
/// an ingest batch, the Hampel scorer grades one, the § 60 Abs. 2 confirmation
/// loop chases estimates already written — all of them are triggered by a
/// delivery. Silence triggers nothing, so a broken head-end was invisible until
/// a settlement run came up short.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_silent_measuring_point_is_found_reported_once_and_closed_on_return() {
    use edmd::config::SurveillanceConfig;
    use edmd::surveillance::{DeliveryState, run_surveillance_sweep};

    let (repo, pool, _pg, _wh) = setup().await;
    let now = OffsetDateTime::now_utc();
    let cfg = SurveillanceConfig {
        typ2_enabled: true,
        typ2_silent_after_hours: 36,
        enabled: true,
        silent_after_hours: 36,
        min_coverage_pct: 95.0,
        coverage_window_days: 7,
        sweep_interval_secs: 3600,
        max_events_per_sweep: 500,
    };

    // A point that delivered hourly up to three days ago and then went dark.
    // Both halves matter: it has real history, so it is a *stopped* point rather
    // than one that never started, and closing it later needs that history back.
    let hourly = |malo: &str, from_hours_ago: i64, to_hours_ago: i64| -> Vec<MeterRead> {
        (to_hours_ago..from_hours_ago)
            .rev()
            .map(|h| {
                let mut r = read(
                    malo,
                    now - Duration::hours(h),
                    "1.0",
                    Sparte::Strom,
                    "1-0:1.8.0",
                );
                r.dtm_to = r.dtm_from + Duration::hours(1);
                r
            })
            .collect()
    };
    repo.store_reads(validated(hourly("51238696012", 7 * 24, 3 * 24)))
        .await
        .expect("store history up to three days ago");

    // A point delivering right up to now, at full coverage across the window.
    repo.store_reads(validated(hourly("51238696781", 7 * 24, 0)))
        .await
        .expect("store fresh reads");

    // ── First sweep: the stale point is opened ────────────────────────────────
    let first = run_surveillance_sweep(&repo, &cfg, "9910000000001", None, None).await;
    let silent: Vec<_> = first
        .findings
        .iter()
        .filter(|f| f.state == DeliveryState::Silent)
        .collect();
    assert_eq!(silent.len(), 1, "one silent point: {:?}", first.findings);
    assert_eq!(silent[0].malo_id, "51238696012");
    assert!(
        silent[0].hours_silent >= 71,
        "roughly three days dark, got {}",
        silent[0].hours_silent
    );
    assert_eq!(first.newly_overdue, 1, "the transition is reported");
    assert!(
        !first.findings.iter().any(|f| f.malo_id == "51238696781"),
        "a point delivering to the present is not a finding"
    );

    // ── Second sweep, nothing changed: silent, not re-announced ───────────────
    let second = run_surveillance_sweep(&repo, &cfg, "9910000000001", None, None).await;
    assert_eq!(
        second.findings.len(),
        1,
        "the point is still open in the register"
    );
    assert_eq!(
        second.newly_overdue, 0,
        "a standing fault must not re-emit — this is what made the old \
         append-only compliance log a daily event storm"
    );

    // The register holds exactly one row for it, not one per sweep.
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM delivery_surveillance WHERE tenant = $1 AND malo_id = $2",
    )
    .bind("9910000000001")
    .bind("51238696012")
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(rows, 1, "one row per point, re-sighted in place");

    // ── The point delivers again ──────────────────────────────────────────────
    // Backfilling the dark days is what a real recovery looks like — the head-end
    // reconnects and replays, or § 60 Abs. 2 Ersatzwerte fill the hole. A single
    // fresh interval would end the *silence* but leave the window uncovered, so
    // the point would move SILENT → UNDER_COVERED and stay open, which is the
    // correct behaviour and not what this test is about.
    repo.store_reads(validated(hourly("51238696012", 3 * 24, 0)))
        .await
        .expect("store the backfill");

    let third = run_surveillance_sweep(&repo, &cfg, "9910000000001", None, None).await;
    assert_eq!(third.resumed, 1, "the recovered point is closed");
    let resolved: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT resolved_at FROM delivery_surveillance WHERE tenant = $1 AND malo_id = $2",
    )
    .bind("9910000000001")
    .bind("51238696012")
    .fetch_one(&pool)
    .await
    .expect("row survives, resolved");
    assert!(resolved.is_some(), "resolution is recorded, not deleted");
}

/// Surveillance is tenant-scoped: one tenant's silence is not another's.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn surveillance_does_not_cross_tenants() {
    use edmd::config::SurveillanceConfig;
    use edmd::surveillance::run_surveillance_sweep;

    let (repo, _pool, _pg, _wh) = setup().await;
    let now = OffsetDateTime::now_utc();
    let cfg = SurveillanceConfig::default();

    let mut other = read(
        "51238696012",
        now - Duration::days(5),
        "2.5",
        Sparte::Strom,
        "1-0:1.8.0",
    );
    other.tenant = "9910000000002".to_owned();
    repo.store_reads(validated(vec![other]))
        .await
        .expect("store other tenant's stale read");

    let report = run_surveillance_sweep(&repo, &cfg, "9910000000001", None, None).await;
    assert!(
        report.findings.is_empty(),
        "another tenant's silent point is not this tenant's finding: {:?}",
        report.findings
    );
    assert_eq!(report.points_scanned, 0);
}

// ── §14a compliance register ──────────────────────────────────────────────────

/// A standing §14a fault is announced once, not once per sweep.
///
/// An append-only compliance log — a row and a CloudEvent per open issue per
/// daily sweep — gives a gateway on an expired certificate one
/// `de.messwert.cls.compliance-issue` a day for as long as nobody fixes it: for
/// a fleet, an unbounded event stream saying the same thing forever.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_standing_compliance_fault_is_announced_once_and_closed_when_fixed() {
    use edmd::smgw::run_cls_compliance_sweep;
    use edmd::smgw_model::{CertificateType, GatewayCertificate, GatewayStatus, SmgwSession};
    use time::macros::date;

    let (_repo, pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";

    let session = |valid_to: time::Date, status: GatewayStatus| SmgwSession {
        device_id: "SMGW-0001".to_owned(),
        firmware_version: "1.2.3".to_owned(),
        msb_mp_id: "9900000000001".to_owned(),
        malo_id: "51238696012".to_owned(),
        status,
        certificates: vec![GatewayCertificate {
            serial_number: "AB12".to_owned(),
            cert_type: CertificateType::Tls,
            subject_cn: "SMGW-0001".to_owned(),
            issuer_cn: "Smart Meter CA".to_owned(),
            valid_from: date!(2020 - 01 - 01),
            valid_to,
            is_revoked: false,
            revoked_at: None,
        }],
        cls_channels: Vec::new(),
        last_contact_at: Some(OffsetDateTime::now_utc()),
        installed_at: date!(2020 - 01 - 01),
    };

    async fn store(pool: &sqlx::PgPool, tenant: &str, s: &SmgwSession, status: &str) {
        sqlx::query(
            r"INSERT INTO smgw_sessions
                  (malo_id, tenant, device_id, msb_mp_id, gateway_status, session)
              VALUES ($1,$2,$3,$4,$5,$6)
              ON CONFLICT (malo_id, tenant) DO UPDATE
              SET gateway_status = EXCLUDED.gateway_status,
                  session        = EXCLUDED.session",
        )
        .bind(&s.malo_id)
        .bind(tenant)
        .bind(&s.device_id)
        .bind(&s.msb_mp_id)
        .bind(status)
        .bind(serde_json::to_value(s).expect("serialise"))
        .execute(pool)
        .await
        .expect("store session");
    }

    // An expired TLS certificate — a fault that will not fix itself.
    let expired = session(date!(2021 - 01 - 01), GatewayStatus::Operational);
    store(&pool, tenant, &expired, "OPERATIONAL").await;

    let first = run_cls_compliance_sweep(&pool, tenant, None, None, 30, 2).await;
    assert_eq!(first.sessions_with_issues, 1);
    assert_eq!(
        first.newly_opened, 1,
        "the fault is announced on the way in"
    );

    // Three more sweeps change nothing.
    for _ in 0..3 {
        let again = run_cls_compliance_sweep(&pool, tenant, None, None, 30, 2).await;
        assert_eq!(
            again.newly_opened, 0,
            "a standing fault must not re-announce on every sweep"
        );
        assert_eq!(again.resolved, 0, "and it is still open, not flapping");
    }

    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM cls_compliance_issues WHERE tenant = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(
        rows, 1,
        "one row per issue, re-sighted in place — not per sweep"
    );

    // The certificate is renewed.
    let renewed = session(
        (OffsetDateTime::now_utc() + Duration::days(365)).date(),
        GatewayStatus::Operational,
    );
    store(&pool, tenant, &renewed, "OPERATIONAL").await;

    let after = run_cls_compliance_sweep(&pool, tenant, None, None, 30, 2).await;
    assert_eq!(after.sessions_with_issues, 0, "the fault is gone");
    assert_eq!(after.resolved, 1, "and the register closes it");

    let resolved: Option<OffsetDateTime> = sqlx::query_scalar(
        "SELECT resolved_at FROM cls_compliance_issues WHERE tenant = $1 AND device_id = $2",
    )
    .bind(tenant)
    .bind("SMGW-0001")
    .fetch_one(&pool)
    .await
    .expect("row survives");
    assert!(resolved.is_some(), "resolution is recorded, not deleted");
}

/// A gateway that has been physically swapped out is not still reported.
///
/// `REPLACED` is a historical record. The sweep scanned it anyway — the
/// `gateway_status` column was promoted out of the JSONB specifically so this
/// filter would be an index lookup, and the filter was never written — so a
/// decommissioned device reported its expired certificate every day forever.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_replaced_gateway_is_not_swept() {
    use edmd::smgw::run_cls_compliance_sweep;
    use edmd::smgw_model::{CertificateType, GatewayCertificate, GatewayStatus, SmgwSession};
    use time::macros::date;

    let (_repo, pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";

    let session = SmgwSession {
        device_id: "SMGW-OLD".to_owned(),
        firmware_version: "1.0.0".to_owned(),
        msb_mp_id: "9900000000001".to_owned(),
        malo_id: "51238696012".to_owned(),
        status: GatewayStatus::Replaced,
        certificates: vec![GatewayCertificate {
            serial_number: "DEAD".to_owned(),
            cert_type: CertificateType::Tls,
            subject_cn: "SMGW-OLD".to_owned(),
            issuer_cn: "Smart Meter CA".to_owned(),
            valid_from: date!(2019 - 01 - 01),
            valid_to: date!(2020 - 01 - 01),
            is_revoked: false,
            revoked_at: None,
        }],
        cls_channels: Vec::new(),
        last_contact_at: None,
        installed_at: date!(2019 - 01 - 01),
    };

    sqlx::query(
        r"INSERT INTO smgw_sessions
              (malo_id, tenant, device_id, msb_mp_id, gateway_status, session)
          VALUES ($1,$2,$3,$4,'REPLACED',$5)",
    )
    .bind(&session.malo_id)
    .bind(tenant)
    .bind(&session.device_id)
    .bind(&session.msb_mp_id)
    .bind(serde_json::to_value(&session).expect("serialise"))
    .execute(&pool)
    .await
    .expect("store replaced session");

    let report = run_cls_compliance_sweep(&pool, tenant, None, None, 30, 2).await;
    assert_eq!(
        report.sessions_scanned, 0,
        "a swapped-out gateway is history, not fleet"
    );
    assert_eq!(report.newly_opened, 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// Values edmd itself authors must become current
// ─────────────────────────────────────────────────────────────────────────────

/// A stated MSCONS version outranks the transaction-time fallback by design, so
/// anything edmd authors has to assert itself *above* the incumbent rather than
/// rely on arriving later.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_correction_supersedes_a_reading_that_carried_an_mscons_version() {
    let (repo, pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    // The original arrives *with* the version its network operator assigned —
    // 14 digits, as the MSCONS AHB mandates.
    let mut original = read("51238696012", t, "3.5", Sparte::Strom, "1-0:1.8.0");
    original.mscons_version = Some(20_260_801_120_000);
    repo.store_reads(validated(vec![original]))
        .await
        .expect("store original");

    repo.store_corrections(&[CorrectionRecord {
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

    // The correction carries no version of its own, so it used to fall back to a
    // 13-digit timestamp, lose to the stated 14-digit version, and leave the
    // original standing — while still writing the audit row that says otherwise.
    let reads = repo.query(&window("51238696012", t)).await.expect("query");
    assert_eq!(reads.len(), 1);
    assert_eq!(
        reads[0].quantity_kwh,
        kwh("4.0"),
        "the correction must be the value a query returns, not merely stored"
    );

    let audit_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM meter_read_corrections WHERE malo_id = $1")
            .bind("51238696012")
            .fetch_one(&pool)
            .await
            .expect("count audit rows");
    assert_eq!(
        audit_rows, 1,
        "the § 146 Abs. 4 AO audit row describes a change that happened"
    );
}

/// § 60 Abs. 2 MsbG Ersatzwertbildung stands in for an unusable measurement, and
/// the unusable measurement is exactly the one likely to carry a stated version.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn an_ersatzwert_displaces_a_faulty_reading_that_carried_an_mscons_version() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    let mut faulty = read("51238696012", t, "0", Sparte::Strom, "1-0:1.8.0");
    faulty.quality = QualityFlag::Faulty;
    faulty.mscons_version = Some(20_260_801_120_000);
    repo.store_reads(validated(vec![faulty]))
        .await
        .expect("store faulty");

    let mut ersatz = read("51238696012", t, "2.75", Sparte::Strom, "1-0:1.8.0");
    ersatz.quality = QualityFlag::Substituted;
    ersatz.source = IngestionSource::AutoSubstitute;
    repo.store_reads(validated(vec![ersatz]))
        .await
        .expect("store Ersatzwert");

    let reads = repo.query(&window("51238696012", t)).await.expect("query");
    assert_eq!(reads.len(), 1);
    assert_eq!(
        reads[0].quality,
        QualityFlag::Substituted,
        "the Ersatzwert must replace the FAULTY reading it was generated for"
    );
    assert_eq!(reads[0].quantity_kwh, kwh("2.75"));
}

// ─────────────────────────────────────────────────────────────────────────────
// Register selection
// ─────────────────────────────────────────────────────────────────────────────

/// `1.8.0 = 1.8.1 + 1.8.2`. A dual-tariff meter that reports all three must not
/// have its consumption invoiced twice.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_billing_period_does_not_add_the_total_register_to_its_own_tariff_split() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let t = OffsetDateTime::now_utc() - Duration::days(1);

    repo.store_reads(validated(vec![
        read("51238696012", t, "10.0", Sparte::Strom, "1-0:1.8.0"),
        read("51238696012", t, "6.0", Sparte::Strom, "1-0:1.8.1"),
        read("51238696012", t, "4.0", Sparte::Strom, "1-0:1.8.2"),
        // Einspeisung on the same measuring point, and Blindarbeit beside it:
        // neither is part of the Arbeitsmenge.
        read("51238696012", t, "7.0", Sparte::Strom, "1-0:2.8.0"),
        read("51238696012", t, "9.0", Sparte::Strom, "1-0:3.8.0"),
    ]))
    .await
    .expect("store registers");

    let day = metering::calendar::local_day(t);
    let period = repo
        .billing_period(&BillingPeriodQuery {
            malo_id: "51238696012".to_owned(),
            period_from: day,
            period_to: day,
            tenant: "9910000000001".to_owned(),
            sparte: Sparte::Strom,
        })
        .await
        .expect("billing period")
        .expect("a period was computed");

    assert_eq!(
        period.arbeitsmenge_kwh,
        kwh("10.0"),
        "the total register is the Arbeitsmenge; its HT/NT split and the \
         Einspeisung and Blindarbeit registers must not be added to it"
    );
    assert_eq!(
        period.arbeitsmenge_ht_kwh,
        Some(kwh("6.0")),
        "HT is reported"
    );
    assert_eq!(
        period.arbeitsmenge_nt_kwh,
        Some(kwh("4.0")),
        "NT is reported"
    );
}

/// A gap in the consumption register is a gap however busy the *other* registers
/// on the same measuring point are.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_feed_in_reading_does_not_block_substituting_the_consumption_register() {
    use edmd::server::{SubstituteRequest, run_substitute_values};
    use time::format_description::well_known::Rfc3339;

    let (repo, _pool, _pg, _wh) = setup().await;
    let malo = "51238696129";
    let bezug = "1-0:1.8.0";
    let einspeisung = "1-0:2.8.0";

    let gap_from = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");
    let gap_to = gap_from + Duration::minutes(150);
    let week = Duration::days(7);

    // A reference week on the consumption register.
    let prior: Vec<MeterRead> = (0..672)
        .map(|i| {
            let start = gap_from - week + Duration::minutes(15 * i);
            read(malo, start, &format!("{}.5", i + 1), Sparte::Strom, bezug)
        })
        .collect();
    repo.store_reads(validated(prior))
        .await
        .expect("store prior week");

    // The consumption register delivers nothing in the gap window — but the
    // measuring point's feed-in register delivers normally throughout it, as a
    // prosumer's does. Scoped per MaLo instead of per register, every slot read
    // as "already carries a billable reading" and the substitution was skipped:
    // the § 60 Abs. 2 obligation could never be discharged.
    let feed_in: Vec<MeterRead> = (0..10)
        .map(|n| {
            read(
                malo,
                gap_from + Duration::minutes(15 * n),
                "3.0",
                Sparte::Strom,
                einspeisung,
            )
        })
        .collect();
    repo.store_reads(validated(feed_in))
        .await
        .expect("store the feed-in window");

    let req = SubstituteRequest {
        gap_from: gap_from.format(&Rfc3339).expect("rfc3339"),
        gap_to: gap_to.format(&Rfc3339).expect("rfc3339"),
        interval_secs: Some(900),
        method: Some("PriorPeriodAverage".to_owned()),
        prior_days: Some(7),
        operator_id: Some("TEST-OPERATOR".to_owned()),
        sparte: None,
        reason: Some("MeterFault".to_owned()),
        obis_code: Some(bezug.to_owned()),
    };
    let status = run_substitute_values(&repo, "9910000000001", malo, &req)
        .await
        .status();
    assert_eq!(
        status.as_u16(),
        201,
        "the feed-in register's readings must not mark the consumption \
         register's slots as already measured"
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

    let substituted: Vec<_> = stored
        .iter()
        .filter(|r| r.quality == QualityFlag::Substituted)
        .collect();
    assert_eq!(
        substituted.len(),
        10,
        "every slot of the consumption register must have been substituted: {stored:#?}"
    );
    // The feed-in readings are untouched real measurements.
    assert_eq!(
        stored
            .iter()
            .filter(|r| r.obis_code.as_deref() == Some(einspeisung))
            .count(),
        10,
        "the feed-in register must be left exactly as delivered"
    );
}

/// Art. 17 erasure has to reach the tables that name a MaLo under some *other*
/// column name, and the ones that bury it inside JSON.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn erasure_removes_virtual_meter_configs_naming_the_subject() {
    let (pool, _url, _wh_uri, _pg, _wh) = boot().await;
    let tenant = "9910000000001";
    let erased = "51238696012";
    let other = "51238696129";

    // Three configs: one whose own virtual MaLo is the subject, one that draws on
    // the subject as a *source* inside `rule_json`, and one unrelated.
    let unrelated = "51238696450";
    for (vmalo, rule) in [
        (
            erased,
            serde_json::json!({"kind": "SUM", "source_malo_ids": [other]}),
        ),
        (
            other,
            serde_json::json!({"kind": "SUM", "source_malo_ids": [erased, other]}),
        ),
        (
            unrelated,
            serde_json::json!({"kind": "SUM", "source_malo_ids": [unrelated]}),
        ),
    ] {
        sqlx::query(
            "INSERT INTO virtual_meter_configs (virtual_malo_id, rule_type, rule_json, tenant)
             VALUES ($1, 'SUM', $2, $3)",
        )
        .bind(vmalo)
        .bind(&rule)
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("insert virtual meter config");
    }

    // The statement the erasure transaction runs.
    let deleted = sqlx::query(
        r"DELETE FROM virtual_meter_configs
           WHERE tenant = $2
             AND (virtual_malo_id = $1
                  OR jsonb_path_exists(
                         rule_json,
                         '$.** ? (@ == $mid)',
                         jsonb_build_object('mid', $1::text)
                     ))",
    )
    .bind(erased)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("erase virtual meter configs")
    .rows_affected();

    assert_eq!(
        deleted, 2,
        "both the subject's own virtual meter and the one naming it as a source \
         must go; the unrelated community must not"
    );

    let survivors: i64 =
        sqlx::query_scalar("SELECT count(*) FROM virtual_meter_configs WHERE tenant = $1")
            .bind(tenant)
            .fetch_one(&pool)
            .await
            .expect("count survivors");
    assert_eq!(survivors, 1, "the unrelated config is untouched");

    // And nothing anywhere still spells the erased MaLo out.
    let lingering: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM virtual_meter_configs
          WHERE tenant = $2 AND rule_json::text LIKE '%' || $1 || '%'",
    )
    .bind(erased)
    .bind(tenant)
    .fetch_one(&pool)
    .await
    .expect("scan for lingering identifiers");
    assert_eq!(lingering, 0, "no rule may still name the erased subject");
}

/// An Ersatzwert is filed under the register whose gap it fills.
///
/// The request may omit `obis_code`, in which case the substitution picks the
/// point's dominant energy register — and it must write the value back under
/// *that* register, not under the request's. An unlabelled reading is the
/// canonical **total** register (`domain::register`), so on a dual-tariff point
/// that reports only HT and NT, one unlabelled substitute makes the whole
/// month's HT/NT series read as a decomposition of it and every aggregate over
/// the period collapses to the substitute alone.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_substitute_is_filed_under_the_register_it_fills() {
    use edmd::server::{SubstituteRequest, run_substitute_values};
    use time::format_description::well_known::Rfc3339;

    let (repo, _pool, _pg, _wh) = setup().await;
    let malo = "51238696129";
    let ht = "1-0:1.8.1";
    let nt = "1-0:1.8.2";
    let tenant = "9910000000001";

    let gap_from = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");
    let gap_to = gap_from + Duration::minutes(150);
    let week = Duration::days(7);

    // A reference week on both tariff registers, and no total register anywhere.
    // HT carries the Lastgang at quarter-hours and NT reports hourly, so HT is
    // the point's dominant energy register — which is the one a request naming
    // none substitutes.
    let mut prior: Vec<MeterRead> = Vec::new();
    for i in 0..672 {
        let start = gap_from - week + Duration::minutes(15 * i);
        prior.push(read(malo, start, "4.0", Sparte::Strom, ht));
        if i % 4 == 0 {
            prior.push(read(malo, start, "1.0", Sparte::Strom, nt));
        }
    }
    repo.store_reads(validated(prior))
        .await
        .expect("store prior week");

    // The HT register goes dark for the whole gap window; NT keeps delivering on
    // its own hourly cadence.
    let nt_window: Vec<MeterRead> = (0..3)
        .map(|n| {
            read(
                malo,
                gap_from + Duration::minutes(60 * n),
                "1.0",
                Sparte::Strom,
                nt,
            )
        })
        .collect();
    repo.store_reads(validated(nt_window))
        .await
        .expect("store the NT window");

    let req = SubstituteRequest {
        gap_from: gap_from.format(&Rfc3339).expect("rfc3339"),
        gap_to: gap_to.format(&Rfc3339).expect("rfc3339"),
        interval_secs: None,
        method: Some("PriorPeriodAverage".to_owned()),
        prior_days: Some(7),
        operator_id: Some("TEST-OPERATOR".to_owned()),
        sparte: None,
        reason: Some("MeterFault".to_owned()),
        // Deliberately unnamed: the point's dominant energy register is chosen.
        obis_code: None,
    };
    let status = run_substitute_values(&repo, tenant, malo, &req)
        .await
        .status();
    assert_eq!(status.as_u16(), 201, "the HT gap is fillable");

    let stored = repo
        .query(&TimeSeriesQuery {
            malo_id: malo.to_owned(),
            from: gap_from,
            to: gap_to,
            sparte: None,
            tenant: tenant.to_owned(),
        })
        .await
        .expect("read the gap window back");

    let substituted: Vec<_> = stored
        .iter()
        .filter(|r| r.quality == QualityFlag::Substituted)
        .collect();
    assert!(!substituted.is_empty(), "something was substituted");
    assert!(
        substituted.iter().all(|r| r.obis_code.is_some()),
        "an Ersatzwert must name the register it stands in for, never land \
         unlabelled: {substituted:#?}"
    );

    // The consequence that matters: the period's Arbeitsmenge is still the two
    // tariff registers summed, not the substitute alone.
    let period = repo
        .billing_period(&BillingPeriodQuery {
            malo_id: malo.to_owned(),
            period_from: gap_from.date(),
            period_to: gap_from.date(),
            tenant: tenant.to_owned(),
            sparte: Sparte::Strom,
        })
        .await
        .expect("aggregate the day")
        .expect("the day has readings");
    assert!(
        period.arbeitsmenge_kwh > kwh("10"),
        "an unlabelled substitute must not swallow the tariff registers: {period:#?}"
    );
}

/// Article 17 erasure has to reach the ESA Typ-2 store too.
///
/// A Typ-2 value is non-authoritative for *settlement*, which says nothing about
/// whether it is personal data: it is a quarter-hourly consumption series against
/// a MaLo-ID exactly like the billed one. With the subject registry attached only
/// to the authoritative table, erasure destroyed one mapping and left every ESA
/// Typ-2 reading fully attributable — the one store an erasure could not reach.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn erasure_unlinks_the_esa_typ2_store_as_well() {
    let (pool, url, warehouse_uri, _pg, _wh) = boot().await;
    let (reads_store, typ2_store, _zsg_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");
    let typ2 = MeterStoreTyp2Repository::new(typ2_store);
    let tenant = "9910000000001";
    let malo = "51238696012";

    let t = OffsetDateTime::now_utc() - Duration::days(1);
    typ2.store_typ2_reads(&[Typ2Read {
        malo_id: malo.to_owned(),
        melo_id: None,
        dtm_from: t,
        dtm_to: t + Duration::minutes(15),
        quantity_kwh: kwh("9.0"),
        quality: QualityFlag::Measured,
        pid: 13027,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        tenant: tenant.to_owned(),
        delivery_path: Typ2DeliveryPath::default(),
        sender_mp_id: None,
        bestellung_ref: Some("ESABE0000000001".to_owned()),
        received_at: None,
    }])
    .await
    .expect("store typ2 read");

    // The Typ-2 write enrolled the same `(tenant, MaLo)` subject the
    // authoritative store uses, so the mapping exists and covers both.
    let subject: Option<String> =
        sqlx::query_scalar("SELECT subject_ref FROM meterstore_subject_map WHERE natural_id = $1")
            .bind(edmd::store::subject_natural_id(tenant, malo))
            .fetch_optional(&pool)
            .await
            .expect("query the subject map");
    let subject = subject.expect("a Typ-2 write enrols an erasure subject");

    // Erasure destroys it, and the readings become unattributable.
    reads_store
        .subject_registry()
        .expect("the reads store carries the registry")
        .erase(
            &meterstore::SubjectRef::new(subject).expect("subject ref"),
            "DSGVO Art. 17",
            "test",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("erase the subject");

    let remaining: Option<String> =
        sqlx::query_scalar("SELECT subject_ref FROM meterstore_subject_map WHERE natural_id = $1")
            .bind(edmd::store::subject_natural_id(tenant, malo))
            .fetch_optional(&pool)
            .await
            .expect("re-query the subject map");
    assert!(
        remaining.is_none(),
        "one erasure must unlink the Typ-2 readings along with the billed ones"
    );
}

/// A Zählerstandsgang differences into a Lastgang, and both halves are kept.
///
/// BK6-24-174 („Datenübermittlung ZSG", wirksam 06.06.2025) puts the
/// differencing at the Messstellenbetreiber, which is what edmd is. The
/// readings are the primary record — § 146 Abs. 4 AO requires the original to
/// stay recoverable, and a stored difference cannot reproduce the register
/// values it came from.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_zaehlerstandsgang_differences_into_a_lastgang() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";
    let base = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");

    // Four quarter-hourly register readings: 1000 → 1002.5 → 1004.8 → 1007.
    let readings: Vec<edmd::domain::MeterReading> = [
        ("1000.0", 0i64),
        ("1002.5", 1),
        ("1004.8", 2),
        ("1007.0", 3),
    ]
    .into_iter()
    .map(|(v, i)| edmd::domain::MeterReading {
        malo_id: malo.to_owned(),
        read_at: base + Duration::minutes(15 * i),
        zaehlerstand: kwh(v),
        quality: QualityFlag::Measured,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        melo_id: Some(TEST_MELO.to_owned()),
        tenant: tenant.to_owned(),
        source: IngestionSource::DirectPush,
        sender_mp_id: Some("9900000000001".to_owned()),
        push_session: Some("ZSG-TEST-1".to_owned()),
    })
    .collect();

    let stored = repo.store_readings(&readings).await.expect("store the ZSG");
    assert_eq!(stored, 4);

    // The readings come back as the meter displayed them.
    let back = repo
        .readings(malo, base, base + Duration::hours(1), tenant)
        .await
        .expect("read the ZSG back");
    assert_eq!(back.len(), 4);
    assert_eq!(back[0].zaehlerstand, kwh("1000.0"));
    assert_eq!(
        back[3].zaehlerstand,
        kwh("1007.0"),
        "a Zählerstand is stored unconverted — the difference is what gets converted"
    );

    // Redelivery overwrites rather than duplicating, like an interval redelivery.
    repo.store_readings(&readings).await.expect("redeliver");
    let again = repo
        .readings(malo, base, base + Duration::hours(1), tenant)
        .await
        .expect("re-read");
    assert_eq!(again.len(), 4, "a redelivered ZSG upserts on its key");
}

/// § 40 Abs. 2 Nr. 6 EnWG: the invoice's opening and closing register reading.
///
/// The pair comes from the Zählerstandsgang, and the bounds are **at or
/// before** each end — a reading dated after the period end did not hold at the
/// period end. An aggregate that cannot fill them builds an invoice missing a
/// statutory line.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_billing_period_reports_its_opening_and_closing_zaehlerstand() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";
    // A Berlin January, so the aggregate's window and the readings agree.
    let day = time::Date::from_calendar_date(2026, time::Month::January, 15).expect("date");
    let midnight =
        OffsetDateTime::from_unix_timestamp(1_768_431_600).expect("2026-01-15T00:00 CET");

    let reading = |offset_min: i64, value: &str| edmd::domain::MeterReading {
        malo_id: malo.to_owned(),
        read_at: midnight + Duration::minutes(offset_min),
        zaehlerstand: kwh(value),
        quality: QualityFlag::Measured,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        melo_id: Some(TEST_MELO.to_owned()),
        tenant: tenant.to_owned(),
        source: IngestionSource::DirectPush,
        sender_mp_id: None,
        push_session: None,
    };
    repo.store_readings(&[
        // Before the day: the opening reading.
        reading(-15, "5000.0"),
        reading(60, "5002.0"),
        // The last reading inside the day: the closing one.
        reading(23 * 60, "5010.0"),
        // The next day's first reading must NOT become the closing Zählerstand.
        reading(25 * 60, "5099.0"),
    ])
    .await
    .expect("store the readings");

    // Some intervals, so the aggregate has an Arbeitsmenge to report at all.
    let intervals: Vec<MeterRead> = (0..4)
        .map(|i| {
            read(
                malo,
                midnight + Duration::minutes(15 * i),
                "2.5",
                Sparte::Strom,
                "1-0:1.8.0",
            )
        })
        .collect();
    repo.store_reads(validated(intervals))
        .await
        .expect("store the Lastgang");

    let period = repo
        .billing_period(&BillingPeriodQuery {
            malo_id: malo.to_owned(),
            period_from: day,
            period_to: day,
            tenant: tenant.to_owned(),
            sparte: Sparte::Strom,
        })
        .await
        .expect("aggregate")
        .expect("the day has readings");

    assert_eq!(
        period.zaehlerstand_anfang,
        Some(kwh("5000.0")),
        "the opening reading is the last one at or before the period start"
    );
    assert_eq!(
        period.zaehlerstand_ende,
        Some(kwh("5010.0")),
        "the closing reading is the last one at or before the period END — the \
         next day's reading did not hold at the period end: {period:#?}"
    );
}

/// Article 17 erasure reaches the Zählerstandsgang.
///
/// The ZSG is the *primary* measurement — every derived interval and the
/// § 40 Abs. 2 Nr. 6 EnWG opening/closing reading come from it — and it is a
/// Buchungsbeleg (§ 147 Abs. 1 AO), so the values survive and the identity does
/// not. One subject registry spans the catalog's tables, so the same erasure
/// that unlinks the intervals unlinks these.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn erasure_unlinks_the_zaehlerstandsgang() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";
    let at = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");

    repo.store_readings(&[edmd::domain::MeterReading {
        malo_id: malo.to_owned(),
        read_at: at,
        zaehlerstand: kwh("4321.0"),
        quality: QualityFlag::Measured,
        sparte: Sparte::Strom,
        obis_code: Some("1-0:1.8.0".to_owned()),
        melo_id: Some(TEST_MELO.to_owned()),
        tenant: tenant.to_owned(),
        source: IngestionSource::DirectPush,
        sender_mp_id: None,
        push_session: None,
    }])
    .await
    .expect("store a Zählerstand");

    let registry = repo
        .zsg_store()
        .subject_registry()
        .expect("the Zählerstandsgang store has a subject registry");
    let natural = edmd::store::subject_natural_id(tenant, malo);
    let subject = registry
        .lookup(&natural)
        .await
        .expect("lookup")
        .expect("storing a Zählerstand registers the MaLo as a subject");

    repo.zsg_store()
        .erase_subject(
            &subject,
            "Art. 17 request",
            "dpo",
            OffsetDateTime::now_utc(),
        )
        .await
        .expect("erase subject");

    assert!(
        registry.lookup(&natural).await.expect("lookup").is_none(),
        "the mapping must be destroyed, leaving the readings unattributable"
    );

    // The measurement itself survives — § 147 Abs. 1 AO keeps the Buchungsbeleg.
    let kept = repo
        .readings(
            malo,
            at - Duration::hours(1),
            at + Duration::hours(1),
            tenant,
        )
        .await
        .expect("read back");
    assert_eq!(kept.len(), 1, "the reading is retained, only unlinked");
}

// billing path for an SLP point with no interval metering at all.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_completed_ablesung_reaches_the_reading_store() {
    let (repo, pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";

    // A year apart, as a Jahresablesung is.
    let last_year = OffsetDateTime::from_unix_timestamp(1_735_689_600).expect("2025-01-01T00:00Z");
    let this_year = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");

    for (at, value) in [(last_year, "14230.0"), (this_year, "17845.0")] {
        let id = uuid::Uuid::new_v4();
        sqlx::query(
            "INSERT INTO ablese_auftraege
               (id, malo_id, tenant, anlass, auftraggeber_rolle, geplant_am, sparte)
             VALUES ($1,$2,$3,'JAHRESABLESUNG','LF',$4,'STROM')",
        )
        .bind(id)
        .bind(malo)
        .bind(tenant)
        .bind(at.date())
        .execute(&pool)
        .await
        .expect("create the order");

        // What `complete_reading_order` does, against the same pool.
        sqlx::query(
            "UPDATE ablese_auftraege
             SET status='AUSGEFUEHRT', zaehlerstand_kwh=$1, ausgefuehrt_am=$2
             WHERE id=$3 AND tenant=$4",
        )
        .bind(kwh(value))
        .bind(at)
        .bind(id)
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("complete the order");

        repo.store_readings(&[edmd::domain::MeterReading {
            malo_id: malo.to_owned(),
            read_at: at,
            zaehlerstand: kwh(value),
            quality: QualityFlag::Measured,
            sparte: Sparte::Strom,
            obis_code: Some("1-0:1.8.0".to_owned()),
            melo_id: Some(TEST_MELO.to_owned()),
            tenant: tenant.to_owned(),
            source: IngestionSource::Manual,
            sender_mp_id: None,
            push_session: None,
        }])
        .await
        .expect("file the Zählerstand");
    }

    let stored = repo
        .readings(malo, last_year, this_year, tenant)
        .await
        .expect("read them back");
    assert_eq!(stored.len(), 2, "both Ablesungen reached the reading store");

    // The SLP billing path: the difference between two annual readings.
    let start =
        metering::reading::MeterReading::measured(stored[0].read_at, stored[0].zaehlerstand);
    let end = metering::reading::MeterReading::measured(stored[1].read_at, stored[1].zaehlerstand);
    let consumption = metering::reading::consumption_between(
        &start,
        &end,
        &metering::reading::LastgangConfig::default(),
    )
    .expect("two clean readings difference cleanly");
    assert_eq!(
        consumption,
        kwh("3615.0"),
        "17845 − 14230 — the whole billing basis for an SLP point"
    );
}

/// §42c Energy Sharing: the allocation endpoint resolves a community by plant.
///
/// A GGV rule is written *per tenant*, so a community is the set of rules
/// sharing a `plant_melo_id`, and that is what the lookup matches. Reaching for
/// `rule_json["source_malo_ids"]` instead — a key a serialised `AggregationRule`
/// carries under no variant — yields an empty list and a 422 on every
/// allocation.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_sharing_community_is_the_set_of_rules_sharing_a_plant() {
    let (repo, pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let plant = "51238696129";
    let tenant_a = "51238696012";
    let tenant_b = "51238696293";

    // Two participants on one plant, plus a decoy on a different plant.
    for (vid, tenant_melo, plant_melo) in [
        ("VIRT-A", tenant_a, plant),
        ("VIRT-B", tenant_b, plant),
        ("VIRT-OTHER", tenant_a, "51238696137"),
    ] {
        let rule = serde_json::json!({
            "kind": "GGV_CONSTANT_ALLOCATION",
            "plant_melo_id": plant_melo,
            "tenant_melo_id": tenant_melo,
            "fraction": "0.5",
        });
        sqlx::query(
            "INSERT INTO virtual_meter_configs
               (virtual_malo_id, display_name, rule_type, rule_json, sparte, valid_from, tenant)
             VALUES ($1,$1,'GGV_CONSTANT_ALLOCATION',$2,'STROM',heute(),$3)",
        )
        .bind(vid)
        .bind(&rule)
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("insert the rule");
    }

    // The lookup the handler runs: variant-agnostic, whole-value, bound.
    let found: Vec<String> = sqlx::query_scalar(
        r"SELECT virtual_malo_id FROM virtual_meter_configs
          WHERE tenant = $2
            AND rule_type IN ('GGV_CONSTANT_ALLOCATION','GGV_PROPORTIONAL_ALLOCATION')
            AND jsonb_path_exists(
                    rule_json,
                    '$.**.plant_melo_id ? (@ == $p)',
                    jsonb_build_object('p', $1::text))
          ORDER BY virtual_malo_id",
    )
    .bind(plant)
    .bind(tenant)
    .fetch_all(&pool)
    .await
    .expect("resolve the community");

    assert_eq!(
        found,
        vec!["VIRT-A".to_owned(), "VIRT-B".to_owned()],
        "the community is exactly the rules naming this plant — not the one on another"
    );

    // And the tempting key is not in the stored rule at all.
    let raw: serde_json::Value = sqlx::query_scalar(
        "SELECT rule_json FROM virtual_meter_configs WHERE virtual_malo_id='VIRT-A' AND tenant=$1",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await
    .expect("read the rule back");
    assert!(
        raw.get("source_malo_ids").is_none(),
        "`AggregationRule` is externally tagged and carries no `source_malo_ids`: {raw}"
    );
    let _ = &repo;
}

/// The portfolio aggregate goes through the register projection.
///
/// It was `SUM("value") … GROUP BY "malo_id"` over every row — the §2.6 defect
/// at portfolio scale, in an endpoint whose own comment calls the result
/// "portfolio-wide MMM". Its single-MaLo sibling had gone through
/// `domain::register` for exactly these reasons; this one had not.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn the_portfolio_aggregate_does_not_double_count_a_dual_tariff_prosumer() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";
    let base = OffsetDateTime::now_utc() - Duration::days(2);

    // A dual-tariff prosumer: a total register, its own HT/NT decomposition,
    // a feed-in register, and a reactive one. Only the 10 kWh total is Bezug.
    let mut batch: Vec<MeterRead> = Vec::new();
    for i in 0..4 {
        let at = base + Duration::minutes(15 * i);
        batch.push(read(malo, at, "2.5", Sparte::Strom, "1-0:1.8.0"));
        batch.push(read(malo, at, "1.5", Sparte::Strom, "1-0:1.8.1"));
        batch.push(read(malo, at, "1.0", Sparte::Strom, "1-0:1.8.2"));
        batch.push(read(malo, at, "9.0", Sparte::Strom, "1-0:2.8.0"));
        batch.push(read(malo, at, "7.0", Sparte::Strom, "1-0:3.8.0"));
    }
    repo.store_reads(validated(batch))
        .await
        .expect("store the prosumer's registers");

    // The scan the handler runs, then the projection it applies.
    let store = repo.store();
    let sql = format!(
        r#"SELECT "malo_id", "obis_code", SUM("value") AS total_kwh,
                  MIN("from") AS span_from, MAX("to") AS span_to
             FROM "{table}"
            WHERE "tenant" = $1 AND "quality" NOT IN ('FAULTY','UNKNOWN')
            GROUP BY "malo_id", "obis_code""#,
        table = store.resolved_table(),
    );
    let rows = store
        .query_with_params(
            &sql,
            vec![datafusion::scalar::ScalarValue::Utf8(Some(
                tenant.to_owned(),
            ))],
        )
        .await
        .expect("scan")
        .to_json()
        .expect("decode");

    // `to_json` goes through arrow's `ArrayWriter`, which renders a Decimal128 as
    // a JSON **number** — so a caller must not assume a string.
    let value_of = |r: &serde_json::Value| -> Decimal {
        match &r["total_kwh"] {
            serde_json::Value::String(s) => s.parse().unwrap_or_default(),
            serde_json::Value::Number(n) => n.to_string().parse().unwrap_or_default(),
            _ => Decimal::ZERO,
        }
    };

    // The unprojected sum, for contrast.
    let raw: Decimal = rows.iter().map(value_of).sum();
    assert_eq!(
        raw,
        kwh("84.0"),
        "the unprojected sum mixes feed-in, kvarh and a double-counted tariff split"
    );

    // Projected: only the total Bezug register, 4 × 2.5 kWh.
    let registers: Vec<metering::MeterInterval> = rows
        .iter()
        .map(|r| metering::MeterInterval {
            from: base,
            to: base + Duration::hours(1),
            value: value_of(r),
            quality: QualityFlag::Measured,
            obis_code: r["obis_code"].as_str().and_then(|s| s.parse().ok()),
        })
        .collect();
    let projected: Decimal =
        edmd::domain::energy_intervals_from(registers, edmd::domain::EnergyDirection::Bezug)
            .iter()
            .map(|iv| iv.value)
            .sum();
    assert_eq!(
        projected,
        kwh("10.0"),
        "the Bezug is the total register alone — not it plus its own HT/NT split, \
         plus the feed-in, plus a kvarh channel"
    );
}

/// A Zählerstand is stored in the unit the **register** counts.
///
/// Not the Sparte's billing unit. § 25 Nr. 4 MessEV converts the *difference*
/// between two readings; a register value rewritten into kWh is no longer the
/// number on the meter, and § 40 Abs. 2 Nr. 6 EnWG puts that number on an
/// invoice for a customer to check. A gas register counts m³, so it round-trips
/// as the volume it displayed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn a_gas_zaehlerstand_round_trips_unconverted() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let tenant = "9910000000001";
    let malo = "51238696129";
    let at = OffsetDateTime::from_unix_timestamp(1_767_225_600).expect("2026-01-01T00:00Z");

    // 4321 m³ on the register — not the ~45 600 kWh_Hs it settles as.
    repo.store_readings(&[edmd::domain::MeterReading {
        malo_id: malo.to_owned(),
        read_at: at,
        zaehlerstand: kwh("4321.0"),
        quality: QualityFlag::Measured,
        sparte: Sparte::Gas,
        obis_code: Some("7-1:3.0.0".to_owned()),
        melo_id: Some(TEST_MELO.to_owned()),
        tenant: tenant.to_owned(),
        source: IngestionSource::DirectPush,
        sender_mp_id: None,
        push_session: None,
    }])
    .await
    .expect("store a gas Zählerstand");

    let back = repo
        .readings(
            malo,
            at - Duration::hours(1),
            at + Duration::hours(1),
            tenant,
        )
        .await
        .expect("read back");
    assert_eq!(back.len(), 1, "one reading stored, one read back");
    assert_eq!(
        back[0].zaehlerstand,
        kwh("4321.0"),
        "a gas register counts m³ and is stored as it displayed, unconverted"
    );
    assert_eq!(back[0].sparte, Sparte::Gas);
}

/// Kapitel 4.6 has **two** delivery paths, and both have to reach the store.
///
/// 4.6.1 arrives as MSCONS 13027 over AS4; **4.6.2** arrives as XML straight
/// from the iMS over SM-PKI and never touches market communication at all. Two
/// of the seven Pflichtprodukte BNetzA *Mitteilung Nr. 3* obliges every MSB to
/// serve — `9991 00000 312 1` and `9991 00000 313 9` — are 4.6.2 products, and
/// mako could order them while `Typ2DeliveryPath::SmgwDirect` was a variant
/// nothing ever wrote.
///
/// Both land in the **same** non-authoritative table, told apart by
/// `delivery_path`, and both carry the subscription that ordered them.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn both_kapitel_46_delivery_paths_reach_the_typ2_store() {
    let (pool, url, warehouse_uri, _pg, _wh) = boot().await;
    let (_reads_store, typ2_store, _zsg_store, _cold) = build_stores(
        pool.clone(),
        &url,
        &warehouse_uri,
        TieringConfig::default(),
        &WarehouseAuth::default(),
    )
    .await
    .expect("build meterstore tiers");
    let typ2 = MeterStoreTyp2Repository::new(typ2_store);

    let t = OffsetDateTime::now_utc() - Duration::days(1);
    let typ2_read = |path: Typ2DeliveryPath, obis: &str, order: &str, kwh: &str| Typ2Read {
        malo_id: "51238696012".to_owned(),
        melo_id: Some("DE0004392189912345678901234567890".to_owned()),
        dtm_from: t,
        dtm_to: t + Duration::minutes(15),
        quantity_kwh: kwh.parse().expect("decimal"),
        quality: QualityFlag::Measured,
        pid: 13027,
        sparte: Sparte::Strom,
        obis_code: Some(obis.to_owned()),
        tenant: "9910000000001".to_owned(),
        delivery_path: path,
        sender_mp_id: Some("9900357000004".to_owned()),
        bestellung_ref: Some(order.to_owned()),
        received_at: None,
    };

    typ2.store_typ2_reads(&[
        // 4.6.1 — the EDIFACT back-end path.
        typ2_read(
            Typ2DeliveryPath::MsconsBackend,
            "1-1:1.29.0",
            "ESABE0000000001",
            "1.5",
        ),
        // 4.6.2 — straight from the iMS over SM-PKI.
        typ2_read(
            Typ2DeliveryPath::SmgwDirect,
            "1-1:2.29.0",
            "ESABE0000000002",
            "2.5",
        ),
    ])
    .await
    .expect("store both paths");

    let got = typ2
        .query_typ2(&window("51238696012", t))
        .await
        .expect("query typ2");
    assert_eq!(got.len(), 2);

    let smgw = got
        .iter()
        .find(|r| r.delivery_path == Typ2DeliveryPath::SmgwDirect)
        .expect("the SMGW delivery reads back as one");
    assert_eq!(smgw.bestellung_ref.as_deref(), Some("ESABE0000000002"));
    assert_eq!(smgw.obis_code.as_deref(), Some("1-1:2.29.0"));

    let backend = got
        .iter()
        .find(|r| r.delivery_path == Typ2DeliveryPath::MsconsBackend)
        .expect("the EDIFACT delivery reads back as one");
    assert_eq!(backend.bestellung_ref.as_deref(), Some("ESABE0000000001"));

    // Two subscriptions at one Meldepunkt, told apart by the order they were
    // delivered under — which is what the surveillance sweep keys on.
    assert_ne!(smgw.bestellung_ref, backend.bestellung_ref);
}

/// `?as_of=` has to describe the **whole** measuring point, like the ordinary
/// read does.
///
/// A prosumer reports import beside export at the same instants, and a
/// dual-tariff point HT beside NT. meterstore refuses to fold two channels into
/// one `MeasurementSeries` — correctly, since that puts two values at every
/// instant — so a transaction-time read that collected without splitting
/// answered **500** for exactly the points whose corrections it exists to
/// reconstruct. Every existing as-of test used a single register, so the fold
/// looked fine.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL + filesystem Iceberg warehouse)"]
async fn query_as_of_describes_every_register_of_a_prosumer() {
    let (repo, _pool, _pg, _wh) = setup().await;
    let interval = OffsetDateTime::now_utc() - Duration::days(1);
    let delivered = OffsetDateTime::now_utc() - Duration::hours(3);

    // Import and export at the same instants — one measuring point, two
    // channels.
    for (obis, kwh_value) in [("1-0:1.8.0", "3.5"), ("1-0:2.8.0", "1.25")] {
        let mut r = read("51238696012", interval, kwh_value, Sparte::Strom, obis);
        r.valid_from_tx = Some(delivered);
        repo.store_reads(validated(vec![r]))
            .await
            .expect("store channel");
    }

    let q = window("51238696012", interval);
    let now = repo.query(&q).await.expect("query");
    assert_eq!(now.len(), 2, "the ordinary read describes both channels");

    let as_of = repo
        .query_as_of(&q, OffsetDateTime::now_utc())
        .await
        .expect("a transaction-time read must not refuse a prosumer");
    assert_eq!(as_of.len(), 2, "and so does the transaction-time read");

    let mut by_register: Vec<_> = as_of
        .iter()
        .map(|r| (r.obis_code.clone().unwrap_or_default(), r.quantity_kwh))
        .collect();
    by_register.sort();
    assert_eq!(
        by_register,
        vec![
            ("1-0:1.8.0".to_owned(), kwh("3.5")),
            ("1-0:2.8.0".to_owned(), kwh("1.25")),
        ],
        "neither channel is folded into the other"
    );

    // Before either was delivered, the point was unknown — not half-known.
    let before = repo
        .query_as_of(&q, delivered - Duration::hours(1))
        .await
        .expect("query_as_of");
    assert!(before.is_empty());
}
