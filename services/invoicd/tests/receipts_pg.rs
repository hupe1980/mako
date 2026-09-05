//! Real-PostgreSQL tests for `invoic_receipts` — the § 147 AO audit trail and
//! the ERP outbox.
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
use invoicd::pg::receipts::{
    DEAD_LETTER_ATTEMPTS, DIRECTION_INBOUND, DIRECTION_OUTBOUND, claim_erp_pending,
    dead_letter_erp, dispatch_target, record_erp_failure, upsert_receipt,
};

/// Container guard the test holds until it ends — dropping it removes the
/// container (no leak, no external reaper).
type PgContainer = testcontainers::ContainerAsync<testcontainers_modules::postgres::Postgres>;

const TENANT: &str = "9900111000002";
/// Long enough that a second claim in the same test cannot re-take the rows.
const LEASE: i64 = 30;

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
    Some((pool, container))
}

fn receipt(process_id: Uuid, direction: &str) -> ReceiptRow {
    ReceiptRow {
        process_id,
        invoice_ref: Some("INV-2026-000123".to_owned()),
        rechnungsnummer: Some("R-2026-000123".to_owned()),
        pid: 31002,
        direction: direction.to_owned(),
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

/// Both direction constants satisfy the `direction` CHECK — the writers and the
/// schema agree. A capitalised literal rejects every INSERT.
#[tokio::test]
async fn both_direction_constants_pass_the_check() {
    let (pool, _guard) = pool_or_skip!();

    for direction in [DIRECTION_INBOUND, DIRECTION_OUTBOUND] {
        let id = Uuid::new_v4();
        upsert_receipt(&pool, &receipt(id, direction))
            .await
            .unwrap_or_else(|e| panic!("{direction} must satisfy the direction CHECK: {e}"));

        let stored: String =
            sqlx::query_scalar("SELECT direction FROM invoic_receipts WHERE process_id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .expect("read the receipt back");
        assert_eq!(stored, direction);
    }

    let err = upsert_receipt(&pool, &receipt(Uuid::new_v4(), "Inbound")).await;
    assert!(err.is_err(), "'Inbound' violates the direction CHECK");
}

/// An inbound receipt without its INVOIC message reference is refused.
///
/// It could be checked but never answered: `makod` routes the REMADV by that
/// reference and nothing else. Discovering it at the Zahlungsziel instead of at
/// the INSERT is a day too late.
#[tokio::test]
async fn an_inbound_receipt_without_a_message_reference_is_refused() {
    let (pool, _guard) = pool_or_skip!();

    let mut row = receipt(Uuid::new_v4(), DIRECTION_INBOUND);
    row.invoice_ref = None;
    assert!(
        upsert_receipt(&pool, &row).await.is_err(),
        "an unanswerable inbound receipt must not be stored"
    );

    // A self-issued outbound document has no inbound message reference and is
    // identified by its own invoice number instead.
    let mut out = receipt(Uuid::new_v4(), DIRECTION_OUTBOUND);
    out.invoice_ref = None;
    upsert_receipt(&pool, &out)
        .await
        .expect("outbound needs no inbound reference");
}

/// A redelivery refreshes the check result and never loses the MaLo.
///
/// `malo_id` is genuinely absent from some payloads — it is read from the
/// Rechnung when present and from the event otherwise — so the upsert
/// `COALESCE`s it rather than taking `EXCLUDED`. Overwriting it with `NULL`
/// would drop the row out of `GET /api/v1/zahlungsstatus/{malo_id}`, which is
/// the only view that answers "has this delivery point's invoice been paid".
#[tokio::test]
async fn a_redelivery_refreshes_the_check_and_keeps_the_malo() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("first delivery");

    let mut second = receipt(id, DIRECTION_INBOUND);
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

/// The message-reference constraint is enforced on **every** write, including a
/// redelivery — PostgreSQL evaluates a table CHECK on the proposed tuple before
/// the `ON CONFLICT` arbiter, so a thin redelivery is refused rather than
/// silently nulling the routing key.
///
/// The ingest path relies on it: `handler::extract` refuses an event with no
/// `invoice_ref` and dead-letters it, so a receipt without one can only arrive
/// through a bug, and this makes that bug loud.
#[tokio::test]
async fn the_message_reference_constraint_holds_on_redelivery_too() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("first delivery");

    let mut thin = receipt(id, DIRECTION_INBOUND);
    thin.invoice_ref = None;
    assert!(
        upsert_receipt(&pool, &thin).await.is_err(),
        "a redelivery without the routing key must be refused, not silently applied"
    );

    let invoice_ref: Option<String> =
        sqlx::query_scalar("SELECT invoice_ref FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read back");
    assert_eq!(
        invoice_ref.as_deref(),
        Some("INV-2026-000123"),
        "the stored reference is untouched by the refused write"
    );
}

/// A re-dispatch reads the routing key and the answering PID from the receipt.
/// Sending `process_id` in the message reference's place reached no workflow.
#[tokio::test]
async fn a_redispatch_finds_the_message_reference_and_pid() {
    let (pool, _guard) = pool_or_skip!();

    let process_id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(process_id, DIRECTION_INBOUND))
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

    // Another tenant's id is not visible, whatever the row id.
    assert!(
        dispatch_target(&pool, id, "9900999000009")
            .await
            .expect("query")
            .is_none()
    );
}

/// A permanently rejected receipt leaves the outbox for good. The attempt cap
/// is what makes it unselectable: `erp_next_attempt_at` is `NOT NULL` and
/// cannot carry a sentinel.
#[tokio::test]
async fn a_dead_lettered_receipt_is_never_claimed_again() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("persist the receipt");

    let pending = claim_erp_pending(&pool, TENANT, 10, 0)
        .await
        .expect("claim");
    assert_eq!(pending.len(), 1, "a fresh receipt is due immediately");

    dead_letter_erp(&pool, id).await.expect("dead-letter");

    let attempts: i16 =
        sqlx::query_scalar("SELECT erp_attempts FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read attempts");
    assert_eq!(attempts, DEAD_LETTER_ATTEMPTS);

    let pending = claim_erp_pending(&pool, TENANT, 10, 0)
        .await
        .expect("claim");
    assert!(
        pending.is_empty(),
        "a dead-lettered row is out of the outbox"
    );
}

