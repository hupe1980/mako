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
use vertragd::pg::{self, Initiator};

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
    sqlx::query(
        "INSERT INTO kunden (id, tenant, kundentyp, haushaltskunde) VALUES ($1, $2, 'B2C', true)",
    )
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
        vertragsart: None,
        bundle_code: None,
        vertragsbeginn: d,
        vertragsende: None,
        kuendigungsfrist_monate: None,
        preisgarantie_bis: None,
        abrechnungszyklus: None,
        auto_renewal: None,
        renewal_monate: None,
        standort_bezeichnung: None,
        standort_adresse: None,
        zahlungsziel_tage: None,
        erp_contract_id: Some(erp_id.to_owned()),
        notizen: None,
        komponenten: vec![pg::CreateKomponenteInput {
            sparte: "STROM".to_owned(),
            malo_id: Some(MALO.to_owned()),
            melo_id: None,
            nb_mp_id: Some("9900000000001".to_owned()),
            product_code: "STROM-BASIS-2026".to_owned(),
            lieferbeginn: d,
            lieferende: None,
            fulfillment_data: None,
        }],
    }
}

/// A MaLo-ID valid under both check-digit schemes in use across the workspace.
const MALO: &str = "51238696012";

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

    // A real BO4E `Geschaeftspartner`: a natural person is named by
    // `vorname`/`nachname`, an organisation by `organisationsname`, and BO4E
    // defines no `name1` — a reader probing for one leaves BT-44 empty here.
    // The `kontaktwege` entry is deliberate: `rubo4e` types `kontaktwert` as a
    // `Decimal`, so a naive typed read of this object fails *whole* and
    // silently empties the buyer.
    sqlx::query(
        "UPDATE kunden SET geschaeftspartner = $2::jsonb, umsatzsteuer_id = $3 WHERE id = $1",
    )
    .bind(kunde)
    .bind(
        r#"{"_typ":"GESCHAEFTSPARTNER","vorname":"Erika","nachname":"Mustermann",
            "kontaktwege":[{"_typ":"KONTAKTWEG","kontaktart":"E_MAIL",
                            "kontaktwert":"erika@example.test",
                            "istBevorzugterKontaktweg":true}],
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

    let buyer = pg::fetch_rechnungsempfaenger_by_malo(&pool, MALO, tenant)
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
    // Where the document is *sent*. No EN 16931 BT carries it, and resolving it
    // separately from the party the document is addressed to is how a notice
    // ends up addressed to one person and delivered to another.
    assert_eq!(
        buyer.email.as_deref(),
        Some("erika@example.test"),
        "the bevorzugter E_MAIL Kontaktweg reaches the projection — and the object it sits \
         in still parses, which a naive typed read of `kontaktwert` would not manage",
    );

    // Tenant-scoped: another tenant must not read this customer's address.
    assert!(
        pg::fetch_rechnungsempfaenger_by_malo(&pool, MALO, "9800000000008")
            .await
            .expect("query succeeds")
            .is_none(),
        "the buyer projection must not leak across tenants",
    );

    // An unknown MaLo is absent, not an error — billingd falls back to the stub.
    assert!(
        pg::fetch_rechnungsempfaenger_by_malo(&pool, "99999999044", tenant)
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

    let buyer = pg::fetch_rechnungsempfaenger_by_malo(&pool, MALO, tenant)
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
            r#"{"_typ":"GESCHAEFTSPARTNER","organisationsname":"Musterfiliale GmbH",
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
        malo_id: "51238696012".to_owned(),
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
            r#"{"_typ":"GESCHAEFTSPARTNER","organisationsname":"WEG Sonnenhof Verwaltungs-GmbH",
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
        .bind(r#"{"_typ":"GESCHAEFTSPARTNER","organisationsname":"Hausverwaltung Neu GmbH"}"#)
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
    assert_eq!(candidates[0].malo_id, MALO);
}

// ── Kündigung — the whole termination is one transaction ─────────────────────

/// Terminating a contract must set the term it now actually has, enqueue the
/// Lieferende UTILMD *and* the Schlussablesung, and record that the § 41 Abs. 8
/// Nr. 2 EnWG Textform confirmation is owed.
///
/// All of it is SQL — an UPDATE, two conditional enqueues keyed on a unique
/// index, and a status guard — so only a real database proves it. The previous
/// implementation left `vertragsende` untouched, which meant a terminated
/// contract went on advertising its old term to billingd and the portal.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_kuendigung_ends_the_term_and_enqueues_both_obligations() {
    let Some((pool, _pg)) = test_pool("kuendigung").await else {
        return;
    };
    let tenant = "9800000000010";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("K-1"))
            .await
            .expect("create");
    activate(&pool, created.id).await;

    let vertrag = pg::fetch_vertrag(&pool, created.id, tenant)
        .await
        .expect("fetch")
        .expect("exists");
    let lieferende = time::macros::date!(2027 - 03 - 31);
    let input = pg::KuendigungInput {
        lieferende,
        grund: vertragd::domain::Kuendigungsgrund::Ordentlich,
        preisanpassung_wirksam_zum: None,
        eingang: None,
        bemerkung: None,
    };

    let mut tx = pool.begin().await.expect("begin");
    let result = pg::kuendige_vertrag(
        &mut tx,
        &vertrag,
        &input,
        time::macros::date!(2026 - 10 - 01),
        tenant,
    )
    .await
    .expect("kuendigen");
    pg::mark_kuendigung_bestaetigt(&mut *tx, created.id, tenant)
        .await
        .expect("confirm");
    tx.commit().await.expect("commit");

    assert_eq!(
        result.dispatched.len(),
        1,
        "the STROM component is dispatched"
    );

    // Supply does not end today. The customer is supplied until 2027-03-31 and
    // has to be invoiced for it, so the component stays billable until then —
    // marking it BEENDET at once took the remaining months and the
    // Schlussrechnung out of the § 40b feed.
    let komp: (String, Option<time::Date>) =
        sqlx::query_as("SELECT status, lieferende FROM vertragskomponenten WHERE vertrag_id=$1")
            .bind(created.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(komp.0, "AKTIV", "supply runs until the Lieferende");
    assert_eq!(komp.1, Some(lieferende), "and the date it ends is recorded");
    assert_eq!(
        pg::list_billing_candidates(&pool, tenant)
            .await
            .expect("candidates")
            .len(),
        1,
        "a terminated contract stays billable until supply actually ends"
    );

    let after = pg::fetch_vertrag(&pool, created.id, tenant)
        .await
        .expect("refetch")
        .expect("exists");
    assert_eq!(after.status, "GEKÜNDIGT");
    assert_eq!(
        after.vertragsende,
        Some(lieferende),
        "the contract now ends when supply does"
    );
    assert_eq!(after.kuendigung_zum, Some(lieferende));
    assert!(
        !after.auto_renewal,
        "a terminated contract must not renew itself while its notice runs out"
    );
    assert!(
        after.kuendigungsbestaetigung_am.is_some(),
        "§ 41 Abs. 8 Nr. 2 EnWG: the Textform confirmation is recorded as owed"
    );

    let kinds: Vec<String> =
        sqlx::query_scalar("SELECT kind FROM outbound_tasks WHERE tenant=$1 ORDER BY kind")
            .bind(tenant)
            .fetch_all(&pool)
            .await
            .expect("tasks");
    assert!(
        kinds.contains(&"LIEFERENDE".to_owned()),
        "the NB is told supply ends: {kinds:?}"
    );
    assert!(
        kinds.contains(&"ABLESUNG_ENDE".to_owned()),
        "the Schlussablesung is ordered independently of the UTILMD: {kinds:?}"
    );
}

/// Creating a contract enqueues exactly one Lieferbeginn, and re-posting the
/// same `erp_contract_id` enqueues none — the unique `dedupe_key` is the guard,
/// so it only holds against a real database.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_registration_is_enqueued_once_however_often_the_contract_is_posted() {
    let Some((pool, _pg)) = test_pool("dispatch_dedupe").await else {
        return;
    };
    let tenant = "9800000000011";
    let kunde = make_kunde(&pool, tenant).await;
    let input = vertrag_input("D-1");

    let first = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("create");
    assert_eq!(first.dispatched, 1);
    let second = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("replay");
    assert_eq!(second.dispatched, 0, "a replay registers nothing again");

    let n: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbound_tasks WHERE kind='LIEFERBEGINN' AND tenant=$1",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n, 1, "one queued UTILMD, not two");

    // The contract is now waiting on the NB, which is what the API reports.
    let v = pg::fetch_vertrag(&pool, first.id, tenant)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v.status, "IN_BEARBEITUNG");
}

