//! INVOIC receipt persistence — the `invoic_receipts` table.
//!
//! Every INVOIC `invoicd` handles is written here **before** the REMADV/COMDIS
//! answer is dispatched to `makod`. A received INVOIC is a received invoice — a
//! Buchungsbeleg — so § 147 Abs. 3 AO / § 14b UStG require 8-year retention,
//! complete and unaltered (GoBD).
//!
//! # Idempotency
//!
//! Inserts use `ON CONFLICT (process_id) DO UPDATE`, so a redelivered
//! CloudEvent is safe: the second delivery refreshes the check result and
//! leaves `received_at` alone.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// Direction value for received INVOICs (NB/MSB → LF). Must stay in the
/// `invoic_receipts.direction` CHECK list.
pub const DIRECTION_INBOUND: &str = "inbound";
/// Direction value for self-issued INVOICs (LF → NB, PID 31006).
pub const DIRECTION_OUTBOUND: &str = "outbound";

/// `erp_attempts` value at which an ERP delivery is terminally dead-lettered.
///
/// [`claim_erp_pending`] excludes rows that have reached it, so it is the single
/// definition of "unselectable" — `erp_next_attempt_at` is `NOT NULL` and must
/// never be used as the sentinel.
pub const DEAD_LETTER_ATTEMPTS: i16 = 5;

/// The BO4E schema version a `Rechnung` was read under.
///
/// Server-derived provenance, taken from the type that parsed it rather than
/// written as a literal: a hard-coded release string in four call sites keeps
/// claiming the old version for a whole release after `rubo4e` moves on, and
/// the column is what a later reader uses to decide how to interpret the JSONB.
#[must_use]
pub fn bo4e_version(rechnung: &rubo4e::current::Rechnung) -> &'static str {
    use rubo4e::Bo4eObject as _;
    rechnung.schema_version()
}

/// A row in `invoic_receipts`.
#[derive(Debug)]
pub struct ReceiptRow {
    /// Workflow process ID from the CloudEvent `subject`.
    pub process_id: Uuid,
    /// EDIFACT INVOIC message reference (BGM 1004) — the key `makod` routes the
    /// answer command by. `None` only for self-issued outbound documents.
    pub invoice_ref: Option<String>,
    /// Invoice number, when the document states one.
    pub rechnungsnummer: Option<String>,
    /// BDEW Prüfidentifikator.
    pub pid: i16,
    /// [`DIRECTION_INBOUND`] or [`DIRECTION_OUTBOUND`].
    pub direction: String,
    /// GLN of the party that issued the invoice.
    pub sender_mp_id: String,
    /// GLN of the receiver.
    pub receiver_gln: String,
    /// MaLo-ID extracted at ingest — indexed, so payment-status queries need no
    /// JSONB scan.
    pub malo_id: Option<String>,
    /// The BO4E Rechnung exactly as received.
    pub rechnung: serde_json::Value,
    /// Schema version the Rechnung was read under — see [`bo4e_version`].
    pub bo4e_version: String,
    /// Plausibility outcome; must satisfy the `outcome` CHECK.
    pub outcome: String,
    /// Serialised plausibility findings.
    pub findings: serde_json::Value,
    /// Zahlungsziel from INVOIC `DTM+92`.
    pub pay_by: Option<OffsetDateTime>,
    pub received_at: OffsetDateTime,
    pub checked_at: OffsetDateTime,
    /// Set once the answer command has been accepted by `makod`.
    pub dispatched_at: Option<OffsetDateTime>,
    pub tenant: String,
}

