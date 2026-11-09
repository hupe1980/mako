//! Real-PostgreSQL guards for the §20b EnWG Netzzugang request registry, the
//! MSB-Rahmenvertrag registry (both Sparten), the trading-partner registry
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

use mako_markt::domain::{MarktpartnerId, Sparte};
use mako_markt::{
    error::MdmError,
    repository::{NetzzugangAktion, NetzzugangAntrag, NetzzugangAntragTyp, NetzzugangStatus},
};
use mako_service::cedar::{CedarEnforcer, CedarPrincipal};
use marktd::pg::{
    PgMsbRahmenvertragRepository, PgNbEnergiemixRepository, PgNetzzugangRepository,
    msb_rahmenvertrag::{FASSUNG_GAS, MsbRvBefund, MsbRvFilter, MsbRvStatus, MsbRvUpsertRequest},
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
        "read-msb-rv",
        "write-msb-rv",
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

// ── MSB-Rahmenvertrag registry (both Sparten) ─────────────────────────────────

fn rv(sparte: Sparte, status: MsbRvStatus) -> MsbRvUpsertRequest {
    let nb = match sparte {
        Sparte::Strom => NB,
        Sparte::Gas => GNB,
    };
    let mut rec: MsbRvUpsertRequest = serde_json::from_value(serde_json::json!({
        "sparte": sparte,
        "nb_mp_id": nb,
        "msb_mp_id": MSB,
        "valid_from": "2026-10-01",
    }))
    .expect("body decodes");
    rec.status = status;
    rec
}

fn mp(id: &str) -> MarktpartnerId {
    MarktpartnerId::new(id).expect("13 digits")
}

/// The `PUT` body is gated: a `vertrag` naming another BO is refused as the
/// request deserialises, before any handler code runs. That is the whole point
/// of `Bo4e<Vertrag>`: a bare `serde_json::Value` here would store whatever
/// arrived.
#[test]
fn the_upsert_body_gates_its_bo4e_vertrag() {
    let body = |vertrag: serde_json::Value| {
        serde_json::json!({
            "sparte": "GAS", "nb_mp_id": GNB, "msb_mp_id": MSB, "valid_from": "2026-10-01",
            "vertrag": vertrag,
        })
    };
    let ok: MsbRvUpsertRequest =
        serde_json::from_value(body(serde_json::json!({ "vertragsnummer": "RV-1" })))
            .expect("a Vertrag with no _typ is accepted; the gate injects it");
    assert_eq!(
        ok.vertrag
            .expect("present")
            .canonical_json()
            .expect("serialisable")["_typ"],
        "VERTRAG"
    );
    let err = serde_json::from_value::<MsbRvUpsertRequest>(body(
        serde_json::json!({ "_typ": "MARKTLOKATION" }),
    ))
    .expect_err("a Marktlokation is not a Vertrag");
    assert!(err.to_string().contains("expected a BO4E VERTRAG"), "{err}");

    // `deny_unknown_fields`: a body naming a field the API does not have is a
    // refusal, not a silently discarded value — `gnb_mp_id` included, which
    // is the Gas-only name the registry no longer has.
    for unknown in ["vertragsnummer", "gnb_mp_id"] {
        let mut b = body(serde_json::json!({}));
        b[unknown] = serde_json::json!(GNB);
        let err = serde_json::from_value::<MsbRvUpsertRequest>(b)
            .expect_err("not a field of this request");
        assert!(
            err.to_string().contains("unknown field"),
            "{unknown}: {err}"
        );
    }
}

/// Re-submitting the same business key `(sparte, nb, msb, valid_from)` without
/// an id is an idempotent update: the id stays stable, the version increments,
/// and a `signed_at` on record survives an update that omits it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_upsert_is_idempotent_on_natural_key() {
    let Some((pool, _pg)) = test_pool("rv_idempotent").await else {
        return;
    };
    let repo = PgMsbRahmenvertragRepository::new(pool);

    let (id1, v1) = repo
        .upsert(TENANT, &rv(Sparte::Gas, MsbRvStatus::Angeboten))
        .await
        .unwrap();
    assert_eq!(v1, 1);

    let mut concluded = rv(Sparte::Gas, MsbRvStatus::Abgeschlossen);
    concluded.signed_at = Some(time::macros::datetime!(2026 - 09 - 15 08:00:00 UTC));
    let (id2, v2) = repo.upsert(TENANT, &concluded).await.unwrap();
    assert_eq!(id1, id2, "business-key re-submit keeps the id stable");
    assert_eq!(v2, 2);

    // A later update without signed_at must not clear the conclusion date.
    let (id3, v3) = repo
        .upsert(TENANT, &rv(Sparte::Gas, MsbRvStatus::Abgeschlossen))
        .await
        .unwrap();
    assert_eq!(id1, id3);
    assert_eq!(v3, 3);

    let rec = repo.get(TENANT, id1).await.unwrap().unwrap();
    assert_eq!(rec.status, MsbRvStatus::Abgeschlossen);
    assert_eq!(rec.version, 3);
    assert_eq!(rec.fassung.as_deref(), Some(FASSUNG_GAS), "the Gas default");
    assert!(
        rec.signed_at.is_some(),
        "signed_at survives partial updates"
    );

    // One row, not three — and tenant-scoped.
    let all = MsbRvFilter::default();
    assert_eq!(repo.list(TENANT, &all).await.unwrap().len(), 1);
    assert!(repo.get("9900000000000", id1).await.unwrap().is_none());
}

