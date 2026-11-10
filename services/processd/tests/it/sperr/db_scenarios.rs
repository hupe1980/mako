//! DB-backed scenarios for the Sperrauftrag queue.
//!
//! Run with `just test-processd-db`, or:
//! `cargo test -p processd --test it -- db_scenarios:: --include-ignored`
#![expect(
    clippy::disallowed_methods,
    reason = "a test connects straight to its throwaway container; pool sizing and lifetimes are a deployment concern"
)]

use processd::sperr::model::{Arbeitszeit, OrderStatus, OrderType};
use processd::sperr::pg::{self, CreateOrderRequest, Treffpunkt};
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

/// An outcome commits with the IFTSTA 21039 it owes: the field team's report
/// never waits on makod, and the Auftragsstatus cannot be lost behind it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_outcome_commits_with_its_iftsta_command() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    let outcome = pg::Outcome::Failed {
        reason: "Zutritt verweigert",
        pruefschritt_code: Some("A04"),
        endgueltig: true,
    };
    let reported = pg::report_outcome(&pool, id, TENANT, &outcome)
        .await
        .unwrap();
    assert_eq!(
        reported,
        pg::Reported::Recorded {
            iftsta_queued: true
        }
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, OrderStatus::Failed);
    assert_eq!(row.pruefschritt_code.as_deref(), Some("A04"));
    assert!(row.iftsta_dispatched_at.is_none(), "not delivered yet");
    assert_eq!(row.iftsta_attempts, 0);

    let body: serde_json::Value = sqlx::query_scalar(
        "SELECT envelope->'body' FROM event_outbox WHERE kind = 'command' AND event_id = $1",
    )
    .bind(pg::iftsta_key(id))
    .fetch_one(&pool)
    .await
    .expect("the IFTSTA is owed in the outbox");
    assert_eq!(body["payload"]["auftragsstatus_code"], "Z13");
    assert_eq!(body["payload"]["pruefschritt_code"], "A04");
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
        pg::report_outcome(&pool, id, TENANT, &executed)
            .await
            .unwrap(),
        pg::Reported::Recorded { .. }
    ));

    let failed = pg::Outcome::Failed {
        reason: "Zutritt verweigert",
        pruefschritt_code: Some("A04"),
        endgueltig: true,
    };
    assert_eq!(
        pg::report_outcome(&pool, id, TENANT, &failed)
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
    processd::sperr::events::storniert(&mut tx, TENANT, id, "51238696012", "9900012345678")
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

/// A dead-lettered IFTSTA is escalated, once.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_dead_lettered_iftsta_is_escalated_once() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    pg::report_outcome(&pool, id, TENANT, &executed())
        .await
        .unwrap();
    // Still being delivered: not stuck.
    assert!(
        pg::list_stuck_iftsta(&pool, TENANT)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query(
        "UPDATE event_outbox SET dead_lettered_at = now(), attempts = 8, \
         last_error = 'connection refused' WHERE event_id = $1",
    )
    .bind(pg::iftsta_key(id))
    .execute(&pool)
    .await
    .unwrap();

    let stuck = pg::list_stuck_iftsta(&pool, TENANT).await.unwrap();
    assert_eq!(stuck.len(), 1);
    assert_eq!(stuck[0].attempts, 8, "the attempts actually made");
    assert_eq!(stuck[0].last_error, "connection refused");
    assert!(processd::sperr::worker::sweep(&pool, TENANT).await.unwrap());
    assert!(
        pg::list_stuck_iftsta(&pool, TENANT)
            .await
            .unwrap()
            .is_empty(),
        "an escalated order is announced once, not on every sweep"
    );
    assert_eq!(ausstehend(&pool).await, 1);

    let s = pg::stats_pg(&pool, TENANT).await.unwrap();
    assert_eq!(s.iftsta_outstanding, 1);
    assert_eq!(s.iftsta_stuck, 1);
}

fn executed() -> pg::Outcome<'static> {
    pg::Outcome::Executed {
        at: time::OffsetDateTime::now_utc(),
        note: None,
        pruefschritt_code: Some("A01"),
    }
}

