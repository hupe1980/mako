//! Integration tests for `processd` PostgreSQL queries using testcontainers.
//!
//! # Why testcontainers?
//!
//! All processd SQL queries use `sqlx::query(…)` — dynamic, untype-checked.
//! This test module spins up a real PostgreSQL container, applies all migrations,
//! and exercises every query in the `pg` module to catch:
//!
//! - Column name mismatches (e.g. after a rename migration)
//! - Missing NOT NULL constraints surfaced at insert time
//! - Type-binding errors (e.g. wrong type for a UUID column)
//! - Missing rows / incorrect WHERE clauses
//!
//! These are exactly the errors that `sqlx::query!()` compile-time macros would
//! catch statically. Until the `.sqlx/` cache is generated (see `just sqlx-prepare`)
//! and the queries are migrated to `query!()`, this test suite is the safety net.
//!
//! # Running
//!
//! ```bash
//! # Requires Docker running locally:
//! cargo test --test sql_integration -p processd
//! ```
//!
//! # sqlx-prepare alternative
//!
//! Run `just sqlx-prepare` to generate the `.sqlx/` offline cache, then
//! migrate queries to `sqlx::query_as!()` for compile-time checking.

use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

// ── Container lifecycle helper ────────────────────────────────────────────────

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

async fn pg_pool() -> (sqlx::PgPool, PgContainer) {
    use testcontainers::ImageExt;
    let container = Postgres::default()
        .with_tag("17-alpine")
        .start()
        .await
        .expect("start postgres");
    let port = container.get_host_port_ipv4(5432).await.expect("get port");
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");

    (pool, container)
}

// ── Approval queue ────────────────────────────────────────────────────────────

#[tokio::test]
async fn approval_queue_enqueue_list_approve() {
    let (pool, _pg) = pg_pool().await;
    let queue = processd::pg::PgApprovalQueue::new(pool.clone());

    let id = Uuid::new_v4();
    let process_id = Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    use processd::pg::approval::{ApprovalQueueEntry, QueueStatus};

    // Enqueue
    queue
        .enqueue(
            &ApprovalQueueEntry {
                id,
                ..ApprovalQueueEntry::pending(
                    process_id,
                    55001,
                    Some("12345678989".to_owned()),
                    "test E_0624 event".to_owned(),
                    now + time::Duration::minutes(45),
                    "9900357000004".to_owned(),
                )
            }
            .with_commands(
                "gpke.nb-lieferende.bestaetigen",
                "gpke.nb-lieferende.ablehnen",
                None,
            ),
        )
        .await
        .expect("enqueue approval entry");

    // List pending
    let pending = queue
        .list("9900357000004", Some(QueueStatus::Pending), 50)
        .await
        .expect("list pending");
    assert_eq!(pending.len(), 1, "expected 1 pending entry");
    assert_eq!(pending[0].id, id);
    assert_eq!(pending[0].pid, 55001);
    assert!(matches!(pending[0].status, QueueStatus::Pending));

    // Find by ID
    let entry = queue
        .find_by_id(id, "9900357000004")
        .await
        .expect("find_by_id")
        .expect("entry exists");
    assert_eq!(entry.process_id, process_id);
    assert_eq!(
        entry.approve_command.as_deref(),
        Some("gpke.nb-lieferende.bestaetigen")
    );

    // Claim: the decision is taken before any market command is dispatched.
    let claimed = queue
        .claim(
            id,
            "9900357000004",
            QueueStatus::Approved,
            "operator@example.com",
        )
        .await
        .expect("claim")
        .expect("a Pending entry is claimable");
    assert!(matches!(claimed.status, QueueStatus::Approved));
    assert!(claimed.decided_at.is_some(), "the claim stamps decided_at");

    // A second operator cannot claim the same entry — one decision, one command.
    assert!(
        queue
            .claim(
                id,
                "9900357000004",
                QueueStatus::Rejected,
                "operator@example.com"
            )
            .await
            .expect("second claim")
            .is_none(),
        "an already-decided entry must not be claimable again"
    );

    // Releasing the claim (dispatch failed) makes it retryable.
    queue.unclaim(id, "9900357000004").await.expect("unclaim");
    let released = queue
        .find_by_id(id, "9900357000004")
        .await
        .expect("find released")
        .expect("entry still exists");
    assert!(matches!(released.status, QueueStatus::Pending));
    assert!(released.decided_at.is_none());

    queue
        .claim(
            id,
            "9900357000004",
            QueueStatus::Approved,
            "operator@example.com",
        )
        .await
        .expect("re-claim")
        .expect("a released entry is claimable again");

    // Expire stale (no stale entries — all decided)
    let expired_count = queue.expire_stale().await.expect("expire stale");
    assert_eq!(expired_count, 0, "decided entries must not be expired");
}

