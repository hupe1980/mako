//! DB-backed scenarios for `sperrd`.
//!
//! Run with `just test-sperrd-db`, or:
//! `DOCKER_HOST=… cargo test -p sperrd --test db_scenarios -- --ignored`

use sperrd::model::{Arbeitszeit, OrderStatus, OrderType};
use sperrd::pg::{self, CreateOrderRequest, Treffpunkt};
use sqlx::PgPool;
use time::macros::date;
use uuid::Uuid;

const TENANT: &str = "9900357000004";

type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

/// Start a throwaway `postgres:17-alpine`. `None` when Docker is unavailable, so
/// the tests skip rather than fail on a machine without it.
async fn setup() -> Option<(PgPool, PgContainer)> {
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
    let pool = PgPool::connect(&url).await.ok()?;
    sqlx::migrate!("./migrations").run(&pool).await.ok()?;
    // `event_outbox` is created by mako-service at startup, not by a migration,
    // so the harness has to create it too — otherwise any path that announces a
    // CloudEvent fails here and works in production.
    mako_service::outbox::ensure_schema(&pool).await.ok()?;
    Some((pool, container))
}

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &Uuid::new_v4().simple().to_string()[..12])
}

fn order(process_id: Option<String>) -> CreateOrderRequest {
    CreateOrderRequest {
        malo_id: "51238696012".to_owned(),
        lf_mp_id: "9900012345678".to_owned(),
        order_type: OrderType::Sperrung,
        process_id,
        ausfuehrung_am: Some(date!(2026 - 09 - 01)),
        fruehestens_am: None,
        arbeitszeit: None,
        treffpunkt: Treffpunkt {
            hinweis: Some("Zählerschrank im Hof".to_owned()),
            strasse: Some("Musterstraße 12".to_owned()),
            plz: Some("10115".to_owned()),
            ort: Some("Berlin".to_owned()),
            land: Some("DE".to_owned()),
        },
        hinweis: Some("Hund im Garten".to_owned()),
    }
}

/// The ORDERS fields the field team needs survive the round-trip: which date is
/// a requirement and which an earliest-start, and where the technician goes.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_order_round_trips_with_its_orders_fields() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let req = order(Some(uniq("proc")));
    let id = pg::create_order_pg(&pool, TENANT, &req)
        .await
        .unwrap()
        .expect("inserted");

    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .expect("found");
    assert_eq!(row.order_type, OrderType::Sperrung);
    assert_eq!(row.status, OrderStatus::Pending);
    assert_eq!(row.ausfuehrung_am, Some(date!(2026 - 09 - 01)));
    assert_eq!(row.fruehestens_am, None);
    assert_eq!(row.treffpunkt_ort.as_deref(), Some("Berlin"));
    assert_eq!(row.hinweis.as_deref(), Some("Hund im Garten"));
    assert_eq!(
        row.pruefidentifikator,
        Some(17115),
        "an order with a market process records the PID it arrived on"
    );
    assert_eq!(row.iftsta_attempts, 0);
}

/// A redelivered ORDERS must not queue a second disconnection.
///
/// BDEW AS4 mandates ReceptionAwareness retry with a stable MessageId, so a
/// duplicate is the expected case, not an anomaly.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_redelivered_orders_does_not_queue_a_second_disconnection() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let process = uniq("proc");
    let first = pg::create_order_pg(&pool, TENANT, &order(Some(process.clone())))
        .await
        .unwrap();
    assert!(first.is_some());
    let second = pg::create_order_pg(&pool, TENANT, &order(Some(process)))
        .await
        .unwrap();
    assert!(
        second.is_none(),
        "the same market process must map to exactly one work order"
    );

    // Two operator-created orders, which have no process, are *not* duplicates
    // of each other — NULL is distinct under a plain UNIQUE.
    assert!(
        pg::create_order_pg(&pool, TENANT, &order(None))
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        pg::create_order_pg(&pool, TENANT, &order(None))
            .await
            .unwrap()
            .is_some()
    );
}

