//! Real-PostgreSQL guards for the contract-lifecycle invariants that live in
//! SQL, not in Rust: idempotent supply-contract creation (no duplicate
//! Lieferbeginn), the Stornierung state guard, and tenant-scoped mutation.
//!
//! PostgreSQL is self-managed via testcontainers (a Docker daemon is the only
//! requirement); the tests skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-vertragd-db
//! ```

use sqlx::PgPool;
use uuid::Uuid;
use vertragd::pg;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

async fn test_pool(_test_name: &str) -> Option<(PgPool, PgContainer)> {
    let (url, container) = pg_container().await?;
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::raw_sql(SCHEMA)
        .execute(&pool)
        .await
        .expect("apply schema");
    Some((pool, container))
}

async fn make_kunde(pool: &PgPool, tenant: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO kunden (id, tenant, kundentyp) VALUES ($1, $2, 'B2C')")
        .bind(id)
        .bind(tenant)
        .execute(pool)
        .await
        .expect("insert kunde");
    id
}

fn vertrag_input(erp_id: &str) -> pg::CreateVersorgungsvertragInput {
    let d = time::macros::date!(2026 - 10 - 01);
    pg::CreateVersorgungsvertragInput {
        rahmenvertrag_id: None,
        kundentyp: "B2C".to_owned(),
        bundle_code: None,
        vertragsbeginn: d,
        vertragsende: None,
        kuendigungsfrist_monate: None,
        preisgarantie_bis: None,
        auto_renewal: None,
        standort_bezeichnung: None,
        erp_contract_id: Some(erp_id.to_owned()),
        notizen: None,
        komponenten: vec![pg::CreateKomponenteInput {
            sparte: "STROM".to_owned(),
            malo_id: Some("51238696781".to_owned()),
            melo_id: None,
            nb_mp_id: Some("9900000000001".to_owned()),
            product_code: "STROM-BASIS-2026".to_owned(),
            lieferbeginn: d,
            lieferende: None,
            fulfillment_data: None,
        }],
    }
}

// ── D3 — idempotent creation prevents a duplicate Lieferbeginn ────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn reposting_same_erp_contract_id_dispatches_no_second_lieferbeginn() {
    let Some((pool, _pg)) = test_pool("idempotent_create").await else {
        return;
    };
    let tenant = "9800000000002";
    let kunde = make_kunde(&pool, tenant).await;
    let input = vertrag_input("ERP-CONTRACT-1");

    let first = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("first create");
    assert!(first.is_new, "first POST is a genuine insert");
    assert_eq!(
        first.komponenten.len(),
        1,
        "one component to dispatch on first create"
    );

    // Re-POST the same erp_contract_id — the handler dispatches over
    // `komponenten`, which MUST be empty so no second UTILMD fires.
    let second = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("idempotent replay");
    assert!(!second.is_new, "second POST is a conflict replay");
    assert_eq!(second.id, first.id, "same contract returned");
    assert!(
        second.komponenten.is_empty(),
        "an idempotent replay dispatches nothing — this is what stops the duplicate Lieferbeginn"
    );

    let komp_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vertragskomponenten WHERE vertrag_id = $1")
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(komp_count, 1, "no duplicate component rows either");
}

// ── D2 — Stornierung state guard ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn stornierung_is_refused_on_an_active_contract() {
    let Some((pool, _pg)) = test_pool("storniere_guard").await else {
        return;
    };
    let tenant = "9800000000002";
    let kunde = make_kunde(&pool, tenant).await;
    let inserted =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-2"))
            .await
            .expect("create");

    // ANGELEGT → Stornierung allowed.
    pg::storniere_vertrag(&pool, inserted.id, tenant)
        .await
        .expect("stornieren an ANGELEGT contract");
    let status: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(inserted.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "STORNIERT");

    // Now force a second contract to AKTIV and prove Stornierung is refused.
    let active =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-3"))
            .await
            .expect("create");
    sqlx::query("UPDATE versorgungsvertraege SET status = 'AKTIV' WHERE id = $1")
        .bind(active.id)
        .execute(&pool)
        .await
        .unwrap();
    let err = pg::storniere_vertrag(&pool, active.id, tenant).await;
    assert!(
        err.is_err(),
        "Stornierung of an AKTIV contract must be refused (that path is Kündigung)"
    );
    let still_active: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(active.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_active, "AKTIV", "the active contract is untouched");
}