/// Insert or refresh a receipt.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure. The caller must **not** dispatch
/// the market answer when this fails: an answered invoice missing from the
/// audit trail is the failure this table exists to prevent.
pub async fn upsert_receipt(pool: &PgPool, row: &ReceiptRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        r"INSERT INTO invoic_receipts
            (process_id, invoice_ref, rechnungsnummer, pid, direction, sender_mp_id,
             receiver_gln, malo_id, rechnung, bo4e_version, outcome, findings,
             pay_by, received_at, checked_at, dispatched_at, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
          ON CONFLICT (process_id) DO UPDATE SET
            outcome         = EXCLUDED.outcome,
            findings        = EXCLUDED.findings,
            pay_by          = EXCLUDED.pay_by,
            checked_at      = EXCLUDED.checked_at,
            bo4e_version    = EXCLUDED.bo4e_version,
            -- COALESCE, not EXCLUDED: a redelivery whose payload has lost the
            -- MaLo or the message reference must not erase the one already
            -- recorded — the second is what a re-dispatch routes by.
            malo_id         = COALESCE(EXCLUDED.malo_id, invoic_receipts.malo_id),
            invoice_ref     = COALESCE(EXCLUDED.invoice_ref, invoic_receipts.invoice_ref),
            rechnungsnummer = COALESCE(EXCLUDED.rechnungsnummer, invoic_receipts.rechnungsnummer)",
    )
    .bind(row.process_id)
    .bind(row.invoice_ref.as_deref())
    .bind(row.rechnungsnummer.as_deref())
    .bind(row.pid)
    .bind(&row.direction)
    .bind(&row.sender_mp_id)
    .bind(&row.receiver_gln)
    .bind(row.malo_id.as_deref())
    .bind(&row.rechnung)
    .bind(&row.bo4e_version)
    .bind(&row.outcome)
    .bind(&row.findings)
    .bind(row.pay_by)
    .bind(row.received_at)
    .bind(row.checked_at)
    .bind(row.dispatched_at)
    .bind(&row.tenant)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record that the market answer has gone out.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn mark_dispatched(
    pool: &PgPool,
    process_id: Uuid,
    dispatched_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invoic_receipts SET dispatched_at = $1 WHERE process_id = $2")
        .bind(dispatched_at)
        .bind(process_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// What a re-dispatch needs: the answer's routing key and the outcome that
/// decides which command to send.
#[derive(Debug)]
pub struct DispatchTarget {
    pub process_id: Uuid,
    pub pid: i16,
    pub outcome: String,
    /// The EDIFACT message reference. `None` means the answer cannot be routed.
    pub invoice_ref: Option<String>,
    pub already_dispatched: bool,
}

/// Read what is needed to re-dispatch a receipt's answer.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn dispatch_target(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> Result<Option<DispatchTarget>, sqlx::Error> {
    /// `(process_id, pid, outcome, invoice_ref, dispatched_at)`
    type Row = (Uuid, i16, String, Option<String>, Option<OffsetDateTime>);
    let row: Option<Row> = sqlx::query_as(
        r"SELECT process_id, pid, outcome, invoice_ref, dispatched_at
          FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(process_id, pid, outcome, invoice_ref, dispatched_at)| DispatchTarget {
            process_id,
            pid,
            outcome,
            invoice_ref,
            already_dispatched: dispatched_at.is_some(),
        },
    ))
}

/// Mark an ERP notification as delivered.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn mark_erp_notified(
    pool: &PgPool,
    process_id: Uuid,
    delivered_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invoic_receipts SET erp_notified_at = $1 WHERE process_id = $2")
        .bind(delivered_at)
        .bind(process_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Count a failed ERP delivery and schedule the retry.
///
/// Backoff: 30 s → 5 min → 30 min → 2 h, then the attempt cap stops it.
///
/// The terminal attempt keeps the last backoff rather than an "infinite" one:
/// `now() + (i64::MAX/2 * INTERVAL '1 second')` raises `interval out of range`,
/// which aborted the very UPDATE that would have raised `erp_attempts` to the
/// cap — so the row stayed at 4 and was retried forever. [`DEAD_LETTER_ATTEMPTS`]
/// alone makes it unselectable.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn record_erp_failure(
    pool: &PgPool,
    process_id: Uuid,
    attempts: i16,
) -> Result<(), sqlx::Error> {
    let delay_secs: i64 = match attempts {
        0 => 30,
        1 => 300,
        2 => 1_800,
        _ => 7_200,
    };
    sqlx::query(
        r"UPDATE invoic_receipts
          SET erp_attempts = erp_attempts + 1,
              erp_next_attempt_at = now() + ($1 * INTERVAL '1 second')
          WHERE process_id = $2",
    )
    .bind(delay_secs)
    .bind(process_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Dead-letter an ERP notification after a **permanent** rejection (a 4xx — a
/// mis-addressed or malformed webhook no retry can fix).
///
/// Sets `erp_attempts` to the cap in one statement, so the outbox worker never
/// re-queries it and it surfaces as dead-lettered
/// (`erp_attempts >= 5 AND erp_notified_at IS NULL`).
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn dead_letter_erp(pool: &PgPool, process_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invoic_receipts SET erp_attempts = $2 WHERE process_id = $1")
        .bind(process_id)
        .bind(DEAD_LETTER_ATTEMPTS)
        .execute(pool)
        .await?;
    Ok(())
}

/// A receipt awaiting ERP notification, as claimed by the outbox worker.
#[derive(Debug)]
pub struct ErpPendingRow {
    pub process_id: Uuid,
    pub pid: i16,
    pub direction: String,
    pub sender_mp_id: String,
    pub outcome: String,
    pub pay_by: Option<OffsetDateTime>,
    pub findings_count: i64,
    pub erp_attempts: i16,
    pub dispatched: bool,
}

/// Claim the next batch of receipts awaiting ERP notification.
///
/// # Why this is an `UPDATE … RETURNING`
///
/// A `SELECT … FOR UPDATE SKIP LOCKED` on the pool holds nothing: sqlx runs a
/// pooled statement in its own implicit transaction, which commits as the query
/// returns, so the row locks are gone before the caller sees the rows and two
/// replicas claim the same batch.
///
/// Pushing `erp_next_attempt_at` forward *as part of the claim* makes it atomic
/// without holding a transaction across the HTTP deliveries that follow: a
/// concurrent worker's `erp_next_attempt_at <= now()` does not match. The lease
/// equals the poll interval, so a worker that dies mid-batch releases its claim
/// on the next tick instead of stranding the rows.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn claim_erp_pending(
    pool: &PgPool,
    tenant: &str,
    limit: i64,
    lease_secs: i64,
) -> Result<Vec<ErpPendingRow>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            i16,
            String,
            String,
            String,
            Option<OffsetDateTime>,
            i64,
            i16,
            bool,
        ),
    >(
        r"UPDATE invoic_receipts SET erp_next_attempt_at = now() + ($4 * INTERVAL '1 second')
          WHERE process_id IN (
              SELECT process_id FROM invoic_receipts
               WHERE tenant = $1
                 AND erp_notified_at IS NULL
                 AND erp_attempts < $3
                 AND erp_next_attempt_at <= now()
               ORDER BY erp_next_attempt_at
               LIMIT $2
               FOR UPDATE SKIP LOCKED
          )
          RETURNING process_id, pid, direction, sender_mp_id, outcome, pay_by,
                    -- ::bigint: jsonb_array_length returns int4, which does not
                    -- decode into the i64 below — the claim failed on every row.
                    jsonb_array_length(findings)::bigint, erp_attempts,
                    dispatched_at IS NOT NULL",
    )
    .bind(tenant)
    .bind(limit)
    .bind(DEAD_LETTER_ATTEMPTS)
    .bind(lease_secs)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(
                process_id,
                pid,
                direction,
                sender_mp_id,
                outcome,
                pay_by,
                findings_count,
                erp_attempts,
                dispatched,
            )| ErpPendingRow {
                process_id,
                pid,
                direction,
                sender_mp_id,
                outcome,
                pay_by,
                findings_count,
                erp_attempts,
                dispatched,
            },
        )
        .collect())
}

/// Close a dispute after operator negotiation.
///
/// Returns `true` when a row moved; `false` when the receipt was absent or not
/// in `Dispute`.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn resolve_dispute(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    note: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r"UPDATE invoic_receipts
          SET outcome = 'Resolved',
              dispute_resolved_at = now(),
              dispute_resolution_note = $3
          WHERE id = $1 AND tenant = $2 AND outcome = 'Dispute'",
    )
    .bind(id)
    .bind(tenant)
    .bind(note)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Confirm the ERP has seen the money.
///
/// Returns `true` when a row moved; `false` when the receipt was absent or
/// already confirmed.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn confirm_payment(pool: &PgPool, id: Uuid, tenant: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r"UPDATE invoic_receipts SET payment_confirmed_at = now()
          WHERE id = $1 AND tenant = $2 AND payment_confirmed_at IS NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
