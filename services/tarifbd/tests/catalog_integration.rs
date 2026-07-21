//! Real-PostgreSQL guards for the tarifbd invariants that live in SQL: that a
//! product PUT actually writes the `NOT NULL tenant` column, that reads are
//! tenant-scoped, that a Tarifwechsel is atomic, and that `erp_angebot_id`
//! makes Angebot creation idempotent.
//!
//! ```bash
//! docker run -d --name tarifbd-test -e POSTGRES_PASSWORD=test \
//!     -e POSTGRES_DB=tarifbd -p 55437:5432 postgres:17-alpine
//! export TARIFBD_TEST_DATABASE_URL="postgres://postgres:test@localhost:55437/tarifbd"
//! cargo test -p tarifbd --test catalog_integration -- --include-ignored
//! ```

use sqlx::PgPool;
use tarifbd::pg;

const SCHEMA: &str = include_str!("../migrations/0001_schema.sql");

async fn test_pool(test_name: &str) -> Option<PgPool> {
    let base = std::env::var("TARIFBD_TEST_DATABASE_URL").ok()?;
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

fn strom_product(code: &str) -> pg::ProductUpsertRequest {
    pg::ProductUpsertRequest {
        category: "STROM".to_owned(),
        name: format!("Test {code}"),
        sparte: Some("STROM".to_owned()),
        register_count: Some("Eintarif".to_owned()),
        kundentyp: Some("Haushalt".to_owned()),
        dyn_source: None,
        valid_from: Some("2026-01-01".to_owned()),
        valid_to: None,
        data: serde_json::json!({}),
        bo4e_version: "v202607.0.0".to_owned(),
        product_status: "PUBLISHED".to_owned(),
        energiemix: None,
        oekolabel: None,
    }
}

// ── C1 — the product write path actually writes tenant ────────────────────────

#[tokio::test]
#[ignore = "requires TARIFBD_TEST_DATABASE_URL"]
async fn upsert_product_writes_tenant_and_reads_are_tenant_scoped() {
    let Some(pool) = test_pool("tenant_scope").await else {
        return;
    };
    let tenant_a = "9900000000001";
    let tenant_b = "9900000000002";

    // The whole point of C1: this INSERT used to violate `tenant NOT NULL`
    // because the column was never bound. It must now succeed.
    pg::upsert_product(&pool, tenant_a, tenant_a, "P-1", strom_product("P-1"))
        .await
        .expect("product PUT must write the tenant column");

    let stored: String =
        sqlx::query_scalar("SELECT tenant FROM products WHERE product_code = 'P-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored, tenant_a);

    // Tenant A reads its product; tenant B (same lf_mp_id path) sees nothing.
    let seen_by_a = pg::fetch_product(&pool, tenant_a, tenant_a, "P-1")
        .await
        .expect("read");
    assert!(seen_by_a.is_some(), "owner reads its own product");
    let seen_by_b = pg::fetch_product(&pool, tenant_a, tenant_b, "P-1")
        .await
        .expect("read");
    assert!(
        seen_by_b.is_none(),
        "a different tenant must not read another operator's product by lf_mp_id"
    );
}

// ── H4 — Tarifwechsel assignment is atomic and tenant-scoped ──────────────────

#[tokio::test]
#[ignore = "requires TARIFBD_TEST_DATABASE_URL"]
async fn product_assignment_and_tarifwechsel_preserve_one_active_row() {
    let Some(pool) = test_pool("assign").await else {
        return;
    };
    let tenant = "9900000000001";
    pg::upsert_product(&pool, tenant, tenant, "P-A", strom_product("P-A"))
        .await
        .expect("product A");
    pg::upsert_product(&pool, tenant, tenant, "P-B", strom_product("P-B"))
        .await
        .expect("product B");

    pg::assign_product(
        &pool,
        "51238696781",
        tenant,
        tenant,
        pg::AssignProductRequest {
            product_code: "P-A".to_owned(),
            assigned_from: "2026-02-01".to_owned(),
        },
    )
    .await
    .expect("initial assignment");

    // Tarifwechsel to P-B: the close+insert run in one transaction, so there is
    // always exactly one active (assigned_to IS NULL) row.
    pg::assign_product(
        &pool,
        "51238696781",
        tenant,
        tenant,
        pg::AssignProductRequest {
            product_code: "P-B".to_owned(),
            assigned_from: "2026-06-01".to_owned(),
        },
    )
    .await
    .expect("tarifwechsel");

    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM customer_products
         WHERE malo_id = '51238696781' AND assigned_to IS NULL",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        active, 1,
        "exactly one active assignment after Tarifwechsel"
    );

    let cur = pg::get_customer_product(&pool, "51238696781", tenant, tenant)
        .await
        .expect("read")
        .expect("has active product");
    assert_eq!(cur.product_code, "P-B", "the new product is active");

    let total: i64 =
        sqlx::query_scalar("SELECT count(*) FROM customer_products WHERE malo_id = '51238696781'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(total, 2, "the old assignment is preserved as history");
}

// ── H7 — erp_angebot_id idempotency ───────────────────────────────────────────

#[tokio::test]
#[ignore = "requires TARIFBD_TEST_DATABASE_URL"]
async fn erp_angebot_id_lookup_finds_existing_quotation() {
    let Some(pool) = test_pool("angebot_idem").await else {
        return;
    };
    let tenant = "9900000000001";
    let req = pg::CreateAngebotRequest {
        lf_mp_id: Some(tenant.to_owned()),
        kunden_id: None,
        interessent_name: Some("ACME GmbH".to_owned()),
        contact_email: None,
        contact_phone: None,
        gueltig_bis: Some("2026-12-31".to_owned()),
        lieferbeginn: None,
        laufzeit_monate: Some(24),
        positionen: vec![],
        varianten: None,
        erp_angebot_id: Some("ERP-Q-1".to_owned()),
        notizen: None,
    };
    let id = pg::insert_angebot(
        &pool,
        tenant,
        tenant,
        "ANG-2026-00001",
        &req,
        &serde_json::json!([]),
        &serde_json::json!([]),
        &serde_json::json!({}),
        None,
        None,
        time::macros::date!(2026 - 12 - 31),
        None,
    )
    .await
    .expect("insert angebot");

    // A retry with the same erp_angebot_id resolves to the existing quotation
    // rather than minting a duplicate.
    let found = pg::fetch_angebot_id_by_erp_id(&pool, tenant, "ERP-Q-1")
        .await
        .expect("lookup");
    assert_eq!(found.map(|(fid, _)| fid), Some(id));

    // A different tenant with the same erp_angebot_id string sees nothing.
    let cross = pg::fetch_angebot_id_by_erp_id(&pool, "9900000000002", "ERP-Q-1")
        .await
        .expect("lookup");
    assert!(cross.is_none(), "erp_angebot_id lookup is tenant-scoped");
}
