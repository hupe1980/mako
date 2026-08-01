//! Real-PostgreSQL guards for the tarifbd invariants that live in SQL: that a
//! product PUT actually writes the `NOT NULL tenant` column, that reads are
//! tenant-scoped, that a Tarifwechsel is atomic, and that `erp_angebot_id`
//! makes Angebot creation idempotent.
//!
//! PostgreSQL is self-managed via testcontainers (a Docker daemon is the only
//! requirement); the tests skip gracefully when Docker is unavailable:
//!
//! ```bash
//! just test-tarifbd-db
//! ```

use sqlx::PgPool;
use tarifbd::pg;

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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn upsert_product_writes_tenant_and_reads_are_tenant_scoped() {
    let Some((pool, _pg)) = test_pool("tenant_scope").await else {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn product_assignment_and_tarifwechsel_preserve_one_active_row() {
    let Some((pool, _pg)) = test_pool("assign").await else {
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
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn erp_angebot_id_lookup_finds_existing_quotation() {
    let Some((pool, _pg)) = test_pool("angebot_idem").await else {
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

// ── nEHS price series — upsert, latest-at-or-before, source discipline ────────

#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn nehs_price_upsert_latest_and_source_check() {
    use rust_decimal::dec;
    use time::macros::date;

    let Some((pool, _pg)) = test_pool("nehs").await else {
        return;
    };

    let put = |d, eur, src: Option<&str>| {
        pg::upsert_nehs_price(
            &pool,
            d,
            pg::NehsImportRequest {
                eur_per_t: eur,
                source: src.map(str::to_owned),
            },
        )
    };

    // Two weekly auction points (the EEX series is weekly from 01.07.2026).
    put(date!(2026 - 07 - 01), dec!(63.50), Some("auktion"))
        .await
        .expect("first auction price");
    put(date!(2026 - 07 - 08), dec!(64.25), Some("auktion"))
        .await
        .expect("second auction price");

    // Latest at-or-before: a mid-week billing date resolves to the 01.07 price.
    let hit = pg::latest_nehs_price(&pool, date!(2026 - 07 - 05))
        .await
        .expect("query")
        .expect("price expected");
    assert_eq!(hit, (date!(2026 - 07 - 01), dec!(63.50)));

    // On the second auction date, that price wins.
    let hit = pg::latest_nehs_price(&pool, date!(2026 - 07 - 08))
        .await
        .expect("query")
        .expect("price expected");
    assert_eq!(hit, (date!(2026 - 07 - 08), dec!(64.25)));

    // Before the series begins: no price at all.
    assert!(
        pg::latest_nehs_price(&pool, date!(2026 - 06 - 30))
            .await
            .expect("query")
            .is_none(),
        "no price before the first series entry"
    );

    // Re-import on the same date replaces the row (ON CONFLICT DO UPDATE).
    put(date!(2026 - 07 - 08), dec!(65.00), Some("nachkauf"))
        .await
        .expect("same-date re-import");
    let hit = pg::latest_nehs_price(&pool, date!(2026 - 07 - 08))
        .await
        .expect("query")
        .expect("price expected");
    assert_eq!(hit, (date!(2026 - 07 - 08), dec!(65.00)));

    // Omitted source defaults to 'manual'.
    put(date!(2026 - 07 - 15), dec!(68), None)
        .await
        .expect("manual default");
    let src: String = sqlx::query_scalar("SELECT source FROM nehs_prices WHERE price_date = $1")
        .bind(date!(2026 - 07 - 15))
        .fetch_one(&pool)
        .await
        .expect("read back source");
    assert_eq!(src, "manual");

    // Unknown source is rejected in the pg layer (handler surfaces 422)…
    let err = put(date!(2026 - 07 - 22), dec!(60), Some("boerse"))
        .await
        .expect_err("unknown source must be rejected");
    assert!(err.to_string().contains("unknown source"), "got: {err}");

    // …and the SQL CHECK constraint backstops writes that bypass it.
    let raw = sqlx::query(
        "INSERT INTO nehs_prices (price_date, eur_per_t, source) VALUES ($1, $2, 'boerse')",
    )
    .bind(date!(2026 - 07 - 29))
    .bind(dec!(60))
    .execute(&pool)
    .await;
    assert!(
        raw.is_err(),
        "CHECK must reject unknown source at SQL level"
    );
}

/// §41a 15-min MTU: a 96-entry quarter-hour import round-trips as 96 points
/// keyed on distinct UTC MTU starts; a legacy 60-min import is stored as 24
/// rows but fetched as 96 quarter-hours (expanded).
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn epex_15min_and_hourly_roundtrip_to_quarter_hours() {
    use rust_decimal::{Decimal, dec};
    use time::macros::date;

    let Some((pool, _pg)) = test_pool("epex_mtu").await else {
        return;
    };

    // 15-min import: 96 quarter-hours, price = slot index ct/kWh.
    let day = date!(2026 - 01 - 15);
    let prices: Vec<Decimal> = (0..96).map(Decimal::from).collect();
    pg::upsert_epex_day(
        &pool,
        day,
        pg::EpexImportRequest {
            prices: prices.clone(),
            mtu_minutes: Some(15),
            source: Some("epex-spot-day-ahead".to_owned()),
        },
    )
    .await
    .expect("15-min import must succeed");

    let points = pg::fetch_epex_day(&pool, day)
        .await
        .expect("fetch")
        .expect("some");
    assert_eq!(points.len(), 96, "96 quarter-hours expected");
    // Strictly increasing, 15-min-spaced MTU starts; prices preserved in order.
    for (i, p) in points.iter().enumerate() {
        assert_eq!(p.avg_ct_kwh, Decimal::from(i));
        if i > 0 {
            assert_eq!((p.mtu_start - points[i - 1].mtu_start).whole_minutes(), 15);
        }
    }

    // Wrong count must be rejected.
    let bad = pg::upsert_epex_day(
        &pool,
        day,
        pg::EpexImportRequest {
            prices: vec![dec!(1); 24],
            mtu_minutes: Some(15),
            source: None,
        },
    )
    .await;
    assert!(bad.is_err(), "24 entries at 15-min MTU must be rejected");

    // Legacy 60-min import: 24 rows stored, fetched as 96 quarter-hours where
    // each hour's price repeats four times.
    let hday = date!(2025 - 01 - 15);
    let hourly: Vec<Decimal> = (0..24).map(Decimal::from).collect();
    pg::upsert_epex_day(
        &pool,
        hday,
        pg::EpexImportRequest {
            prices: hourly,
            mtu_minutes: Some(60),
            source: Some("epex-spot-day-ahead".to_owned()),
        },
    )
    .await
    .expect("hourly import must succeed");
    let hpoints = pg::fetch_epex_day(&pool, hday)
        .await
        .expect("fetch")
        .expect("some");
    assert_eq!(hpoints.len(), 96, "hourly expands to 96 quarter-hours");
    assert_eq!(hpoints[0].avg_ct_kwh, dec!(0));
    assert_eq!(hpoints[3].avg_ct_kwh, dec!(0)); // 4th quarter of hour 0
    assert_eq!(hpoints[4].avg_ct_kwh, dec!(1)); // 1st quarter of hour 1
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
