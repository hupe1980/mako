//! Real-PostgreSQL guards for the Zuordnungsermächtigung store.
//!
//! A BKV authorises „je ZRT, BG, BK und LF ab einem bestimmten Zeitpunkt"
//! (BK6-24-174 Anlage 3 (MaBiS) Kap. 10.2.1, S. 119) by UTILMD 55071 and
//! withdraws by 55072, after which „können keine Zuordnungen mit Gültigkeit
//! nach dem Deaktivierungsdatum mehr durchgeführt werden" (Kap. 10.3.1, S. 121).
//! The NB's `E_0622` Prüfschritt 60 reads this store. The messages reach it as
//! the `ProcessCompleted` the `mabis-zp-lifecycle` workflow itself queues, so
//! a key renamed on one side fails here.
//!
//! ```bash
//! just test-marktd-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use mako_engine::types::{MarktpartnerCode, MessageRef, Pruefidentifikator};
use mako_engine::workflow::Workflow;
use mako_mabis::{
    MabisZpLifecycleWorkflow, ZpLifecycleCommand, ZpLifecycleState, ZpVorgang,
    ZuordnungsermaechtigungAngaben,
};
use mako_markt::repository::ZuordnungsermaechtigungRecord;
use marktd::handlers::event_ingest::derive_zuordnungsermaechtigung;
use marktd::pg::PgZuordnungsermaechtigungRepository;
use sqlx::PgPool;
use time::macros::date;

const SCHEMA: &str = include_str!("../../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const BKV: &str = "9900000000019";
const LF: &str = "9900357000011";
const BK: &str = "11XBK-LF-------9";
const BG: &str = "11YB-TENNET----U";

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

/// The `ProcessCompleted` payload the workflow queues for a 55071 / 55072 of
/// the test key with `datum` (`YYYYMMDD`).
fn notification(vorgang: ZpVorgang, datum: &str) -> (u32, serde_json::Value) {
    let pid = match vorgang {
        ZpVorgang::Aktivierung => 55_071,
        ZpVorgang::Deaktivierung => 55_072,
    };
    let out = MabisZpLifecycleWorkflow::handle(
        &ZpLifecycleState::New,
        ZpLifecycleCommand::ReceiveZuordnungsermaechtigung {
            pid: Pruefidentifikator::new(pid).expect("valid PID"),
            vorgang,
            sender: MarktpartnerCode::new(BKV),
            receiver: MarktpartnerCode::new(TENANT),
            document_date: "202608201000+00".to_owned(),
            angaben: ZuordnungsermaechtigungAngaben {
                lf_mp_id: LF.to_owned(),
                bilanzkreis: BK.to_owned(),
                bilanzierungsgebiet: BG.to_owned(),
                zeitreihentyp: "SLS".to_owned(),
                datum: datum.to_owned(),
                nachrichtendatum: "20260820".to_owned(),
            },
            message_ref: MessageRef::new("MSG-1"),
            validation_passed: true,
            validation_errors: vec![],
        },
    )
    .expect("the workflow records it");
    let [entry] = &out.outbox[..] else {
        panic!("one ProcessCompleted, got {:?}", out.outbox);
    };
    assert_eq!(&*entry.message_type, "ProcessCompleted");
    (pid, entry.payload.clone())
}

async fn ingest(pool: &PgPool, vorgang: ZpVorgang, datum: &str) {
    let (pid, data) = notification(vorgang, datum);
    let mut tx = pool.begin().await.expect("begin");
    derive_zuordnungsermaechtigung(
        &mut tx,
        TENANT,
        mako_events::mako::PROCESS_COMPLETED,
        Some(pid),
        &data,
    )
    .await
    .expect("ingest");
    tx.commit().await.expect("commit");
}

async fn on_file(pool: &PgPool) -> Vec<ZuordnungsermaechtigungRecord> {
    PgZuordnungsermaechtigungRepository::new(pool.clone())
        .fuer_lieferant(TENANT, LF)
        .await
        .expect("read")
}

/// A 55071 activates the Ermächtigung from its Bilanzierungsbeginn, and a
/// 55072 ends it on its Deaktivierungsdatum, with the day it was sent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_55071_activates_and_a_55072_ends_it() {
    let Some((pool, _pg)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    ingest(&pool, ZpVorgang::Aktivierung, "20260801").await;
    let [z] = &on_file(&pool).await[..] else {
        panic!("one Ermächtigung");
    };
    assert_eq!(
        (
            z.bkv_mp_id.as_str(),
            z.bilanzkreis.as_str(),
            z.bilanzierungsgebiet.as_str()
        ),
        (BKV, BK, BG)
    );
    assert_eq!(z.zeitreihentyp, "SLS");
    assert_eq!((z.valid_from, z.valid_to), (date!(2026 - 08 - 01), None));

    ingest(&pool, ZpVorgang::Deaktivierung, "20261001").await;
    let [z] = &on_file(&pool).await[..] else {
        panic!("still one Ermächtigung");
    };
    assert_eq!(z.valid_to, Some(date!(2026 - 10 - 01)));
    assert_eq!(
        z.deaktivierung_nachrichtendatum,
        Some(date!(2026 - 08 - 20))
    );
    assert!(z.gilt_am(date!(2026 - 09 - 30)));
    assert!(!z.gilt_am(date!(2026 - 10 - 01)));

    // Redelivered, neither message changes what is on file.
    ingest(&pool, ZpVorgang::Aktivierung, "20260801").await;
    ingest(&pool, ZpVorgang::Deaktivierung, "20261001").await;
    assert_eq!(on_file(&pool).await.len(), 1);
}

/// A 55072 for an Ermächtigung nobody activated ends nothing and writes
/// nothing (MaBiS Kap. 10.3.2 Nr. 2 — the NB clears it with the BKV).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_55072_without_an_active_ermaechtigung_writes_nothing() {
    let Some((pool, _pg)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    ingest(&pool, ZpVorgang::Deaktivierung, "20261001").await;
    assert!(on_file(&pool).await.is_empty());
}

/// Two rows of one key may not claim the same day: the exclusion constraint
/// refuses a second, overlapping one, however it is written.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn two_ermaechtigungen_of_one_key_cannot_overlap() {
    let Some((pool, _pg)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    ingest(&pool, ZpVorgang::Aktivierung, "20260801").await;
    let err = sqlx::query(
        "INSERT INTO zuordnungsermaechtigungen (tenant, bkv_mp_id, lf_mp_id, bilanzkreis, \
         bilanzierungsgebiet, zeitreihentyp, valid_from) \
         VALUES ($1, $2, $3, $4, $5, 'SLS', '2026-09-01')",
    )
    .bind(TENANT)
    .bind(BKV)
    .bind(LF)
    .bind(BK)
    .bind(BG)
    .execute(&pool)
    .await
    .expect_err("overlap refused");
    assert_eq!(
        err.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23P01"),
        "{err}"
    );
    // An Aktivierung after a Deaktivierung opens a new, adjoining row.
    ingest(&pool, ZpVorgang::Deaktivierung, "20261001").await;
    ingest(&pool, ZpVorgang::Aktivierung, "20261101").await;
    let rows = on_file(&pool).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].valid_from, date!(2026 - 11 - 01));
}