/// A Stornierung before supply began must also withdraw the registration still
/// waiting in the queue — otherwise the worker registers a contract the
/// customer cancelled.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn stornierung_withdraws_a_registration_that_has_not_left_the_queue() {
    let Some((pool, _pg)) = test_pool("storno_withdraws").await else {
        return;
    };
    let tenant = "9800000000012";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("S-1"))
            .await
            .expect("create");

    pg::storniere_vertrag(&pool, created.id, tenant)
        .await
        .expect("storniere");

    let offen: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbound_tasks
          WHERE tenant=$1 AND kind='LIEFERBEGINN'
            AND completed_at IS NULL AND dead_lettered_at IS NULL",
    )
    .bind(tenant)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(offen, 0, "nothing is left to register");
}

/// Once the Lieferende has passed, supply ends and the contract closes.
/// Without the transition a terminated contract sits in GEKÜNDIGT for ever
/// with components nominally still in supply.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn supply_that_has_run_out_closes_its_contract() {
    let Some((pool, _pg)) = test_pool("close_due").await else {
        return;
    };
    let tenant = "9800000000016";
    let kunde = make_kunde(&pool, tenant).await;
    // Supply that actually started: the schema refuses a Lieferende before the
    // Lieferbeginn, which is the point of the constraint.
    let mut input = vertrag_input("C-1");
    input.vertragsbeginn = time::macros::date!(2020 - 01 - 01);
    input.komponenten[0].lieferbeginn = time::macros::date!(2020 - 01 - 01);
    let created = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("create");
    activate(&pool, created.id).await;

    // A Lieferende still ahead changes nothing.
    sqlx::query("UPDATE vertragskomponenten SET lieferende = heute() + 30 WHERE vertrag_id=$1")
        .bind(created.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        pg::close_due_supply(&pool, tenant)
            .await
            .unwrap()
            .is_empty(),
        "supply that is still running is not closed"
    );

    // A Lieferende in the past ends it.
    sqlx::query("UPDATE vertragskomponenten SET lieferende = heute() - 1 WHERE vertrag_id=$1")
        .bind(created.id)
        .execute(&pool)
        .await
        .unwrap();
    let closed = pg::close_due_supply(&pool, tenant).await.unwrap();
    assert_eq!(closed, vec![created.id]);

    let after = pg::fetch_vertrag(&pool, created.id, tenant)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.status, "ABGELAUFEN");
    assert!(
        after.completed_at.is_some(),
        "the § 147 AO retention clock starts here"
    );
    assert!(
        pg::list_billing_candidates(&pool, tenant)
            .await
            .unwrap()
            .is_empty(),
        "and the component leaves the billing feed"
    );
}

// ── Outbound queue bookkeeping ───────────────────────────────────────────────

/// A failing task is retried on a growing delay and then given up on, and an
/// operator can put it back. All of it is SQL — an interval computed from a
/// bind parameter and a boundary at which retrying stops — so only a real
/// database proves the obligation neither spins for ever nor vanishes.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_failing_task_backs_off_then_dead_letters_then_can_be_requeued() {
    let Some((pool, _pg)) = test_pool("outbound_backoff").await else {
        return;
    };
    let tenant = "9800000000017";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("O-1"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;

    let (id, attempts): (uuid::Uuid, i32) = sqlx::query_as(
        "SELECT id, attempts FROM outbound_tasks WHERE komp_id=$1 AND kind='LIEFERBEGINN'",
    )
    .bind(komp)
    .fetch_one(&pool)
    .await
    .expect("the registration was enqueued");
    assert_eq!(attempts, 0);

    // Two failures: still queued, and not before the backoff has elapsed.
    let mut conn = pool.acquire().await.unwrap();
    vertragd::outbound::record_failure(&mut conn, id, 1, "processd unreachable")
        .await
        .unwrap();
    let due_now: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbound_tasks
          WHERE id=$1 AND next_attempt_at <= now() AND dead_lettered_at IS NULL",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(due_now, 0, "the retry waits");

    let (first, second): (time::OffsetDateTime, time::OffsetDateTime) = {
        let a: time::OffsetDateTime =
            sqlx::query_scalar("SELECT next_attempt_at FROM outbound_tasks WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        vertragd::outbound::record_failure(&mut conn, id, 4, "processd unreachable")
            .await
            .unwrap();
        let b: time::OffsetDateTime =
            sqlx::query_scalar("SELECT next_attempt_at FROM outbound_tasks WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        (a, b)
    };
    assert!(second > first, "the delay grows with the attempt count");

    // Enough failures and it stops being retried and becomes visible instead.
    vertragd::outbound::record_failure(&mut conn, id, 8, "processd unreachable")
        .await
        .unwrap();
    let dead = vertragd::outbound::list_dead_lettered(&pool, tenant, 10)
        .await
        .unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].kind, "LIEFERBEGINN");
    assert_eq!(dead[0].attempts, 8);

    assert!(
        vertragd::outbound::retry_dead_lettered(&pool, tenant, id)
            .await
            .unwrap()
    );
    assert!(
        vertragd::outbound::list_dead_lettered(&pool, tenant, 10)
            .await
            .unwrap()
            .is_empty(),
        "the requeued task is no longer the operator's problem"
    );
    // Another tenant cannot requeue it.
    assert!(
        !vertragd::outbound::retry_dead_lettered(&pool, "9800000000099", id)
            .await
            .unwrap()
    );
}

