//! Real-PostgreSQL tests for `invoic_receipts` — the § 147 AO audit trail and
//! the ERP notification it concludes with.
//!
//! Uses testcontainers (self-managing Docker); skipped gracefully when no
//! Docker daemon is reachable.
//!
//! # Why this exists
//!
//! Every statement here is a runtime-checked SQL string against a schema the
//! compiler never sees. Constraint spellings, `INTERVAL` arithmetic and the
//! locking semantics of a pooled query are all things that compile and only a
//! live database can rule on.

use time::OffsetDateTime;
use time::macros::datetime;
use uuid::Uuid;

use invoicd::pg::ReceiptRow;
use invoicd::pg::receipts::{dispatch_target, store_antwort, upsert_receipt};

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "9900111000002";

async fn pg_pool() -> Option<(sqlx::PgPool, PgContainer)> {
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

    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .ok()?;
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("apply invoicd schema");
    mako_service::outbox::ensure_schema(&pool)
        .await
        .expect("outbox schema");
    Some((pool, container))
}

fn receipt(process_id: Uuid) -> ReceiptRow {
    ReceiptRow {
        process_id,
        invoice_ref: "INV-2026-000123".to_owned(),
        rechnungsnummer: Some("R-2026-000123".to_owned()),
        pid: 31002,
        sender_mp_id: "9900357000004".to_owned(),
        receiver_gln: TENANT.to_owned(),
        malo_id: Some("51238696012".to_owned()),
        rechnung: serde_json::json!({ "_typ": "RECHNUNG" }),
        bo4e_version: "v202607.0.0".to_owned(),
        outcome: "Ok".to_owned(),
        findings: serde_json::json!([]),
        pay_by: Some(datetime!(2026-03-01 00:00 UTC)),
        received_at: OffsetDateTime::now_utc(),
        checked_at: OffsetDateTime::now_utc(),
        dispatched_at: None,
        tenant: TENANT.to_owned(),
        bestellung_ref: None,
        rechnungstyp: None,
        offen: serde_json::json!([]),
    }
}

macro_rules! pool_or_skip {
    () => {
        match pg_pool().await {
            Some(v) => v,
            None => {
                eprintln!("skipping: no Docker daemon");
                return;
            }
        }
    };
}

/// Two tenants may hold the same `process_id`, and neither write reaches the
/// other's row.
///
/// `process_id` is read from the inbound CloudEvent's `subject`. Keyed
/// globally, one sender's redelivery rewrites the outcome, the findings and the
/// Zahlungsziel of another tenant's Buchungsbeleg — a § 147 AO record altered
/// by a party that never saw it.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_process_id_is_unique_per_tenant_and_not_across_them() {
    let (pool, _guard) = pool_or_skip!();
    const OTHER: &str = "9900357000004";

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id))
        .await
        .expect("the first tenant stores its receipt");

    let mut foreign = receipt(id);
    foreign.tenant = OTHER.to_owned();
    foreign.outcome = "Dispute".to_owned();
    upsert_receipt(&pool, &foreign)
        .await
        .expect("the same process id under another tenant is its own row");

    let outcomes: Vec<(String, String)> =
        sqlx::query_as("SELECT tenant, outcome FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_all(&pool)
            .await
            .expect("read both rows back");
    assert_eq!(outcomes.len(), 2, "one row per tenant");
    assert_eq!(
        outcomes
            .iter()
            .find(|(t, _)| t == TENANT)
            .map(|(_, o)| o.as_str()),
        Some("Ok"),
        "the second tenant's write reached the first tenant's receipt"
    );

    // Within one tenant the pair is still unique: a redelivery updates.
    let mut again = receipt(id);
    again.outcome = "Warn".to_owned();
    upsert_receipt(&pool, &again)
        .await
        .expect("a redelivery is an update");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("count");
    assert_eq!(rows, 2, "a redelivery must not add a row");
}