async fn ausstehend(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM event_outbox WHERE ce_type = $1")
        .bind(mako_events::sperr::IFTSTA_AUSSTEHEND)
        .fetch_one(pool)
        .await
        .unwrap()
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

/// GPKE Teil 2 § 3.5.1.2 Nr. 5: „Der NB führt bis zu zwei Sperrversuche
/// innerhalb eines Sperrauftrags durch."
///
/// The first unsuccessful visit is recorded and the order stays in the queue —
/// no IFTSTA, because the Lieferant is still owed a second attempt. The second
/// closes it and reports `Z13 gescheitert`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_first_sperrversuch_does_not_close_the_order() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("zwei-versuche"))))
        .await
        .unwrap()
        .unwrap();

    let versuch = |reason| pg::Outcome::Failed {
        reason,
        pruefschritt_code: Some("A04"),
        endgueltig: false,
    };

    let first = pg::report_outcome(&pool, id, TENANT, &versuch("niemand angetroffen"))
        .await
        .unwrap();
    assert_eq!(
        first,
        pg::Reported::VersuchNotiert { sperrversuche: 1 },
        "the first attempt is recorded, not reported"
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, processd::sperr::model::OrderStatus::Pending);
    assert_eq!(row.sperrversuche, 1);
    assert!(
        row.iftsta_faellig_am.is_none(),
        "no IFTSTA Frist runs while the order is still open"
    );

    let second = pg::report_outcome(&pool, id, TENANT, &versuch("erneut niemand"))
        .await
        .unwrap();
    assert_eq!(
        second,
        pg::Reported::Recorded {
            iftsta_queued: true
        },
        "the second attempt exhausts the allowance and closes the order"
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, processd::sperr::model::OrderStatus::Failed);
    assert_eq!(row.sperrversuche, 2);
    assert!(
        row.iftsta_faellig_am.is_some(),
        "the IFTSTA Frist starts when the Sperrauftrag is abgeschlossen"
    );
}

/// A legal or factual impossibility (§ 3.5.1.2 Nr. 5: „gerichtliche Verfügung",
/// „lebenserhaltende medizinische Geräte") closes the order on the first
/// report — there is nothing a second visit could change.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_endgueltige_verhinderung_closes_the_order_at_once() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("verfuegung"))))
        .await
        .unwrap()
        .unwrap();

    let reported = pg::report_outcome(
        &pool,
        id,
        TENANT,
        &pg::Outcome::Failed {
            reason: "gerichtliche Verfügung untersagt die Sperrung",
            pruefschritt_code: Some("A05"),
            endgueltig: true,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        reported,
        pg::Reported::Recorded {
            iftsta_queued: true
        }
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, processd::sperr::model::OrderStatus::Failed);
}

/// The 6-Werktage execution window is computed on insert and drives the
/// overdue sweep — an order whose window has passed is announced once.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_execution_window_is_stored_and_swept() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let mut req = order(Some(uniq("frist")));
    // A Sperrtermin well in the past: the 6-WT window has long expired.
    req.fruehestens_am =
        Some(time::Date::from_calendar_date(2026, time::Month::January, 5).unwrap());
    req.ausfuehrung_am = None;
    let id = pg::create_order_pg(&pool, TENANT, &req)
        .await
        .unwrap()
        .unwrap();

    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.ausfuehrung_faellig_am,
        pg::ausfuehrung_faellig_am(None, req.fruehestens_am),
        "the window is stored, not recomputed by every reader"
    );

    let overdue = pg::list_ausfuehrung_ueberfaellig(&pool, TENANT)
        .await
        .unwrap();
    assert!(
        overdue.iter().any(|(oid, ..)| *oid == id),
        "an order past its window shows up in the sweep"
    );
    pg::mark_ausfuehrung_escalated(&pool, id, TENANT)
        .await
        .unwrap();
    let overdue = pg::list_ausfuehrung_ueberfaellig(&pool, TENANT)
        .await
        .unwrap();
    assert!(
        !overdue.iter().any(|(oid, ..)| *oid == id),
        "a missed Frist is announced once, not on every sweep"
    );
}