// ── The valid-time product assignment ────────────────────────────────────────

/// A billing period containing a Tarifwechsel must come back as **two** slices
/// that tile it exactly — that is the whole reason the assignment is temporal.
/// Asking only for the current product billed the entire period at whichever
/// tariff happened to be in force on the day the run executed.
///
/// The slice boundaries are SQL — a `daterange` overlap plus a `GREATEST`/`CASE`
/// clip — so only a real database proves them.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_period_containing_a_tarifwechsel_comes_back_as_two_tiling_slices() {
    let Some((pool, _pg)) = test_pool("produkt_slices").await else {
        return;
    };
    let tenant = "9800000000020";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-1"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;

    // The contract opened on STROM-BASIS-2026 from its Lieferbeginn.
    let mut conn = pool.acquire().await.unwrap();
    pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-NEU",
        time::macros::date!(2026 - 11 - 15),
        Some("Tarifwechsel"),
        Initiator::Lieferant,
        false,
        None,
    )
    .await
    .expect("tarifwechsel");

    let slices = pg::malo_slices(
        &pool,
        tenant,
        MALO,
        time::macros::date!(2026 - 11 - 01),
        time::macros::date!(2026 - 11 - 30),
    )
    .await
    .expect("slices");

    assert_eq!(slices.len(), 2, "the switch splits the period");
    assert_eq!(slices[0].product_code, "STROM-BASIS-2026");
    assert_eq!(slices[0].gueltig_von, time::macros::date!(2026 - 11 - 01));
    assert_eq!(
        slices[0].gueltig_bis,
        Some(time::macros::date!(2026 - 11 - 15)),
        "exclusive end — the 15th belongs to the new product, not to both"
    );
    assert_eq!(slices[1].product_code, "STROM-NEU");
    assert_eq!(slices[1].gueltig_von, time::macros::date!(2026 - 11 - 15));

    // The slices tile the period exactly: no day billed twice, none unpriced.
    let covered: i64 = slices
        .iter()
        .map(|s| (s.gueltig_bis.unwrap() - s.gueltig_von).whole_days())
        .sum();
    assert_eq!(covered, 30, "November has 30 days, each covered once");
}

/// A future-dated Tarifwechsel is simply a slice that starts in the future —
/// there is no pending state and nothing applies it on the day.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_future_tarifwechsel_is_invisible_until_it_starts() {
    let Some((pool, _pg)) = test_pool("produkt_future").await else {
        return;
    };
    let tenant = "9800000000021";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-2"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;

    let mut conn = pool.acquire().await.unwrap();
    pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-NEU",
        time::macros::date!(2027 - 01 - 01),
        None,
        Initiator::Lieferant,
        false,
        None,
    )
    .await
    .expect("schedule");

    let vorher = pg::produkte::produkt_am(&pool, komp, time::macros::date!(2026 - 12 - 31))
        .await
        .unwrap()
        .expect("a product is in force");
    assert_eq!(vorher.product_code, "STROM-BASIS-2026");
    let nachher = pg::produkte::produkt_am(&pool, komp, time::macros::date!(2027 - 01 - 01))
        .await
        .unwrap()
        .expect("a product is in force");
    assert_eq!(nachher.product_code, "STROM-NEU");

    // And its § 41 Abs. 5 notice is owed, with the previous product named.
    let offen = pg::offene_preisanpassungen(&pool, tenant, time::macros::date!(2026 - 10 - 01))
        .await
        .expect("query");
    assert_eq!(offen.len(), 1);
    assert_eq!(offen[0].neues_produkt, "STROM-NEU");
    assert_eq!(
        offen[0].bisheriges_produkt.as_deref(),
        Some("STROM-BASIS-2026")
    );
    assert!(
        offen[0].haushaltskunde,
        "the query carries the § 3 Nr. 57 fact the notice period depends on"
    );
}

/// The database itself must refuse two products for one component on one day.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn overlapping_product_slices_are_unrepresentable() {
    let Some((pool, _pg)) = test_pool("produkt_overlap").await else {
        return;
    };
    let tenant = "9800000000022";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-3"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;

    let clash = sqlx::query(
        "INSERT INTO komponenten_produkte (tenant, komp_id, product_code, gueltig_von, gueltig_bis)
         VALUES ($1, $2, 'STROM-X', DATE '2026-11-01', DATE '2027-01-01')",
    )
    .bind(tenant)
    .bind(komp)
    .execute(&pool)
    .await;
    assert!(
        clash.is_err(),
        "kp_no_overlap must refuse a slice overlapping the initial one"
    );

    // Abutting is not overlapping: the half-open range makes the end date the
    // first day of the next slice.
    let ok = sqlx::query(
        "UPDATE komponenten_produkte SET gueltig_bis = DATE '2027-01-01' WHERE komp_id = $1",
    )
    .bind(komp)
    .execute(&pool)
    .await;
    assert!(ok.is_ok());
    sqlx::query(
        "INSERT INTO komponenten_produkte (tenant, komp_id, product_code, gueltig_von)
         VALUES ($1, $2, 'STROM-X', DATE '2027-01-01')",
    )
    .bind(tenant)
    .bind(komp)
    .execute(&pool)
    .await
    .expect("abutting slices are fine");
}

/// A change dated behind a later one would silently reprice a period the
/// operator already decided about — and, for an announced change, one the
/// customer has been told about.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_replay_is_idempotent_and_a_backdated_change_is_refused() {
    let Some((pool, _pg)) = test_pool("produkt_replay").await else {
        return;
    };
    let tenant = "9800000000023";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-4"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let mut conn = pool.acquire().await.unwrap();

    let wechsel = |c: &str, d: time::Date| (c.to_owned(), d);
    for (code, d) in [
        wechsel("STROM-NEU", time::macros::date!(2027 - 01 - 01)),
        wechsel("STROM-NEU", time::macros::date!(2027 - 01 - 01)),
    ] {
        pg::produkte::tarifwechsel(
            &mut conn,
            tenant,
            komp,
            &code,
            d,
            None,
            Initiator::Lieferant,
            false,
            None,
        )
        .await
        .expect("a replay of the same change must succeed");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM komponenten_produkte WHERE komp_id = $1")
        .bind(komp)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 2, "the initial slice plus one change — not two changes");

    let backdated = pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-ALT",
        time::macros::date!(2026 - 12 - 01),
        None,
        Initiator::Lieferant,
        true,
        None,
    )
    .await;
    assert!(
        backdated.is_err(),
        "backdating behind a later change would reprice a decided period"
    );
}

