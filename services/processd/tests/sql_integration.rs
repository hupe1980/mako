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
//! catch statically. `processd`'s queries are the runtime `sqlx::query` form, so
//! this suite is what checks them.
//!
//! # Running
//!
//! ```bash
//! # Requires Docker running locally:
//! cargo test --test sql_integration -p processd
//! ```
//!
//! Moving to `sqlx::query_as!()` would replace it with a compile-time check,
//! and needs an offline `.sqlx/` cache the workspace does not carry.

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
            )
            .with_followup(
                "gpke.zuordnung.beenden",
                serde_json::json!({
                    "malo_id": "12345678989",
                    "empfaenger_mp_id": "9900111000002",
                    "transaktionsgrund": "ZC8",
                    "process_date": "2026-10-01",
                }),
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
    // The Meldepflicht rides the claim. It is dispatched after the answer and
    // states what was true when the decision was taken, so it has to come back
    // off the row rather than be rebuilt at approval time.
    assert_eq!(
        claimed.followup_command.as_deref(),
        Some("gpke.zuordnung.beenden")
    );
    assert_eq!(
        claimed
            .followup_payload
            .as_ref()
            .and_then(|p| p.get("process_date"))
            .and_then(serde_json::Value::as_str),
        Some("2026-10-01")
    );

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
///
/// `E_0608` is an NB tree, so `pg::neuanlage` compiles only for an NB build —
/// an `msb-only` or `lf-only` binary has no Prüflauf to remember.
#[cfg(any(feature = "role-nb-strom", feature = "role-nb-gas"))]
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

// ── The two-phase Anmeldung ───────────────────────────────────────────────────

/// The Meldepflicht facts survive the wait for the LFA.
///
/// A Geschäftsvorfall 3 asks **every** Tranchen-LFA, and `E_0623` Prüfschritte
/// 510/520 count over all their answers — so the waiting row must not resolve on
/// the first one to arrive. It resolves when the last outstanding LFA answers,
/// or when the 09:00 Frist lapses and the rest are silence.
#[tokio::test]
async fn a_tranchierte_anmeldung_waits_for_every_lfa() {
    let (pool, _pg) = pg_pool().await;
    let repo = processd::pg::abmeldeanfrage::PgAbmeldeanfrageRepository::new(pool);
    let tenant = "9900357000004";
    let process_id = Uuid::new_v4();
    let lfa = ["9900111000002", "9900111000003", "9900111000004"];

    let rec = processd::pg::abmeldeanfrage::AbmeldeanfrageRecord {
        antworten: serde_json::json!({}),
        anmeldung_process_id: process_id,
        malo_id: "51238696781".to_owned(),
        lfn_mp_id: "9900555000005".to_owned(),
        lfa_mp_ids: lfa.iter().map(|s| (*s).to_owned()).collect(),
        pid: 55077,
        anfrage: serde_json::json!({ "pid": 55077 }),
        meldung: serde_json::json!({}),
        received_at: time::OffsetDateTime::now_utc(),
        tenant: tenant.to_owned(),
    };
    repo.record(&rec).await.expect("record");

    let antwort = |code: &str| serde_json::json!({ "antwortcode": code, "zustimmung": true });

    // The first two leave the row waiting: two thirds of the Marktlokation is
    // not the Marktlokation.
    for lf in &lfa[..2] {
        assert!(
            repo.record_antwort(process_id, tenant, lf, &antwort("A40"), false)
                .await
                .expect("record answer")
                .is_none(),
            "{lf} answered, but others are still outstanding"
        );
    }

    // The last one resolves it, and every answer is there to count.
    let resolved = repo
        .record_antwort(process_id, tenant, lfa[2], &antwort("A40"), false)
        .await
        .expect("record last answer")
        .expect("the last answer resolves the row");
    for lf in &lfa {
        assert_eq!(
            resolved.antworten[*lf]["antwortcode"], "A40",
            "every LFA's answer is carried into the decision"
        );
    }

    // And it stays resolved: a redelivered answer must not decide twice.
    assert!(
        repo.record_antwort(process_id, tenant, lfa[0], &antwort("A40"), false)
            .await
            .expect("redelivery")
            .is_none()
    );
}

/// A lapsed 09:00 Frist resolves the row whatever is still outstanding: the LFA
/// that did not answer answered with silence, which GPKE Teil 2 § 2.1.2 makes a
/// Zustimmung — „Verstreicht die Frist, ohne dass eine Antwort beim NB eingeht,
/// gilt dies als Bestätigung nach Fall a)".
#[tokio::test]
async fn a_lapsed_frist_resolves_a_tranchierte_anmeldung_at_once() {
    let (pool, _pg) = pg_pool().await;
    let repo = processd::pg::abmeldeanfrage::PgAbmeldeanfrageRepository::new(pool);
    let tenant = "9900357000004";
    let process_id = Uuid::new_v4();

    repo.record(&processd::pg::abmeldeanfrage::AbmeldeanfrageRecord {
        antworten: serde_json::json!({}),
        anmeldung_process_id: process_id,
        malo_id: "51238696781".to_owned(),
        lfn_mp_id: "9900555000005".to_owned(),
        lfa_mp_ids: vec!["9900111000002".to_owned(), "9900111000003".to_owned()],
        pid: 55077,
        anfrage: serde_json::json!({ "pid": 55077 }),
        meldung: serde_json::json!({}),
        received_at: time::OffsetDateTime::now_utc(),
        tenant: tenant.to_owned(),
    })
    .await
    .expect("record");

    // One of the two answered; the Frist lapses on the other.
    let resolved = repo
        .record_antwort(
            process_id,
            tenant,
            "9900111000002",
            &serde_json::json!({ "antwortcode": null }),
            true,
        )
        .await
        .expect("lapse")
        .expect("a lapsed Frist resolves the row even with an LFA outstanding");
    assert!(
        resolved.antworten.get("9900111000003").is_none(),
        "the silent LFA carries no entry — the tree reads that as a Zustimmung"
    );
}

/// Phase one writes the Anmeldung's facts and phase two runs hours later, when
/// the LFA has answered or its 09:00 window has lapsed. The Beendigung der
/// Zuordnung (55037 / 44037) states the **Altlieferant** and the
/// **Zuordnungsende** — neither of which is in the serialised `AnmeldungAnfrage`
/// and both of which the projection has moved on from by then — so they travel
/// with the waiting row or the Meldung cannot be sent at all.
#[tokio::test]
async fn a_waiting_anmeldung_carries_its_meldepflicht_facts() {
    let (pool, _pg) = pg_pool().await;
    let repo = processd::pg::abmeldeanfrage::PgAbmeldeanfrageRepository::new(pool);
    let tenant = "9900357000004";
    let process_id = Uuid::new_v4();

    let meldung = serde_json::json!({
        "sparte": "strom",
        "lfn_mp_id": "9900555000005",
        "zuordnungsbeginn": "2026-10-01",
        "vorgangsnummer": "VG-4711",
        "tranche": false,
        "altlieferant": "9900111000002",
    });
    let rec = processd::pg::abmeldeanfrage::AbmeldeanfrageRecord {
        antworten: serde_json::json!({}),
        anmeldung_process_id: process_id,
        malo_id: "51238696781".to_owned(),
        lfn_mp_id: "9900555000005".to_owned(),
        lfa_mp_ids: vec!["9900111000002".to_owned()],
        pid: 55001,
        anfrage: serde_json::json!({ "pid": 55001 }),
        meldung: meldung.clone(),
        received_at: time::OffsetDateTime::now_utc(),
        tenant: tenant.to_owned(),
    };

    use processd::pg::abmeldeanfrage::Waiting;
    assert_eq!(
        repo.record(&rec).await.expect("record"),
        Waiting::Recorded,
        "first write lands"
    );
    // The row is written before the Anfrage goes out, so a redelivery that
    // arrives before the dispatch succeeded must send it rather than return —
    // nothing else ever will: no 55010 means no 09:00 window and no lapse.
    assert_eq!(
        repo.record(&rec).await.expect("re-record"),
        Waiting::Unsent,
        "a row whose Anfrage never went out is not a duplicate"
    );
    repo.mark_anfrage_sent(process_id, tenant)
        .await
        .expect("stamp the dispatch");
    assert_eq!(
        repo.record(&rec).await.expect("re-record after dispatch"),
        Waiting::AlreadySent,
        "a redelivered Anmeldung must not ask the LFA twice"
    );

    let pending = repo.list_pending(tenant, 10).await.expect("list pending");
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].meldung, meldung,
        "the operator view carries it too"
    );

    let taken = repo
        .take(process_id, tenant)
        .await
        .expect("take")
        .expect("the row is waiting");
    assert_eq!(
        taken.meldung, meldung,
        "phase two reads back exactly what phase one froze"
    );

    assert!(
        repo.take(process_id, tenant)
            .await
            .expect("re-take")
            .is_none(),
        "the LFA's answer and the 09:00 lapse race; only one may resume"
    );
    // …and once the decision is made, a late redelivery is a duplicate, not an
    // unsent Anfrage: the row is resolved and there is nothing left to send.
    assert_eq!(
        repo.record(&rec).await.expect("record after resolve"),
        Waiting::AlreadySent
    );
}