/// The retry budget terminates **without any outcome write at all**.
///
/// Every claim is an attempt, and the claim is what counts it. This is the
/// property that survives the bad day: a worker killed between the POST and the
/// outcome update, or a database that refuses the update, records nothing — and
/// when the counter lived in that update, the row came back due with its budget
/// untouched and the ERP was POSTed the same receipt every lease period for
/// ever. Not one `record_erp_failure` or `dead_letter_erp` runs here.
#[tokio::test]
async fn the_retry_budget_terminates_even_when_no_outcome_is_ever_recorded() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("persist the receipt");

    // A zero lease makes the row due again at once, so this is the retry loop
    // compressed in time.
    for attempt in 0..DEAD_LETTER_ATTEMPTS {
        let claimed = claim_erp_pending(&pool, TENANT, 10, 0)
            .await
            .expect("claim");
        assert_eq!(claimed.len(), 1, "attempt {attempt} should still be due");
        assert_eq!(
            claimed[0].erp_attempts, attempt,
            "the claim must report the attempts made before this one"
        );
    }

    let attempts: i16 =
        sqlx::query_scalar("SELECT erp_attempts FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read attempts");
    assert_eq!(attempts, DEAD_LETTER_ATTEMPTS, "the budget is exhausted");

    let pending = claim_erp_pending(&pool, TENANT, 10, 0)
        .await
        .expect("claim");
    assert!(
        pending.is_empty(),
        "the receipt is still being POSTed to the ERP after its budget ran out — the \
         attempt counter did not advance because no outcome write ever landed"
    );
}