// ── § 41 Abs. 5 EnWG — the notice, and who owes it ───────────────────────────

/// A deployment that renders the Preisänderungsanzeige itself, so the price
/// lines are the document's content.
fn worker_config_rendernd(tenant: &str) -> vertragd::config::VertragdConfig {
    let mut cfg = worker_config(tenant);
    cfg.outputd_url = Some("http://outputd.invalid".to_owned());
    cfg
}

/// A deployment with no `outputd`, so the CloudEvent is the notice.
fn worker_config(tenant: &str) -> vertragd::config::VertragdConfig {
    serde_json::from_value(serde_json::json!({
        "database": { "url": "postgres://unused-by-the-worker" },
        "tenant": tenant,
        "lf_mp_id": tenant,
        "processd_url": "http://processd.invalid",
        "accountingd_url": "http://accountingd.invalid",
        "edmd_url": "http://edmd.invalid",
        "allow_insecure_no_auth": true,
    }))
    .expect("worker config")
}

/// The announced price lines a valid notice states (§ 41 Abs. 5 Satz 3 EnWG).
fn umfang() -> serde_json::Value {
    serde_json::json!([{
        "bezeichnung": "Arbeitspreis",
        "einheit": "ct/kWh",
        "bisher": "31.20",
        "neu": "34.90",
    }])
}

async fn schedule_slice(
    pool: &PgPool,
    tenant: &str,
    komp: Uuid,
    ab: time::Date,
    initiator: Initiator,
    preise: Option<serde_json::Value>,
) {
    let mut conn = pool.acquire().await.unwrap();
    pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-NEU",
        ab,
        Some("Anpassung der Beschaffungskosten"),
        initiator,
        false,
        preise.as_ref(),
    )
    .await
    .expect("schedule");
}

async fn angekuendigt(pool: &PgPool, komp: Uuid) -> Vec<(bool, i32, Option<String>)> {
    sqlx::query_as::<_, (bool, i32, Option<String>)>(
        "SELECT preisanpassung_notif_sent, notif_versuche, notif_letzter_fehler
           FROM komponenten_produkte WHERE komp_id = $1 AND grund <> 'Vertragsschluss'",
    )
    .bind(komp)
    .fetch_all(pool)
    .await
    .expect("slice state")
}

async fn anzeigen(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM event_outbox WHERE ce_type = $1")
        .bind(mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG)
        .fetch_one(pool)
        .await
        .expect("count")
}

/// A slice whose notice cannot be made valid must not be recorded as notified.
///
/// This deployment renders the document, so the price lines are its content and
/// a slice scheduled without them cannot be announced. The change takes effect
/// on its Wirksamkeit whatever the worker did, so marking it sent before a
/// notice exists leaves the customer with a higher price, no
/// Preisänderungsanzeige, no § 41 Abs. 5 Satz 4 Sonderkündigungsrecht — and
/// nothing anywhere saying so. The attempt is recorded on the slice and the
/// sweep keeps owing it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_notice_that_cannot_be_rendered_is_never_recorded_as_sent() {
    let Some((pool, _pg)) = test_pool("notice_failure").await else {
        return;
    };
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    let tenant = "9800000000030";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-41-1"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let wirksam = mako_fristen::heute() + time::Duration::days(90);

    // Scheduled without the Umfang the rendered notice has to state.
    schedule_slice(&pool, tenant, komp, wirksam, Initiator::Lieferant, None).await;

    let cfg = worker_config_rendernd(tenant);
    vertragd::workers::preisanpassung(&pool, &cfg)
        .await
        .expect("the sweep survives a notice it cannot issue");

    let state = angekuendigt(&pool, komp).await;
    assert_eq!(state.len(), 1);
    let (sent, versuche, fehler) = &state[0];
    assert!(
        !sent,
        "no notice exists, so the slice must still owe one — this is the flag that decides \
         whether the customer is ever told"
    );
    assert_eq!(*versuche, 1, "the failed attempt is counted");
    assert!(
        fehler.as_deref().is_some_and(|f| f.contains("§ 41 Abs. 5")),
        "the recorded reason names what is missing, got {fehler:?}"
    );
    assert_eq!(
        anzeigen(&pool).await,
        0,
        "an announcement that states no Umfang is not a Preisänderungsanzeige and is not sent"
    );

    // And the sweep keeps owing it: a second run tries again.
    vertragd::workers::preisanpassung(&pool, &cfg)
        .await
        .expect("second sweep");
    assert_eq!(
        angekuendigt(&pool, komp).await[0].1,
        2,
        "retried, not dropped"
    );
}

/// Where the CloudEvent is the notice, the change is announced without price
/// lines — and the event says the Umfang is missing from it.
///
/// § 41 Abs. 5 Satz 3 EnWG is about what the *customer* is told, and the ERP
/// composing the letter holds the price sheets it is told from. Refusing the
/// change here would break that integration without any customer learning more.
/// `umfang_vollstaendig` is what keeps the absence from reading as "nothing
/// changed": a letter that states no Umfang is not a valid notice on any
/// channel, and the composer is the one who can still fix that.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_cloudevent_notice_is_issued_without_price_lines() {
    let Some((pool, _pg)) = test_pool("notice_erp_umfang").await else {
        return;
    };
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    let tenant = "9800000000038";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-41-8"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let wirksam = mako_fristen::heute() + time::Duration::days(90);
    schedule_slice(&pool, tenant, komp, wirksam, Initiator::Lieferant, None).await;

    vertragd::workers::preisanpassung(&pool, &worker_config(tenant))
        .await
        .expect("sweep");

    assert!(
        angekuendigt(&pool, komp).await[0].0,
        "the notice went out — the ERP composes the letter"
    );
    assert_eq!(anzeigen(&pool).await, 1);
    let envelope: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM event_outbox WHERE ce_type = $1")
            .bind(mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG)
            .fetch_one(&pool)
            .await
            .expect("envelope");
    let data = envelope.pointer("/data").expect("data");
    assert_eq!(
        data.pointer("/umfang_vollstaendig"),
        Some(&serde_json::json!(false)),
        "the event says this service states no Umfang, so the composer must"
    );
    assert_eq!(
        data.pointer("/sonderkuendigungsrecht/besteht"),
        Some(&serde_json::json!(true)),
        "the § 41 Abs. 5 Satz 4 termination right is stated either way"
    );
}

