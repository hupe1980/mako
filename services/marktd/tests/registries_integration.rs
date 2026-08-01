//! Real-PostgreSQL guards for the §20b EnWG Netzzugang request registry and
//! the Gas MSB-Rahmenvertrag registry (GeLi Gas 3.0).
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use mako_markt::{
    error::MdmError,
    repository::{NetzzugangAktion, NetzzugangAntrag, NetzzugangAntragTyp, NetzzugangStatus},
};
use mako_service::cedar::{CedarEnforcer, CedarPrincipal};
use marktd::pg::{
    PgMsbRahmenvertragGasRepository, PgNetzzugangRepository,
    msb_rahmenvertrag_gas::{MsbRahmenvertragGas, MsbRvGasStatus},
};
use sqlx::PgPool;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const GNB: &str = "9870112700007";
const MSB: &str = "9900357000004";
const NB: &str = "9900987654321";

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

// ── Authorization scopes ──────────────────────────────────────────────────────

/// The shipped Cedar policy set parses and covers the registry scopes: any
/// role may read/write same-tenant, cross-tenant is denied, and an
/// unregistered action falls through to Cedar's default deny.
#[test]
fn cedar_policy_registers_the_registry_scopes() {
    let enforcer = CedarEnforcer::from_policy_str(include_str!("../policies/marktd.cedar"))
        .expect("policies/marktd.cedar parses");
    let principal = CedarPrincipal {
        sub: "user-1".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec!["LF".to_owned()],
    };

    for scope in [
        "read-netzzugang",
        "write-netzzugang",
        "read-msb-rv-gas",
        "write-msb-rv-gas",
    ] {
        assert!(
            enforcer.check(&principal, scope, TENANT).is_ok(),
            "{scope} must be allowed same-tenant for any role"
        );
        assert!(
            enforcer.check(&principal, scope, "9900000000000").is_err(),
            "{scope} must be denied cross-tenant"
        );
    }
    assert!(
        enforcer
            .check(&principal, "write-netzzugang-unknown", TENANT)
            .is_err(),
        "unregistered actions stay default-deny"
    );
}

// ── Gas MSB-Rahmenvertrag registry ────────────────────────────────────────────

fn rv(status: MsbRvGasStatus) -> MsbRahmenvertragGas {
    MsbRahmenvertragGas {
        id: Uuid::nil(),
        tenant: TENANT.to_owned(),
        gnb_mp_id: GNB.to_owned(),
        msb_mp_id: MSB.to_owned(),
        fassung: "KoV XV Anlage 8".to_owned(),
        status,
        signed_at: None,
        valid_from: time::macros::date!(2026 - 10 - 01),
        valid_to: None,
        vertrag: serde_json::json!({}),
        version: 0,
    }
}

/// Re-submitting the same business key `(gnb, msb, valid_from)` without an id
/// is an idempotent update: the id stays stable, the version increments, and
/// a `signed_at` on record survives an update that omits it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_gas_upsert_is_idempotent_on_natural_key() {
    let Some((pool, _pg)) = test_pool("rv_idempotent").await else {
        return;
    };
    let repo = PgMsbRahmenvertragGasRepository::new(pool);

    let (id1, v1) = repo.upsert(&rv(MsbRvGasStatus::Angeboten)).await.unwrap();
    assert_eq!(v1, 1);

    let mut concluded = rv(MsbRvGasStatus::Abgeschlossen);
    concluded.signed_at = Some(time::macros::datetime!(2026 - 09 - 15 08:00:00 UTC));
    let (id2, v2) = repo.upsert(&concluded).await.unwrap();
    assert_eq!(id1, id2, "business-key re-submit keeps the id stable");
    assert_eq!(v2, 2);

    // A later update without signed_at must not clear the conclusion date.
    let (id3, v3) = repo
        .upsert(&rv(MsbRvGasStatus::Abgeschlossen))
        .await
        .unwrap();
    assert_eq!(id1, id3);
    assert_eq!(v3, 3);

    let rec = repo.get(TENANT, id1).await.unwrap().unwrap();
    assert_eq!(rec.status, MsbRvGasStatus::Abgeschlossen);
    assert_eq!(rec.version, 3);
    assert!(
        rec.signed_at.is_some(),
        "signed_at survives partial updates"
    );

    // One row, not three — and tenant-scoped.
    assert_eq!(repo.list(TENANT, None, None).await.unwrap().len(), 1);
    assert!(repo.get("9900000000000", id1).await.unwrap().is_none());
}

/// A stale caller-supplied version is rejected with `VersionConflict`;
/// the matching version proceeds.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_gas_stale_version_is_rejected() {
    let Some((pool, _pg)) = test_pool("rv_conflict").await else {
        return;
    };
    let repo = PgMsbRahmenvertragGasRepository::new(pool);

    repo.upsert(&rv(MsbRvGasStatus::Angeboten)).await.unwrap(); // version 1

    let mut fresh = rv(MsbRvGasStatus::Abgeschlossen);
    fresh.version = 1;
    let (_, v2) = repo.upsert(&fresh).await.unwrap();
    assert_eq!(v2, 2);

    let mut stale = rv(MsbRvGasStatus::Beendet);
    stale.version = 1; // lost update: someone else wrote version 2 in between
    let err = repo.upsert(&stale).await.unwrap_err();
    assert!(
        matches!(err, MdmError::VersionConflict { .. }),
        "expected VersionConflict, got {err:?}"
    );

    // The stale write changed nothing.
    let rows = repo.list(TENANT, None, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, MsbRvGasStatus::Abgeschlossen);
    assert_eq!(rows[0].version, 2);
}