/// A failed dispatch **keeps** the field report and queues a retry.
///
/// The report is a fact about the physical world; the dispatch is not. Rolling
/// the claim back to `pending` would discard the first because the second
/// failed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_failed_dispatch_keeps_the_report_and_queues_a_retry() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    // Unroutable makod endpoint: every dispatch fails.
    let makod = std::sync::Arc::new(mako_markt::makod_client::MakodClient::new(
        "http://127.0.0.1:1",
        secrecy::SecretString::from("test"),
    ));
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();

    let outcome = pg::Outcome::Failed {
        reason: "Zutritt verweigert",
        pruefschritt_code: Some("A04"),
    };
    let reported = pg::report_outcome(&pool, &makod, id, TENANT, &outcome)
        .await
        .unwrap();
    assert_eq!(
        reported,
        pg::Reported::Recorded {
            iftsta_dispatched: false
        },
        "the outcome is recorded and the caller is told the IFTSTA did not go out"
    );

    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.status,
        OrderStatus::Failed,
        "the report is NOT rolled back"
    );
    assert_eq!(row.fail_reason.as_deref(), Some("Zutritt verweigert"));
    assert_eq!(row.pruefschritt_code.as_deref(), Some("A04"));
    assert!(row.iftsta_dispatched_at.is_none());
    assert_eq!(row.iftsta_attempts, 1, "the failed attempt is counted");
    assert!(row.iftsta_last_error.is_some());

    // …and it is on the retry queue.
    let claimed = pg::claim_iftsta_retry(&pool, TENANT).await.unwrap();
    assert_eq!(claimed.map(|o| o.id), Some(id));
}

/// A second outcome for the same order is refused.
///
/// Execute and fail race in the field. Without the claim guard the Lieferant
/// receives an Ausführungs- *and* a Fehlmeldung for one order, with no way to
/// tell which is true.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_order_reaches_exactly_one_terminal_outcome() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let makod = std::sync::Arc::new(mako_markt::makod_client::MakodClient::new(
        "http://127.0.0.1:1",
        secrecy::SecretString::from("test"),
    ));
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();

    let executed = pg::Outcome::Executed {
        at: time::OffsetDateTime::now_utc(),
        note: Some("TW-2026-0714-001"),
        pruefschritt_code: Some("A01"),
    };
    assert!(matches!(
        pg::report_outcome(&pool, &makod, id, TENANT, &executed)
            .await
            .unwrap(),
        pg::Reported::Recorded { .. }
    ));

    let failed = pg::Outcome::Failed {
        reason: "Zutritt verweigert",
        pruefschritt_code: Some("A04"),
    };
    assert_eq!(
        pg::report_outcome(&pool, &makod, id, TENANT, &failed)
            .await
            .unwrap(),
        pg::Reported::NotFound,
        "the claim guard refuses the second outcome"
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, OrderStatus::Executed);
    assert!(row.fail_reason.is_none());
}

/// Only `pending` orders can be withdrawn, and the cancellation announces itself.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn cancelling_is_pending_only_and_announced() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let cancelled = pg::cancel_order_pg(&mut *tx, id, TENANT).await.unwrap();
    assert!(cancelled.is_some());
    sperrd::events::storniert(&mut tx, TENANT, id, "51238696012", "9900012345678")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let queued: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_outbox WHERE ce_type = $1 AND envelope->'data'->>'order_id' = $2",
    )
    .bind(mako_events::sperr::STORNIERT)
    .bind(id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(queued, 1, "de.sperr.storniert is on the outbox");

    // A terminal order stays terminal.
    let again = pg::cancel_order_pg(&pool, id, TENANT).await.unwrap();
    assert!(
        again.is_none(),
        "a cancelled order cannot be cancelled twice"
    );
}

