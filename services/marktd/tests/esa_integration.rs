//! Real-PostgreSQL guards for the ESA consent registry (§49 Abs. 2 Nr. 9 MsbG).
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```

use mako_markt::repository::{
    ConsentCode, ConsentPerspective, EinwilligungRecord, EinwilligungRepository as _,
    EsaFrameworkAgreement,
};
use marktd::pg::PgEinwilligungRepository;
use sqlx::PgPool;
use uuid::Uuid;

const MSB: &str = "9900357000004";

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const ESA: &str = "9905550000005";

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

fn consent(an_ref: &str, locations: &[&str]) -> EinwilligungRecord {
    EinwilligungRecord {
        id: Uuid::nil(),
        tenant: TENANT.to_owned(),
        anschlussnutzer_ref: an_ref.to_owned(),
        esa_mp_id: ESA.to_owned(),
        location_ids: locations.iter().map(|s| (*s).to_owned()).collect(),
        scope: "werte".to_owned(),
        granted_at: time::OffsetDateTime::now_utc(),
        valid_from: time::macros::date!(2026 - 01 - 01),
        valid_to: None,
        revoked_at: None,
        evidence_uri: Some("s3://consents/an-1.pdf".to_owned()),
        evidence_hash: Some("deadbeef".to_owned()),
    }
}

/// Grant → list → revoke returns the record → revoke again is a no-op.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn consent_lifecycle() {
    let Some((pool, _pg)) = test_pool("lifecycle").await else {
        return;
    };
    let repo = PgEinwilligungRepository::new(pool);

    let id = repo.grant(consent("AN-1", &["51238696780"])).await.unwrap();
    let active = repo.list_for_esa(TENANT, ESA).await.unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].location_ids, vec!["51238696780".to_owned()]);
    // Evidence is stored verbatim, never validated.
    assert_eq!(active[0].evidence_hash.as_deref(), Some("deadbeef"));

    // Revoke returns the record (so the caller can fire the 17008 Abbestellung).
    let revoked = repo.revoke(TENANT, id).await.unwrap();
    assert!(revoked.is_some());
    assert_eq!(revoked.unwrap().esa_mp_id, ESA);
    // Revoking again is a no-op — the Abbestellung fires exactly once.
    assert!(repo.revoke(TENANT, id).await.unwrap().is_none());
    // No active consents remain.
    assert!(repo.list_for_esa(TENANT, ESA).await.unwrap().is_empty());
}

/// A new grant supersedes the active consent for the same Anschlussnutzer.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn new_grant_supersedes_active_consent() {
    let Some((pool, _pg)) = test_pool("supersede").await else {
        return;
    };
    let repo = PgEinwilligungRepository::new(pool);

    repo.grant(consent("AN-2", &["51238696780"])).await.unwrap();
    // Second grant for the same (tenant, esa, Anschlussnutzer) must succeed by
    // superseding the first — the partial UNIQUE index stays satisfied.
    let id2 = repo
        .grant(consent("AN-2", &["51238696780", "51238696781"]))
        .await
        .unwrap();
    let active = repo.list_for_esa(TENANT, ESA).await.unwrap();
    assert_eq!(active.len(), 1, "only the latest consent is active");
    assert_eq!(active[0].id, id2);
    assert_eq!(active[0].location_ids.len(), 2);
}

/// The inbound-message gate: active consent allows, revocation blocks, a fresh
/// grant re-allows, an absent record is self-assertion, and an unestablished
/// framework agreement blocks regardless of consent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn consent_check_gates_inbound_messages() {
    let Some((pool, _pg)) = test_pool("check").await else {
        return;
    };
    let repo = PgEinwilligungRepository::new(pool);
    let loc = "51238696780";
    let msb_in = ConsentPerspective::MsbInbound;
    let esa_out = ConsentPerspective::EsaOutbound;

    // No record: self-assertion for the MSB (allow), no lawful basis for the ESA
    // (block). This is the asymmetry the outbound direction depends on.
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, msb_in)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::SelfAssertion);
    assert!(d.allowed);
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, esa_out)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::NoConsent);
    assert!(!d.allowed);

    // Active consent → allow from both sides.
    let id = repo.grant(consent("AN-9", &[loc])).await.unwrap();
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, msb_in)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::Active);
    assert!(d.allowed);
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, esa_out)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::Active);
    assert!(d.allowed);

    // Revoked, none superseding → the Widerruf clearing case blocks either way.
    repo.revoke(TENANT, id).await.unwrap();
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, msb_in)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::Revoked);
    assert!(!d.allowed);
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, esa_out)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::Revoked);

    // A fresh grant re-allows.
    repo.grant(consent("AN-9", &[loc])).await.unwrap();
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, msb_in)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::Active);

    // A framework agreement on record but not established blocks everything.
    repo.upsert_framework(EsaFrameworkAgreement {
        tenant: TENANT.to_owned(),
        msb_mp_id: MSB.to_owned(),
        esa_mp_id: ESA.to_owned(),
        signed_at: None,
        edi_agreement: false,
        cert_state: "pending".to_owned(),
    })
    .await
    .unwrap();
    let d = repo
        .consent_check(TENANT, ESA, MSB, loc, msb_in)
        .await
        .unwrap();
    assert_eq!(d.code, ConsentCode::FrameworkRejected);
    assert!(!d.allowed);
}

/// Tenant isolation: another tenant cannot read or revoke a consent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn consent_is_tenant_scoped() {
    let Some((pool, _pg)) = test_pool("tenant").await else {
        return;
    };
    let repo = PgEinwilligungRepository::new(pool);
    let id = repo.grant(consent("AN-3", &["51238696780"])).await.unwrap();

    assert!(repo.get("9900000000000", id).await.unwrap().is_none());
    assert!(repo.revoke("9900000000000", id).await.unwrap().is_none());
    assert!(repo.get(TENANT, id).await.unwrap().is_some());
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