/// A redelivery refreshes the check result and never loses the MaLo.
///
/// `malo_id` is genuinely absent from some payloads — it is read from the
/// Rechnung when present and from the event otherwise — so the upsert
/// `COALESCE`s it rather than taking `EXCLUDED`. Overwriting it with `NULL`
/// would drop the row out of `GET /api/v1/zahlungsstatus/{malo_id}`, which is
/// the only view that answers "has this delivery point's invoice been paid".
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_redelivery_refreshes_the_check_and_keeps_the_malo() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id))
        .await
        .expect("first delivery");

    let mut second = receipt(id);
    second.malo_id = None;
    second.outcome = "Warn".to_owned();
    second.findings = serde_json::json!([{ "kind": "TariffDeviation" }]);
    upsert_receipt(&pool, &second).await.expect("redelivery");

    let (invoice_ref, malo_id, outcome, findings): (
        Option<String>,
        Option<String>,
        String,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT invoice_ref, malo_id, outcome, findings FROM invoic_receipts WHERE process_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("read back");

    assert_eq!(invoice_ref.as_deref(), Some("INV-2026-000123"));
    assert_eq!(malo_id.as_deref(), Some("51238696012"), "the MaLo survives");
    assert_eq!(outcome, "Warn", "the fresh check result does replace");
    assert_eq!(
        findings.as_array().map(Vec::len),
        Some(1),
        "and so do the findings"
    );
}

/// A re-dispatch reads the routing key and the answering PID from the receipt.
/// Sending `process_id` in the message reference's place reached no workflow.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_redispatch_finds_the_message_reference_and_pid() {
    let (pool, _guard) = pool_or_skip!();

    let process_id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(process_id))
        .await
        .expect("persist");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM invoic_receipts WHERE process_id = $1")
        .bind(process_id)
        .fetch_one(&pool)
        .await
        .expect("read id");

    let target = dispatch_target(&pool, id, TENANT)
        .await
        .expect("query")
        .expect("found");
    assert_eq!(target.process_id, process_id);
    assert_eq!(target.invoice_ref.as_deref(), Some("INV-2026-000123"));
    assert_eq!(target.pid, 31002);
    assert!(!target.already_dispatched);
    assert!(
        target.antwort.is_none(),
        "nothing recorded before the check answered"
    );

    // The answer is recorded before it is sent, and a re-dispatch reads back
    // exactly it — the Antwortcode and the German Ablehnungsgrund included.
    let antwort = serde_json::json!({
        "invoice_ref": "INV-2026-000123",
        "antwort_code": "A70",
        "antwort_codeliste": "E_0406",
        "ablehnungsgrund": "Position 1 (Arbeitspreis): Einzelpreis 0,31200 EUR/kWh.",
    });
    store_antwort(&pool, TENANT, process_id, &antwort)
        .await
        .expect("record the answer");
    let target = dispatch_target(&pool, id, TENANT)
        .await
        .expect("query")
        .expect("found");
    assert_eq!(target.antwort, Some(antwort));

    // Another tenant's id is not visible, whatever the row id.
    assert!(
        dispatch_target(&pool, id, "9900999000009")
            .await
            .expect("query")
            .is_none()
    );
}

