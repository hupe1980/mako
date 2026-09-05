//! An overlapping validity window is the caller's mistake, not an outage.
//!
//! `0001_initial.sql` declares eight `EXCLUDE USING gist` constraints, one per
//! table whose rows carry a `[valid_from, valid_to)` window. They exist because
//! every "who was responsible on this date" read filters by that window: two
//! rows covering one day give the query two answers and it returns whichever the
//! planner reached first.
//!
//! `tests/temporal_constraints_integration.rs` pins that the *database* refuses
//! the overlap. This file pins what the caller is then told. PostgreSQL raises
//! `23P01` (`exclusion_violation`) — not `23514` (`check_violation`), which is
//! the only code the repositories used to translate, and only on one table. So
//! an operator who backdated an MSB assignment, or filed a price sheet whose
//! window overlapped the one already stored, got `500 internal`: the server
//! saying it broke, when what broke was the request, and when what the operator
//! needed to hear was which of their own dates to move.
//!
//! Every constraint is reached through the repository the API writes through, so
//! what is pinned is the error a request actually receives — including its HTTP
//! status, which `MdmError::status_u16` derives.
//!
//! PostgreSQL is self-managed via testcontainers; only a Docker daemon is
//! required.

use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        MaloRepository, MeloMsbRepository, MeloRepository, NbContractRecord, NbContractRepository,
        PreisblattDienstleistungRepository, PreisblattHardwareRepository, PreisblattKaRepository,
        PreisblattMessungRepository, PreisblattRepository, PreisblattSource, Rollenzuordnung,
    },
};
use marktd::pg::{
    PgMaloRepository, PgMeloMsbRepository, PgMeloRepository, PgNbContractRepository,
    PgPreisblattDienstleistungRepository, PgPreisblattHardwareRepository, PgPreisblattKaRepository,
    PgPreisblattMessungRepository, PgPreisblattRepository,
};
use sqlx::PgPool;
use time::macros::date;

const SCHEMA: &str = include_str!("../migrations/0001_initial.sql");
const TENANT: &str = "9900357000004";
const BO4E: &str = "v202607.0.0";

/// Deliberately check-digit-invalid, like every example ID in this repo: a
/// valid one names a real company.
const MALO: &str = "51238696012";
const MELO: &str = "DE0001112223334445556667778889990";
const NB: &str = "9900000000001";
const MSB: &str = "9900000000002";

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

async fn test_pool() -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

/// Start a fresh throwaway `postgres:17-alpine`. `None` when Docker is absent.
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

/// Assert the repository answered `422` and said something the caller can act
/// on, rather than `500`.
///
/// `500` is the specific regression: it tells an operator the server is broken
/// and gives them nothing to change. The message must at least name the
/// constraint that refused, since that is what says *which* window collided.
fn assert_unprocessable(err: &MdmError, constraint: &str, what: &str) {
    assert_eq!(
        err.status_u16(),
        422,
        "{what}: an overlapping [valid_from, valid_to) is the caller's mistake, but the \
         repository answered {} — `{err}`",
        err.status_u16()
    );
    let MdmError::Unprocessable { reason } = err else {
        panic!("{what}: expected MdmError::Unprocessable, got {err:?}");
    };
    assert!(
        reason.contains(constraint),
        "{what}: the reason must name the constraint that refused, so the operator knows \
         which window collided — got: {reason}"
    );
    assert!(
        reason.contains("valid_from"),
        "{what}: the reason must tell the caller what to move — got: {reason}"
    );
}

fn malo_id() -> MaloId {
    MALO.parse().expect("valid MaLo-ID")
}

fn bo4e_malo() -> rubo4e::current::Marktlokation {
    serde_json::from_value(serde_json::json!({
        "_typ": "MARKTLOKATION",
        "marktlokationsId": MALO,
    }))
    .expect("valid BO4E Marktlokation")
}

fn preisblatt(von: &str, bis: Option<&str>) -> serde_json::Value {
    serde_json::json!({
        "_typ": "PREISBLATT",
        "bezeichnung": "Netzentgelte",
        "gueltigkeit": { "startdatum": von, "enddatum": bis },
    })
}

// ── rollenzuordnungen ─────────────────────────────────────────────────────────

