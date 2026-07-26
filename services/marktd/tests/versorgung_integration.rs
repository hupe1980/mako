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

    // 55005 (Bestätigung Lieferende): end → Unbeliefert, active LF cleared.
    vs.end_supply(&m, TENANT, "9900000000001", Some(uuid::Uuid::new_v4()))
        .await
        .expect("end");
    let ended = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(ended.lieferstatus.to_string(), "Unbeliefert");
    assert!(ended.lf_mp_id.is_none());
    assert!(ended.eog_seit.is_none());

    // 55013 (Anmeldung/Zuordnung EOG completed): the Grundversorger becomes
    // the supplier of record — §38 EnWG Ersatzversorgung, eog_seit anchors
    // the 3-month maximum (may be retroactive).
    vs.begin_eog_supply(
        &m,
        TENANT,
        "9922222222222",
        "9900000000001",
        mako_markt::repository::LieferStatus::Ersatzversorgung,
        Some(time::macros::date!(2026 - 11 - 15)),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("begin eog");
    let eog = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(eog.lieferstatus.to_string(), "Ersatzversorgung");
    assert_eq!(eog.lf_mp_id.as_deref(), Some("9922222222222"));
    assert_eq!(eog.eog_seit, Some(time::macros::date!(2026 - 11 - 15)));

    // A regular switch confirmation ends the fallback supply and clears
    // the §38 clock.
    vs.announce_lf_next(
        &m,
        TENANT,
        "9911111111111",
        Some(time::macros::date!(2027 - 01 - 01)),
        "9900000000001",
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("announce during EoG");
    vs.confirm_supply(&m, TENANT, Some(uuid::Uuid::new_v4()))
        .await
        .expect("confirm ends EoG");
    let back = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(back.lieferstatus.to_string(), "Beliefert");
    assert!(
        back.eog_seit.is_none(),
        "confirm_supply clears the §38 clock"
    );

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
        hist_count >= 6,
        "announce+confirm+end+eog+announce+confirm each recorded, got {hist_count}"
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

// ── Per-MeLo dated MSB timeline (WiM Teil 2 UC 4.1.1) ─────────────────────────

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn melo_msb_timeline_resolves_the_responsible_msb_at_a_past_date() {
    use mako_markt::repository::MeloMsbRepository as _;
    use marktd::pg::PgMeloMsbRepository;
    use time::macros::date;

    let Some(pool) = test_pool("melo_msb").await else {
        return;
    };
    let tenant = "9900000000002";
    let melo = "DE0001112223334445556667778889990";

    // The FK requires the MeLo to exist.
    sqlx::query("INSERT INTO melo (melo_id, data) VALUES ($1, '{}'::jsonb)")
        .bind(melo)
        .execute(&pool)
        .await
        .expect("seed melo");

    let repo = PgMeloMsbRepository::new(pool.clone());

    // MSB-A from 2024-01-01, then MSB-B from 2025-06-01 (closes A).
    repo.assign_msb(tenant, melo, "9900000000010", date!(2024 - 01 - 01))
        .await
        .expect("assign A");
    repo.assign_msb(tenant, melo, "9900000000011", date!(2025 - 06 - 01))
        .await
        .expect("assign B");

    // Point-in-time resolution.
    assert_eq!(
        repo.find_msb_at(tenant, melo, date!(2024 - 06 - 01))
            .await
            .unwrap()
            .as_deref(),
        Some("9900000000010"),
        "mid-2024 → MSB-A"
    );
    assert_eq!(
        repo.find_msb_at(tenant, melo, date!(2025 - 07 - 01))
            .await
            .unwrap()
            .as_deref(),
        Some("9900000000011"),
        "mid-2025 → MSB-B"
    );
    assert_eq!(
        repo.find_msb_at(tenant, melo, date!(2025 - 06 - 01))
            .await
            .unwrap()
            .as_deref(),
        Some("9900000000011"),
        "the switch date itself → MSB-B (valid_from inclusive)"
    );
    assert!(
        repo.find_msb_at(tenant, melo, date!(2023 - 01 - 01))
            .await
            .unwrap()
            .is_none(),
        "before any assignment → none"
    );

    // History is newest-first with the older row closed at the switch date.
    let hist = repo.history(tenant, melo).await.unwrap();
    assert_eq!(hist.len(), 2);
    assert_eq!(hist[0].msb_mp_id, "9900000000011");
    assert!(
        hist[0].valid_to.is_none(),
        "current assignment is open-ended"
    );
    assert_eq!(hist[1].msb_mp_id, "9900000000010");
    assert_eq!(hist[1].valid_to, Some(date!(2025 - 06 - 01)));
}

// ── BO4E Bilanzierung — first-class temporal resource (BO #3) ─────────────────

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn bilanzierung_temporal_resource_resolves_by_point_in_time() {
    use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository as _};
    use marktd::pg::PgBilanzierungRepository;
    use time::macros::datetime;

    let Some(pool) = test_pool("bilanzierung").await else {
        return;
    };
    let tenant = "9900000000002";
    let malo = "51238696780";

    let mk = |beginn, ende, bk: &str| BilanzierungRecord {
        malo_id: malo.to_owned(),
        bilanzierungsbeginn: beginn,
        bilanzierungsende: ende,
        bilanzkreis: Some(bk.to_owned()),
        aggregationsverantwortung: Some("NB".to_owned()),
        prognosegrundlage: Some("SLP".to_owned()),
        fallgruppenzuordnung: None,
        data: serde_json::json!({
            "_typ": "BILANZIERUNG",
            "marktlokationsId": malo,
            "bilanzierungsbeginn": beginn.format(&time::format_description::well_known::Rfc3339).unwrap(),
            "bilanzkreis": bk,
        }),
        bo4e_version: "v202607.0.0".to_owned(),
        tenant: tenant.to_owned(),
        updated_at: time::OffsetDateTime::now_utc(),
    };

    let repo = PgBilanzierungRepository::new(pool.clone());
    // BK "A" valid 2024-01-01 .. 2025-06-01, then BK "B" open-ended.
    repo.upsert(&mk(
        datetime!(2024-01-01 00:00 UTC),
        Some(datetime!(2025-06-01 00:00 UTC)),
        "11YWA-------BK-A",
    ))
    .await
    .expect("upsert A");
    repo.upsert(&mk(
        datetime!(2025-06-01 00:00 UTC),
        None,
        "11YWB-------BK-B",
    ))
    .await
    .expect("upsert B");

    async fn bk_at(
        repo: &PgBilanzierungRepository,
        tenant: &str,
        malo: &str,
        dt: time::OffsetDateTime,
    ) -> Option<String> {
        repo.find_at(tenant, malo, dt)
            .await
            .unwrap()
            .and_then(|r| r.bilanzkreis)
    }
    assert_eq!(
        bk_at(&repo, tenant, malo, datetime!(2024-06-01 00:00 UTC))
            .await
            .as_deref(),
        Some("11YWA-------BK-A")
    );
    assert_eq!(
        bk_at(&repo, tenant, malo, datetime!(2025-07-01 00:00 UTC))
            .await
            .as_deref(),
        Some("11YWB-------BK-B")
    );
    assert_eq!(
        bk_at(&repo, tenant, malo, datetime!(2025-06-01 00:00 UTC))
            .await
            .as_deref(),
        Some("11YWB-------BK-B"),
        "the switch instant resolves to the newer Bilanzierung (beginn inclusive)"
    );
    assert!(
        bk_at(&repo, tenant, malo, datetime!(2023-01-01 00:00 UTC))
            .await
            .is_none(),
        "before any Bilanzierung → none"
    );

    // Re-upsert on the same beginn overwrites (idempotent natural key).
    repo.upsert(&mk(
        datetime!(2025-06-01 00:00 UTC),
        None,
        "11YWC-------BK-C",
    ))
    .await
    .expect("re-upsert B");
    let hist = repo.history(tenant, malo).await.unwrap();
    assert_eq!(hist.len(), 2, "still two rows after same-key re-upsert");
    assert_eq!(hist[0].bilanzkreis.as_deref(), Some("11YWC-------BK-C"));
    assert_eq!(
        hist[1].bilanzierungsende,
        Some(datetime!(2025-06-01 00:00 UTC))
    );
}

#[tokio::test]
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn bilanzierung_write_derives_the_malo_fallgruppe_column() {
    use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository as _};
    use marktd::pg::PgBilanzierungRepository;
    use time::macros::datetime;

    let Some(pool) = test_pool("biz_derive").await else {
        return;
    };
    let tenant = "9900000000002";
    let malo = "51238696780";

    // The MaLo (Marktlokation) must exist for the derive to land.
    sqlx::query("INSERT INTO malo (malo_id, sparte, data) VALUES ($1, 'GAS', '{}'::jsonb)")
        .bind(malo)
        .execute(&pool)
        .await
        .expect("seed malo");

    let repo = PgBilanzierungRepository::new(pool.clone());
    // A currently-effective Bilanzierung carrying the GaBi Fallgruppe.
    repo.upsert(&BilanzierungRecord {
        malo_id: malo.to_owned(),
        bilanzierungsbeginn: datetime!(2024-01-01 00:00 UTC),
        bilanzierungsende: None,
        bilanzkreis: Some("11YWBK-------X".to_owned()),
        aggregationsverantwortung: Some("NB".to_owned()),
        prognosegrundlage: Some("SLP".to_owned()),
        fallgruppenzuordnung: Some("GABI_RLM_MIT_TAGESBAND".to_owned()),
        data: serde_json::json!({"_typ": "BILANZIERUNG", "marktlokationsId": malo}),
        bo4e_version: "v202607.0.0".to_owned(),
        tenant: tenant.to_owned(),
        updated_at: time::OffsetDateTime::now_utc(),
    })
    .await
    .expect("upsert");

    // The authoritative Bilanzierung derived the denormalised malo column.
    let fg: Option<String> = sqlx::query_scalar("SELECT fallgruppe FROM malo WHERE malo_id = $1")
        .bind(malo)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fg.as_deref(),
        Some("GABI_RLM_MIT_TAGESBAND"),
        "writing a currently-effective Bilanzierung derives malo.fallgruppe"
    );

    // A NOT-yet-effective (future) Bilanzierung must NOT overwrite the current value.
    repo.upsert(&BilanzierungRecord {
        malo_id: malo.to_owned(),
        bilanzierungsbeginn: datetime!(2099-01-01 00:00 UTC),
        bilanzierungsende: None,
        bilanzkreis: None,
        aggregationsverantwortung: None,
        prognosegrundlage: None,
        fallgruppenzuordnung: Some("GABI_RLM_OHNE_TAGESBAND".to_owned()),
        data: serde_json::json!({"_typ": "BILANZIERUNG"}),
        bo4e_version: "v202607.0.0".to_owned(),
        tenant: tenant.to_owned(),
        updated_at: time::OffsetDateTime::now_utc(),
    })
    .await
    .expect("upsert future");
    let fg2: Option<String> = sqlx::query_scalar("SELECT fallgruppe FROM malo WHERE malo_id = $1")
        .bind(malo)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fg2.as_deref(),
        Some("GABI_RLM_MIT_TAGESBAND"),
        "a future-dated Bilanzierung does not touch the current derived value"
    );
}