/// An IFTSTA past its Frist is escalated while its command is still being
/// retried: the Lieferant is owed the Auftragsstatus by the 1. WT nach Abschluss
/// (GPKE Teil 2 § 3.5.1.2 Nr. 5), whatever the delivery is doing.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_overdue_iftsta_is_escalated_while_still_retrying() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    pg::report_outcome(&pool, id, TENANT, &executed())
        .await
        .unwrap();
    sqlx::query("UPDATE sperr_orders SET iftsta_faellig_am = heute() - 1 WHERE id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let stuck = pg::list_stuck_iftsta(&pool, TENANT).await.unwrap();
    assert_eq!(stuck.len(), 1);
    assert_eq!(
        stuck[0].attempts, 0,
        "the attempts actually made, not a cap"
    );
    assert!(processd::sperr::worker::sweep(&pool, TENANT).await.unwrap());
    assert!(
        pg::list_stuck_iftsta(&pool, TENANT)
            .await
            .unwrap()
            .is_empty()
    );
}

/// An operator-created order owes no IFTSTA: no Lieferant is waiting, so it is
/// closed in the transaction that records its outcome and nothing is queued.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn an_operator_order_is_closed_without_an_iftsta() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(None))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pg::report_outcome(&pool, id, TENANT, &executed())
            .await
            .unwrap(),
        pg::Reported::Recorded {
            iftsta_queued: false
        }
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.iftsta_ref.as_deref(), Some("local"));
    assert!(row.iftsta_dispatched_at.is_some());
    let queued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM event_outbox WHERE kind = 'command'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(queued, 0);
}

/// The order the dispatcher reads states the earliest Sperrtermin under
/// `[policy] msb_vorlauf_lesart`, counted from the day the order arrived: a
/// date five Werktage out is inside the MSB's window under the default
/// `additiv` and admissible under `wortlaut`.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_order_view_states_the_msb_window_under_the_reading() {
    use mako_service::policy::Policy;
    use processd::config::{MsbVorlaufLesart, ProcessdPolicy};
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let heute = mako_fristen::heute();
    let fuenf_wt = mako_fristen::add_werktage(heute, 5, mako_fristen::HolidayCalendar::BdewMaKo);
    let mut req = order(Some(uniq("proc")));
    req.ausfuehrung_am = Some(fuenf_wt);
    let id = pg::create_order_pg(&pool, TENANT, &req)
        .await
        .unwrap()
        .expect("inserted");
    let row = || async {
        pg::fetch_order_pg(&pool, id, TENANT)
            .await
            .unwrap()
            .expect("found")
    };

    let additiv =
        pg::SperrOrderView::new(row().await, &ProcessdPolicy::default().msb_vorlauf_lesart);
    let fenster = additiv.msb_fenster.expect("a Sperrauftrag names a date");
    assert!(!fenster.eingehalten, "{fenster:?}");
    assert_eq!(
        fenster.fruehester_sperrtermin,
        mako_fristen::add_werktage(heute, 6, mako_fristen::HolidayCalendar::BdewMaKo)
    );
    let json = serde_json::to_value(pg::SperrOrderView::new(
        row().await,
        &ProcessdPolicy::default().msb_vorlauf_lesart,
    ))
    .unwrap();
    assert_eq!(json["msb_fenster"]["lesart"]["value"], "additiv");
    assert_eq!(
        json["malo_id"], "51238696012",
        "the row stays flat beside the window"
    );

    let wortlaut =
        pg::SperrOrderView::new(row().await, &Policy::explicit(MsbVorlaufLesart::Wortlaut));
    assert!(wortlaut.msb_fenster.expect("dated").eingehalten);
}

