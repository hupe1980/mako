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

/// Bring a freshly created contract into the state billing actually sees.
///
/// `insert_versorgungsvertrag` leaves both rows `ANGELEGT`; the buyer projection
/// (like `fetch_vertrag_by_malo`) deliberately only resolves *active* supply,
/// because a contract that has not started has no invoice to carry a buyer.
async fn activate(pool: &PgPool, vertrag_id: Uuid) {
    sqlx::query("UPDATE versorgungsvertraege SET status='AKTIV' WHERE id=$1")
        .bind(vertrag_id)
        .execute(pool)
        .await
        .expect("activate contract");
    sqlx::query("UPDATE vertragskomponenten SET status='AKTIV' WHERE vertrag_id=$1")
        .bind(vertrag_id)
        .execute(pool)
        .await
        .expect("activate component");
}

// ── BG-7 buyer projection — the join billingd's e-invoice depends on ─────────

/// `fetch_rechnungsempfaenger_by_malo` must resolve the Kunde behind a MaLo.
///
/// `billingd` holds no customer master: without this projection its EN 16931
/// buyer is synthesised from the MaLo-ID and the invoice fails XRechnung on
/// BR-DE-8 (BT-52 city) and BR-DE-9 (BT-53 post code). The whole chain is SQL —
/// a three-table join plus a status filter — so nothing but a real database
/// proves it. The BO4E address is read out of `geschaeftspartner` JSONB, which a
/// compile-time check cannot verify either.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_bg7_buyer_resolves_from_the_kunde_behind_the_malo() {
    let Some((pool, _pg)) = test_pool("bg7_buyer").await else {
        return;
    };
    let tenant = "9800000000009";
    let kunde = make_kunde(&pool, tenant).await;

    // A BO4E Geschaeftspartner with a nested Adresse, as the portal stores it.
    sqlx::query(
        "UPDATE kunden SET geschaeftspartner = $2::jsonb, umsatzsteuer_id = $3 WHERE id = $1",
    )
    .bind(kunde)
    .bind(
        r#"{"name1":"Erika Mustermann",
            "adresse":{"strasse":"Beispielweg","hausnummer":"7",
                       "postleitzahl":"10115","ort":"Berlin","landescode":"DE"}}"#,
    )
    .bind("DE987654321")
    .execute(&pool)
    .await
    .expect("populate kunde master data");
    // § 13b flag: master data, projected with the buyer so billingd can derive
    // reverse charge without a second lookup.
    sqlx::query("UPDATE kunden SET stromwiederverkaeufer = true WHERE id = $1")
        .bind(kunde)
        .execute(&pool)
        .await
        .expect("flag the kunde as Stromwiederverkäufer");

    let v =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-BG7-1"))
            .await
            .expect("create contract");
    activate(&pool, v.id).await;

    let buyer = pg::fetch_rechnungsempfaenger_by_malo(&pool, "51238696781", tenant)
        .await
        .expect("query succeeds")
        .expect("a Kunde is on file for this MaLo");

    assert_eq!(buyer.name.as_deref(), Some("Erika Mustermann"));
    // Straße and Hausnummer are separate BO4E fields and one BT-50 line.
    assert_eq!(buyer.line1.as_deref(), Some("Beispielweg 7"));
    assert_eq!(buyer.post_code.as_deref(), Some("10115"));
    assert_eq!(buyer.city.as_deref(), Some("Berlin"));
    assert_eq!(buyer.country.as_deref(), Some("DE"));
    assert_eq!(buyer.vat_id.as_deref(), Some("DE987654321"));
    assert!(
        buyer.stromwiederverkaeufer,
        "the § 13b flag must travel with the BG-7 projection",
    );

    // Tenant-scoped: another tenant must not read this customer's address.
    assert!(
        pg::fetch_rechnungsempfaenger_by_malo(&pool, "51238696781", "9800000000008")
            .await
            .expect("query succeeds")
            .is_none(),
        "the buyer projection must not leak across tenants",
    );

    // An unknown MaLo is absent, not an error — billingd falls back to the stub.
    assert!(
        pg::fetch_rechnungsempfaenger_by_malo(&pool, "99999999999", tenant)
            .await
            .expect("query succeeds")
            .is_none(),
    );
}

