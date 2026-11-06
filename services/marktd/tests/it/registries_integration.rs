//! Real-PostgreSQL guards for the §20b EnWG Netzzugang request registry, the
//! Gas MSB-Rahmenvertrag registry (GeLi Gas 3.0), the trading-partner registry
//! and the § 42 EnWG grid-area Energiemix.
//!
//! ```bash
//! PostgreSQL is self-managed via testcontainers (only a Docker daemon is
//! required); tests skip gracefully when Docker is unavailable:
//!
//! just test-marktd-db
//! ```
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use mako_markt::{
    error::MdmError,
    repository::{NetzzugangAktion, NetzzugangAntrag, NetzzugangAntragTyp, NetzzugangStatus},
};
use mako_service::cedar::{CedarEnforcer, CedarPrincipal};
use marktd::pg::{
    PgMsbRahmenvertragGasRepository, PgNbEnergiemixRepository, PgNetzzugangRepository,
    msb_rahmenvertrag_gas::{MsbRvGasStatus, MsbRvGasUpsertRequest},
    nb_energiemix::NbEnergiemixWrite,
};
use sqlx::PgPool;
use uuid::Uuid;

const SCHEMA: &str = include_str!("../../migrations/0001_initial.sql");
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
    let enforcer = CedarEnforcer::from_policy_str(include_str!("../../policies/marktd.cedar"))
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

fn rv(status: MsbRvGasStatus) -> MsbRvGasUpsertRequest {
    MsbRvGasUpsertRequest {
        id: Uuid::nil(),
        tenant: TENANT.to_owned(),
        gnb_mp_id: GNB.to_owned(),
        msb_mp_id: MSB.to_owned(),
        fassung: "KoV XV Anlage 8".to_owned(),
        status,
        signed_at: None,
        valid_from: time::macros::date!(2026 - 10 - 01),
        valid_to: None,
        vertrag: None,
        version: 0,
    }
}

/// The `PUT` body is gated: a `vertrag` naming another BO is refused as the
/// request deserialises, before any handler code runs. That is the whole point
/// of `Bo4e<Vertrag>`: a bare `serde_json::Value` here would store whatever
/// arrived.
#[test]
fn the_upsert_body_gates_its_bo4e_vertrag() {
    let body = |vertrag: serde_json::Value| {
        serde_json::json!({
            "gnb_mp_id": GNB, "msb_mp_id": MSB, "valid_from": "2026-10-01",
            "vertrag": vertrag,
        })
    };
    let ok: MsbRvGasUpsertRequest =
        serde_json::from_value(body(serde_json::json!({ "vertragsnummer": "RV-1" })))
            .expect("a Vertrag with no _typ is accepted; the gate injects it");
    assert_eq!(
        ok.vertrag
            .expect("present")
            .canonical_json()
            .expect("serialisable")["_typ"],
        "VERTRAG"
    );
    let err = serde_json::from_value::<MsbRvGasUpsertRequest>(body(
        serde_json::json!({ "_typ": "MARKTLOKATION" }),
    ))
    .expect_err("a Marktlokation is not a Vertrag");
    assert!(err.to_string().contains("expected a BO4E VERTRAG"), "{err}");

    // `deny_unknown_fields`: a body naming a field the API does not have is a
    // refusal, not a silently discarded value.
    let err = serde_json::from_value::<MsbRvGasUpsertRequest>(serde_json::json!({
        "gnb_mp_id": GNB, "msb_mp_id": MSB, "valid_from": "2026-10-01",
        "vertragsnummer": "RV-1",
    }))
    .expect_err("`vertragsnummer` is not a field of this request");
    assert!(err.to_string().contains("unknown field"), "{err}");
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

// ── Trading partners ─────────────────────────────────────────────────────────

/// A partner reads back as written — every AS4 endpoint of `makoadresse` (a
/// TEXT[]) and the EDI agreement makod's INVOIC check reads. `find` is the one
/// read path REST and the MCP `get_partner` tool share.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_partner_reads_back_with_every_endpoint_and_its_edi_agreement() {
    let Some((pool, _pg)) = test_pool("partner_roundtrip").await else {
        return;
    };
    let repo = marktd::pg::PgPartnerRepository::new(pool.clone());
    let mp_id = mako_markt::domain::MarktpartnerId::try_from(NB).expect("an MP-ID");
    let partner: mako_markt::repository::PartnerRecord =
        serde_json::from_value(serde_json::json!({
            "mp_id": NB,
            "display_name": "Netz Nord",
            "makoadresse": ["https://as4-a.example.de/msh", "https://as4-b.example.de/msh"],
            "edi_zustimmung": true,
            "edi_extrahierbar": false,
        }))
        .expect("a partner record");
    repo.upsert(partner).await.expect("upsert");

    let stored = repo.find(&mp_id).await.expect("find").expect("stored");
    assert_eq!(
        stored.makoadresse,
        [
            "https://as4-a.example.de/msh",
            "https://as4-b.example.de/msh"
        ]
    );
    assert_eq!(stored.edi_zustimmung, Some(true));
    assert_eq!(stored.edi_extrahierbar, Some(false));
}

// ── § 42 EnWG grid-area Energiemix ───────────────────────────────────────────

