//! Real-PostgreSQL guards for the contract-lifecycle invariants that live in
//! SQL, not in Rust: idempotent supply-contract creation (no duplicate
//! Lieferbeginn), the Stornierung state guard, and tenant-scoped mutation.
//!
//! ```bash
//! docker run -d --name vertragd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=vertragd -p 55436:5432 postgres:17-alpine
//! export VERTRAGD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55436/vertragd"
//! cargo test -p vertragd --test dispatch_integration -- --include-ignored
//! ```

use sqlx::PgPool;
use uuid::Uuid;
use vertragd::pg;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("VERTRAGD_TEST_DATABASE_URL").ok()?;
    let admin = PgPool::connect(&base).await.ok()?;
    let schema = format!("t_{test_name}");
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
    for stmt in split_statements(SCHEMA) {
        sqlx::query(&stmt)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("schema stmt failed: {e}\n{stmt}"));
    }
    Some(pool)
}

fn split_statements(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_dollar = false;
    for line in sql.lines() {
        if line.matches("$$").count() % 2 == 1 {
            in_dollar = !in_dollar;
        }
        cur.push_str(line);
        cur.push('\n');
        if !in_dollar && line.trim_end().ends_with(';') {
            let s = cur.trim().to_owned();
            if !s.is_empty() && !s.lines().all(|l| l.trim().starts_with("--")) {
                out.push(s);
            }
            cur.clear();
        }
    }
    out
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
#[ignore = "requires VERTRAGD_TEST_DATABASE_URL"]
async fn reposting_same_erp_contract_id_dispatches_no_second_lieferbeginn() {
    let Some(pool) = test_pool("idempotent_create").await else {
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
#[ignore = "requires VERTRAGD_TEST_DATABASE_URL"]
async fn stornierung_is_refused_on_an_active_contract() {
    let Some(pool) = test_pool("storniere_guard").await else {
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
#[ignore = "requires VERTRAGD_TEST_DATABASE_URL"]
async fn update_vertrag_status_is_tenant_scoped() {
    let Some(pool) = test_pool("tenant_scope").await else {
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
