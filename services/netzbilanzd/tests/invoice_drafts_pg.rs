//! Real-PostgreSQL tests for `invoice_drafts` — the invoice parties in particular.
//!
//! Uses testcontainers (self-managing Docker) — runs under `cargo test` when a
//! Docker daemon is available, skipped gracefully otherwise.
//!
//! # Why this exists
//!
//! `invoice_drafts` used to call its two party columns `nb_mp_id` and
//! `lf_mp_id`, on the assumption that a draft is always "from the NB, to the
//! LF". PID 31009 (MSB-Rechnung) breaks that assumption in both directions: the
//! *Anwendungsübersicht der Prüfidentifikatoren* 4.0 lists seven Anwendungsfälle
//! and the sender is the **MSB** in every one, with the NB, LF or ESA receiving.
//!
//! `netzbilanzd` stored it the other way round, so a 31009 draft named the party
//! owed money as the one billing for it. The columns are now `sender_mp_id` /
//! `recipient_mp_id`, and these tests pin the round-trip — the queries are
//! runtime-checked SQL strings, so a rename that compiles can still fail here.

use rust_decimal::dec;
use time::macros::date;

use invoic_checker::check::CheckOutcome;
use netzbilanzd::pg::{fetch_draft, insert_correction_draft, list_drafts_pg, upsert_draft};

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

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
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply netzbilanzd schema");
    Some((pool, container))
}

const MSB: &str = "9900999000001";
const NB: &str = "9900357000004";
const LF: &str = "9900111000002";
const ESA: &str = "9905550000005";
const MALO: &str = "51238696780";
const MALO_2: &str = "51238696781";
const MALO_3: &str = "51238696782";

/// A 31009 draft stores the MSB as sender and the NB / LF / ESA as recipient,
/// and reads back the same way through both read paths.
#[tokio::test]
async fn msb_rechnung_round_trips_with_the_msb_as_sender() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    // A distinct MaLo per case: `upsert_draft` is idempotent on
    // (tenant, malo_id, period, pid), so reusing one MaLo would update a single
    // draft rather than create three.
    for (idx, (recipient, malo)) in [(NB, MALO), (LF, MALO_2), (ESA, MALO_3)].iter().enumerate() {
        let id = upsert_draft(
            &pool,
            "t1",
            malo,
            MSB,       // sender — the Messstellenbetreiber issues 31009
            recipient, // recipient — NB, LF or ESA per Anwendungsfall
            31009,
            date!(2026 - 01 - 01),
            date!(2026 - 01 - 31),
            serde_json::json!({ "_typ": "RECHNUNG" }),
            dec!(17.85),
            CheckOutcome::Ok,
        )
        .await
        .expect("insert a 31009 draft");

        let row = fetch_draft(&pool, id)
            .await
            .expect("fetch")
            .expect("the draft exists");
        assert_eq!(
            row.sender_mp_id, MSB,
            "PID 31009 is issued by the MSB, not to it"
        );
        assert_eq!(row.recipient_mp_id, *recipient);
        assert_eq!(row.pid, 31009);

        // The sender filter must match the MSB — filtering by the NB would find
        // nothing, which is what the old column naming invited callers to do.
        let by_sender = list_drafts_pg(&pool, None, None, Some(MSB), 100)
            .await
            .expect("list by sender");
        assert_eq!(
            by_sender.len(),
            idx + 1,
            "every 31009 draft so far is filed under the MSB as sender"
        );
        assert!(by_sender.iter().all(|r| r.sender_mp_id == MSB));
    }
}

/// An NNE draft (31001) keeps the ordinary direction: NB → LF. The rename is a
/// naming fix, not a semantic change for the PIDs that were already correct.
#[tokio::test]
async fn nne_rechnung_keeps_the_netzbetreiber_as_sender() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    let id = upsert_draft(
        &pool,
        "t1",
        MALO,
        NB,
        LF,
        31001,
        date!(2026 - 01 - 01),
        date!(2026 - 01 - 31),
        serde_json::json!({ "_typ": "RECHNUNG" }),
        dec!(1234.56),
        CheckOutcome::Ok,
    )
    .await
    .expect("insert an NNE draft");

    let row = fetch_draft(&pool, id)
        .await
        .expect("fetch")
        .expect("the draft exists");
    assert_eq!(row.sender_mp_id, NB);
    assert_eq!(row.recipient_mp_id, LF);
}

/// A Storno / Korrektur inherits the original draft's tenant. Hardcoding
/// `'default'` filed a correction of tenant B's invoice under someone else's
/// tenant — invisible to B's tenant-scoped reads.
#[tokio::test]
async fn correction_inherits_the_original_tenant() {
    let Some((pool, _guard)) = pg_pool().await else {
        eprintln!("skipping: no Docker daemon");
        return;
    };

    let original = upsert_draft(
        &pool,
        "tenant-b",
        MALO,
        NB,
        LF,
        31001,
        date!(2026 - 02 - 01),
        date!(2026 - 02 - 28),
        serde_json::json!({ "_typ": "RECHNUNG", "rechnungsnummer": "NNE-2026-02" }),
        dec!(1234.56),
        CheckOutcome::Ok,
    )
    .await
    .expect("insert the original draft");

    let storno = insert_correction_draft(&pool, original, "Ablesefehler", None)
        .await
        .expect("insert a Storno");

    let row = fetch_draft(&pool, storno)
        .await
        .expect("fetch")
        .expect("the correction exists");
    assert_eq!(row.rechnungsart, "STORNORECHNUNG");
    assert_eq!(
        row.gross_eur_units, -123_456_000,
        "Storno negates the gross"
    );
    assert_eq!(row.original_draft_id, Some(original));

    let tenant: String = sqlx::query_scalar("SELECT tenant FROM invoice_drafts WHERE id = $1")
        .bind(storno)
        .fetch_one(&pool)
        .await
        .expect("read the correction tenant");
    assert_eq!(tenant, "tenant-b", "the correction belongs to tenant-b");
}