/// With the Umfang stated, the notice goes out — once — and the slice is then
/// marked, in the same transaction that enqueued it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_issuable_notice_is_sent_once_and_then_marked() {
    let Some((pool, _pg)) = test_pool("notice_sent").await else {
        return;
    };
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    let tenant = "9800000000031";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-41-2"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let wirksam = mako_fristen::heute() + time::Duration::days(90);
    schedule_slice(
        &pool,
        tenant,
        komp,
        wirksam,
        Initiator::Lieferant,
        Some(umfang()),
    )
    .await;

    let cfg = worker_config(tenant);
    vertragd::workers::preisanpassung(&pool, &cfg)
        .await
        .expect("sweep");

    assert!(angekuendigt(&pool, komp).await[0].0, "the notice went out");
    assert_eq!(anzeigen(&pool).await, 1);

    // The § 41 Abs. 5 Satz 4 termination right is stated in the notice itself.
    let envelope: serde_json::Value =
        sqlx::query_scalar("SELECT envelope FROM event_outbox WHERE ce_type = $1")
            .bind(mako_events::vertrag::PREISAENDERUNG_ANKUENDIGUNG)
            .fetch_one(&pool)
            .await
            .expect("envelope");
    let data = envelope.pointer("/data").expect("data");
    assert_eq!(
        data.pointer("/sonderkuendigungsrecht/besteht"),
        Some(&serde_json::json!(true))
    );
    assert!(
        data.pointer("/umfang/0/neu").is_some(),
        "the notice carries the Umfang, so a recipient composing its own letter has it"
    );

    // A second run announces nothing: the obligation is settled.
    vertragd::workers::preisanpassung(&pool, &cfg)
        .await
        .expect("second sweep");
    assert_eq!(anzeigen(&pool).await, 1, "one change, one announcement");
}

/// A tariff the customer asked for is not announced as one the supplier
/// imposed.
///
/// § 41 Abs. 5 Satz 1 EnWG binds a supplier exercising a reserved right to
/// change the contract, and Satz 4 gives the termination right *because* the
/// supplier exercised it. Announcing a customer's own switch tells them they
/// may cancel when the law gives them no such right.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_customer_initiated_switch_is_not_announced() {
    let Some((pool, _pg)) = test_pool("notice_initiator").await else {
        return;
    };
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    let tenant = "9800000000032";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-41-3"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let wirksam = mako_fristen::heute() + time::Duration::days(90);
    schedule_slice(&pool, tenant, komp, wirksam, Initiator::Kunde, None).await;

    let heute = mako_fristen::heute();
    assert!(
        pg::offene_preisanpassungen(&pool, tenant, heute)
            .await
            .expect("query")
            .is_empty(),
        "a switch the customer asked for owes no § 41 Abs. 5 notice, whatever the flag says"
    );

    vertragd::workers::preisanpassung(&pool, &worker_config(tenant))
        .await
        .expect("sweep");
    assert_eq!(
        anzeigen(&pool).await,
        0,
        "no Preisänderungsanzeige, and so no Sonderkündigungsrecht the customer does not have"
    );
}

/// A pending § 41 Abs. 5 notice cannot be cancelled by re-POSTing the same
/// change under a different initiator.
///
/// The Tarifwechsel write is idempotent on `(komp_id, gueltig_von)`, so a
/// replay updates the slice in place. Letting it also rewrite `initiator` and
/// `preisanpassung_notif_sent` made the obligation disposable: a supplier price
/// rise with its notice still owed, re-sent as `KUNDE`, left the announcement
/// queue, the customer was never told, and the breach report came back clean
/// because the record — not the fact — had changed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_replay_cannot_cancel_a_pending_price_change_notice() {
    let Some((pool, _pg)) = test_pool("notice_initiator_flip").await else {
        return;
    };
    let tenant = "9800000000039";
    let kunde = make_kunde(&pool, tenant).await;
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("P-41-9"))
            .await
            .expect("create");
    let komp = created.komponenten[0].id;
    let heute = mako_fristen::heute();
    let wirksam = heute + time::Duration::days(90);
    schedule_slice(
        &pool,
        tenant,
        komp,
        wirksam,
        Initiator::Lieferant,
        Some(umfang()),
    )
    .await;
    assert_eq!(
        pg::offene_preisanpassungen(&pool, tenant, heute)
            .await
            .expect("query")
            .len(),
        1,
        "the supplier's price rise owes a notice"
    );

    let mut conn = pool.acquire().await.unwrap();
    let umgeschrieben = pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-NEU",
        wirksam,
        None,
        Initiator::Kunde,
        true,
        None,
    )
    .await;
    assert!(
        umgeschrieben.is_err(),
        "a pending Preisänderungsanzeige must not be cancelled by relabelling the change"
    );

    let offen = pg::offene_preisanpassungen(&pool, tenant, heute)
        .await
        .expect("query");
    assert_eq!(offen.len(), 1, "the notice is still owed");
    assert!(
        offen[0].angekuendigte_preise.is_some(),
        "and it still states the Umfang it was scheduled with"
    );

    // The ordinary replay — same initiator, a corrected product — still works,
    // and the obligation stays open.
    pg::produkte::tarifwechsel(
        &mut conn,
        tenant,
        komp,
        "STROM-NEU-KORRIGIERT",
        wirksam,
        None,
        Initiator::Lieferant,
        false,
        None,
    )
    .await
    .expect("a replay that keeps the initiator is the idempotent case");
    let offen = pg::offene_preisanpassungen(&pool, tenant, heute)
        .await
        .expect("query");
    assert_eq!(offen.len(), 1);
    assert_eq!(offen[0].neues_produkt, "STROM-NEU-KORRIGIERT");
}

/// A supplier-initiated change already in force whose notice never went out is
/// a breach, and the sweep has to be able to find it — the announcement query
/// only ever looks forward.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_unannounced_change_already_in_force_is_reported() {
    let Some((pool, _pg)) = test_pool("notice_breach").await else {
        return;
    };
    let tenant = "9800000000033";
    let kunde = make_kunde(&pool, tenant).await;
    let heute = mako_fristen::heute();
    // Supply that started before today, so a change can already be in force.
    let mut input = vertrag_input("P-41-4");
    let start = heute - time::Duration::days(60);
    input.vertragsbeginn = start;
    input.komponenten[0].lieferbeginn = start;
    let created = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("create");
    let komp = created.komponenten[0].id;

    // Written around the API, which refuses to schedule this.
    schedule_slice(
        &pool,
        tenant,
        komp,
        heute - time::Duration::days(1),
        Initiator::Lieferant,
        None,
    )
    .await;

    let breach = pg::unangekuendigt_wirksame(&pool, tenant, heute)
        .await
        .expect("query");
    assert_eq!(breach.len(), 1, "the price changed and nobody was told");
    assert_eq!(breach[0].komp_id, komp);
    assert!(
        pg::offene_preisanpassungen(&pool, tenant, heute)
            .await
            .expect("query")
            .is_empty(),
        "it can no longer be announced — its Wirksamkeit has passed"
    );
}

