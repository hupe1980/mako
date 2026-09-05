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

const PERIOD_FROM: time::Date = time::macros::date!(2026 - 06 - 01);
const PERIOD_TO: time::Date = time::macros::date!(2026 - 06 - 30);

async fn a_run(pool: &sqlx::PgPool) -> Uuid {
    run_for(pool, time::macros::datetime!(2026-07-14 05:07:09 UTC), None).await
}

/// A run over the June 2026 Bilanzierungsmonat. `corrects` is what makes it a
/// correction rather than a first filing (or a retry of one).
async fn run_for(
    pool: &sqlx::PgPool,
    version: time::OffsetDateTime,
    corrects: Option<Uuid>,
) -> Uuid {
    mabis_syncd::pg::insert_run(
        pool,
        mabis_syncd::pg::InsertRunParams {
            bilanzierungsgebiet_id: GEBIET,
            period_from: PERIOD_FROM,
            period_to: PERIOD_TO,
            version,
            abrechnungslauf: mabis_syncd::pg::Abrechnungslauf::Bka,
            phase: mabis_syncd::pg::SubmissionPhase::Erstaufschlag,
            corrects_run_id: corrects,
            sender_mp_id: TENANT,
            receiver_mp_id: "9900077000006",
            tenant: TENANT,
        },
    )
    .await
    .expect("create the run")
}

/// What a run started now must not file again.
async fn skip_list(pool: &sqlx::PgPool, corrects: Option<Uuid>) -> Vec<String> {
    mabis_syncd::pg::acked_territories_for_period(pool, TENANT, PERIOD_FROM, PERIOD_TO, corrects)
        .await
        .expect("read the acked territories")
}

/// A run spanning several territories records each one, and a **retry** — which
/// is a new run — still skips the ones that reached the BIKO.
///
/// This is the defect the period key exists for. The skip list used to be read
/// with the *new* run's id, which by construction has no `submission_series`
/// rows yet, so it was always empty and every retry re-filed the territories the
/// BIKO had already acked. An acked Summenzeitreihe cannot be withdrawn, so that
/// is a second binding filing for a settled month, not a duplicate.
#[tokio::test]
async fn a_retry_does_not_re_file_what_the_biko_already_acked() {
    let (pool, _guard) = pool_or_skip!();
    let first = a_run(&pool).await;

    let north = mabis_syncd::pg::insert_series(
        &pool,
        first,
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
        first,
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

    let series = mabis_syncd::pg::list_series(&pool, first)
        .await
        .expect("list");
    assert_eq!(series.len(), 2, "both territories are recorded");

    // The retry: a *new* run for the same Bilanzierungsmonat, before it has
    // filed anything of its own.
    let retry = run_for(
        &pool,
        time::macros::datetime!(2026-07-14 06:11:00 UTC),
        None,
    )
    .await;
    assert!(
        mabis_syncd::pg::list_series(&pool, retry)
            .await
            .expect("list")
            .is_empty(),
        "the retry has filed nothing yet — a run-keyed skip list is empty here"
    );

    assert_eq!(
        skip_list(&pool, None).await,
        vec!["11YMAKO-TEST-01U".to_owned()],
        "the retry re-files only what did not reach the BIKO — an acked \
         Summenzeitreihe cannot be withdrawn"
    );
}

/// A correction (§9.8.1) re-files an acked territory on purpose, and a retry of
/// that correction must not.
///
/// Both are keyed on the same fact: every run answering one negative
/// Prüfmitteilung carries the same `corrects_run_id`, and a first filing carries
/// `NULL`. Without that in the key a correction would skip the very territory it
/// exists to correct.
#[tokio::test]
async fn a_correction_re_files_what_a_retry_would_skip() {
    let (pool, _guard) = pool_or_skip!();
    let first = a_run(&pool).await;
    let filed = mabis_syncd::pg::insert_series(
        &pool,
        first,
        GEBIET,
        "DE0001112223334445556667778889990",
        120,
        2880,
        &Decimal::from(4_500),
    )
    .await
    .expect("record");
    mabis_syncd::pg::mark_series_acked(&pool, filed, "MSCONS-2026-07-0001", None)
        .await
        .expect("it reached the BIKO");

    assert!(
        skip_list(&pool, Some(first)).await.is_empty(),
        "a correction of that run files the territory again under a new version \
         — that is what a correction is"
    );

    // The correction goes out, and is itself acked.
    let correction = run_for(
        &pool,
        time::macros::datetime!(2026-08-03 09:00:00 UTC),
        Some(first),
    )
    .await;
    let corrected = mabis_syncd::pg::insert_series(
        &pool,
        correction,
        GEBIET,
        "DE0001112223334445556667778889990",
        120,
        2880,
        &Decimal::from(4_620),
    )
    .await
    .expect("record");
    mabis_syncd::pg::mark_series_acked(&pool, corrected, "MSCONS-2026-08-0007", None)
        .await
        .expect("the correction reached the BIKO");

    assert_eq!(
        skip_list(&pool, Some(first)).await,
        vec![GEBIET.to_owned()],
        "a retry of the correction must not send the correction twice"
    );
    assert_eq!(
        skip_list(&pool, None).await,
        vec![GEBIET.to_owned()],
        "and the first filing's own lineage still counts its ack"
    );
}

/// A retry that files nothing — every territory was already with the BIKO — is
/// still recorded as `acked`, and carries **no** message reference of its own.
///
/// The alternative is copying an earlier run's reference in, which points an
/// auditor at a message this run never sent. The row-level constraint that
/// forbids that on a *series* must not be extended to the run.
#[tokio::test]
async fn a_run_that_filed_nothing_is_acked_without_a_reference() {
    let (pool, _guard) = pool_or_skip!();
    let run_id = a_run(&pool).await;

    mabis_syncd::pg::mark_acked(&pool, run_id, None, None)
        .await
        .expect("a run that re-filed nothing is complete, not failed");

    let (status, message_ref): (String, Option<String>) =
        sqlx::query_as("SELECT status, message_ref FROM submission_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .expect("read the run");
    assert_eq!(status, "acked");
    assert_eq!(message_ref, None);
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

/// One series per territory per run: two rows for the same territory in one run
/// would leave the run's own record ambiguous about whether that territory was
/// filed. (`acked_territories_for_period` de-duplicates across runs, because a
/// territory is legitimately filed once per run of a month.)
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