/// A Kunde with no master data yields a buyer with empty terms, not an error.
///
/// `geschaeftspartner` is nullable and operator-populated. billingd must be able
/// to tell "no Kunde" (fall back to the supply-site stub) from "a Kunde with
/// nothing filled in", and neither may fail the billing run.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_kunde_without_master_data_yields_empty_buyer_terms() {
    let Some((pool, _pg)) = test_pool("bg7_buyer_empty").await else {
        return;
    };
    let tenant = "9800000000010";
    let kunde = make_kunde(&pool, tenant).await;
    let v =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-BG7-2"))
            .await
            .expect("create contract");
    activate(&pool, v.id).await;

    let buyer = pg::fetch_rechnungsempfaenger_by_malo(&pool, "51238696781", tenant)
        .await
        .expect("query succeeds")
        .expect("the Kunde row exists even with no master data");
    assert!(buyer.name.is_none() && buyer.city.is_none() && buyer.post_code.is_none());
}

/// A Sammelrechnung's buyer is the Rahmenvertrag holder, not a site's customer.
///
/// The bundled B2B document is addressed to the framework-contract holder, so
/// `billingd` cannot derive it from the MaLo list it enumerates. This is the
/// second projection over `kunden`, reached by a different join.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_rahmenvertrag_holder_resolves_as_the_bundled_invoice_buyer() {
    let Some((pool, _pg)) = test_pool("bg7_rahmenvertrag").await else {
        return;
    };
    let tenant = "9800000000011";
    let kunde = make_kunde(&pool, tenant).await;
    sqlx::query("UPDATE kunden SET geschaeftspartner = $2::jsonb WHERE id = $1")
        .bind(kunde)
        .bind(
            r#"{"name1":"Musterfiliale GmbH",
                "adresse":{"strasse":"Zentrale","hausnummer":"1",
                           "postleitzahl":"20095","ort":"Hamburg","landescode":"DE"}}"#,
        )
        .execute(&pool)
        .await
        .expect("populate holder master data");

    let rv_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO rahmenvertraege (id, kunden_id, tenant, gueltig_von)
         VALUES ($1, $2, $3, DATE '2026-01-01')",
    )
    .bind(rv_id)
    .bind(kunde)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("insert rahmenvertrag");

    let buyer = pg::fetch_rechnungsempfaenger_by_rahmenvertrag(&pool, rv_id, tenant)
        .await
        .expect("query succeeds")
        .expect("the holder is on file");
    assert_eq!(buyer.name.as_deref(), Some("Musterfiliale GmbH"));
    assert_eq!(buyer.city.as_deref(), Some("Hamburg"));

    assert!(
        pg::fetch_rechnungsempfaenger_by_rahmenvertrag(&pool, rv_id, "9800000000012")
            .await
            .expect("query succeeds")
            .is_none(),
        "the holder projection must not leak across tenants",
    );
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

