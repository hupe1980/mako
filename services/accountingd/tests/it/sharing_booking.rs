//! The § 42c Energy-Sharing credit billingd puts on an invoice is the amount
//! accountingd books.
//!
//! billingd's `the_allocated_share_is_the_credit_accountingd_books` bills a
//! SHARING period from edmd's allocation and pins the
//! `de.billing.rechnung.erstellt` fields accountingd reads in
//! `services/billingd/tests/fixtures/sharing_rechnung_erstellt.json`. This
//! books that fixture through the webhook, the way the event arrives, so a
//! credit lost on either side of the seam fails one of the two tests.

use testcontainers::{ContainerAsync, ImageExt as _, runners::AsyncRunner as _};
use testcontainers_modules::postgres::Postgres;

/// The dev configuration's tenant.
const TENANT: &str = "9900357000004";

const FIXTURE: &str =
    include_str!("../../../billingd/tests/fixtures/sharing_rechnung_erstellt.json");

async fn app() -> Option<(axum::Router, sqlx::PgPool, ContainerAsync<Postgres>)> {
    let pg = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .ok()?;
    let port = pg.get_host_port_ipv4(5432).await.expect("port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    #[expect(
        clippy::disallowed_methods,
        reason = "the test connects straight to its throwaway container; pool sizing is a deployment concern"
    )]
    let pool = sqlx::PgPool::connect(&url).await.expect("connect");
    let mut cfg: toml::Value =
        toml::from_str(include_str!("../../../../dev/config/accountingd.toml"))
            .expect("the dev configuration parses");
    cfg["database"]["url"] = toml::Value::String(url);
    if let Some(db) = cfg["database"].as_table_mut() {
        db.remove("migration_url");
    }
    let cfg = cfg.try_into().expect("an accountingd configuration");
    let app = mako_service::build_for_test_on::<accountingd::Accountingd>(cfg, pool.clone())
        .await
        .expect("the router builds");
    Some((app, pool, pg))
}

/// **Invariant: the receivable booked for a SHARING invoice is the invoice's
/// gross, § 42c credit included.**
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_sharing_invoice_is_booked_at_its_gross() {
    use tower::ServiceExt as _;
    let Some((app, pool, _pg)) = app().await else {
        return;
    };
    let mut data: serde_json::Value = serde_json::from_str(FIXTURE).expect("the fixture");
    data["record_id"] = serde_json::json!(uuid::Uuid::new_v4());
    let malo = data["malo_id"].as_str().expect("malo_id").to_owned();
    let gross: rust_decimal::Decimal = data["rechnung"]["gesamtbrutto"]["wert"]
        .as_str()
        .expect("gesamtbrutto")
        .parse()
        .expect("a decimal");
    let ce = serde_json::json!({
        "specversion": "1.0",
        "id": "ce-sharing-1",
        "source": format!("urn:mako:billingd:{TENANT}"),
        "type": "de.billing.rechnung.erstellt",
        "time": "2026-08-03T08:00:00Z",
        "subject": malo,
        "datacontenttype": "application/json",
        "data": data,
    });
    let response = app
        .oneshot(
            axum::http::Request::post("/webhook")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(ce.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(response.status().is_success(), "{}", response.status());

    let booked: i64 =
        sqlx::query_scalar("SELECT balance_ct FROM accounts WHERE tenant = $1 AND malo_id = $2")
            .bind(TENANT)
            .bind(&malo)
            .fetch_one(&pool)
            .await
            .expect("the account");
    assert_eq!(
        rust_decimal::Decimal::from(booked),
        gross * rust_decimal::Decimal::ONE_HUNDRED,
        "the receivable is the invoice's gross, the § 42c credit included"
    );
}