// ── Anmeldung decisions ───────────────────────────────────────────────────────

#[tokio::test]
async fn anmeldung_decisions_insert_and_list() {
    let (pool, _pg) = pg_pool().await;
    let repo = processd::pg::PgAnmeldungRepository::new(pool.clone());

    use processd::pg::anmeldung::{AnmeldungDecision, AnmeldungDecisionRecord};

    let process_id = Uuid::new_v4();
    let now = time::OffsetDateTime::now_utc();

    repo.insert(&AnmeldungDecisionRecord {
        id: Uuid::new_v4(),
        process_id,
        pid: 55001,
        malo_id: "12345678989".to_owned(),
        lf_mp_id: "9900100000001".to_owned(),
        decision: AnmeldungDecision::Accept,
        antwortcode: None,
        detail: None,
        initiator_is_affiliate: false,
        decided_at: now,
        tenant: "9900357000004".to_owned(),
    })
    .await
    .expect("insert Accept decision");

    let records = repo
        .list("9900357000004", 50)
        .await
        .expect("list decisions");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].process_id, process_id);
    assert!(matches!(records[0].decision, AnmeldungDecision::Accept));

    // STP rate: 1 accept out of 1 → 100%
    let rate = repo
        .stp_rate("9900357000004", 7)
        .await
        .expect("stp_rate")
        .unwrap_or(0.0);
    assert!(
        (rate - 1.0).abs() < f64::EPSILON,
        "100% STP rate for single Accept, got {rate}"
    );
}

// ── Neuanlage — the E_0608 60-Werktage Prüflauf ──────────────────────────────