// ── DSGVO Art. 17 ─────────────────────────────────────────────────────────────

/// Erasure is refused while supply runs, and succeeds once it has ended — and
/// it must survive a customer with more than one portal login, which is every
/// B2B customer. Writing one pseudonym into both identity rows violated
/// `UNIQUE (tenant, oidc_sub)` and failed the whole request.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn erasure_waits_for_the_contract_to_end_and_survives_several_logins() {
    let Some((pool, _pg)) = test_pool("erasure").await else {
        return;
    };
    let tenant = "9800000000013";
    let kunde = make_kunde(&pool, tenant).await;
    for sub in ["auth0|ceo", "auth0|buchhaltung"] {
        pg::upsert_identitaet(
            &pool,
            kunde,
            tenant,
            &pg::UpsertIdentitaetInput {
                oidc_sub: sub.to_owned(),
                email: Some(format!("{sub}@example.test")),
                display_name: None,
                rolle: None,
                standort_filter: None,
            },
        )
        .await
        .expect("identity");
    }
    let created =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("G-1"))
            .await
            .expect("create");
    activate(&pool, created.id).await;

    let refused = pg::anonymize_kunde(&pool, kunde, tenant, "dpo", None, false)
        .await
        .expect("query");
    match refused {
        vertragd::pg::gdpr::ErasureOutcome::Refused {
            laufende_vertraege, ..
        } => assert_eq!(laufende_vertraege.len(), 1, "the running contract is named"),
        other => panic!("erasure must wait for the contract to end, got {other:?}"),
    }

    sqlx::query("UPDATE versorgungsvertraege SET status='ABGELAUFEN' WHERE id=$1")
        .bind(created.id)
        .execute(&pool)
        .await
        .unwrap();

    let done = pg::anonymize_kunde(&pool, kunde, tenant, "dpo", None, false)
        .await
        .expect("erase");
    assert!(matches!(
        done,
        vertragd::pg::gdpr::ErasureOutcome::Anonymized { .. }
    ));

    let subs: Vec<String> =
        sqlx::query_scalar("SELECT oidc_sub FROM kunden_identitaeten WHERE kunden_id=$1")
            .bind(kunde)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(subs.len(), 2);
    assert!(subs.iter().all(|s| s.starts_with("anon:")));
    assert_ne!(subs[0], subs[1], "each login gets its own pseudonym");

    let emails: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM kunden_identitaeten WHERE kunden_id=$1 AND email IS NOT NULL",
    )
    .bind(kunde)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(emails, 0);

    // The contract row survives for § 147 Abs. 3 AO, without its personal data.
    let (adresse, count): (Option<serde_json::Value>, i64) = sqlx::query_as(
        "SELECT standort_adresse, count(*) OVER () FROM versorgungsvertraege WHERE kunden_id=$1",
    )
    .bind(kunde)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "the commercial record is retained");
    assert!(adresse.is_none(), "the supply address is personal data too");
}

// ── Sammelrechnung site enumeration ──────────────────────────────────────────

/// Every site of a Rahmenvertrag must report the product **that site** is on.
/// Reading the contract's `bundle_code` here gave billingd a bundle name where
/// it needed a tariff, and gave every site of the framework the same one.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn each_sammelrechnung_site_reports_its_own_product() {
    let Some((pool, _pg)) = test_pool("sammelrechnung").await else {
        return;
    };
    let tenant = "9800000000014";
    let kunde = make_kunde(&pool, tenant).await;
    let rahmen = pg::insert_rahmenvertrag(
        &pool,
        kunde,
        tenant,
        &pg::CreateRahmenvertragInput {
            gueltig_von: time::macros::date!(2026 - 01 - 01),
            gueltig_bis: None,
            kuendigungsfrist_monate: None,
            auto_renewal: None,
            renewal_monate: None,
            preisanpassungsformel: None,
            portfolio_rabatt_prozent: None,
            rechnungsstellung: Some("SAMMEL".to_owned()),
            sammelrechnung_intervall: None,
            erp_rahmenvertrag_id: Some("RV-1".to_owned()),
            angebot_id: None,
            notizen: None,
        },
    )
    .await
    .expect("rahmenvertrag");

    for (i, (malo, produkt)) in [
        (MALO, "STROM-NORD-2026"),
        ("51238696782", "STROM-SUED-2026"),
    ]
    .into_iter()
    .enumerate()
    {
        let mut input = vertrag_input(&format!("RV-SITE-{i}"));
        input.rahmenvertrag_id = Some(rahmen);
        input.bundle_code = Some("KONZERN-BUNDLE".to_owned());
        input.standort_bezeichnung = Some(format!("Werk {i}"));
        input.komponenten[0].malo_id = Some(malo.to_owned());
        input.komponenten[0].product_code = produkt.to_owned();
        let created = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
            .await
            .expect("site");
        activate(&pool, created.id).await;
    }

    // The sites' supply starts in the future, so no product is in force today —
    // and they must still be enumerated, because a bundle that silently omits a
    // site under-bills it.
    let sites = pg::list_rahmenvertrag_malos(&pool, rahmen, tenant)
        .await
        .expect("sites");
    assert_eq!(
        sites.len(),
        2,
        "every site of the framework contract appears"
    );
    assert!(
        sites.iter().all(|s| s.product_code.is_none()),
        "supply has not started, so no product is in force today"
    );

    // Once supply is running, each site reports its own product — reading the
    // contract's `bundle_code` here named a bundle where billing needed a
    // tariff, and gave every site of the framework the same one.
    sqlx::query("UPDATE komponenten_produkte SET gueltig_von = heute() - 1")
        .execute(&pool)
        .await
        .unwrap();
    let mut sites = pg::list_rahmenvertrag_malos(&pool, rahmen, tenant)
        .await
        .expect("sites");
    sites.sort_by(|a, b| a.product_code.cmp(&b.product_code));
    assert_eq!(sites[0].product_code.as_deref(), Some("STROM-NORD-2026"));
    assert_eq!(sites[1].product_code.as_deref(), Some("STROM-SUED-2026"));
    assert!(
        sites
            .iter()
            .all(|s| s.product_code.as_deref() != Some("KONZERN-BUNDLE")),
        "the bundle name is not a tariff"
    );
}