/// A dispute resolution has its time and its decider or neither — a
/// `Resolved` outcome without them loses when, and by whom, it was closed.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_resolution_cannot_be_recorded_without_its_timestamp() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    let mut row = receipt(id);
    row.outcome = "Resolved".to_owned();
    assert!(
        upsert_receipt(&pool, &row).await.is_err(),
        "'Resolved' without dispute_resolved_at must be refused"
    );

    // The write path that sets both together is accepted.
    row.outcome = "Dispute".to_owned();
    upsert_receipt(&pool, &row).await.expect("persist dispute");
    let receipt_id: Uuid =
        sqlx::query_scalar("SELECT id FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read id");
    assert!(
        invoicd::pg::receipts::resolve_dispute(
            &pool,
            receipt_id,
            TENANT,
            "anna.sachbearbeiterin",
            Some("NB korrigiert")
        )
        .await
        .expect("resolve"),
        "the resolution path sets outcome, timestamp and decider together"
    );
    let by: Option<String> =
        sqlx::query_scalar("SELECT dispute_resolved_by FROM invoic_receipts WHERE id = $1")
            .bind(receipt_id)
            .fetch_one(&pool)
            .await
            .expect("read decider");
    assert_eq!(by.as_deref(), Some("anna.sachbearbeiterin"));

    // A resolution with no decider is refused by the table itself.
    let unattributed =
        sqlx::query("UPDATE invoic_receipts SET dispute_resolved_by = NULL WHERE id = $1")
            .bind(receipt_id)
            .execute(&pool)
            .await;
    assert!(
        unattributed.is_err(),
        "a 'Resolved' row without its decider must be refused"
    );
}

