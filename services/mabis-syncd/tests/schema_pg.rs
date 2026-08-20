//! The schema applies, and the per-territory table enforces what it promises.
//!
//! # Why this is a test
//!
//! `submission_series` exists because the run row could not say what actually
//! went out: it carried one `bilanzierungsgebiet_id` — the configured
//! *fallback*, not the territories the MaLos sit in — and one `message_ref`,
//! the first submission's. A run spanning four territories recorded one and
//! lost three, and a failure on the fourth marked the whole run failed while
//! three binding Summenzeitreihen were already with the BIKO and cannot be
//! withdrawn.
//!
//! Its constraints are the part that has to hold: an `acked` row without a
//! message reference is a filing nobody can point at, and two rows for the same
//! territory in one run mean the retry logic cannot tell what to skip.

use rust_decimal::Decimal;
use uuid::Uuid;

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "9900357000004";
const GEBIET: &str = "11YMAKO-TEST-01U";

async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
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
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("src/migrations")
        .run(&pool)
        .await
        .expect("apply mabis-syncd schema");
    Some((pool, container))
}

macro_rules! pool_or_skip {
    () => {
        match pg_pool().await {
            Some(v) => v,
            None => {
                eprintln!("skipping: no Docker daemon");
                return;
            }
        }
    };
}

async fn a_run(pool: &sqlx::PgPool) -> Uuid {
    mabis_syncd::pg::insert_run(
        pool,
        mabis_syncd::pg::InsertRunParams {
            bilanzierungsgebiet_id: GEBIET,
            period_from: time::macros::date!(2026 - 06 - 01),
            period_to: time::macros::date!(2026 - 06 - 30),
            version: time::macros::datetime!(2026-07-14 05:07:09 UTC),
            abrechnungslauf: mabis_syncd::pg::Abrechnungslauf::Bka,
            phase: mabis_syncd::pg::SubmissionPhase::Erstaufschlag,
            corrects_run_id: None,
            sender_mp_id: TENANT,
            receiver_mp_id: "9900077000006",
            tenant: TENANT,
        },
    )
    .await
    .expect("create the run")
}

/// A run spanning several territories records each one, and only the ones that
/// reached the BIKO are skipped by a retry.
#[tokio::test]
async fn each_territory_is_recorded_and_only_acked_ones_are_skipped() {
    let (pool, _guard) = pool_or_skip!();
    let run_id = a_run(&pool).await;

    let north = mabis_syncd::pg::insert_series(
        &pool,
        run_id,
        "11YMAKO-TEST-01U",
        "DE0001112223334445556667778889990",
        120,
        2880,
        &Decimal::from(4_500),
    )
    .await
    .expect("record the northern series");
    let south = mabis_syncd::pg::insert_series(
        &pool,
        run_id,
        "11YMAKO-QUALIT-5",
        "DE0001112223334445556667778889991",
        80,
        2880,
        &Decimal::from(3_100),
    )
    .await
    .expect("record the southern series");

    mabis_syncd::pg::mark_series_acked(&pool, north, "MSCONS-2026-07-0001", None)
        .await
        .expect("the north reached the BIKO");
    mabis_syncd::pg::mark_series_failed(&pool, south, "makod returned 503")
        .await
        .expect("the south did not");

    let series = mabis_syncd::pg::list_series(&pool, run_id)
        .await
        .expect("list");
    assert_eq!(series.len(), 2, "both territories are recorded");

    let acked = mabis_syncd::pg::acked_territories(&pool, run_id)
        .await
        .expect("acked");
    assert_eq!(
        acked,
        vec!["11YMAKO-TEST-01U".to_owned()],
        "a retry re-files only what did not reach the BIKO — an acked \
         Summenzeitreihe cannot be withdrawn"
    );
}

/// An `acked` series must name what was filed.
///
/// Without the reference there is nothing to correlate the BIKO's Datenstatus
/// or Prüfmitteilung against, so the filing exists and cannot be followed.
#[tokio::test]
async fn an_acked_series_cannot_omit_its_message_reference() {
    let (pool, _guard) = pool_or_skip!();
    let run_id = a_run(&pool).await;
    let id = mabis_syncd::pg::insert_series(
        &pool,
        run_id,
        GEBIET,
        "DE0001112223334445556667778889990",
        1,
        96,
        &Decimal::ONE,
    )
    .await
    .expect("record");

    let err = sqlx::query("UPDATE submission_series SET status = 'acked' WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await;
    assert!(
        err.is_err(),
        "'acked' without a message_ref must violate ss_acked_has_reference"
    );
}

/// One series per territory per run: two rows for the same territory would make
/// `acked_territories` ambiguous about what a retry may skip.
#[tokio::test]
async fn a_territory_appears_at_most_once_per_run() {
    let (pool, _guard) = pool_or_skip!();
    let run_id = a_run(&pool).await;

    mabis_syncd::pg::insert_series(&pool, run_id, GEBIET, "ZP1", 1, 96, &Decimal::ONE)
        .await
        .expect("first");
    assert!(
        mabis_syncd::pg::insert_series(&pool, run_id, GEBIET, "ZP1", 1, 96, &Decimal::ONE)
            .await
            .is_err(),
        "a second series for the same territory in one run must be refused"
    );
}

/// Deleting a run takes its series with it — the audit trail is the run, and a
/// series orphaned from it says nothing.
#[tokio::test]
async fn series_are_owned_by_their_run() {
    let (pool, _guard) = pool_or_skip!();
    let run_id = a_run(&pool).await;
    mabis_syncd::pg::insert_series(&pool, run_id, GEBIET, "ZP1", 1, 96, &Decimal::ONE)
        .await
        .expect("record");

    sqlx::query("DELETE FROM submission_runs WHERE id = $1")
        .bind(run_id)
        .execute(&pool)
        .await
        .expect("delete the run");

    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM submission_series")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(remaining, 0);
}
