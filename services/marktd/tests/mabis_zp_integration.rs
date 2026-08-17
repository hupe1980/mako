//! Real-PostgreSQL guards for the Bilanzierungsgebiet → MaBiS-Zählpunkt store.
//!
//! The assignment decides which identifier goes on the wire as MSCONS SG6
//! `LOC+172` (the Meldepunkt) as opposed to `LOC+107` (the Bilanzierungsgebiet).
//! Both are free text at the MIG level, so a Summenzeitreihe filed under the
//! wrong Meldepunkt parses, validates, and is indistinguishable to the BIKO from
//! a correct one. These tests pin the invariants that make that
//! unrepresentable rather than merely discouraged.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use mako_markt::{
    domain::Sparte,
    repository::{MabisZpRecord, MabisZpRepository},
};
use mako_service::cedar::{CedarEnforcer, CedarPrincipal};
use marktd::pg::PgMabisZpRepository;
use sqlx::PgPool;
use time::OffsetDateTime;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const OTHER_TENANT: &str = "9900987654321";
const EIC: &str = "11YAPG4CTRDNZ--P";
const ZP: &str = "DE0004030099000000000000000012345";

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

fn record(eic: &str, zp: &str, tenant: &str) -> MabisZpRecord {
    MabisZpRecord {
        bilanzierungsgebiet: eic.to_owned(),
        mabis_zp_id: zp.to_owned(),
        sparte: Sparte::Strom,
        source: "manual".to_owned(),
        tenant: tenant.to_owned(),
        updated_at: OffsetDateTime::now_utc(),
    }
}

#[test]
fn cedar_policy_registers_the_mabis_zp_scopes() {
    let enforcer = CedarEnforcer::from_policy_str(include_str!("../policies/marktd.cedar"))
        .expect("policies/marktd.cedar parses");
    let principal = CedarPrincipal {
        sub: "user-1".to_owned(),
        tenant: TENANT.to_owned(),
        roles: vec!["NB".to_owned()],
    };

    for scope in ["read-mabis-zp", "write-mabis-zp"] {
        assert!(
            enforcer.check(&principal, scope, TENANT).is_ok(),
            "{scope} must be allowed same-tenant"
        );
        assert!(
            enforcer.check(&principal, scope, OTHER_TENANT).is_err(),
            "{scope} must be denied cross-tenant"
        );
    }
}

#[tokio::test]
async fn an_unassigned_territory_returns_none_rather_than_a_fallback() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    // The whole point of the store: absence is a distinct, actionable answer.
    // `mabis-syncd` turns this into a refused submission — never into "use the
    // Bilanzierungsgebiet EIC".
    assert!(
        repo.find(EIC, TENANT).await.expect("query").is_none(),
        "an unassigned territory must resolve to None"
    );
}

#[tokio::test]
async fn upsert_is_idempotent_and_replaces_the_assignment() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    repo.upsert(record(EIC, ZP, TENANT)).await.expect("insert");
    let first = repo.find(EIC, TENANT).await.unwrap().expect("present");
    assert_eq!(first.mabis_zp_id, ZP);

    // Re-assignment (a corrected Meldepunkt) must replace, not duplicate.
    let corrected = "DE0004030099000000000000000099999";
    repo.upsert(record(EIC, corrected, TENANT))
        .await
        .expect("upsert");
    let second = repo.find(EIC, TENANT).await.unwrap().expect("present");
    assert_eq!(second.mabis_zp_id, corrected);
    assert_eq!(
        repo.list(TENANT).await.unwrap().len(),
        1,
        "upsert must replace the row, not add one"
    );
}

#[tokio::test]
async fn the_meldepunkt_can_never_equal_the_bilanzierungsgebiet() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    // This substitution is the exact defect the table exists to prevent, and it
    // is invisible once on the wire — so it is rejected by the database, not
    // only by the handler that happens to be in front of it today.
    let err = repo.upsert(record(EIC, EIC, TENANT)).await;
    assert!(
        err.is_err(),
        "storing the Bilanzierungsgebiet EIC as the Meldepunkt must be rejected"
    );
}

/// The inequality check alone is not enough: it only rules out *this* territory's
/// own EIC. Territory A's EIC assigned as territory B's Meldepunkt is still a
/// Bilanzierungsgebiet code masquerading as a Zählpunkt, and it would read as
/// valid master data until a submission run refused it — long after the
/// assignment was made and by someone else.
///
/// A Zählpunktbezeichnung is 33 characters and an EIC is 16, so the length is
/// what separates them.
#[tokio::test]
async fn another_territorys_eic_is_rejected_as_a_meldepunkt() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    // A different, well-formed 16-character EIC — passes `mabis_zp_not_the_gebiet`.
    let other_territory_eic = "11XOTHERGRIDBGXY";
    assert_ne!(other_territory_eic, EIC);
    let err = repo.upsert(record(EIC, other_territory_eic, TENANT)).await;
    assert!(
        err.is_err(),
        "a 16-character EIC is not a Zählpunktbezeichnung and must be rejected \
         even when it differs from this territory's own code"
    );

    // The 33-character form is what the column is for.
    repo.upsert(record(EIC, ZP, TENANT))
        .await
        .expect("a well-formed Zählpunktbezeichnung is accepted");
}

#[tokio::test]
async fn an_empty_meldepunkt_is_rejected() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    // An empty LOC+172 would be filed as a blank Meldepunkt.
    assert!(repo.upsert(record(EIC, "   ", TENANT)).await.is_err());
}

#[tokio::test]
async fn assignments_are_tenant_scoped() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    repo.upsert(record(EIC, ZP, TENANT)).await.expect("insert");

    // The same territory EIC can legitimately appear in two deployments; one
    // tenant's assignment must never resolve for the other.
    assert!(repo.find(EIC, OTHER_TENANT).await.unwrap().is_none());
    assert!(repo.list(OTHER_TENANT).await.unwrap().is_empty());

    let other_zp = "DE0004030099000000000000000055555";
    repo.upsert(record(EIC, other_zp, OTHER_TENANT))
        .await
        .expect("insert other tenant");
    assert_eq!(
        repo.find(EIC, TENANT).await.unwrap().unwrap().mabis_zp_id,
        ZP,
        "the other tenant's write must not overwrite this one"
    );
    assert_eq!(
        repo.find(EIC, OTHER_TENANT)
            .await
            .unwrap()
            .unwrap()
            .mabis_zp_id,
        other_zp
    );
}

#[tokio::test]
async fn list_is_ordered_and_scoped() {
    let Some((pool, _c)) = test_pool().await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let repo = PgMabisZpRepository::new(pool);

    for (eic, zp) in [
        ("11YZZZZZZZZZZ--1", "DE0004030099000000000000000000003"),
        ("11YAAAAAAAAAA--H", "DE0004030099000000000000000000001"),
        ("11YMMMMMMMMMM--X", "DE0004030099000000000000000000002"),
    ] {
        repo.upsert(record(eic, zp, TENANT)).await.expect("insert");
    }

    let all = repo.list(TENANT).await.expect("list");
    let eics: Vec<&str> = all.iter().map(|r| r.bilanzierungsgebiet.as_str()).collect();
    assert_eq!(
        eics,
        vec!["11YAAAAAAAAAA--H", "11YMMMMMMMMMM--X", "11YZZZZZZZZZZ--1"],
        "list must be ascending by Bilanzierungsgebiet"
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
