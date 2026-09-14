//! Real-PostgreSQL guards that a write keyed on a caller-supplied identifier
//! stays inside the tenant it came from.
//!
//! Three tables here are keyed on an identifier the caller chooses — a
//! `contract_id` in the request path, a `netzzugang_antraege.id` in the request
//! body, a `zaehler_saisons.id` in the payload — and each key is unique across
//! the whole table rather than per tenant. An `ON CONFLICT … DO UPDATE` on such
//! a key reaches another tenant's row, and no Cedar policy sees it: the check
//! upstream compares the caller against the deployment's tenant, which the row
//! it is about to overwrite has nothing to do with.
//!
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable.

use mako_markt::error::MdmError;
use mako_markt::repository::{
    NbContractRecord, NbContractRepository as _, NetzzugangAktion, NetzzugangAntrag,
    NetzzugangAntragTyp, NetzzugangStatus, ZaehlzeitRegisterRecord, ZaehlzeitRepository as _,
    ZaehlzeitSaisonRecord,
};
use marktd::pg::{PgNbContractRepository, PgNetzzugangRepository, PgZaehlzeitRepository};
use sqlx::PgPool;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");

const TENANT_A: &str = "9900357000004";
const TENANT_B: &str = "9900111000002";
const MALO: &str = "51238696012";

/// The Postgres container guard a test holds until it ends — dropping it removes
/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

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

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

async fn seed_malo(pool: &PgPool) {
    sqlx::query("INSERT INTO malo (malo_id, sparte, data) VALUES ($1, 'STROM', '{}'::jsonb)")
        .bind(MALO)
        .execute(pool)
        .await
        .expect("seed malo");
}

fn contract(contract_id: &str, tenant: &str, netzebene: &str) -> NbContractRecord {
    NbContractRecord {
        contract_id: contract_id.to_owned(),
        malo_id: MALO.parse().expect("a valid MaLo-ID"),
        nb_mp_id: "9900000000001".to_owned(),
        sparte: mako_markt::domain::Sparte::Strom,
        netzebene: netzebene.to_owned(),
        bilanzierungsmethode: "SLP".to_owned(),
        billing_schedule: mako_markt::repository::BillingSchedule::Monthly,
        netznutzer_mp_id: "9900222000008".to_owned(),
        netznutzer_typ: mako_markt::repository::NetznutzerTyp::default(),
        valid_from: time::macros::date!(2026 - 01 - 01),
        valid_to: None,
        data: serde_json::json!({}),
        vertragsart: None,
        vertragsstatus: None,
        tenant: tenant.to_owned(),
        version: 0,
    }
}

/// A `contract_id` is the global primary key and arrives in the request path.
/// A second tenant naming the same one is refused, and the stored contract is
/// untouched.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_network_contract_of_another_tenant_is_never_overwritten() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    seed_malo(&pool).await;
    let repo = PgNbContractRepository::new(pool.clone());

    repo.upsert(contract("NNV-4711", TENANT_A, "NS"))
        .await
        .expect("the owning tenant stores its contract");

    let err = repo
        .upsert(contract("NNV-4711", TENANT_B, "HS"))
        .await
        .expect_err("a foreign tenant must not write this contract_id");
    assert!(
        matches!(err, MdmError::Forbidden { .. }),
        "the refusal must be an authorization answer, got: {err}"
    );

    let stored = repo
        .find("NNV-4711", TENANT_A)
        .await
        .expect("read back")
        .expect("the owner's contract is still there");
    assert_eq!(
        stored.netzebene, "NS",
        "the stored contract was overwritten"
    );
    assert_eq!(stored.tenant, TENANT_A);
    assert_eq!(stored.version, 1, "the refused write bumped the version");

    // The foreign tenant sees nothing under that id either.
    assert!(
        repo.find("NNV-4711", TENANT_B)
            .await
            .expect("read back")
            .is_none()
    );
}

fn antrag(id: Uuid, tenant: &str, netzanschluss_id: &str) -> NetzzugangAntrag {
    NetzzugangAntrag {
        id,
        tenant: tenant.to_owned(),
        antrag_typ: NetzzugangAntragTyp::Zaehlpunktanordnung,
        aktion: NetzzugangAktion::Bestellung,
        netzanschluss_id: netzanschluss_id.to_owned(),
        nb_mp_id: "9900000000001".to_owned(),
        antragsteller_ref: "ref-1".to_owned(),
        status: NetzzugangStatus::Erfasst,
        payload: serde_json::json!({}),
        platform_ref: None,
        created_at: time::OffsetDateTime::UNIX_EPOCH,
        submitted_at: None,
    }
}