/// The NB's walk reads what the store answers: an Anmeldung the 55071 covers
/// passes `E_0622` Prüfschritt 60; once a 55072 ends the Ermächtigung before
/// its Zuordnungsbeginn, the same pending Anmeldung is refused `A05`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_55072_makes_the_walk_of_a_pending_anmeldung_refuse() {
    let Some((pool, _pg)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    ingest(&pool, ZpVorgang::Aktivierung, "20260801").await;

    let walk = |ermaechtigungen: Vec<ZuordnungsermaechtigungRecord>| {
        let anfrage = mako_pruefung::AnmeldungAnfrage {
            pid: 55_001,
            process_id: uuid::Uuid::nil(),
            malo_id: "51238696012".to_owned(),
            new_supplier_gln: LF.to_owned(),
            grid_operator_gln: TENANT.to_owned(),
            bilanzierungsgebiet: Some(BG.to_owned()),
            process_date: date!(2026 - 10 - 15),
            sparte: mako_markt::domain::Sparte::Strom,
            messtyp: mako_pruefung::Messtyp::Slp,
            transaktionsgrund: Some("E03".to_owned()),
            marktlokationsart: mako_pruefung::nb::Marktlokationsart::Verbrauchend,
            erzeugung: None,
            abmeldeanfrage: mako_pruefung::Abmeldeanfrage::NichtErforderlich,
            ruhend: mako_pruefung::RuhendeAnmeldung::default(),
            bilanzkreis: Some(BK.to_owned()),
            zuordnungsermaechtigungen: Some(ermaechtigungen),
        };
        let versorgung = mako_markt::repository::VersorgungsStatusRecord {
            malo_id: "51238696012".parse().expect("valid MaLo"),
            lieferstatus: mako_markt::repository::LieferStatus::Grundversorgung,
            zuordnungen: Vec::new(),
            lieferende: None,
            msb_mp_id: None,
            nb_mp_id: TENANT.to_owned(),
            eog_seit: None,
            last_process_id: None,
            updated_at: time::macros::datetime!(2026-09-01 00:00 UTC),
            tenant: TENANT.to_owned(),
            version: 1,
        };
        let grid = mako_pruefung::MaloGridRecord {
            malo_id: "51238696012".to_owned(),
            nb_mp_id: TENANT.to_owned(),
            bilanzierungsgebiet: Some(BG.to_owned()),
            netzgebiet: None,
        };
        mako_pruefung::evaluate(
            &anfrage,
            Some(&versorgung),
            Some(&grid),
            true,
            time::macros::datetime!(2026-09-01 10:00 UTC),
            &mako_pruefung::NetzCheckConfig::default(),
        )
    };

    let vorher = walk(on_file(&pool).await);
    assert!(vorher.is_accept(), "{vorher:?}");

    ingest(&pool, ZpVorgang::Deaktivierung, "20261001").await;
    let nachher = walk(on_file(&pool).await);
    assert_eq!(nachher.antwortcode(), Some("A05"), "{nachher:?}");
    assert_eq!(nachher.ebd(), Some("E_0622"));
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