/// The receipt event commits with the receipt's conclusion and survives an ERP
/// outage: undelivered while the ERP is down, delivered once when it returns.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn the_receipt_event_survives_an_erp_outage_and_is_delivered_once() {
    let (pool, _guard) = pool_or_skip!();
    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id)).await.expect("persist");
    let ce = mako_service::CloudEvent::new(
        mako_service::source("invoicd", TENANT),
        mako_events::invoic::RECEIPT_SETTLED,
        id.to_string(),
        serde_json::json!({ "process_id": id.to_string(), "dispatched": true }),
    );
    invoicd::pg::receipts::conclude(
        &pool,
        TENANT,
        id,
        Some(OffsetDateTime::now_utc()),
        Some(&ce),
    )
    .await
    .expect("conclude");

    let fast = mako_service::outbox::OutboxConfig {
        lease: std::time::Duration::ZERO,
        backoff: vec![std::time::Duration::ZERO],
        ..Default::default()
    };
    // The ERP is down: nothing listens on port 9.
    mako_service::outbox::OutboxWorker::new(pool.clone(), "http://127.0.0.1:9/erp", None)
        .with_config(fast.clone())
        .flush_once()
        .await
        .expect("flush");
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM event_outbox WHERE ce_type = $1 AND delivered_at IS NULL",
    )
    .bind(mako_events::invoic::RECEIPT_SETTLED)
    .fetch_one(&pool)
    .await
    .expect("the receipt event is on record");
    assert_eq!(pending, 1, "the event survives the outage");

    // The ERP returns.
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = std::sync::Arc::clone(&calls);
    let app = axum::Router::new().route(
        "/erp",
        axum::routing::post(move || {
            let seen = std::sync::Arc::clone(&seen);
            async move {
                seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/erp", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let worker = mako_service::outbox::OutboxWorker::new(pool.clone(), url, None).with_config(fast);
    worker.flush_once().await.expect("flush");
    worker.flush_once().await.expect("flush");
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "delivered exactly once"
    );
}

// ── What a person states for the Prüfschritte no INVOIC answers ──────────────

use invoicd::pg::feststellungen::{
    Feststellungen, Leistungsnachweis, feststellen, feststellungen_of, gehalten_fuer_bestellung,
    leistung_erbracht, record_leistungsnachweis,
};
use time::macros::date;

fn nachweis(von: time::Date, bis: time::Date, erbracht: bool) -> Leistungsnachweis {
    Leistungsnachweis {
        rechnungssteller_mp_id: "9900357000004".to_owned(),
        bestellung_ref: "ORDERS-17007-1".to_owned(),
        artikel_id: "9990001100002".to_owned(),
        von,
        bis,
        erbracht,
        nachweis: "Werte für März per MSCONS 13027 erhalten".to_owned(),
    }
}

/// Prüfschritt 310 reads the Leistungsnachweise covering the position's
/// period: a covering one answers, an uncovered period stays unknown, and two
/// covering ones that disagree answer neither way.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn a_leistungsnachweis_answers_the_positions_it_covers() {
    let (pool, _pg) = pool_or_skip!();
    record_leistungsnachweis(
        &pool,
        TENANT,
        &nachweis(date!(2026 - 03 - 01), date!(2026 - 03 - 31), true),
        "op",
    )
    .await
    .unwrap();
    let frage = |von, bis| {
        leistung_erbracht(
            &pool,
            TENANT,
            "9900357000004",
            "ORDERS-17007-1",
            "9990001100002",
            von,
            bis,
        )
    };
    assert_eq!(
        frage(date!(2026 - 03 - 01), date!(2026 - 03 - 31))
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        frage(date!(2026 - 03 - 10), date!(2026 - 03 - 10))
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        frage(date!(2026 - 03 - 15), date!(2026 - 04 - 15))
            .await
            .unwrap(),
        None
    );
    record_leistungsnachweis(
        &pool,
        TENANT,
        &nachweis(date!(2026 - 03 - 10), date!(2026 - 03 - 20), false),
        "op",
    )
    .await
    .unwrap();
    assert_eq!(
        frage(date!(2026 - 03 - 12), date!(2026 - 03 - 12))
            .await
            .unwrap(),
        None,
        "a contradiction answers neither way"
    );
    assert!(
        record_leistungsnachweis(
            &pool,
            TENANT,
            &Leistungsnachweis {
                nachweis: " ".to_owned(),
                ..nachweis(date!(2026 - 05 - 01), date!(2026 - 05 - 31), true)
            },
            "op"
        )
        .await
        .is_err(),
        "a statement rests on something"
    );
}

/// Statements land on a held receipt only, merge over earlier ones, and the
/// receipt is found again for the order a Leistungsnachweis names.
#[tokio::test]
#[ignore = "requires Docker (testcontainers PostgreSQL)"]
async fn statements_land_on_a_held_receipt_only() {
    let (pool, _pg) = pool_or_skip!();
    let process_id = Uuid::new_v4();
    let mut row = receipt(process_id);
    row.pid = 31009;
    row.outcome = "Offen".to_owned();
    row.bestellung_ref = Some("ORDERS-17007-1".to_owned());
    upsert_receipt(&pool, &row).await.unwrap();
    let id: Uuid = sqlx::query_scalar("SELECT id FROM invoic_receipts WHERE process_id = $1")
        .bind(process_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut erste = Feststellungen::default();
    erste.leistung_erbracht.insert(1, false);
    erste.preis_wie_angebot.insert(1, true);
    assert!(
        feststellen(
            &pool,
            TENANT,
            id,
            &erste,
            "geprüft",
            "op",
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
    );
    let mut zweite = Feststellungen::default();
    zweite.leistung_erbracht.insert(1, true);
    assert!(
        feststellen(
            &pool,
            TENANT,
            id,
            &zweite,
            "Nachweis nachgereicht",
            "op",
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap()
    );
    let f = feststellungen_of(&pool, TENANT, process_id).await.unwrap();
    assert_eq!(
        f.leistung_erbracht.get(&1),
        Some(&true),
        "the later statement wins"
    );
    assert_eq!(
        f.preis_wie_angebot.get(&1),
        Some(&true),
        "the earlier one stays"
    );

    let gehalten = gehalten_fuer_bestellung(&pool, TENANT, "9900357000004", "ORDERS-17007-1")
        .await
        .unwrap();
    assert_eq!(gehalten.len(), 1);
    assert_eq!(gehalten[0].data["bestellung_ref"], "ORDERS-17007-1");

    row.outcome = "Ok".to_owned();
    upsert_receipt(&pool, &row).await.unwrap();
    assert!(
        !feststellen(
            &pool,
            TENANT,
            id,
            &zweite,
            "zu spät",
            "op",
            OffsetDateTime::now_utc()
        )
        .await
        .unwrap(),
        "a decided receipt takes no statement"
    );
}