/// The § 42b GGV operator resolves to a BG-7 buyer by community id.
///
/// The bundled GGV Sammelrechnung bills the operator, who is a Kunde behind
/// `ggv_betreiber` — the one buyer path keyed by a GGV id rather than a MaLo
/// or a contract. The chain is SQL (mapping row → kunden JSONB projection),
/// so a real database proves it; the upsert's move-the-pointer semantics and
/// the missing-Kunde refusal are pinned alongside.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_ggv_bundle_buyer_resolves_from_the_betreiber_kunde() {
    let Some((pool, _pg)) = test_pool("ggv_betreiber").await else {
        return;
    };
    let tenant = "9800000000009";
    let kunde = make_kunde(&pool, tenant).await;
    sqlx::query("UPDATE kunden SET geschaeftspartner = $2::jsonb WHERE id = $1")
        .bind(kunde)
        .bind(
            r#"{"name1":"WEG Sonnenhof Verwaltungs-GmbH",
                "adresse":{"strasse":"Solarweg","hausnummer":"1",
                           "postleitzahl":"10115","ort":"Berlin","landescode":"DE"}}"#,
        )
        .execute(&pool)
        .await
        .expect("populate operator master data");

    // A Kunde that does not exist (or belongs to another tenant) is refused
    // with `false`, not written: the handler answers 404 from this.
    assert!(
        !pg::upsert_ggv_betreiber(&pool, tenant, "GGV-SONNENHOF", Uuid::new_v4())
            .await
            .expect("upsert against missing kunde"),
        "an unknown kunden_id must be refused",
    );
    assert!(
        !pg::upsert_ggv_betreiber(&pool, "9800000000001", "GGV-SONNENHOF", kunde)
            .await
            .expect("upsert across tenants"),
        "another tenant's Kunde must be refused",
    );
    assert!(
        pg::fetch_rechnungsempfaenger_by_ggv(&pool, "GGV-SONNENHOF", tenant)
            .await
            .expect("fetch before mapping")
            .is_none(),
        "no Betreiber recorded yet",
    );

    assert!(
        pg::upsert_ggv_betreiber(&pool, tenant, "GGV-SONNENHOF", kunde)
            .await
            .expect("record the operator"),
    );
    let buyer = pg::fetch_rechnungsempfaenger_by_ggv(&pool, "GGV-SONNENHOF", tenant)
        .await
        .expect("fetch the bundle buyer")
        .expect("the operator is the buyer");
    assert_eq!(
        buyer.name.as_deref(),
        Some("WEG Sonnenhof Verwaltungs-GmbH")
    );
    assert_eq!(buyer.line1.as_deref(), Some("Solarweg 1"));
    assert_eq!(buyer.post_code.as_deref(), Some("10115"));
    assert_eq!(buyer.city.as_deref(), Some("Berlin"));

    // Re-PUT moves the pointer — the mapping is operator-correctable, unlike
    // the append-only stores; the previous operator leaves no residue.
    let kunde2 = make_kunde(&pool, tenant).await;
    sqlx::query("UPDATE kunden SET geschaeftspartner = $2::jsonb WHERE id = $1")
        .bind(kunde2)
        .bind(r#"{"name1":"Hausverwaltung Neu GmbH"}"#)
        .execute(&pool)
        .await
        .expect("populate second operator");
    assert!(
        pg::upsert_ggv_betreiber(&pool, tenant, "GGV-SONNENHOF", kunde2)
            .await
            .expect("move the pointer"),
    );
    let moved = pg::fetch_rechnungsempfaenger_by_ggv(&pool, "GGV-SONNENHOF", tenant)
        .await
        .expect("fetch after move")
        .expect("the new operator answers");
    assert_eq!(moved.name.as_deref(), Some("Hausverwaltung Neu GmbH"));

    // Tenant scoping on the read side too.
    assert!(
        pg::fetch_rechnungsempfaenger_by_ggv(&pool, "GGV-SONNENHOF", "9800000000001")
            .await
            .expect("cross-tenant read")
            .is_none(),
        "another tenant must not see this community's operator",
    );
}

/// §40b EnWG: the billing feed must contain a component the MaKo confirmed.
///
/// Nothing ever promotes a component from BESTAETIGT to AKTIV, so requiring
/// AKTIV alone left the feed to billingd permanently empty — every scheduled
/// invoice for every customer simply never ran.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_confirmed_component_is_a_billing_candidate() {
    let Some((pool, _pg)) = test_pool("billing_candidates").await else {
        return;
    };
    let tenant = "9800000000002";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-BILL-1"))
            .await
            .expect("create");

    // Fresh contract: nothing is in supply yet.
    assert!(
        pg::list_billing_candidates(&pool, tenant)
            .await
            .expect("list")
            .is_empty(),
        "an ANGELEGT component is not billable"
    );

    // The state a confirmed MaKo Lieferbeginn actually leaves behind.
    sqlx::query("UPDATE versorgungsvertraege SET status='AKTIV' WHERE id=$1")
        .bind(created.id)
        .execute(&pool)
        .await
        .expect("activate contract");
    sqlx::query("UPDATE vertragskomponenten SET status='BESTAETIGT' WHERE vertrag_id=$1")
        .bind(created.id)
        .execute(&pool)
        .await
        .expect("confirm component");

    let candidates = pg::list_billing_candidates(&pool, tenant)
        .await
        .expect("list");
    assert_eq!(candidates.len(), 1, "the confirmed component is billable");
    assert_eq!(candidates[0].malo_id, "51238696781");
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