// ── D18 — tenant-scoped mutation ──────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn update_vertrag_status_is_tenant_scoped() {
    let Some((pool, _pg)) = test_pool("tenant_scope").await else {
        return;
    };
    let tenant = "9800000000002";
    let other = "9800000000099";
    let kunde = make_kunde(&pool, tenant).await;
    let inserted =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-4"))
            .await
            .expect("create");

    // A caller presenting the wrong tenant cannot mutate this contract.
    pg::update_vertrag_status(&pool, inserted.id, other, "GEKÜNDIGT")
        .await
        .expect("query runs");
    let status: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(inserted.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_ne!(status, "GEKÜNDIGT", "wrong-tenant update must not apply");

    // The right tenant succeeds.
    pg::update_vertrag_status(&pool, inserted.id, tenant, "GEKÜNDIGT")
        .await
        .expect("right-tenant update");
    let status: String =
        sqlx::query_scalar("SELECT status FROM versorgungsvertraege WHERE id = $1")
            .bind(inserted.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(status, "GEKÜNDIGT");
}
/// The Postgres container guard a test holds until it ends — dropping it removes
fn agg_input(
    price: &str,
    von: time::Date,
    bis: Option<time::Date>,
) -> pg::UpsertAggregatorvertragInput {
    use std::str::FromStr as _;
    pg::UpsertAggregatorvertragInput {
        vpp_id: "VPP-1".to_owned(),
        malo_id: "51238696780".to_owned(),
        aggregator_mp_id: "9900357000004".to_owned(),
        capacity_price_eur_per_kwh: rust_decimal::Decimal::from_str(price).unwrap(),
        vertragsbeginn: von,
        vertragsende: bis,
        mwst_rate_override: None,
        kunden_id: None,
    }
}

/// §41e EnWG: a SteuerbareRessource may have at most one Aggregatorvertrag in
/// force at any instant. The `agg_no_overlap` GiST exclusion constraint enforces
/// it in SQL — the predecessor table keyed only on `(sr_id, tenant, valid_from)`
/// and happily stored two overlapping contracts.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn overlapping_aggregatorvertraege_are_refused() {
    let Some((pool, _c)) = test_pool("agg_overlap").await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let t = "9900357000004";

    // 2026-01-01 .. open-ended
    pg::upsert_aggregatorvertrag(
        &pool,
        t,
        "C1234567890123456789012345678901",
        &agg_input("0.12", time::macros::date!(2026 - 01 - 01), None),
    )
    .await
    .expect("first contract inserted");

    // Starts inside the open-ended window -> must be refused.
    let err = pg::upsert_aggregatorvertrag(
        &pool,
        t,
        "C1234567890123456789012345678901",
        &agg_input("0.15", time::macros::date!(2026 - 06 - 01), None),
    )
    .await
    .expect_err("overlapping contract must be refused");
    assert!(
        format!("{err:?}").contains("agg_no_overlap"),
        "expected the exclusion constraint to fire, got: {err:?}"
    );
}

/// A back-to-back succession (`[a, b)` then `[b, …)`) must be accepted — the
/// range is half-open, so touching endpoints do not overlap.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn back_to_back_aggregatorvertraege_are_allowed() {
    use std::str::FromStr as _;

    let Some((pool, _c)) = test_pool("agg_succession").await else {
        eprintln!("skipping: Docker unavailable");
        return;
    };
    let t = "9900357000004";
    let sr = "C1234567890123456789012345678901";

    pg::upsert_aggregatorvertrag(
        &pool,
        t,
        sr,
        &agg_input(
            "0.12",
            time::macros::date!(2026 - 01 - 01),
            Some(time::macros::date!(2026 - 07 - 01)),
        ),
    )
    .await
    .expect("first contract");

    pg::upsert_aggregatorvertrag(
        &pool,
        t,
        sr,
        &agg_input("0.15", time::macros::date!(2026 - 07 - 01), None),
    )
    .await
    .expect("succeeding contract must be accepted");

    // The lookup must select by the dispatch date, not by "latest".
    let before =
        pg::find_active_aggregatorvertrag(&pool, t, sr, time::macros::date!(2026 - 03 - 01))
            .await
            .unwrap()
            .expect("contract in force in March");
    assert_eq!(
        before.capacity_price_eur_per_kwh,
        rust_decimal::Decimal::from_str("0.12").unwrap()
    );

    let after =
        pg::find_active_aggregatorvertrag(&pool, t, sr, time::macros::date!(2026 - 09 - 01))
            .await
            .unwrap()
            .expect("contract in force in September");
    assert_eq!(
        after.capacity_price_eur_per_kwh,
        rust_decimal::Decimal::from_str("0.15").unwrap()
    );
}

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