/// A stale caller-supplied version is rejected with `VersionConflict`;
/// the matching version proceeds.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_stale_version_is_rejected() {
    let Some((pool, _pg)) = test_pool("rv_conflict").await else {
        return;
    };
    let repo = PgMsbRahmenvertragRepository::new(pool);

    repo.upsert(TENANT, &rv(Sparte::Gas, MsbRvStatus::Angeboten))
        .await
        .unwrap(); // version 1

    let mut fresh = rv(Sparte::Gas, MsbRvStatus::Abgeschlossen);
    fresh.version = 1;
    let (_, v2) = repo.upsert(TENANT, &fresh).await.unwrap();
    assert_eq!(v2, 2);

    let mut stale = rv(Sparte::Gas, MsbRvStatus::Beendet);
    stale.version = 1; // lost update: someone else wrote version 2 in between
    let err = repo.upsert(TENANT, &stale).await.unwrap_err();
    assert!(
        matches!(err, MdmError::VersionConflict { .. }),
        "expected VersionConflict, got {err:?}"
    );

    // The stale write changed nothing.
    let rows = repo.list(TENANT, &MsbRvFilter::default()).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, MsbRvStatus::Abgeschlossen);
    assert_eq!(rows[0].version, 2);
}

/// A different `valid_from` is a different conclusion — a second row, not an
/// update of the first — once the first is closed before it starts. Two
/// windows claiming the same day are refused, because the lookup must find at
/// most one contract for a day.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn msb_rv_valid_from_is_part_of_the_key_and_windows_do_not_overlap() {
    let Some((pool, _pg)) = test_pool("rv_key").await else {
        return;
    };
    let repo = PgMsbRahmenvertragRepository::new(pool);

    let open = rv(Sparte::Gas, MsbRvStatus::Abgeschlossen);
    let (id1, _) = repo.upsert(TENANT, &open).await.unwrap();
    let mut successor = rv(Sparte::Gas, MsbRvStatus::Angeboten);
    successor.valid_from = time::macros::date!(2027 - 10 - 01);
    let err = repo.upsert(TENANT, &successor).await.unwrap_err();
    assert!(
        matches!(err, MdmError::Unprocessable { .. }),
        "an overlapping window is the caller's to fix, got {err:?}"
    );

    let mut closed = open.clone();
    closed.valid_to = Some(time::macros::date!(2027 - 10 - 01));
    repo.upsert(TENANT, &closed).await.unwrap();
    let (id2, v2) = repo.upsert(TENANT, &successor).await.unwrap();
    assert_ne!(id1, id2);
    assert_eq!(v2, 1);
    assert_eq!(
        repo.list(TENANT, &MsbRvFilter::default())
            .await
            .unwrap()
            .len(),
        2
    );
}

