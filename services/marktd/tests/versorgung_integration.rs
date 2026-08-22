//! Real-PostgreSQL guards for the VersorgungsStatus state machine and the
//! preisblatt read path — the invariants billingd and invoicd trust.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use mako_markt::domain::MaloId;
use mako_markt::repository::VersorgungsStatusRepository as _;
use marktd::pg::PgVersorgungsStatusRepository;
use sqlx::PgPool;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const MALO: &str = "51238696012"; // valid checksum

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

fn malo() -> MaloId {
    MALO.parse().expect("valid MaLo")
}

// ── The 55004/44004 gap: a cancelled Lieferbeginn clears lf_mp_id_next ─────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn cancelled_lieferbeginn_clears_the_announced_future_supplier() {
    let Some((pool, _pg)) = test_pool("clear_lf_next").await else {
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

    // Ablehnung Anmeldung: the announcement must be reset, or the next
    // supplier's Anmeldung is rejected against a stale marker.
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

/// A second supplier's Anmeldung must **not** displace the pending one.
///
/// `marktd` writes `lf_mp_id_next` while ingesting the `process.initiated`,
/// *before* fanning the event out to `processd` — so by the time `mako-pruefung`
/// runs its EBD `E_0622` Prüfschritt 70 check, the Anmeldung under evaluation
/// has already written its own marker. The check therefore compares MP-IDs
/// rather than testing for presence, and that is only meaningful if the *first*
/// announcement survives: an overwriting upsert lets every Anmeldung find its
/// own MP-ID there, so `A06` „Andere Anmeldung in Bearbeitung" never fires.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_competing_anmeldung_does_not_displace_the_pending_one() {
    let Some((pool, _pg)) = test_pool("competing_anmeldung").await else {
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
    .expect("first announce");
    let first = vs.find(&m, TENANT).await.expect("find").expect("row");

    // A different supplier announces while the first is still pending.
    vs.announce_lf_next(
        &m,
        TENANT,
        "9922222222222",
        Some(time::macros::date!(2026 - 11 - 01)),
        "9900000000001",
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("second announce");

    let after = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(
        after.lf_mp_id_next.as_deref(),
        Some("9911111111111"),
        "the first announcement must survive so A06 can be decided against it"
    );
    assert_eq!(
        after.version, first.version,
        "a losing announcement is a no-op — no version bump, no history row"
    );

    // The *same* supplier re-sending (corrected date, at-least-once redelivery)
    // still updates its own announcement.
    vs.announce_lf_next(
        &m,
        TENANT,
        "9911111111111",
        Some(time::macros::date!(2026 - 10 - 15)),
        "9900000000001",
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("re-announce");
    let corrected = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(
        corrected.lf_next_lieferbeginn,
        Some(time::macros::date!(2026 - 10 - 15)),
        "the holder of the announcement may correct its own date"
    );
}

/// The erzeugende-Marktlokation Anmeldung (55077 → 55078 / 55080) drives the
/// same projection as the verbrauchende one (55001 → 55002 / 55003).
///
/// A `derive_supply_state` that matched only 55001 would leave an EEG-/KWKG-MaLo's
/// supplier change invisible: no announcement for the NB's duplicate check to
/// see, and nothing for the confirmation to promote.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_erzeugende_malo_anmeldung_drives_the_same_projection() {
    use marktd::handlers::event_ingest::derive_supply_state;

    let Some((pool, _pg)) = test_pool("erz_malo_anmeldung").await else {
        return;
    };
    let vs = PgVersorgungsStatusRepository::new(pool.clone());
    let m = malo();
    let mut tx = pool.begin().await.expect("begin");

    derive_supply_state(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_INITIATED,
        Some(55_077),
        &serde_json::json!({
            "malo_id":       m.to_string(),
            "new_supplier":  "9911111111111",
            "grid_operator": "9900000000001",
            "process_date":  "2026-10-01",
        }),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("55077 announce");

    derive_supply_state(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(55_078),
        &serde_json::json!({ "malo_id": m.to_string() }),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("55078 confirm");

    tx.commit().await.expect("commit");

    let after = vs.find(&m, TENANT).await.expect("find").expect("row");
    assert_eq!(
        after.lf_mp_id.as_deref(),
        Some("9911111111111"),
        "55078 must promote the announcement 55077 made"
    );
    assert_eq!(
        after.lieferstatus,
        mako_markt::repository::LieferStatus::Beliefert
    );
    assert_eq!(
        after.lieferbeginn,
        Some(time::macros::date!(2026 - 10 - 01))
    );
}

/// `versorgung.changed` announces a transition, so a delivery that changed
/// nothing must not emit one — a redelivered Ablehnung would otherwise describe
/// a state that has not moved.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_no_op_derivation_emits_no_versorgung_changed() {
    use marktd::handlers::event_ingest::derive_supply_state;

    let Some((pool, _pg)) = test_pool("no_op_derivation").await else {
        return;
    };
    let m = malo();
    let mut tx = pool.begin().await.expect("begin");

    // No announcement pending: the Ablehnung has nothing to clear.
    let evts = derive_supply_state(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(55_003),
        &serde_json::json!({ "malo_id": m.to_string() }),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("55003 on a MaLo with no announcement");
    assert!(
        evts.is_empty(),
        "nothing changed, so nothing may be announced; got {evts:?}"
    );

    tx.commit().await.expect("commit");
}

// ── The core supply lifecycle ─────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn announce_confirm_end_walks_the_lieferstatus_and_records_history() {
    let Some((pool, _pg)) = test_pool("lifecycle").await else {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn preisblatt_is_read_by_nb_mp_id_without_a_tenant_column() {
    let Some((pool, _pg)) = test_pool("preisblatt").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, data)
         VALUES ('9900000000001', '2026-01-01', '{\"_typ\":\"PREISBLATTNETZNUTZUNG\"}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("insert preisblatt");

    // An expired sheet and an open-started one for a second NB.
    sqlx::query(
        "INSERT INTO preisblaetter (nb_mp_id, valid_from, valid_to, data)
         VALUES ('9900000000002', '2024-01-01', '2025-01-01', '{}'::jsonb),
                ('9900000000003', NULL, NULL, '{}'::jsonb)",
    )
    .execute(&pool)
    .await
    .expect("insert more preisblaetter");

    // The corrected get_preisblatt query shape: no `tenant`, column is `data`,
    // half-open validity window, NULL valid_from = open-started.
    let q = r"SELECT id, valid_from, data
              FROM preisblaetter
              WHERE nb_mp_id = $1
                AND (valid_from IS NULL OR valid_from <= $2)
                AND (valid_to   IS NULL OR valid_to   >  $2)
              ORDER BY valid_from DESC NULLS LAST LIMIT 1";
    let at = time::macros::date!(2026 - 06 - 01);
    type Row = Option<(uuid::Uuid, Option<time::Date>, serde_json::Value)>;

    let row: Row = sqlx::query_as(q)
        .bind("9900000000001")
        .bind(at)
        .fetch_optional(&pool)
        .await
        .expect("the query must run — the old `WHERE tenant=$1` referenced a missing column");
    assert!(row.is_some(), "the price sheet is found by nb_mp_id");

    let expired: Row = sqlx::query_as(q)
        .bind("9900000000002")
        .bind(at)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(expired.is_none(), "an expired sheet is not returned");

    let open_started: Row = sqlx::query_as(q)
        .bind("9900000000003")
        .bind(at)
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert!(
        open_started.is_some(),
        "an open-started sheet (valid_from IS NULL) still matches"
    );
}

// ── Ingest is one transaction: marker + projection + outbox ───────────────────

/// A failing derivation must leave nothing behind — no idempotency marker, no
/// projection row, no outbox entry — so makod's redelivery can repair it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_rolled_back_ingest_leaves_neither_marker_nor_projection_nor_outbox() {
    use marktd::handlers::event_ingest::derive_supply_state;

    let Some((pool, _pg)) = test_pool("ingest_rollback").await else {
        return;
    };
    let event_id = uuid::Uuid::new_v4().to_string();
    let mut tx = pool.begin().await.unwrap();

    sqlx::query("INSERT INTO processed_events (event_id) VALUES ($1)")
        .bind(&event_id)
        .execute(&mut *tx)
        .await
        .expect("marker");

    // The handler rolls this transaction back on any derivation / enqueue
    // failure; the rollback below stands in for that failure.
    let evts = derive_supply_state(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_INITIATED,
        Some(55001),
        &serde_json::json!({
            "malo_id":      MALO,
            "new_supplier": "9911111111111",
            "process_date": "2026-10-01",
            "nb_mp_id":     "9900000000001",
        }),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("derivation");
    // Every transition announces itself. Until this was wired, the whole
    // EDIFACT-driven lifecycle emitted nothing and only the REST upsert did.
    assert_eq!(evts.len(), 1, "an announce emits versorgung.changed");
    assert_eq!(evts[0].ce_type, mako_events::markt::VERSORGUNG_CHANGED);
    assert_eq!(
        evts[0].marktsparte.as_deref(),
        Some("STROM"),
        "55001 is a Strom PID, so the sparten subscription filter can match it"
    );
    assert_eq!(
        evts[0].data.get("lf_mp_id_next").and_then(|v| v.as_str()),
        Some("9911111111111"),
        "the announced future Lieferant is carried on the event"
    );
    marktd::outbox::enqueue(
        &mut *tx,
        &mako_markt::cloudevents::MarktEvent::new(
            TENANT,
            mako_events::mako::PROCESS_INITIATED,
            MALO.to_owned(),
            serde_json::json!({}),
        ),
        &tokio::sync::Notify::new(),
    )
    .await
    .expect("enqueue");
    tx.rollback().await.unwrap();

    let markers: i64 = sqlx::query_scalar("SELECT count(*) FROM processed_events")
        .fetch_one(&pool)
        .await
        .unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM versorgungsstatus")
        .fetch_one(&pool)
        .await
        .unwrap();
    let log: i64 = sqlx::query_scalar("SELECT count(*) FROM event_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        (markers, rows, log),
        (0, 0, 0),
        "a rolled-back ingest is invisible in all three tables"
    );
}

/// The EoG derivation resolves the NB's pre-deposited default Bilanzkreis, writes
/// `eog_seit` (which the history snapshot must carry) and returns the
/// `eog-begonnen` event for the caller to enqueue on the same transaction.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn eog_derivation_resolves_the_default_bilanzkreis_and_snapshots_eog_seit() {
    use marktd::handlers::event_ingest::derive_supply_state;

    let Some((pool, _pg)) = test_pool("ingest_eog").await else {
        return;
    };
    sqlx::query(
        "INSERT INTO grundversorger (tenant, nb_mp_id, sparte, gv_mp_id, default_bilanzkreis)
         VALUES ($1, '9900000000001', 'STROM', '9922222222222', 'BK-DEFAULT-1')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect("seed grundversorger");

    let mut tx = pool.begin().await.unwrap();
    let evts = derive_supply_state(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(55013),
        &serde_json::json!({
            "malo_id":      MALO,
            "new_supplier": "9922222222222",
            "nb_mp_id":     "9900000000001",
            "process_date": "20261115",
        }),
        Some(uuid::Uuid::new_v4()),
    )
    .await
    .expect("derivation");
    tx.commit().await.unwrap();

    assert_eq!(evts.len(), 2, "eog-begonnen plus versorgung.changed");
    let begonnen = &evts[0];
    assert_eq!(
        begonnen.ce_type,
        mako_events::markt::VERSORGUNG_EOG_BEGONNEN
    );
    assert_eq!(
        begonnen.data.get("bilanzkreis").and_then(|v| v.as_str()),
        Some("BK-DEFAULT-1"),
        "the NB's deposited default BK is resolved, not null"
    );
    assert_eq!(
        begonnen.data.get("sparte").and_then(|v| v.as_str()),
        Some("STROM"),
        "processd keys its EoG case log on this; without it every Gas case \
         was recorded as Strom"
    );
    assert_eq!(evts[1].ce_type, mako_events::markt::VERSORGUNG_CHANGED);
    assert_eq!(
        evts[1].data.get("lieferstatus").and_then(|v| v.as_str()),
        Some("Ersatzversorgung"),
        "versorgung.changed reports the state the transition produced"
    );

    let vs = PgVersorgungsStatusRepository::new(pool.clone());
    let rec = vs.find(&malo(), TENANT).await.unwrap().expect("row");
    assert_eq!(rec.lieferstatus.to_string(), "Ersatzversorgung");
    assert_eq!(rec.eog_seit, Some(time::macros::date!(2026 - 11 - 15)));

    // The history snapshot carries eog_seit, so ?at= reconstructs the §38 clock.
    let at = vs
        .find_at(&malo(), TENANT, time::macros::date!(2099 - 01 - 01))
        .await
        .unwrap()
        .expect("history row");
    assert_eq!(at.eog_seit, rec.eog_seit);
    assert_eq!(
        at.version, rec.version,
        "history records the actual version"
    );
}

// ── Per-MeLo dated MSB timeline (WiM Teil 2 UC 4.1.1) ─────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn melo_msb_timeline_resolves_the_responsible_msb_at_a_past_date() {
    use mako_markt::repository::MeloMsbRepository as _;
    use marktd::pg::PgMeloMsbRepository;
    use time::macros::date;

    let Some((pool, _pg)) = test_pool("melo_msb").await else {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn bilanzierung_temporal_resource_resolves_by_point_in_time() {
    use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository as _};
    use marktd::pg::PgBilanzierungRepository;
    use time::macros::datetime;

    let Some((pool, _pg)) = test_pool("bilanzierung").await else {
        return;
    };
    let tenant = "9900000000002";
    let malo = "51238696012";

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
        "11XMAKO-BK-TEST9",
    ))
    .await
    .expect("upsert A");
    repo.upsert(&mk(
        datetime!(2025-06-01 00:00 UTC),
        None,
        "11XMAKO-BK-0002F",
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
        Some("11XMAKO-BK-TEST9")
    );
    assert_eq!(
        bk_at(&repo, tenant, malo, datetime!(2025-07-01 00:00 UTC))
            .await
            .as_deref(),
        Some("11XMAKO-BK-0002F")
    );
    assert_eq!(
        bk_at(&repo, tenant, malo, datetime!(2025-06-01 00:00 UTC))
            .await
            .as_deref(),
        Some("11XMAKO-BK-0002F"),
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
        "11XMAKO-BK-0003D",
    ))
    .await
    .expect("re-upsert B");
    let hist = repo.history(tenant, malo).await.unwrap();
    assert_eq!(hist.len(), 2, "still two rows after same-key re-upsert");
    assert_eq!(hist[0].bilanzkreis.as_deref(), Some("11XMAKO-BK-0003D"));
    assert_eq!(
        hist[1].bilanzierungsende,
        Some(datetime!(2025-06-01 00:00 UTC))
    );
}

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn bilanzierung_write_derives_the_malo_fallgruppe_column() {
    use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository as _};
    use marktd::pg::PgBilanzierungRepository;
    use time::macros::datetime;

    let Some((pool, _pg)) = test_pool("biz_derive").await else {
        return;
    };
    let tenant = "9900000000002";
    let malo = "51238696012";

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

    // An OLDER overlapping record is currently-effective by its own window but
    // loses `find_at`'s ordering — it must not clobber the derived cache.
    repo.upsert(&BilanzierungRecord {
        malo_id: malo.to_owned(),
        bilanzierungsbeginn: datetime!(2023-01-01 00:00 UTC),
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
    .expect("upsert older overlapping");
    let fg3: Option<String> = sqlx::query_scalar("SELECT fallgruppe FROM malo WHERE malo_id = $1")
        .bind(malo)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fg3.as_deref(),
        Some("GABI_RLM_MIT_TAGESBAND"),
        "the derived cache follows find_at, not the row just written"
    );
}
// ── Optimistic concurrency is enforced in SQL, not read-then-write ────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn two_puts_with_the_same_if_match_cannot_both_win() {
    use mako_markt::domain::Sparte;
    use mako_markt::repository::MaloRepository as _;
    use marktd::pg::PgMaloRepository;

    let Some((pool, _pg)) = test_pool("malo_occ").await else {
        return;
    };
    let repo = PgMaloRepository::new(pool.clone());
    let m = malo();
    let data: rubo4e::current::Marktlokation =
        serde_json::from_value(serde_json::json!({"_typ": "MARKTLOKATION"}))
            .expect("valid BO4E Marktlokation");

    let v1 = repo
        .upsert(&m, Sparte::Strom, &data, vec![], None, "v202607.0.0")
        .await
        .expect("create");
    assert_eq!(v1, 1);

    let v2 = repo
        .upsert(&m, Sparte::Strom, &data, vec![], Some(v1), "v202607.0.0")
        .await
        .expect("first If-Match write wins");
    assert_eq!(v2, 2, "the returned version is the one actually stored");

    let conflict = repo
        .upsert(&m, Sparte::Strom, &data, vec![], Some(v1), "v202607.0.0")
        .await;
    assert!(
        matches!(
            conflict,
            Err(mako_markt::error::MdmError::VersionConflict { .. })
        ),
        "the second writer with the stale If-Match is rejected, not silently applied"
    );
}

// ── A backdated MSB correction leaves exactly one open assignment ─────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_backdated_msb_assignment_does_not_leave_two_open_rows() {
    use mako_markt::repository::MeloMsbRepository as _;
    use marktd::pg::PgMeloMsbRepository;
    use time::macros::date;

    let Some((pool, _pg)) = test_pool("melo_msb_backdated").await else {
        return;
    };
    let tenant = "9900000000002";
    let melo = "DE0001112223334445556667778889990";
    sqlx::query("INSERT INTO melo (melo_id, data) VALUES ($1, '{}'::jsonb)")
        .bind(melo)
        .execute(&pool)
        .await
        .expect("seed melo");

    let repo = PgMeloMsbRepository::new(pool.clone());
    repo.assign_msb(tenant, melo, "9900000000011", date!(2025 - 06 - 01))
        .await
        .expect("current assignment");
    // Backdated correction: an earlier assignment arrives late.
    repo.assign_msb(tenant, melo, "9900000000010", date!(2024 - 01 - 01))
        .await
        .expect("backdated assignment");

    let hist = repo.history(tenant, melo).await.unwrap();
    assert_eq!(
        hist.iter().filter(|z| z.valid_to.is_none()).count(),
        1,
        "only the latest assignment stays open"
    );
    assert_eq!(
        hist.iter()
            .find(|z| z.msb_mp_id == "9900000000010")
            .unwrap()
            .valid_to,
        Some(date!(2025 - 06 - 01)),
        "the backdated row ends where the later one starts"
    );
    assert_eq!(
        repo.find_msb_at(tenant, melo, date!(2024 - 06 - 01))
            .await
            .unwrap()
            .as_deref(),
        Some("9900000000010"),
    );
    assert_eq!(
        repo.find_msb_at(tenant, melo, date!(2025 - 07 - 01))
            .await
            .unwrap()
            .as_deref(),
        Some("9900000000011"),
    );
}

/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a fresh throwaway `postgres:17-alpine` and return its URL plus the
/// container guard. `None` when Docker is unavailable (tests skip gracefully).
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

// ── WiM: the Zuordnung is constitutive, the Anmeldebestätigung is not ─────────

/// The MSB timeline moves on IFTSTA **21012**, from the date the Gesamtvorgang
/// reported — not on the Anmeldebestätigung 55043, which WiM Strom Teil 1
/// Kap. 2.3.2 Nr. 2 calls *vorläufig*.
///
/// Deriving the assignment from 55043 would move the Messlokation up to nine
/// Werktage early (the Realisierungskorridor), and would move it at all in the
/// case where the Gesamtvorgang later fails — which Kap. 2.3.2 Nr. 7 answers by
/// leaving the MSBA in place.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_msb_zuordnung_follows_the_gesamtvorgang_not_the_anmeldebestaetigung() {
    use mako_markt::repository::MeloMsbRepository as _;
    use marktd::handlers::event_ingest::derive_msb_zuordnung;
    use marktd::pg::PgMeloMsbRepository;

    let Some((pool, _pg)) = test_pool("wim_zuordnung").await else {
        return;
    };
    let repo = PgMeloMsbRepository::new(pool.clone());
    let melo = "DE0000000001234567890000000000001";
    let msba = "9900000000001";
    let msbn = "4012345000023";

    sqlx::query("INSERT INTO melo (melo_id, data) VALUES ($1, $2)")
        .bind(melo)
        .bind(serde_json::json!({ "messlokationsId": melo }))
        .execute(&pool)
        .await
        .expect("seed MeLo");

    // The MSBA holds the Messlokation.
    repo.assign_msb(TENANT, melo, msba, time::macros::date!(2024 - 01 - 01))
        .await
        .expect("initial assignment");

    let mut tx = pool.begin().await.expect("begin");

    // The vorläufige Anmeldebestätigung changes nothing.
    derive_msb_zuordnung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(55_043),
        &serde_json::json!({
            "melo_id":   melo,
            "msb_mp_id": msbn,
            "zuordnungsbeginn": "2026-06-01",
        }),
    )
    .await
    .expect("55043 is not a Zuordnung");

    // …and neither does a 21012 that names no date: without one there is no
    // day to assign from, and today would silently disagree with the market.
    derive_msb_zuordnung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(21_012),
        &serde_json::json!({ "melo_id": melo, "msb_mp_id": msbn }),
    )
    .await
    .expect("a dateless 21012 is recorded, not applied");

    tx.commit().await.expect("commit");
    assert_eq!(
        repo.find_msb_at(TENANT, melo, time::macros::date!(2026 - 06 - 15))
            .await
            .expect("find")
            .as_deref(),
        Some(msba),
        "only IFTSTA 21012 with a Zuordnungsbeginn moves the assignment"
    );

    // The Zuordnung itself.
    let mut tx = pool.begin().await.expect("begin");
    derive_msb_zuordnung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(21_012),
        &serde_json::json!({
            "melo_id":          melo,
            "msb_mp_id":        msbn,
            "zuordnungsbeginn": "2026-06-10",
        }),
    )
    .await
    .expect("21012 assigns");
    tx.commit().await.expect("commit");

    // „Die Zuordnung des MSBA endet entsprechend zu diesem Zeitpunkt."
    assert_eq!(
        repo.find_msb_at(TENANT, melo, time::macros::date!(2026 - 06 - 09))
            .await
            .expect("find")
            .as_deref(),
        Some(msba),
        "the day before the Zuordnungsbeginn still belongs to the MSBA"
    );
    assert_eq!(
        repo.find_msb_at(TENANT, melo, time::macros::date!(2026 - 06 - 10))
            .await
            .expect("find")
            .as_deref(),
        Some(msbn),
        "the MSBN holds the Messlokation from the reported date, 00:00 Uhr"
    );

    // At-least-once delivery: the same event twice is the same timeline.
    let mut tx = pool.begin().await.expect("begin");
    derive_msb_zuordnung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(21_012),
        &serde_json::json!({
            "melo_id":          melo,
            "msb_mp_id":        msbn,
            "zuordnungsbeginn": "2026-06-10",
        }),
    )
    .await
    .expect("redelivery");
    tx.commit().await.expect("commit");
    let history = repo.history(TENANT, melo).await.expect("history");
    assert_eq!(
        history.len(),
        2,
        "redelivery must not add a row: {history:?}"
    );
}

/// A Zuordnung for a Messlokation `marktd` does not know must not poison the
/// ingest queue: `melo_msb_zuordnungen.melo_id` is a foreign key, so letting
/// the insert fail would roll the whole transaction back — idempotency marker
/// included — and `makod` would redeliver the same event forever.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_zuordnung_for_an_unknown_melo_is_skipped_not_retried_forever() {
    use marktd::handlers::event_ingest::derive_msb_zuordnung;

    let Some((pool, _pg)) = test_pool("wim_zuordnung_unknown_melo").await else {
        return;
    };
    let mut tx = pool.begin().await.expect("begin");
    derive_msb_zuordnung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(21_012),
        &serde_json::json!({
            "melo_id":          "DE0000000009999999990000000000009",
            "msb_mp_id":        "4012345000023",
            "zuordnungsbeginn": "2026-06-10",
        }),
    )
    .await
    .expect("an unknown MeLo is logged, not an error");
    tx.commit().await.expect("the transaction is still usable");
}