/// The back-off schedule is `record_erp_failure`'s only job now, and an
/// over-large terminal delay would raise `interval out of range` — which used to
/// abort the very UPDATE that raised the counter, leaving the row at 4 and
/// retrying for ever.
#[tokio::test]
async fn the_terminal_backoff_is_an_interval_postgresql_accepts() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    upsert_receipt(&pool, &receipt(id, DIRECTION_INBOUND))
        .await
        .expect("persist the receipt");

    for attempt in 0..DEAD_LETTER_ATTEMPTS {
        record_erp_failure(&pool, id, attempt)
            .await
            .unwrap_or_else(|e| panic!("attempt {attempt} must be schedulable: {e}"));
    }

    let attempts: i16 =
        sqlx::query_scalar("SELECT erp_attempts FROM invoic_receipts WHERE process_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("read attempts");
    assert_eq!(
        attempts, 0,
        "scheduling a back-off must not touch the counter — the claim owns it"
    );
}

/// A claimed batch is not claimable again while its lease holds.
///
/// A pooled `SELECT … FOR UPDATE SKIP LOCKED` does not give this: sqlx runs it
/// in an implicit transaction that commits before the caller sees the rows, so
/// a second worker — a rolling deploy is enough — claims the same batch and the
/// ERP gets every event twice.
#[tokio::test]
async fn a_claimed_batch_is_leased_against_a_second_worker() {
    let (pool, _guard) = pool_or_skip!();

    for _ in 0..3 {
        upsert_receipt(&pool, &receipt(Uuid::new_v4(), DIRECTION_INBOUND))
            .await
            .expect("persist");
    }

    let first = claim_erp_pending(&pool, TENANT, 10, LEASE)
        .await
        .expect("first worker claims");
    assert_eq!(first.len(), 3);

    let second = claim_erp_pending(&pool, TENANT, 10, LEASE)
        .await
        .expect("second worker tries");
    assert!(
        second.is_empty(),
        "a leased batch must not be handed to a second worker: {second:?}"
    );
}

/// The claim never crosses a tenant boundary, and it reports the state the
/// event carries.
#[tokio::test]
async fn the_claim_is_tenant_scoped_and_carries_the_dispatch_state() {
    let (pool, _guard) = pool_or_skip!();

    let mine = Uuid::new_v4();
    let mut row = receipt(mine, DIRECTION_INBOUND);
    row.dispatched_at = Some(OffsetDateTime::now_utc());
    row.findings = serde_json::json!([{ "kind": "TariffDeviation" }, { "kind": "PeriodInvalid" }]);
    upsert_receipt(&pool, &row).await.expect("persist mine");

    let mut theirs = receipt(Uuid::new_v4(), DIRECTION_INBOUND);
    theirs.tenant = "9900999000009".to_owned();
    upsert_receipt(&pool, &theirs)
        .await
        .expect("persist theirs");

    let claimed = claim_erp_pending(&pool, TENANT, 10, LEASE)
        .await
        .expect("claim");
    assert_eq!(claimed.len(), 1, "another tenant's receipt is not claimed");
    let row = &claimed[0];
    assert_eq!(row.process_id, mine);
    assert!(row.dispatched, "the answer went out");
    // `jsonb_array_length` returns int4; without the `::bigint` cast the whole
    // claim failed to decode and the outbox looked permanently empty.
    assert_eq!(row.findings_count, 2);
}

/// A dispute resolution has both parts or neither — a `Resolved` outcome
/// without the timestamp loses when the operator closed it.
#[tokio::test]
async fn a_resolution_cannot_be_recorded_without_its_timestamp() {
    let (pool, _guard) = pool_or_skip!();

    let id = Uuid::new_v4();
    let mut row = receipt(id, DIRECTION_INBOUND);
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
        invoicd::pg::receipts::resolve_dispute(&pool, receipt_id, TENANT, Some("NB korrigiert"))
            .await
            .expect("resolve"),
        "the resolution path sets outcome and timestamp together"
    );
}
