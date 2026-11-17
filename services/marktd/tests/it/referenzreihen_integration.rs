//! Real-PostgreSQL guards for the Referenzreihen store.
//!
//! The store is bitemporal and append-only: a correction closes the current
//! row's `erfasst` and inserts, so a read at an earlier `stand` returns what was
//! known then; a missing value is `422 referenzwert_fehlt`; an import is
//! idempotent on the file's SHA-256 and its preview writes nothing.
//!
//! ```bash
//! just test-marktd-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use mako_markt::referenzreihen::{Reihe, WertStatus, Zeitraum};
use marktd::referenzimport::Format;
use marktd::referenzreihen::{ImportQuery, importiere, wert_fuer};
use rust_decimal::dec;
use sqlx::PgPool;
use time::OffsetDateTime;

const SCHEMA: &str = include_str!("../../migrations/0001_initial.sql");

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    Some((pool, container))
}

fn query(status: WertStatus, vorschau: bool) -> ImportQuery {
    ImportQuery {
        format: Format::Generisch,
        status,
        quelle_url: "https://www-genesis.destatis.de/".to_owned(),
        veroeffentlicht_am: None,
        vorschau,
    }
}

/// Synthetic figures: the test is about the store, not about any published value.
const VORLAEUFIG: &str = "zeitraum;dim;wert\n2025;;17,25\n";
const ENDGUELTIG: &str = "zeitraum;dim;wert\n2025;;17,50\n";

async fn zeilen(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM referenzwerte")
        .fetch_one(pool)
        .await
        .expect("count")
}

async fn jetzt(pool: &PgPool) -> OffsetDateTime {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(pool)
        .await
        .expect("clock")
}

/// A correction keeps the superseded row, and a read at the
/// earlier `stand` returns it — so a settlement computed on the provisional
/// figure can be reproduced after the final one arrives.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_correction_keeps_the_superseded_row_and_an_earlier_stand_reads_it() {
    let Some((pool, _c)) = test_pool().await else {
        return;
    };
    let reihe = Reihe::KavGrenzpreisStrom;
    let jahr = Zeitraum::Jahr { jahr: 2025 };

    let erst = importiere(
        &pool,
        reihe,
        &query(WertStatus::Vorlaeufig, false),
        VORLAEUFIG.as_bytes(),
    )
    .await
    .expect("provisional import");
    assert_eq!((erst.neu, erst.ersetzt), (1, 0));
    let stand_vorlaeufig = jetzt(&pool).await;

    let korrektur = importiere(
        &pool,
        reihe,
        &query(WertStatus::Endgueltig, false),
        ENDGUELTIG.as_bytes(),
    )
    .await
    .expect("final import");
    assert_eq!((korrektur.neu, korrektur.ersetzt), (0, 1));
    assert_eq!(
        zeilen(&pool).await,
        2,
        "the superseded row is kept, not deleted"
    );

    let heute = wert_fuer(&pool, reihe, jahr, "", None)
        .await
        .expect("current value");
    assert_eq!(heute.wert, dec!(17.50));
    assert_eq!(heute.status, WertStatus::Endgueltig);

    let damals = wert_fuer(&pool, reihe, jahr, "", Some(stand_vorlaeufig))
        .await
        .expect("value as known then");
    assert_eq!(damals.wert, dec!(17.25));
    assert_eq!(damals.status, WertStatus::Vorlaeufig);
    assert!(
        damals.erfasst_bis.is_some(),
        "the provisional row is closed"
    );
}

/// Importing one file twice adds its rows once, and the second
/// answer names the first import.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_twice_imported_file_adds_rows_once() {
    let Some((pool, _c)) = test_pool().await else {
        return;
    };
    let q = query(WertStatus::Endgueltig, false);
    let body = "zeitraum;dim;wert\n2024;;16,1\n2025;;17,5\n".as_bytes();
    let erst = importiere(&pool, Reihe::KavGrenzpreisStrom, &q, body)
        .await
        .expect("first");
    assert_eq!(erst.neu, 2);
    let zweit = importiere(&pool, Reihe::KavGrenzpreisStrom, &q, body)
        .await
        .expect("second");
    assert!(zweit.bereits_importiert);
    assert_eq!(zweit.neu + zweit.ersetzt, 0);
    assert_eq!(
        zweit.frueherer_import.map(|b| b.neu),
        Some(2),
        "the answer names the earlier import"
    );
    assert_eq!(zeilen(&pool).await, 2);
}

/// A preview runs the import and writes nothing — not even the file's hash,
/// so the real import afterwards is not answered „bereits importiert".
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_preview_writes_nothing() {
    let Some((pool, _c)) = test_pool().await else {
        return;
    };
    let vorschau = importiere(
        &pool,
        Reihe::KavGrenzpreisStrom,
        &query(WertStatus::Endgueltig, true),
        ENDGUELTIG.as_bytes(),
    )
    .await
    .expect("preview");
    assert_eq!(vorschau.neu, 1);
    assert_eq!(zeilen(&pool).await, 0);
    let echt = importiere(
        &pool,
        Reihe::KavGrenzpreisStrom,
        &query(WertStatus::Endgueltig, false),
        ENDGUELTIG.as_bytes(),
    )
    .await
    .expect("import");
    assert!(!echt.bereits_importiert);
    assert_eq!(zeilen(&pool).await, 1);
}

/// No value for the period is a refusal naming series, period and
/// publication rule — never a zero, never the neighbouring year.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_missing_value_is_refused_by_name() {
    let Some((pool, _c)) = test_pool().await else {
        return;
    };
    importiere(
        &pool,
        Reihe::KavGrenzpreisStrom,
        &query(WertStatus::Endgueltig, false),
        ENDGUELTIG.as_bytes(),
    )
    .await
    .expect("2025 imported");
    let err = wert_fuer(
        &pool,
        Reihe::KavGrenzpreisStrom,
        Zeitraum::Jahr { jahr: 2026 },
        "",
        None,
    )
    .await
    .expect_err("2026 has no value");
    assert_eq!(err.code(), Some("referenzwert_fehlt"));
    let text = err.to_string();
    assert!(
        text.contains("kav_grenzpreis_strom") && text.contains("2026"),
        "{text}"
    );
}

/// The bitemporal key is the database's: two current values for one series,
/// dimension and period cannot both be stored, whatever the writer does.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_bitemporal_key_refuses_two_current_values_for_one_period() {
    let Some((pool, _c)) = test_pool().await else {
        return;
    };
    let insert = "INSERT INTO referenzwerte (reihe, dim, gilt, wert, status, quelle_url, herkunft)
                  VALUES ('kav_grenzpreis_strom', '',
                          tstzrange('2025-01-01 00:00+01', '2026-01-01 00:00+01', '[)'),
                          $1, 'endgueltig', 'https://x.example', 'api')";
    sqlx::query(insert)
        .bind(dec!(1))
        .execute(&pool)
        .await
        .expect("first");
    let err = sqlx::query(insert)
        .bind(dec!(2))
        .execute(&pool)
        .await
        .expect_err("a second current value for 2025");
    assert_eq!(
        err.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("23P01")
    );
}

/// The Postgres container guard a test holds until it ends.
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine`; `None` when Docker is
/// unavailable.
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
