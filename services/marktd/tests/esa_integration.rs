//! Real-PostgreSQL guards for the ESA consent registry (§49 Abs. 2 Nr. 9 MsbG).
//!
//! ```bash
//! docker run -d --name marktd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=marktd -p 55438:5432 postgres:17-alpine
//! export MARKTD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55438/marktd"
//! cargo test -p marktd --test esa_integration -- --include-ignored
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

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("MARKTD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;
    let schema = format!("esa_{test_name}");
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn consent_lifecycle() {
    let Some(pool) = test_pool("lifecycle").await else {
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn new_grant_supersedes_active_consent() {
    let Some(pool) = test_pool("supersede").await else {
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn consent_check_gates_inbound_messages() {
    let Some(pool) = test_pool("check").await else {
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
#[ignore = "requires MARKTD_TEST_DATABASE_URL"]
async fn consent_is_tenant_scoped() {
    let Some(pool) = test_pool("tenant").await else {
        return;
    };
    let repo = PgEinwilligungRepository::new(pool);
    let id = repo.grant(consent("AN-3", &["51238696780"])).await.unwrap();

    assert!(repo.get("9900000000000", id).await.unwrap().is_none());
    assert!(repo.revoke("9900000000000", id).await.unwrap().is_none());
    assert!(repo.get(TENANT, id).await.unwrap().is_some());
}
