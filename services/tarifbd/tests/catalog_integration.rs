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
    let seen_by_a = pg::fetch_product(&pool, tenant_a, tenant_a, "P-1", None)
        .await
        .expect("read");
    assert!(seen_by_a.is_some(), "owner reads its own product");
    let seen_by_b = pg::fetch_product(&pool, tenant_a, tenant_b, "P-1", None)
        .await
        .expect("read");
    assert!(
        seen_by_b.is_none(),
        "a different tenant must not read another operator's product by lf_mp_id"
    );
}

/// A PUT of the same product version must update, and two tenants must not
/// share a row.
///
/// The unique key was `(lf_mp_id, product_code, valid_from)`: no tenant, so
/// tenant B's PUT overwrote tenant A's row — and NULLs are distinct under a
/// plain UNIQUE, so every PUT of an open-ended product inserted another
/// duplicate that `fetch_product`'s LIMIT 1 then picked among at random.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_product_version_has_one_row_per_tenant_even_without_a_valid_from() {
    let Some((pool, _pg)) = test_pool("product_identity").await else {
        return;
    };
    let tenant_a = "9900000000001";
    let tenant_b = "9900000000002";
    let open_ended = |name: &str| pg::ProductUpsertRequest {
        name: name.to_owned(),
        valid_from: None,
        ..strom_product("P-OPEN")
    };

    for name in ["first", "second", "third"] {
        pg::upsert_product(&pool, tenant_a, tenant_a, "P-OPEN", open_ended(name))
            .await
            .expect("open-ended PUT");
    }
    let rows: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM products WHERE product_code = 'P-OPEN' AND tenant = $1",
    )
    .bind(tenant_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, 1, "a NULL valid_from must conflict, not duplicate");

    let current = pg::fetch_product(&pool, tenant_a, tenant_a, "P-OPEN", None)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(current.name, "third", "the last PUT is the current version");

    // The other tenant's PUT is its own row, not an overwrite.
    pg::upsert_product(&pool, tenant_a, tenant_b, "P-OPEN", open_ended("theirs"))
        .await
        .expect("other tenant PUT");
    let mine = pg::fetch_product(&pool, tenant_a, tenant_a, "P-OPEN", None)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        mine.name, "third",
        "another tenant must not overwrite this row"
    );
}

/// A price version dated in the future is not yet the current one, and a
/// retroactive read gets the version that was valid then.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_future_dated_price_version_is_not_current_yet() {
    let Some((pool, _pg)) = test_pool("product_as_of").await else {
        return;
    };
    let tenant = "9900000000001";
    for (name, from) in [("alt", "2026-01-01"), ("neu", "2099-01-01")] {
        pg::upsert_product(
            &pool,
            tenant,
            tenant,
            "P-VER",
            pg::ProductUpsertRequest {
                name: name.to_owned(),
                valid_from: Some(from.to_owned()),
                ..strom_product("P-VER")
            },
        )
        .await
        .expect("versioned PUT");
    }

    let today = pg::fetch_product(&pool, tenant, tenant, "P-VER", None)
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(
        today.name, "alt",
        "a version valid from 2099 is not current"
    );
    assert_eq!(
        today.valid_to,
        Some(time::macros::date!(2098 - 12 - 31)),
        "staging the 2099 version end-dates the one it succeeds, so the two \
         never claim the same day"
    );

    let then = pg::fetch_product(
        &pool,
        tenant,
        tenant,
        "P-VER",
        Some(time::macros::date!(2099 - 06 - 01)),
    )
    .await
    .expect("read")
    .expect("exists");
    assert_eq!(
        then.name, "neu",
        "as_of picks the version valid at that date"
    );
}

