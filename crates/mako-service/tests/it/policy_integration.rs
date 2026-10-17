//! Real-PostgreSQL tests for `service_policy_log`, the runner's record of
//! which value each operator setting first ran under.
//!
//! `#[ignore]`d by default (Docker required):
//!
//! ```bash
//! cargo test -p mako-service --test it -- --include-ignored --test-threads=1 policy_integration::
//! ```

use mako_service::policy::{self, PolicyClass, PolicyEntry, PolicyReport};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn boot() -> sqlx::PgPool {
    let container = Postgres::default().start().await.expect("start postgres");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("postgres port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    Box::leak(Box::new(container));
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .expect("connect");
    policy::ensure_schema(&pool).await.expect("schema");
    pool
}

fn report(regime: &str) -> PolicyReport {
    let mut r = PolicyReport::new().posture_flag("allow_insecure_no_auth", false);
    r.entries.push(PolicyEntry {
        key: "behg_regime".into(),
        value: regime.into(),
        origin: policy::Origin::Explicit,
        class: PolicyClass::Policy,
        quelle: Some("§ 10 Abs. 3 Satz 2 Nr. 4 BEHG".into()),
        doc: Some("@/docs/services/billingd.md#policy-behg-regime".into()),
        insecure: false,
        review_by: None,
        review_grund: None,
    });
    r
}

async fn rows(pool: &sqlx::PgPool, key: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM service_policy_log WHERE service = 'billingd' AND key = $1",
    )
    .bind(key)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Two boots under different values leave two rows and one change with the
/// old value; two identical boots leave one row.
#[tokio::test]
#[ignore = "requires Docker (testcontainers)"]
async fn a_changed_value_appends_a_row_and_an_unchanged_one_does_not() {
    let pool = boot().await;

    let first = policy::record(&pool, "billingd", &report("rvo_bezugspreis"))
        .await
        .unwrap();
    assert!(first.iter().all(|c| c.old.is_none()), "{first:?}");
    let same = policy::record(&pool, "billingd", &report("rvo_bezugspreis"))
        .await
        .unwrap();
    assert!(same.is_empty(), "{same:?}");
    assert_eq!(rows(&pool, "behg_regime").await, 1);

    let changed = policy::record(&pool, "billingd", &report("korridor"))
        .await
        .unwrap();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0].key, "behg_regime");
    assert_eq!(changed[0].old.as_deref(), Some("rvo_bezugspreis"));
    assert_eq!(changed[0].new, "korridor");
    assert_eq!(rows(&pool, "behg_regime").await, 2);
    assert_eq!(rows(&pool, "allow_insecure_no_auth").await, 1);
}