/// The registry holds Strom contracts, and the Sparte is part of the answer:
/// a Gas contract between the same two MP-IDs does not satisfy a Strom
/// lookup, and a Strom row stores no Gas Fassung.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_strom_row_is_stored_and_a_gas_row_does_not_answer_a_strom_lookup() {
    let Some((pool, _pg)) = test_pool("rv_sparte").await else {
        return;
    };
    let repo = PgMsbRahmenvertragRepository::new(pool);
    let day = time::macros::date!(2026 - 11 - 09);

    // A Gas contract with the very MP-ID the Strom lookup names.
    let mut gas = rv(Sparte::Gas, MsbRvStatus::Abgeschlossen);
    gas.nb_mp_id = mp(NB);
    repo.upsert(TENANT, &gas).await.unwrap();
    let p = repo
        .pruefe(TENANT, Sparte::Strom, &mp(NB), &mp(MSB), day)
        .await
        .unwrap();
    assert!(!p.abgeschlossen, "a Gas contract answered a Strom lookup");
    assert_eq!(p.befund, MsbRvBefund::KeinVertrag);
    assert!(
        repo.pruefe(TENANT, Sparte::Gas, &mp(NB), &mp(MSB), day)
            .await
            .unwrap()
            .abgeschlossen
    );

    let (id, _) = repo
        .upsert(TENANT, &rv(Sparte::Strom, MsbRvStatus::Abgeschlossen))
        .await
        .unwrap();
    let stored = repo.get(TENANT, id).await.unwrap().unwrap();
    assert_eq!(stored.sparte, Sparte::Strom);
    assert_eq!(stored.fassung, None, "no Gas Fassung on a Strom row");
    let p = repo
        .pruefe(TENANT, Sparte::Strom, &mp(NB), &mp(MSB), day)
        .await
        .unwrap();
    assert!(p.abgeschlossen);
    assert_eq!(p.vertrag.map(|v| v.id), Some(id));

    let strom_only = MsbRvFilter {
        sparte: Some(Sparte::Strom),
        ..MsbRvFilter::default()
    };
    assert_eq!(repo.list(TENANT, &strom_only).await.unwrap().len(), 1);
}

/// The Gas-only semantics are held by the schema too, not only by the
/// request's own validation: a Strom row with `anpassung_erforderlich` and a
/// Gas row without a Fassung are refused by PostgreSQL.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_schema_limits_the_gas_only_columns_to_gas() {
    let Some((pool, _pg)) = test_pool("rv_gas_only").await else {
        return;
    };
    let insert = |sparte: &'static str, fassung: Option<&'static str>, status: &'static str| {
        sqlx::query(
            "INSERT INTO msb_rahmenvertraege \
               (id, tenant, sparte, nb_mp_id, msb_mp_id, fassung, status, valid_from) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, DATE '2026-10-01')",
        )
        .bind(Uuid::new_v4())
        .bind(TENANT)
        .bind(sparte)
        .bind(NB)
        .bind(MSB)
        .bind(fassung)
        .bind(status)
        .execute(&pool)
    };
    assert!(
        insert("STROM", None, "anpassung_erforderlich")
            .await
            .is_err()
    );
    assert!(insert("GAS", None, "abgeschlossen").await.is_err());
    assert!(insert("STROM", None, "abgeschlossen").await.is_ok());
}

/// The lookup is `false` for a known partner with no contract, with an ended
/// contract (closed before the day, or marked `beendet`) and with one still
/// `angeboten`; only an `abgeschlossen` contract valid on the day is `true`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_lookup_is_true_only_for_a_concluded_contract_valid_that_day() {
    let Some((pool, _pg)) = test_pool("rv_lookup").await else {
        return;
    };
    let repo = PgMsbRahmenvertragRepository::new(pool);
    let check = async |msb: &str, day: time::Date| {
        repo.pruefe(TENANT, Sparte::Strom, &mp(NB), &mp(msb), day)
            .await
            .unwrap()
    };
    let day = time::macros::date!(2026 - 11 - 09);

    // No contract at all.
    let p = check(MSB, day).await;
    assert!(!p.abgeschlossen);
    assert_eq!(p.befund, MsbRvBefund::KeinVertrag);

    // Only offered.
    repo.upsert(TENANT, &rv(Sparte::Strom, MsbRvStatus::Angeboten))
        .await
        .unwrap();
    let p = check(MSB, day).await;
    assert!(!p.abgeschlossen);
    assert_eq!(p.befund, MsbRvBefund::Angeboten);

    // Concluded, but its window closed on the day (valid_to is exclusive).
    let mut ended = rv(Sparte::Strom, MsbRvStatus::Abgeschlossen);
    ended.valid_to = Some(day);
    repo.upsert(TENANT, &ended).await.unwrap();
    let p = check(MSB, day).await;
    assert!(
        !p.abgeschlossen,
        "valid_to is the first day no longer valid"
    );
    assert_eq!(p.befund, MsbRvBefund::KeinVertrag);
    assert!(
        check(MSB, time::macros::date!(2026 - 11 - 08))
            .await
            .abgeschlossen
    );

    // Marked ended while its window is still open.
    let other_msb = "9900357000011";
    let mut beendet = rv(Sparte::Strom, MsbRvStatus::Beendet);
    beendet.msb_mp_id = mp(other_msb);
    repo.upsert(TENANT, &beendet).await.unwrap();
    let p = check(other_msb, day).await;
    assert!(!p.abgeschlossen);
    assert_eq!(p.befund, MsbRvBefund::Beendet);

    // Concluded and open — the one `true`; and not from another tenant.
    let mut current = rv(Sparte::Strom, MsbRvStatus::Abgeschlossen);
    current.valid_from = day;
    repo.upsert(TENANT, &current).await.unwrap();
    assert!(check(MSB, day).await.abgeschlossen);
    assert!(
        !repo
            .pruefe("9900000000000", Sparte::Strom, &mp(NB), &mp(MSB), day)
            .await
            .unwrap()
            .abgeschlossen
    );
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