// ── Auto-renewal (§ 309 Nr. 9 lit. b BGB) ────────────────────────────────────

/// A consumer contract that renews must end up **unbefristet** with at most a
/// month's notice. Extending it by another fixed term is the clause § 309 Nr. 9
/// lit. b BGB forbids, and it is the customer who discovers that.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_consumer_contract_renews_into_an_open_ended_one() {
    let Some((pool, _pg)) = test_pool("renewal").await else {
        return;
    };
    let tenant = "9800000000015";
    let kunde = make_kunde(&pool, tenant).await;
    let mut input = vertrag_input("R-1");
    input.vertragsende = Some(time::macros::date!(2026 - 10 - 31));
    input.auto_renewal = Some(true);
    input.renewal_monate = Some(12);
    input.kuendigungsfrist_monate = Some(3);
    let created = pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &input)
        .await
        .expect("create");
    activate(&pool, created.id).await;

    let today = time::macros::date!(2026 - 11 - 01);
    let overdue = pg::find_auto_renewal_overdue(&pool, tenant, today)
        .await
        .expect("overdue");
    assert_eq!(overdue.len(), 1);
    assert!(
        overdue[0].haushaltskunde,
        "the query carries the § 3 Nr. 57 fact"
    );

    let neu = vertragd::domain::verlaengerung(
        overdue[0].haushaltskunde,
        overdue[0].vertragsende,
        overdue[0].renewal_monate,
        today,
    );
    pg::apply_auto_renewal(&pool, created.id, neu)
        .await
        .expect("renew");

    let after = pg::fetch_vertrag(&pool, created.id, tenant)
        .await
        .unwrap()
        .unwrap();
    assert!(
        after.vertragsende.is_none(),
        "the term is gone, not extended"
    );
    assert_eq!(after.kuendigungsfrist_monate, 1, "§ 309 Nr. 9 lit. b BGB");
    assert!(
        !after.auto_renewal,
        "an open-ended contract has nothing left to renew"
    );
}

/// the container (testcontainers cleans up on `Drop`; no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

// ── Messstellenverträge (§ 9, § 10 MsbG) ─────────────────────────────────────

/// The WiM Kündigung MSB is answered from this row, so two simultaneously
/// active contracts for the same MSB and Messlokation are not representable:
/// the answer would depend on row order.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_second_overlapping_messstellenvertrag_is_refused() {
    use vertragd::pg::{UpsertMessstellenvertragInput, upsert_messstellenvertrag};

    let Some((pool, _pg)) = test_pool("msv_overlap").await else {
        return;
    };
    let tenant = "9900357000004";
    let melo = "DE0000000001234567890000000000001";
    let msb = "9900000000003";

    upsert_messstellenvertrag(
        &pool,
        tenant,
        melo,
        msb,
        &UpsertMessstellenvertragInput {
            vertragsbeginn: time::macros::date!(2024 - 01 - 01),
            kuendigungsfrist_monate: 1,
            kunden_id: None,
            kuendigung_zum: None,
            kuendigung_eingang: None,
            frueher_moeglich: None,
            beendet_am: None,
        },
    )
    .await
    .expect("first contract");

    // A second MSB at the same Messlokation is lawful — an MSB-Wechsel is
    // exactly that — as long as the terms do not overlap.
    let other = upsert_messstellenvertrag(
        &pool,
        tenant,
        melo,
        "4012345000023",
        &UpsertMessstellenvertragInput {
            vertragsbeginn: time::macros::date!(2024 - 01 - 01),
            kuendigungsfrist_monate: 1,
            kunden_id: None,
            kuendigung_zum: None,
            kuendigung_eingang: None,
            frueher_moeglich: None,
            beendet_am: None,
        },
    )
    .await;
    assert!(other.is_ok(), "a different MSB is a different contract");

    // A direct INSERT of a second open term for the *same* MSB is what the
    // exclusion constraint refuses; the repository upserts instead.
    let clash = sqlx::query(
        r"INSERT INTO messstellenvertraege
              (tenant, melo_id, msb_mp_id, vertragsbeginn, kuendigungsfrist_monate)
          VALUES ($1, $2, $3, $4, 1)",
    )
    .bind(tenant)
    .bind(melo)
    .bind(msb)
    .bind(time::macros::date!(2025 - 06 - 01))
    .execute(&pool)
    .await;
    assert!(
        clash.is_err(),
        "msv_no_overlap must refuse a second open term for the same MSB"
    );
}

/// `E_0200` needs three distinct readings, and the store must keep them apart:
/// no row (`ZC9`), a live contract with its next admissible date (`E15`/`Z12`),
/// and one already terminated (`Z34`).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_contract_round_trips_the_three_e0200_readings() {
    use vertragd::pg::{
        UpsertMessstellenvertragInput, find_messstellenvertrag, record_kuendigung,
        upsert_messstellenvertrag,
    };

    let Some((pool, _pg)) = test_pool("msv_readings").await else {
        return;
    };
    let tenant = "9900357000004";
    let melo = "DE0000000001234567890000000000002";
    let msb = "9900000000003";

    assert!(
        find_messstellenvertrag(&pool, tenant, melo, msb)
            .await
            .expect("lookup")
            .is_none(),
        "no contract is the ZC9 case, not an error"
    );

    upsert_messstellenvertrag(
        &pool,
        tenant,
        melo,
        msb,
        &UpsertMessstellenvertragInput {
            vertragsbeginn: time::macros::date!(2024 - 01 - 01),
            kuendigungsfrist_monate: 3,
            kunden_id: None,
            kuendigung_zum: None,
            kuendigung_eingang: None,
            frueher_moeglich: None,
            beendet_am: None,
        },
    )
    .await
    .expect("upsert");

    let live = find_messstellenvertrag(&pool, tenant, melo, msb)
        .await
        .expect("lookup")
        .expect("row");
    let stichtag = time::macros::date!(2026 - 03 - 15);
    assert_eq!(
        live.naechstmoeglich(stichtag, false),
        Some(time::macros::date!(2026 - 06 - 15)),
        "a business customer keeps the contractual three months"
    );
    assert_eq!(
        live.naechstmoeglich(stichtag, true),
        Some(time::macros::date!(2026 - 04 - 15)),
        "§ 309 Nr. 9 lit. c BGB caps a consumer at one month"
    );

    assert!(
        record_kuendigung(
            &pool,
            tenant,
            melo,
            msb,
            time::macros::date!(2026 - 03 - 15),
            time::macros::date!(2026 - 06 - 15),
        )
        .await
        .expect("record")
    );
    let gekuendigt = find_messstellenvertrag(&pool, tenant, melo, msb)
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(
        gekuendigt.kuendigung_zum,
        Some(time::macros::date!(2026 - 06 - 15))
    );
    assert_eq!(
        gekuendigt.naechstmoeglich(stichtag, false),
        None,
        "a terminated contract has no next date — E_0200 answers Z34"
    );
}

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