/// A BO4E `Energiemix` COM whose one share is `prozent` of `erzeugungsart`.
fn energiemix(jahr: i16, erzeugungsart: &str) -> serde_json::Value {
    serde_json::json!({
        "gueltigkeitsjahr": jahr,
        "anteil": [{ "erzeugungsart": erzeugungsart, "anteilProzent": "100" }],
    })
}

fn mix_write<'a>(
    tenant: &'a str,
    year: i16,
    mix: &'a serde_json::Value,
    eeg_kwh: Option<i64>,
) -> NbEnergiemixWrite<'a> {
    NbEnergiemixWrite {
        tenant,
        nb_mp_id: NB,
        gueltig_fuer: year,
        energiemix: mix,
        eeg_einspeisung_kwh: eeg_kwh,
        gesamtentnahme_kwh: Some(9_876_543_210),
    }
}

/// A year's mix reads back as written — the JSON and both kWh figures, which
/// are `NUMERIC` columns and decode only through an exact cast. Without a read
/// year the most recent one answers, with one it answers that year, and
/// `history` lists every year newest first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_energiemix_reads_back_latest_and_by_year() {
    let Some((pool, _pg)) = test_pool("energiemix_roundtrip").await else {
        return;
    };
    let repo = PgNbEnergiemixRepository::new(pool);
    let (mix_2024, mix_2025) = (energiemix(2024, "KOHLE"), energiemix(2025, "WIND"));
    repo.upsert(&mix_write(TENANT, 2025, &mix_2025, Some(123_456_789)))
        .await
        .expect("store 2025");
    repo.upsert(&mix_write(TENANT, 2024, &mix_2024, None))
        .await
        .expect("store 2024");

    let latest = repo
        .find(TENANT, NB, None)
        .await
        .expect("read")
        .expect("a mix is stored");
    assert_eq!(latest.gueltig_fuer, 2025, "the most recent year answers");
    assert_eq!(latest.energiemix, mix_2025);
    assert_eq!(latest.eeg_einspeisung_kwh, Some(123_456_789));
    assert_eq!(latest.gesamtentnahme_kwh, Some(9_876_543_210));

    let older = repo
        .find(TENANT, NB, Some(2024))
        .await
        .expect("read")
        .expect("2024 is stored");
    assert_eq!(older.energiemix, mix_2024);
    assert_eq!(older.eeg_einspeisung_kwh, None);
    assert!(
        repo.find(TENANT, NB, Some(2023))
            .await
            .expect("read")
            .is_none(),
        "a year never published is absent, not the nearest one"
    );

    let years: Vec<i16> = repo
        .history(TENANT, NB)
        .await
        .expect("history")
        .iter()
        .map(|r| r.gueltig_fuer)
        .collect();
    assert_eq!(years, [2025, 2024]);
}

/// Republishing a year replaces its mix and keeps a kWh figure the new write
/// leaves out: the annual totals are known later than the mix.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn republishing_a_year_replaces_the_mix_and_keeps_an_omitted_total() {
    let Some((pool, _pg)) = test_pool("energiemix_republish").await else {
        return;
    };
    let repo = PgNbEnergiemixRepository::new(pool);
    let (first, second) = (energiemix(2025, "KOHLE"), energiemix(2025, "SOLAR"));
    repo.upsert(&mix_write(TENANT, 2025, &first, Some(42)))
        .await
        .expect("first");
    repo.upsert(&mix_write(TENANT, 2025, &second, None))
        .await
        .expect("republish");

    let stored = repo
        .find(TENANT, NB, Some(2025))
        .await
        .expect("read")
        .expect("stored");
    assert_eq!(stored.energiemix, second);
    assert_eq!(stored.eeg_einspeisung_kwh, Some(42));
    assert_eq!(repo.history(TENANT, NB).await.expect("history").len(), 1);
}

/// Every read is scoped to its tenant: another tenant's mix for the same
/// Netzbetreiber and year is neither the latest, nor that year, nor history.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_energiemix_is_scoped_to_its_tenant() {
    let Some((pool, _pg)) = test_pool("energiemix_tenant").await else {
        return;
    };
    let repo = PgNbEnergiemixRepository::new(pool);
    let other = "9900000000003";
    let (ours, theirs) = (energiemix(2024, "WIND"), energiemix(2026, "KOHLE"));
    repo.upsert(&mix_write(TENANT, 2024, &ours, None))
        .await
        .expect("ours");
    repo.upsert(&mix_write(other, 2026, &theirs, None))
        .await
        .expect("theirs");
    repo.upsert(&mix_write(other, 2024, &theirs, None))
        .await
        .expect("theirs, same year");

    let latest = repo
        .find(TENANT, NB, None)
        .await
        .expect("read")
        .expect("ours is stored");
    assert_eq!(
        latest.gueltig_fuer, 2024,
        "the other tenant's 2026 is not ours"
    );
    assert_eq!(latest.tenant, TENANT);
    assert_eq!(
        repo.find(TENANT, NB, Some(2024))
            .await
            .expect("read")
            .expect("ours")
            .energiemix,
        ours
    );
    assert!(
        repo.find(TENANT, NB, Some(2026))
            .await
            .expect("read")
            .is_none()
    );
    let history = repo.history(TENANT, NB).await.expect("history");
    assert_eq!(history.len(), 1);
    assert!(history.iter().all(|r| r.tenant == TENANT));
}