// ── H4 — Tarifwechsel assignment is atomic and tenant-scoped ──────────────────

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
    put(date!(2026 - 07 - 08), dec!(65.00), Some("auktion"))
        .await
        .expect("same-date re-import");
    let hit = pg::latest_nehs_price(&pool, date!(2026 - 07 - 08))
        .await
        .expect("query")
        .expect("price expected");
    assert_eq!(hit, (date!(2026 - 07 - 08), dec!(65.00)));

    // § 10 Abs. 2 BEHG fixes the 2026 corridor at 55–65 EUR/t, so a decimal
    // slip in an auction price is refused rather than quietly under-billing
    // the CO₂ component of every gas invoice by a factor of ten.
    let err = put(date!(2026 - 07 - 15), dec!(6.35), Some("auktion"))
        .await
        .expect_err("6.35 EUR/t is a typo, not a clearing price");
    assert!(err.to_string().contains("Preiskorridor"), "got: {err}");

    // 68 EUR/t is the Mehrmengenpreis of the Nachkauf phase, not an auction
    // clearing price — the two were documented as the same thing.
    put(date!(2026 - 11 - 10), dec!(68), Some("nachkauf"))
        .await
        .expect("the Nachkauf price is valid for its own phase");
    assert!(
        put(date!(2026 - 11 - 17), dec!(68), Some("auktion"))
            .await
            .is_err(),
        "68 EUR/t is above the auction corridor"
    );

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
/// A withdrawn product stops pricing new periods but still prices the past.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_withdrawn_product_no_longer_prices_today_but_still_prices_the_past() {
    let Some((pool, _pg)) = test_pool("valid_to").await else {
        return;
    };
    let tenant = "9900000000027";
    let mut req = strom_product("P-ALT");
    req.valid_from = Some("2026-01-01".to_owned());
    req.valid_to = Some("2026-06-30".to_owned());
    pg::upsert_product(&pool, tenant, tenant, "P-ALT", req)
        .await
        .expect("product");

    assert!(
        pg::fetch_product(
            &pool,
            tenant,
            tenant,
            "P-ALT",
            Some(time::macros::date!(2026 - 03 - 01))
        )
        .await
        .unwrap()
        .is_some(),
        "an invoice for March still needs March's product"
    );
    assert!(
        pg::fetch_product(
            &pool,
            tenant,
            tenant,
            "P-ALT",
            Some(time::macros::date!(2026 - 09 - 01))
        )
        .await
        .unwrap()
        .is_none(),
        "a product withdrawn on 30 June must not price September"
    );
}

/// Scheduling a price change end-dates the version it succeeds, so two
/// versions never claim the same day — and `products_no_overlap` is the
/// backstop for anything that writes around that path.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_scheduled_price_change_closes_the_version_it_succeeds() {
    let Some((pool, _pg)) = test_pool("product_overlap").await else {
        return;
    };
    let tenant = "9900000000028";
    let mut v1 = strom_product("P-X");
    v1.valid_from = Some("2026-01-01".to_owned());
    v1.valid_to = None;
    pg::upsert_product(&pool, tenant, tenant, "P-X", v1)
        .await
        .expect("first version");

    // The ordinary act: stage next year's prices. Requiring the operator to go
    // back and end-date the running version first would turn one act into two,
    // with an unpriced gap whenever they forget.
    let mut v2 = strom_product("P-X");
    v2.valid_from = Some("2027-01-01".to_owned());
    v2.valid_to = None;
    pg::upsert_product(&pool, tenant, tenant, "P-X", v2)
        .await
        .expect("scheduling next year's prices");

    let heute = pg::fetch_product(
        &pool,
        tenant,
        tenant,
        "P-X",
        Some(time::macros::date!(2026 - 06 - 01)),
    )
    .await
    .unwrap()
    .expect("2026 has a price");
    assert_eq!(
        heute.valid_to,
        Some(time::macros::date!(2026 - 12 - 31)),
        "the running version now ends the day before the new one starts"
    );
    assert!(
        pg::fetch_product(
            &pool,
            tenant,
            tenant,
            "P-X",
            Some(time::macros::date!(2027 - 06 - 01))
        )
        .await
        .unwrap()
        .is_some(),
        "and 2027 is priced by the new one"
    );

    // The backstop: a direct write that would leave two versions covering one
    // day is refused by the database, not merely by the code path above.
    let clash = sqlx::query(
        "INSERT INTO products
             (tenant, lf_mp_id, product_code, category, name, data, valid_from, valid_to)
         VALUES ($1, $1, 'P-X', 'STROM', 'schleichend', '{}'::jsonb,
                 DATE '2026-06-01', DATE '2026-09-30')",
    )
    .bind(tenant)
    .execute(&pool)
    .await;
    assert!(
        clash.is_err(),
        "products_no_overlap must refuse a version overlapping an existing one"
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
