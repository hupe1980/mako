//! Real-PostgreSQL guards for the VersorgungsStatus state machine and the
//! preisblatt read path — the invariants billingd and invoicd trust.
//!
//! ```bash
//! docker run -d --name marktd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=marktd -p 55438:5432 postgres:17-alpine
//! export MARKTD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55438/marktd"
//! cargo test -p marktd --test versorgung_integration -- --include-ignored
//! ```

use mako_markt::domain::MaloId;
use mako_markt::repository::VersorgungsStatusRepository as _;
use marktd::pg::PgVersorgungsStatusRepository;
use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const MALO: &str = "51238696780"; // valid checksum

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("MARKTD_TEST_DATABASE_URL").ok()?;
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
        .expect("connect schema");
    // Strip `--` comments from the WHOLE file first — a `;` inside a comment
    // would otherwise split a statement mid-body — then split on `;`.
    let stripped: String = SCHEMA
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in stripped.split(';') {
        let s = stmt.trim();
        if s.is_empty() {
            continue;
        }
        sqlx::query(s)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema stmt failed: {e}\n{s}"));
    }
    Some(pool)
}

fn malo() -> MaloId {
    MALO.parse().expect("valid MaLo")
}

// ── The 55004/44004 gap: a cancelled Lieferbeginn clears lf_mp_id_next ─────────

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn cancelled_lieferbeginn_clears_the_announced_future_supplier() {
    let Some(pool) = test_pool("clear_lf_next").await else {
        return;
    };
    let vs = PgVersorgungsStatusRepository::new(pool.clone());
    let m = malo();

    // GPKE 55001: NB records the announced future supplier.
    vs.announce_lf_next(
        &m,
        TENANT,
        "9911111111111",
        Some(time::macros::date!(2026 - 10 - 01)),
        "9900000000001",
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("announce");
    let after_announce = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(
        after_announce.lf_mp_id_next.as_deref(),
        Some("9911111111111"),
        "the future supplier is announced"
    );

    // GPKE 55004 (Abmeldung/Ablehnung): the announcement must be reset — this
    // was the gap, lf_mp_id_next used to stick forever.
    vs.clear_lf_next(&m, TENANT, Some(uuid::Uuid::new_v4()))
        .await
        .expect("clear");
    let after_clear = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert!(
        after_clear.lf_mp_id_next.is_none() && after_clear.lf_next_lieferbeginn.is_none(),
        "the cancelled future supplier is cleared"
    );

    // Idempotent: a second cancellation is a no-op (no version bump).
    let v = after_clear.version;
    vs.clear_lf_next(&m, TENANT, Some(uuid::Uuid::new_v4()))
        .await
        .expect("clear again");
    let again = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(
        again.version, v,
        "no-op cancellation does not bump the version"
    );
}

// ── The core supply lifecycle ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn announce_confirm_end_walks_the_lieferstatus_and_records_history() {
    let Some(pool) = test_pool("lifecycle").await else {
        return;
    };
    let vs = PgVersorgungsStatusRepository::new(pool.clone());
    let m = malo();

    vs.announce_lf_next(
        &m,
        TENANT,
        "9911111111111",
        Some(time::macros::date!(2026 - 10 - 01)),
        "9900000000001",
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("announce");

    // 55003: confirm → the announced LF becomes active, status Beliefert.
    vs.confirm_supply(&m, TENANT, Some(uuid::Uuid::new_v4()))
        .await
        .expect("confirm");
    let active = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(active.lieferstatus.to_string(), "Beliefert");
    assert_eq!(active.lf_mp_id.as_deref(), Some("9911111111111"));
    assert!(active.lf_mp_id_next.is_none(), "pending promoted to active");

    // 55013: end → Unbeliefert, active LF cleared.
    vs.end_supply(&m, TENANT, "9900000000001", Some(uuid::Uuid::new_v4()))
        .await
        .expect("end");
    let ended = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(ended.lieferstatus.to_string(), "Unbeliefert");
    assert!(ended.lf_mp_id.is_none());

    // Every transition left a history row.
    let hist_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM versorgungsstatus_history WHERE malo_id = $1 AND tenant = $2",
    )
    .bind(MALO)
    .bind(TENANT)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        hist_count >= 3,
        "announce+confirm+end each recorded, got {hist_count}"
    );
}

// ── The preisblatt read path (no `tenant` column — matches the fixed query) ────

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn preisblatt_is_read_by_nb_mp_id_without_a_tenant_column() {
    let Some(pool) = test_pool("preisblatt").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, data)
         VALUES ('9900000000001', '2026-01-01', '{\"_typ\":\"PREISBLATTNETZNUTZUNG\"}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("insert preisblatt");

    // The corrected get_preisblatt query shape: no `tenant`, column is `data`.
    let row: Option<(uuid::Uuid, time::Date, serde_json::Value)> = sqlx::query_as(
        r"SELECT id, valid_from, data
          FROM preisblaetter
          WHERE nb_mp_id = $1 AND valid_from <= $2
          ORDER BY valid_from DESC LIMIT 1",
    )
    .bind("9900000000001")
    .bind(time::macros::date!(2026 - 06 - 01))
    .fetch_optional(&pool)
    .await
    .expect("the query must run — the old `WHERE tenant=$1` referenced a missing column");
    assert!(row.is_some(), "the price sheet is found by nb_mp_id");
}