/// A fake makod answering `status`, recording each command it receives.
async fn fake_makod(
    status: u16,
) -> (
    String,
    std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let rec = std::sync::Arc::clone(&seen);
    let app = axum::Router::new().route(
        "/api/v1/commands",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let rec = std::sync::Arc::clone(&rec);
            async move {
                rec.lock().unwrap().push(body);
                (
                    axum::http::StatusCode::from_u16(status).unwrap(),
                    axum::Json(serde_json::json!({
                        "process_id": "00000000-0000-0000-0000-0000000021a9",
                        "idempotency_key": "k",
                    })),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (url, seen)
}

/// With makod down the IFTSTA survives; when it returns the Auftragsstatus goes
/// out once and the order is stamped dispatched. A command that keeps failing
/// past the shared budget is dead-lettered and escalated from there.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_iftsta_reaches_makod_once_after_an_outage_or_escalates() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let fast = mako_service::outbox::OutboxConfig {
        backoff: vec![std::time::Duration::ZERO],
        lease: std::time::Duration::ZERO,
        ..Default::default()
    };
    let key = secrecy::SecretString::from("test");
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    pg::report_outcome(&pool, id, TENANT, &executed())
        .await
        .unwrap();

    // makod down.
    let (down, _) = fake_makod(503).await;
    processd::sperr::iftsta_worker(pool.clone(), down, key.clone())
        .with_config(fast.clone())
        .flush_once()
        .await
        .unwrap();
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert!(row.iftsta_dispatched_at.is_none());
    assert_eq!(
        row.iftsta_attempts, 1,
        "the attempt is read off the command"
    );
    assert!(row.iftsta_last_error.is_some());

    // makod back.
    let (up, seen) = fake_makod(202).await;
    let worker =
        processd::sperr::iftsta_worker(pool.clone(), up, key.clone()).with_config(fast.clone());
    worker.flush_once().await.unwrap();
    worker.flush_once().await.unwrap();
    assert_eq!(seen.lock().unwrap().len(), 1, "one IFTSTA on the wire");
    assert_eq!(
        seen.lock().unwrap()[0]["command"],
        mako_markt::commands::GPKE_SPERRUNG_BESTAETIGEN
    );
    let row = pg::fetch_order_pg(&pool, id, TENANT)
        .await
        .unwrap()
        .unwrap();
    assert!(row.iftsta_dispatched_at.is_some());
    assert_eq!(
        row.iftsta_ref.as_deref(),
        Some("00000000-0000-0000-0000-0000000021a9")
    );

    // A second order whose command never gets through.
    let other = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("proc"))))
        .await
        .unwrap()
        .unwrap();
    pg::report_outcome(&pool, other, TENANT, &executed())
        .await
        .unwrap();
    let (down, _) = fake_makod(503).await;
    let worker = processd::sperr::iftsta_worker(pool.clone(), down, key).with_config(fast);
    for _ in 0..=mako_service::outbox::OutboxConfig::default().max_attempts {
        worker.flush_once().await.unwrap();
    }
    assert!(processd::sperr::worker::sweep(&pool, TENANT).await.unwrap());
    assert_eq!(ausstehend(&pool).await, 1, "escalated from the dead letter");
}

/// Each state change of an order commits with the CloudEvent that announces
/// it: the arrival, a failed Sperrversuch and the terminal outcome. Enqueued on
/// a transaction of its own after the state committed, a failure between the
/// two leaves the order moved and nothing announcing it — and nothing moves it
/// again, so the event is never sent.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn every_state_change_commits_with_its_announcement() {
    let Some((pool, _pg)) = setup().await else {
        return;
    };
    let announced = |ce_type: &'static str, id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM event_outbox
                  WHERE ce_type = $1 AND envelope->'data'->>'order_id' = $2",
            )
            .bind(ce_type)
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    let id = pg::create_order_pg(&pool, TENANT, &order(Some(uniq("announce"))))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        announced(mako_events::sperr::AUFTRAG_EINGEGANGEN, id).await,
        1,
        "the order commits with its arrival"
    );
    let versuch = pg::Outcome::Failed {
        reason: "niemand angetroffen",
        pruefschritt_code: Some("A04"),
        endgueltig: false,
    };
    pg::report_outcome(&pool, id, TENANT, &versuch)
        .await
        .unwrap();
    assert_eq!(
        announced(mako_events::sperr::VERSUCH_GESCHEITERT, id).await,
        1,
        "a Sperrversuch commits with its announcement"
    );
    pg::report_outcome(&pool, id, TENANT, &executed())
        .await
        .unwrap();
    assert_eq!(
        announced(mako_events::sperr::AUSGEFUEHRT, id).await,
        1,
        "the outcome commits with its announcement"
    );
}