// ── Leader election for the daily lifecycle workers ──────────────────────────

/// Two replicas must not both run a lifecycle worker in the same cycle.
///
/// Before this, `spawn_all` spawned three unguarded loops: every replica ran
/// every worker every 23 hours. Per-contract idempotency serialises repeats of
/// the same run, but two instances that read the same unmarked slice in the same
/// second both build a § 41 Abs. 5 EnWG Preisanpassungsanzeige and both enqueue
/// it — a doubled statutory notice to the customer, and a doubled outbound task
/// behind it.
///
/// The lock is session-level, so this test needs two *connections*: it takes one
/// from the pool exactly as a second replica would.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn only_one_replica_runs_a_lifecycle_worker() {
    let Some((pool, _pg)) = test_pool("worker_lock").await else {
        return;
    };

    for worker in vertragd::workers::Worker::all() {
        let key = worker.lock_key();
        let mut first = pg::try_worker_lock(&pool, key)
            .await
            .unwrap_or_else(|| panic!("{} must win an uncontended lock", worker.name()));

        // The second replica does not get it, and does not block waiting either.
        assert!(
            pg::try_worker_lock(&pool, key).await.is_none(),
            "{}: a second replica must skip the cycle, not run it too",
            worker.name()
        );

        // …and once the holder is done the lock is free again, so the next
        // cycle is not locked out until a pod restarts.
        pg::release_worker_lock(&mut first, key).await;
        drop(first);
        let mut again = pg::try_worker_lock(&pool, key)
            .await
            .unwrap_or_else(|| panic!("{}: the lock must be reusable", worker.name()));
        pg::release_worker_lock(&mut again, key).await;
    }
}

/// The three workers hold **three different** locks: a slow Preisanpassung run
/// must not stop the Ablauf sweep from closing supply that has run out.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_lifecycle_workers_do_not_contend_with_one_another() {
    let Some((pool, _pg)) = test_pool("worker_lock_distinct").await else {
        return;
    };

    let mut held = Vec::new();
    for worker in vertragd::workers::Worker::all() {
        let key = worker.lock_key();
        let guard = pg::try_worker_lock(&pool, key).await.unwrap_or_else(|| {
            panic!(
                "{} must not contend with a worker already running",
                worker.name()
            )
        });
        held.push((key, guard));
    }
    for (key, mut guard) in held {
        pg::release_worker_lock(&mut guard, key).await;
    }
}

/// `run_once_locked` reports the skip rather than silently doing the work.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_locked_out_worker_reports_the_skip() {
    let Some((pool, _pg)) = test_pool("worker_lock_skip").await else {
        return;
    };
    let worker = vertragd::workers::Worker::Preisanpassung;
    let mut held = pg::try_worker_lock(&pool, worker.lock_key())
        .await
        .expect("hold the lock as the other replica");

    let cfg = worker_config("9900000000001");
    let ran = worker
        .run_once_locked(&pool, &cfg)
        .await
        .expect("a skipped cycle is not an error");
    assert!(
        !ran,
        "the cycle must be skipped while another replica holds it"
    );

    pg::release_worker_lock(&mut held, worker.lock_key()).await;
    drop(held);

    let ran = worker
        .run_once_locked(&pool, &cfg)
        .await
        .expect("the cycle runs once the lock is free");
    assert!(ran, "the lock is released, so this replica runs the cycle");
}

// ── Reading-order dedupe key carries the reading date ────────────────────────

/// A Kündigung withdrawn and re-issued to a different Lieferende must move the
/// Schlussablesung with it.
///
/// The key was `ABLESUNG_ENDE:{komp}` with no date, so the second enqueue hit
/// `ON CONFLICT (tenant, dedupe_key) DO NOTHING` and vanished: `edmd` kept the
/// order for the *withdrawn* date, the reading happened on the wrong day, and
/// the Schlussrechnung was built from it. Nothing logged a failure — the
/// enqueue returned "already queued", which is indistinguishable from success.
///
/// The schema comment on `outbound_tasks.dedupe_key` documented the intended
/// shape (`'ABLESUNG_BEGINN:{komp}:{datum}'`) all along; only the code drifted.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_reading_order_is_keyed_by_its_date() {
    use vertragd::outbound;

    let Some((pool, _pg)) = test_pool("ablesung_key").await else {
        return;
    };
    let tenant = "9900000000001";
    let kunde = make_kunde(&pool, tenant).await;
    let vertrag =
        pg::insert_versorgungsvertrag(&pool, kunde, tenant, tenant, &vertrag_input("ERP-ABL-1"))
            .await
            .expect("create contract");
    let komp = vertrag.komponenten.first().expect("one component").id;

    let first = time::macros::date!(2027 - 03 - 31);
    let second = time::macros::date!(2027 - 06 - 30);

    let mut conn = pool.acquire().await.expect("connection");
    assert!(
        outbound::enqueue_superseding(
            &mut conn,
            tenant,
            &outbound::ablesung(komp, "51238696781", true, first),
        )
        .await
        .expect("first order"),
    );

    // A replay of the *same* date is still one order.
    assert!(
        !outbound::enqueue_superseding(
            &mut conn,
            tenant,
            &outbound::ablesung(komp, "51238696781", true, first),
        )
        .await
        .expect("replay"),
        "a same-date re-enqueue is a replay, not a second reading order"
    );

    // A different date is a different order, and it must be written.
    assert!(
        outbound::enqueue_superseding(
            &mut conn,
            tenant,
            &outbound::ablesung(komp, "51238696781", true, second),
        )
        .await
        .expect("re-issued order"),
        "a re-issued Kündigung must reach edmd with the new Lieferende"
    );

    // …and exactly one order is pending: the superseded one is gone rather than
    // dispatched alongside it.
    let pending: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT dedupe_key, payload FROM outbound_tasks
          WHERE tenant = $1 AND kind = 'ABLESUNG_ENDE' AND komp_id = $2
            AND completed_at IS NULL AND dead_lettered_at IS NULL",
    )
    .bind(tenant)
    .bind(komp)
    .fetch_all(&pool)
    .await
    .expect("read queue");
    assert_eq!(pending.len(), 1, "{pending:?}");
    assert_eq!(
        pending[0].1["geplant_am"].as_str(),
        Some("2027-06-30"),
        "edmd must be told the date that now applies: {pending:?}"
    );
    assert!(
        pending[0].0.ends_with(":2027-06-30"),
        "the key names the date: {}",
        pending[0].0
    );
}