/// The §20b request id comes from the JSON body. A second tenant naming an
/// existing one is refused rather than updating it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_netzzugang_request_of_another_tenant_is_never_overwritten() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let repo = PgNetzzugangRepository::new(pool.clone());
    let id = Uuid::new_v4();

    repo.upsert(&antrag(id, TENANT_A, "NA-1"))
        .await
        .expect("the owning tenant stores its request");

    let err = repo
        .upsert(&antrag(id, TENANT_B, "NA-2"))
        .await
        .expect_err("a foreign tenant must not write this id");
    assert!(
        matches!(err, MdmError::Forbidden { .. }),
        "the refusal must be an authorization answer, got: {err}"
    );

    let stored = repo
        .get(TENANT_A, id)
        .await
        .expect("read back")
        .expect("the owner's request is still there");
    assert_eq!(stored.antrag.netzanschluss_id, "NA-1");
    assert_eq!(stored.version, 1, "the refused write bumped the version");
}

async fn seed_register(pool: &PgPool, tenant: &str, zaehler_id: &str) -> Uuid {
    let repo = PgZaehlzeitRepository::new(pool.clone());
    let id = Uuid::new_v4();
    repo.upsert_register(&ZaehlzeitRegisterRecord {
        id,
        zaehler_id: zaehler_id.to_owned(),
        tenant: tenant.to_owned(),
        bezeichnung: "HT".to_owned(),
        zaehlerauspraegung: "HT".to_owned(),
        obis_kennzahl: None,
        einheit: "KWH".to_owned(),
        valid_from: time::macros::date!(2026 - 01 - 01),
        valid_to: None,
        updated_at: time::OffsetDateTime::now_utc(),
    })
    .await
    .expect("seed register");
    id
}

fn saison(id: Uuid, register_id: Uuid, von: time::Time) -> ZaehlzeitSaisonRecord {
    ZaehlzeitSaisonRecord {
        id,
        register_id,
        saison: "GESAMT".to_owned(),
        wochentage: vec![1, 2, 3, 4, 5],
        zeit_von: von,
        zeit_bis: time::macros::time!(22:00),
        updated_at: time::OffsetDateTime::now_utc(),
    }
}

/// `zaehler_saisons` carries no tenant column: it is scoped through its
/// `zaehler_register` parent. A register id from another tenant's request path
/// therefore lists nothing, and a season id cannot be moved onto a foreign
/// register.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_season_window_is_scoped_through_its_register() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let repo = PgZaehlzeitRepository::new(pool.clone());
    let register_a = seed_register(&pool, TENANT_A, "Z-A").await;
    let register_b = seed_register(&pool, TENANT_B, "Z-B").await;

    let window = Uuid::new_v4();
    repo.upsert_saison(&saison(window, register_a, time::macros::time!(07:00)))
        .await
        .expect("the owning tenant stores its window");

    // The parent's tenant decides who may read the child rows.
    assert_eq!(
        repo.list_saisons_by_register(register_a, TENANT_A)
            .await
            .expect("read back")
            .len(),
        1
    );
    assert!(
        repo.list_saisons_by_register(register_a, TENANT_B)
            .await
            .expect("read back")
            .is_empty(),
        "a foreign tenant must not read another register's windows"
    );

    // A caller-supplied id may not be moved onto a register it does not belong
    // to — which is how a window of one tenant would land on another's meter.
    let err = repo
        .upsert_saison(&saison(window, register_b, time::macros::time!(09:00)))
        .await
        .expect_err("the window belongs to another register");
    assert!(
        matches!(err, MdmError::NotFound { .. }),
        "the refusal must name the register, got: {err}"
    );
    let still = repo
        .list_saisons_by_register(register_a, TENANT_A)
        .await
        .expect("read back");
    assert_eq!(still[0].zeit_von, time::macros::time!(07:00));
    assert!(
        repo.list_saisons_by_register(register_b, TENANT_B)
            .await
            .expect("read back")
            .is_empty()
    );

    // A season window whose register does not exist is refused, not stored
    // orphaned.
    let err = repo
        .upsert_saison(&saison(
            Uuid::new_v4(),
            Uuid::new_v4(),
            time::macros::time!(10:00),
        ))
        .await
        .expect_err("no such register");
    assert!(matches!(err, MdmError::NotFound { .. }), "got: {err}");
}