/// Two Netzbetreiber cannot both be responsible for one MaLo on one day, and
/// saying so is a `422`.
///
/// The list arrives whole on `PUT /api/v1/malos/{id}`: the repository deletes
/// the stored assignments and re-inserts the caller's, so a self-overlapping
/// list is caught on insert.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_self_overlapping_rollenzuordnung_list_is_unprocessable() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let repo = PgMaloRepository::new(pool.clone());

    let zuordnung = |from, to| Rollenzuordnung {
        zuordnungstyp: "NB".to_owned(),
        rollencodenummer: NB.to_owned(),
        valid_from: from,
        valid_to: to,
    };

    // Abutting is not overlapping: half-open windows must let a successor start
    // on the day its predecessor ends.
    repo.upsert(
        &malo_id(),
        Sparte::Strom,
        &bo4e_malo(),
        vec![
            zuordnung(date!(2025 - 01 - 01), Some(date!(2026 - 01 - 01))),
            zuordnung(date!(2026 - 01 - 01), None),
        ],
        None,
        BO4E,
    )
    .await
    .expect("an abutting pair is accepted");

    let err = repo
        .upsert(
            &malo_id(),
            Sparte::Strom,
            &bo4e_malo(),
            vec![
                zuordnung(date!(2025 - 01 - 01), Some(date!(2026 - 06 - 01))),
                zuordnung(date!(2026 - 01 - 01), None),
            ],
            None,
            BO4E,
        )
        .await
        .expect_err("an overlapping pair must be refused");
    assert_unprocessable(
        &err,
        "rollenzuordnungen_no_overlap",
        "two NB on one MaLo on one day",
    );
}

// ── melo_msb_zuordnungen ──────────────────────────────────────────────────────

/// A backdated MSB assignment that lands inside an already-closed window.
///
/// `assign_msb` closes the *open* row and ends the new one where the next
/// begins, which handles append and prepend. It does not reopen a closed row,
/// so an assignment backdated into an existing closed span overlaps it — the
/// case WiM Teil 2 UC 4.1.1 cares about, since `find_msb_at` must resolve to
/// exactly one MSB for any past date.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_backdated_msb_assignment_is_unprocessable() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    PgMeloRepository::new(pool.clone(), TENANT)
        .upsert(
            &MELO.parse().expect("valid MeLo-ID"),
            None,
            &serde_json::from_value(serde_json::json!({
                "_typ": "MESSLOKATION",
                "messlokationsId": MELO,
                "sparte": "STROM",
            }))
            .expect("valid BO4E Messlokation"),
            None,
            BO4E,
        )
        .await
        .expect("seed melo");

    let repo = PgMeloMsbRepository::new(pool.clone());
    repo.assign_msb(TENANT, MELO, MSB, date!(2025 - 01 - 01))
        .await
        .expect("the first assignment is accepted");
    repo.assign_msb(TENANT, MELO, NB, date!(2026 - 01 - 01))
        .await
        .expect("a later assignment closes the open one and is accepted");

    // 2025-06-01 falls inside the now-closed [2025-01-01, 2026-01-01).
    let err = repo
        .assign_msb(TENANT, MELO, NB, date!(2025 - 06 - 01))
        .await
        .expect_err("a backdated assignment inside a closed span must be refused");
    assert_unprocessable(
        &err,
        "melo_msb_no_overlap",
        "two MSB for one MeLo on one day",
    );
}

// ── nb_contracts ──────────────────────────────────────────────────────────────

/// Two network contracts for the same MaLo and NB cannot both be in force: a
/// settlement would pick either Netzebene or Bilanzierungsmethode.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_overlapping_netznutzungsvertrag_is_unprocessable() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    PgMaloRepository::new(pool.clone())
        .upsert(&malo_id(), Sparte::Strom, &bo4e_malo(), vec![], None, BO4E)
        .await
        .expect("seed malo");

    let repo = PgNbContractRepository::new(pool.clone());
    let contract = |id: &str, from, to| NbContractRecord {
        contract_id: id.to_owned(),
        malo_id: malo_id(),
        nb_mp_id: NB.to_owned(),
        sparte: Sparte::Strom,
        netzebene: "NS".to_owned(),
        bilanzierungsmethode: "SLP".to_owned(),
        billing_schedule: mako_markt::repository::BillingSchedule::Monthly,
        netznutzer_mp_id: MSB.to_owned(),
        netznutzer_typ: mako_markt::repository::NetznutzerTyp::default(),
        valid_from: from,
        valid_to: to,
        version: 0,
        tenant: TENANT.to_owned(),
        data: serde_json::json!({}),
        vertragsart: Some("NETZNUTZUNGSVERTRAG".to_owned()),
        vertragsstatus: Some("AKTIV".to_owned()),
    };

    repo.upsert(contract(
        "LRV-1",
        date!(2025 - 01 - 01),
        Some(date!(2026 - 01 - 01)),
    ))
    .await
    .expect("the first contract is accepted");
    repo.upsert(contract("LRV-2", date!(2026 - 01 - 01), None))
        .await
        .expect("an abutting successor is not an overlap");

    let err = repo
        .upsert(contract("LRV-3", date!(2026 - 06 - 01), None))
        .await
        .expect_err("a contract overlapping the open one must be refused");
    assert_unprocessable(
        &err,
        "nb_contracts_no_overlap",
        "two network contracts for one MaLo and NB",
    );
}

// ── the five Preisblatt tables ────────────────────────────────────────────────