/// GPKE Teil 2 § 2.2.2 and `E_0608` Prüfschritte 110 / 590: an Anmeldung whose
/// Marktlokation cannot be identified is re-checked daily and only refused once
/// the 60-Werktage window has run out.
///
/// The case log is what makes that possible — a decision engine with no memory
/// between runs cannot count Werktage or evidence the daily attempts.
#[tokio::test]
async fn neuanlage_pruflauf_defers_records_and_resolves() {
    use mako_pruefung::nb::types::Marktlokationsart;
    use processd::pg::neuanlage::{
        NewNeuanlageFall, close_case, due_for_pruefung, fetch_case, list_cases, open_case,
        record_pruefung, set_identifikation,
    };

    let (pool, _pg) = pg_pool().await;
    const TENANT: &str = "9900357000004";
    let process_id = Uuid::new_v4();

    let ut = time::Date::from_calendar_date(2026, time::Month::March, 4).expect("valid");
    let letzter = mako_fristen::add_werktage(ut, 60, mako_fristen::HolidayCalendar::BdewMaKo);
    let new = NewNeuanlageFall {
        process_id,
        pid: 55_600,
        lf_mp_id: "9900100000001".to_owned(),
        marktlokationsart: Marktlokationsart::Verbrauchend,
        veraeusserungsform: None,
        uebertragungstag: ut,
        zuordnungsbeginn: time::Date::from_calendar_date(2026, time::Month::April, 1)
            .expect("valid"),
        letzter_pruefungstag: letzter,
    };

    let id = open_case(&pool, TENANT, &new)
        .await
        .expect("open case")
        .expect("a new case");

    // A redelivered ORDERS must not restart the Prüflauf clock.
    assert!(
        open_case(&pool, TENANT, &new)
            .await
            .expect("second open")
            .is_none(),
        "a redelivery reuses the open case"
    );

    // Day one: due, because no Prüfung has run.
    let today = time::Date::from_calendar_date(2026, time::Month::March, 5).expect("valid");
    let due = due_for_pruefung(&pool, TENANT, today, 100)
        .await
        .expect("due");
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].id, id);
    assert_eq!(due[0].letzter_pruefungstag, letzter);

    // Recording today's Prüfung takes it out of today's batch and counts it —
    // the evidence that the daily obligation was met.
    record_pruefung(&pool, id, TENANT, today)
        .await
        .expect("record");
    assert!(
        due_for_pruefung(&pool, TENANT, today, 100)
            .await
            .expect("due")
            .is_empty(),
        "a case checked today is not due again today"
    );
    let after = fetch_case(&pool, id, TENANT)
        .await
        .expect("fetch")
        .expect("present");
    assert_eq!(after.pruefungen, 1);
    assert_eq!(after.letzte_pruefung_am, Some(today));

    // Tomorrow it is due again.
    let tomorrow = today.next_day().expect("next");
    assert_eq!(
        due_for_pruefung(&pool, TENANT, tomorrow, 100)
            .await
            .expect("due")
            .len(),
        1
    );

    // The operator identifies the Marktlokation: the case becomes due at once,
    // because the next pass now has something to walk the tree with.
    assert!(
        set_identifikation(&pool, id, TENANT, "51238696012")
            .await
            .expect("identify")
    );
    let due = due_for_pruefung(&pool, TENANT, today, 100)
        .await
        .expect("due");
    assert_eq!(due.len(), 1, "identification re-arms today's pass");
    assert_eq!(due[0].malo_id.as_deref(), Some("51238696012"));

    // Answering closes it and states what went out.
    close_case(&pool, id, TENANT, "A09", None)
        .await
        .expect("close");
    let closed = fetch_case(&pool, id, TENANT)
        .await
        .expect("fetch")
        .expect("present");
    assert_eq!(closed.status, "beantwortet");
    assert_eq!(closed.antwortcode.as_deref(), Some("A09"));
    assert!(closed.beantwortet_at.is_some());
    assert!(
        due_for_pruefung(&pool, TENANT, tomorrow, 100)
            .await
            .expect("due")
            .is_empty(),
        "a closed case leaves the Prüflauf"
    );

    // The operator view filters by status.
    let offen = list_cases(&pool, TENANT, Some("offen"), 50)
        .await
        .expect("list");
    assert!(offen.is_empty());
    let all = list_cases(&pool, TENANT, None, 50).await.expect("list");
    assert_eq!(all.len(), 1);
}

/// The `beantwortet` status may not exist without the code that was sent —
/// `SG4 STS+E01` is Muss, so „answered, code unknown" is not a state.
#[tokio::test]
async fn a_closed_neuanlage_case_must_state_its_antwortcode() {
    let (pool, _pg) = pg_pool().await;
    let err = sqlx::query(
        r"INSERT INTO neuanlage_faelle
              (tenant, process_id, pid, lf_mp_id, marktlokationsart, uebertragungstag,
               zuordnungsbeginn, letzter_pruefungstag, status)
          VALUES ('t', gen_random_uuid(), 55600, 'lf', 'VERBRAUCHEND',
                  DATE '2026-03-04', DATE '2026-04-01', DATE '2026-05-28', 'beantwortet')",
    )
    .execute(&pool)
    .await
    .expect_err("the CHECK must refuse a coded-less answer");
    assert!(
        err.to_string().contains("neuanlage_faelle"),
        "expected the table CHECK, got: {err}"
    );
}