/// A different `valid_from` is a different conclusion — a second row, not an
/// update of the first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_gas_valid_from_is_part_of_the_key() {
    let Some((pool, _pg)) = test_pool("rv_key").await else {
        return;
    };
    let repo = PgMsbRahmenvertragGasRepository::new(pool);

    let (id1, _) = repo.upsert(&rv(MsbRvGasStatus::Angeboten)).await.unwrap();
    let mut successor = rv(MsbRvGasStatus::Angeboten);
    successor.valid_from = time::macros::date!(2027 - 10 - 01);
    let (id2, v2) = repo.upsert(&successor).await.unwrap();
    assert_ne!(id1, id2);
    assert_eq!(v2, 1);
    assert_eq!(repo.list(TENANT, None, None).await.unwrap().len(), 2);
}

// ── §20b Netzzugang request registry ──────────────────────────────────────────

fn antrag() -> NetzzugangAntrag {
    NetzzugangAntrag {
        id: Uuid::nil(),
        tenant: TENANT.to_owned(),
        antrag_typ: NetzzugangAntragTyp::Zaehlpunktanordnung,
        aktion: NetzzugangAktion::Bestellung,
        netzanschluss_id: "NA-1".to_owned(),
        nb_mp_id: NB.to_owned(),
        antragsteller_ref: "AN-1".to_owned(),
        status: NetzzugangStatus::Erfasst,
        payload: serde_json::json!({ "zaehlpunkte": 2 }),
        platform_ref: None,
        created_at: time::macros::datetime!(2026 - 07 - 01 12:00:00 UTC),
        submitted_at: None,
    }
}

/// The caller-supplied `created_at` is persisted; the serde epoch default
/// falls back to `now()` instead of writing 1970.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn netzzugang_created_at_is_persisted() {
    let Some((pool, _pg)) = test_pool("nz_created_at").await else {
        return;
    };
    let repo = PgNetzzugangRepository::new(pool);

    let (id, v) = repo.upsert(&antrag()).await.unwrap();
    assert_eq!(v, 1);
    let rec = repo.get(TENANT, id).await.unwrap().unwrap();
    assert_eq!(
        rec.antrag.created_at,
        time::macros::datetime!(2026 - 07 - 01 12:00:00 UTC),
        "caller-supplied created_at must not be dropped"
    );

    let mut defaulted = antrag();
    defaulted.created_at = time::OffsetDateTime::UNIX_EPOCH;
    let (id2, _) = repo.upsert(&defaulted).await.unwrap();
    let rec2 = repo.get(TENANT, id2).await.unwrap().unwrap();
    assert!(
        rec2.antrag.created_at.year() >= 2026,
        "epoch sentinel falls back to now(), got {}",
        rec2.antrag.created_at
    );
}

/// Upserting the same id again is an update and increments the version.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn netzzugang_upsert_by_id_increments_version() {
    let Some((pool, _pg)) = test_pool("nz_upsert").await else {
        return;
    };
    let repo = PgNetzzugangRepository::new(pool);

    let (id, v1) = repo.upsert(&antrag()).await.unwrap();
    assert_eq!(v1, 1);

    let mut again = antrag();
    again.id = id;
    again.status = NetzzugangStatus::Uebermittelt;
    let (id2, v2) = repo.upsert(&again).await.unwrap();
    assert_eq!(id, id2);
    assert_eq!(v2, 2);

    let rows = repo.list(TENANT, None, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].version, 2);
}

/// The status PATCH honours the optional expected version: stale → 412
/// `VersionConflict`, matching → increment, absent → unconditional.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn netzzugang_set_status_guards_the_version() {
    let Some((pool, _pg)) = test_pool("nz_status").await else {
        return;
    };
    let repo = PgNetzzugangRepository::new(pool);

    let (id, _) = repo.upsert(&antrag()).await.unwrap(); // version 1

    let err = repo
        .set_status(TENANT, id, NetzzugangStatus::Uebermittelt, None, Some(99))
        .await
        .unwrap_err();
    assert!(
        matches!(err, MdmError::VersionConflict { .. }),
        "expected VersionConflict, got {err:?}"
    );

    let rec = repo
        .set_status(
            TENANT,
            id,
            NetzzugangStatus::Uebermittelt,
            Some("PLT-42".to_owned()),
            Some(1),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.version, 2);
    assert_eq!(rec.antrag.status, NetzzugangStatus::Uebermittelt);
    assert_eq!(rec.antrag.platform_ref.as_deref(), Some("PLT-42"));
    assert!(
        rec.antrag.submitted_at.is_some(),
        "uebermittelt stamps submitted_at"
    );

    // No expected version → unconditional last-writer-wins, still versioned.
    let rec = repo
        .set_status(TENANT, id, NetzzugangStatus::Bestaetigt, None, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.version, 3);

    // Unknown id stays a 404, not a conflict.
    assert!(
        repo.set_status(
            TENANT,
            Uuid::new_v4(),
            NetzzugangStatus::Bestaetigt,
            None,
            Some(1)
        )
        .await
        .unwrap()
        .is_none()
    );
    // Tenant isolation.
    assert!(
        repo.set_status("9900000000000", id, NetzzugangStatus::Abgelehnt, None, None)
            .await
            .unwrap()
            .is_none()
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