/// Every price sheet family, in one test, because they share one failure: two
/// sheets valid on the same day make the tariff a lottery — `invoic-checker`
/// validates INVOIC plausibility against whichever the read happened to return.
///
/// The `ON CONFLICT (…, valid_from)` upsert absorbs a *re-file of the same start
/// date*; it is a **different** start date whose window overlaps that the
/// exclusion constraint catches, and that used to be a `500`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_overlapping_preisblatt_is_unprocessable_in_every_family() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    let first = preisblatt("2025-01-01", Some("2026-01-01"));
    let abutting = preisblatt("2026-01-01", None);
    let overlapping = preisblatt("2025-06-01", None);
    let src = PreisblattSource::Api;

    // Netzentgelte (NB).
    {
        let repo = PgPreisblattRepository::new(pool.clone());
        repo.upsert(NB, first.clone(), BO4E, src)
            .await
            .expect("the first sheet is accepted");
        repo.upsert(NB, abutting.clone(), BO4E, src)
            .await
            .expect("an abutting successor is not an overlap");
        let err = repo
            .upsert(NB, overlapping.clone(), BO4E, src)
            .await
            .expect_err("an overlapping sheet must be refused");
        assert_unprocessable(&err, "preisblaetter_no_overlap", "Netzentgelte");
    }

    // Messung (MSB).
    {
        let repo = PgPreisblattMessungRepository::new(pool.clone());
        repo.upsert_messung(MSB, first.clone(), BO4E, src)
            .await
            .expect("the first sheet is accepted");
        let err = repo
            .upsert_messung(MSB, overlapping.clone(), BO4E, src)
            .await
            .expect_err("an overlapping sheet must be refused");
        assert_unprocessable(&err, "preisblaetter_messung_no_overlap", "Messung");
    }

    // Konzessionsabgabe (NB, per Sparte and Kundengruppe).
    {
        let repo = PgPreisblattKaRepository::new(pool.clone());
        repo.upsert_ka(NB, "STROM", Some("TARIF"), first.clone(), BO4E, src)
            .await
            .expect("the first sheet is accepted");
        // A different Kundengruppe in the same window is a different sheet.
        repo.upsert_ka(NB, "STROM", Some("SONDER"), first.clone(), BO4E, src)
            .await
            .expect("another Kundengruppe is not an overlap");
        let err = repo
            .upsert_ka(NB, "STROM", Some("TARIF"), overlapping.clone(), BO4E, src)
            .await
            .expect_err("an overlapping sheet must be refused");
        assert_unprocessable(&err, "preisblaetter_ka_no_overlap", "Konzessionsabgabe");
    }

    // Dienstleistung (MSB).
    {
        let repo = PgPreisblattDienstleistungRepository::new(pool.clone());
        repo.upsert_dienstleistung(MSB, first.clone(), BO4E, src)
            .await
            .expect("the first sheet is accepted");
        let err = repo
            .upsert_dienstleistung(MSB, overlapping.clone(), BO4E, src)
            .await
            .expect_err("an overlapping sheet must be refused");
        assert_unprocessable(
            &err,
            "preisblaetter_dienstleistung_no_overlap",
            "Dienstleistung",
        );
    }

    // Hardware (MSB).
    {
        let repo = PgPreisblattHardwareRepository::new(pool.clone());
        repo.upsert_hardware(MSB, first, BO4E, src)
            .await
            .expect("the first sheet is accepted");
        let err = repo
            .upsert_hardware(MSB, overlapping, BO4E, src)
            .await
            .expect_err("an overlapping sheet must be refused");
        assert_unprocessable(&err, "preisblaetter_hardware_no_overlap", "Hardware");
    }
}

// ── the other code the same mapping carries ───────────────────────────────────

/// `23514` was already mapped, but on `lf_zuordnung` alone. The shared mapper
/// carries it for every table now, and this pins that widening it to `23P01`
/// did not lose it.
///
/// Reached through raw SQL: the violation is of a CHECK the typed API cannot
/// express, and what is under test is the mapper, not the enum in front of it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_check_violation_is_still_unprocessable() {
    let Some((pool, _pg)) = test_pool().await else {
        return;
    };
    PgMaloRepository::new(pool.clone())
        .upsert(&malo_id(), Sparte::Strom, &bo4e_malo(), vec![], None, BO4E)
        .await
        .expect("seed malo");

    let raw = sqlx::query(
        "INSERT INTO nb_contracts (contract_id, malo_id, nb_mp_id, sparte, netzebene, \
         bilanzierungsmethode, billing_schedule, netznutzer_mp_id, netznutzer_typ, \
         valid_from, tenant) \
         VALUES ('LRV-1', $1, $2, 'STROM', 'NS', 'SLP', 'MONTHLY', $3, 'BETREIBER', \
         '2030-01-01'::date, $4)",
    )
    .bind(MALO)
    .bind(NB)
    .bind(MSB)
    .bind(TENANT)
    .execute(&pool)
    .await
    .expect_err("an unknown netznutzer_typ must be refused");

    let mapped = marktd::pg::write_error(raw);
    assert_eq!(
        mapped.status_u16(),
        422,
        "a CHECK violation is still the caller's, not an outage: {mapped}"
    );
    assert!(
        mapped.to_string().contains("netznutzer_typ"),
        "the reason must name the constraint that refused: {mapped}"
    );
}