/// The retry budget is bounded and exhaustion is escalated exactly once.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_exhausted_iftsta_is_escalated_once() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE sperr_orders SET status = 'executed', executed_at = now(), \
         iftsta_attempts = $2, iftsta_last_error = 'connection refused' WHERE id = $1",
    )
    .bind(id)
    .bind(pg::IFTSTA_MAX_ATTEMPTS)
    .execute(&pool)
    .await
    .unwrap();

    // Past the budget: the worker no longer hands it out.
    assert!(
        pg::claim_iftsta_retry(&pool, TENANT)
            .await
            .unwrap()
            .is_none(),
        "an exhausted order is not retried forever"
    );

    let stuck = pg::list_stuck_iftsta(&pool, TENANT).await.unwrap();
    assert_eq!(stuck.len(), 1);
    let mut tx = pool.begin().await.unwrap();
    sperrd::events::iftsta_ausstehend(&mut tx, TENANT, id, "51238696012", "9900012345678", "x")
        .await
        .unwrap();
    pg::mark_iftsta_escalated(&mut *tx, id, TENANT)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        pg::list_stuck_iftsta(&pool, TENANT)
            .await
            .unwrap()
            .is_empty(),
        "an escalated order is announced once, not on every sweep"
    );

    let s = pg::stats_pg(&pool, TENANT).await.unwrap();
    assert_eq!(s.iftsta_outstanding, 1);
    assert_eq!(s.iftsta_stuck, 1);
}

/// Tenants do not see each other's disconnection queue.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_queue_is_tenant_isolated() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let mine = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    pg::create_order_pg(&pool, "OTHER", &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();

    let rows = pg::list_orders_pg(&pool, TENANT, None, None, false, 100)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, mine.to_string());
    assert!(
        pg::fetch_order_pg(&pool, mine, "OTHER")
            .await
            .unwrap()
            .is_none(),
        "another tenant cannot read this order by id"
    );
    assert!(
        pg::list_orders_pg(&pool, "", None, None, false, 100)
            .await
            .unwrap()
            .is_empty(),
        "an empty tenant is not a wildcard"
    );
}

/// The database refuses an order carrying both a fixed and an earliest date.
///
/// The API refuses it too (`CreateOrderRequest::validate`); this pins that the
/// constraint holds for anything that reaches the table by another route.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_two_orders_dates_are_mutually_exclusive_in_the_schema() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let err = sqlx::query(
        "INSERT INTO sperr_orders (tenant, malo_id, lf_mp_id, order_type, \
         ausfuehrung_am, fruehestens_am) VALUES ($1, '51238696012', 'LF', 'sperrung', \
         DATE '2026-09-01', DATE '2026-09-03')",
    )
    .bind(TENANT)
    .execute(&pool)
    .await;
    assert!(err.is_err(), "DTM+203 and DTM+469 are alternatives");
}

/// The due list is what the field team works from, ordered soonest first.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_due_list_is_the_field_dispatch_list() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let mut past = order(Some(uniq("proc")));
    past.ausfuehrung_am = Some(date!(2020 - 01 - 01));
    let due_id = pg::create_order_pg(&pool, TENANT, &past)
        .await
        .unwrap()
        .unwrap();

    let mut future = order(Some(uniq("proc")));
    future.ausfuehrung_am = None;
    future.fruehestens_am = Some(date!(2099 - 01 - 01));
    pg::create_order_pg(&pool, TENANT, &future).await.unwrap();

    let due = pg::list_orders_pg(&pool, TENANT, Some(OrderStatus::Pending), None, true, 100)
        .await
        .unwrap();
    assert_eq!(due.len(), 1, "only the order whose date has arrived is due");
    assert_eq!(due[0].id, due_id.to_string());

    let all = pg::list_orders_pg(&pool, TENANT, None, None, false, 100)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, due_id.to_string(), "soonest first");

    let s = pg::stats_pg(&pool, TENANT).await.unwrap();
    assert_eq!(s.pending, 2);
    assert_eq!(s.overdue_pending, 1);
}

/// An Entsperrauftrag carries its `IMD+7081` Arbeitszeit through the store.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_entsperrauftrag_keeps_its_arbeitszeit() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let mut req = order(Some(uniq("proc")));
    req.order_type = OrderType::Entsperrung;
    req.ausfuehrung_am = None;
    req.arbeitszeit = Some(Arbeitszeit::AuchAusserhalb);
    let id = pg::create_order_pg(&pool, TENANT, &req)
        .await
        .unwrap()
        .unwrap();

    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.order_type, OrderType::Entsperrung);
    assert_eq!(row.arbeitszeit, Some(Arbeitszeit::AuchAusserhalb));
    assert_eq!(
        row.pruefidentifikator,
        Some(17117),
        "an Entsperrauftrag arrives on 17117"
    );
}