/// The `PUT` stores the contract and its `de.markt.msb-rahmenvertrag.updated`
/// in one transaction: when the event cannot be enqueued, the request fails
/// and **no contract is stored** — a row without its event would leave every
/// ERP subscriber on the previous contract state with nothing to replay.
/// The `check` handler answers the stored row through the Query extractor.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_put_commits_the_contract_and_its_event_together() {
    use std::sync::Arc;

    use axum::{
        Extension,
        extract::Query,
        http::StatusCode,
        response::{IntoResponse, Response},
    };
    use mako_service::{Json, oidc::JwtClaims};
    use marktd::handlers::{Claims, Tenant, msb_rahmenvertrag as h};

    let Some((pool, _pg)) = test_pool("rv_put_tx").await else {
        return;
    };
    let repo = Arc::new(PgMsbRahmenvertragRepository::new(pool.clone()));
    let enforcer = Arc::new(
        CedarEnforcer::from_policy_str(include_str!("../../policies/marktd.cedar"))
            .expect("policies/marktd.cedar parses"),
    );
    let claims = || {
        Claims(JwtClaims {
            sub: "operator".to_owned(),
            mako_tenant: Some(TENANT.to_owned()),
            mako_roles: vec!["NB".to_owned()],
            mako_sparte: vec!["STROM".to_owned()],
        })
    };
    let put = async |body: serde_json::Value| -> StatusCode {
        let r: Response = h::upsert_msb_rv(
            claims(),
            Extension(repo.clone()),
            Extension(enforcer.clone()),
            Extension(Tenant(TENANT.to_owned())),
            Json(serde_json::from_value(body).expect("body decodes")),
        )
        .await
        .into_response();
        r.status()
    };
    let body = serde_json::json!({
        "sparte": "STROM", "nb_mp_id": NB, "msb_mp_id": MSB,
        "valid_from": "2026-10-01", "status": "abgeschlossen",
    });

    assert_eq!(put(body.clone()).await, StatusCode::OK);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_log WHERE ce_type = 'de.markt.msb-rahmenvertrag.updated' \
           AND sparte = 'STROM'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        events, 1,
        "the event carries its Sparte for the fan-out filter"
    );

    let checked: Response = h::check_msb_rv(
        claims(),
        Extension(repo.clone()),
        Extension(enforcer.clone()),
        Extension(Tenant(TENANT.to_owned())),
        Query(
            serde_json::from_value(serde_json::json!({
                "sparte": "STROM", "nb_mp_id": NB, "msb_mp_id": MSB, "datum": "2026-11-09",
            }))
            .expect("query decodes"),
        ),
    )
    .await
    .into_response();
    assert_eq!(checked.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(checked.into_body(), 1 << 20)
        .await
        .unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer["abgeschlossen"], true);
    assert_eq!(answer["befund"], "abgeschlossen");

    // Make the enqueue fail: the contract write must roll back with it.
    sqlx::raw_sql("DROP TABLE event_log CASCADE")
        .execute(&pool)
        .await
        .unwrap();
    let mut successor = body;
    successor["msb_mp_id"] = serde_json::json!("9900357000011");
    assert_eq!(put(successor).await, StatusCode::INTERNAL_SERVER_ERROR);
    let stored = repo
        .list(
            TENANT,
            &MsbRvFilter {
                msb_mp_id: Some("9900357000011"),
                ..MsbRvFilter::default()
            },
        )
        .await
        .unwrap();
    assert!(stored.is_empty(), "a contract was stored without its event");
}
