//! INVOIC receipt persistence — `invoic_receipts` table.
//!
//! Every INVOIC event handled by `invoicd` is written here **before** the
//! corresponding REMADV/COMDIS command is dispatched to makod.  This satisfies
//! the §22 MessZV / §41 EnWG 3-year retention obligation.
//!
//! ## Idempotency
//!
//! Inserts use `ON CONFLICT (process_id) DO UPDATE` so re-delivered CloudEvents
//! (e.g. after a makod restart) are safe.  A second delivery updates `outcome`,
//! `findings`, and `checked_at` to the latest check result.  `received_at` is
//! never overwritten.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// A single row in `invoic_receipts`.
#[derive(Debug)]
pub struct ReceiptRow {
    /// Workflow process ID from the CloudEvent `subject` field.
    pub process_id: Uuid,
    /// BDEW PID (31001 | 31002 | 31005 | 31006).
    pub pid: i16,
    /// Message flow direction: `"Inbound"` (NB/MSB → LF) or `"Outbound"` (LF selbstausgestellt).
    pub direction: String,
    /// GLN of the counterparty that issued the invoice.
    /// - Inbound: the NB or MSB GLN.
    /// - Outbound: the LF's own tenant GLN (sender = us).
    pub sender_mp_id: String,
    /// GLN of the invoice receiver.
    /// - Inbound: the LF's own tenant GLN.
    /// - Outbound: the NB GLN.
    pub receiver_gln: String,
    /// Full BO4E Rechnung object as received.
    pub rechnung: serde_json::Value,
    /// BO4E schema version string, e.g. `"v202501.0.0"`.
    pub bo4e_version: String,
    /// Plausibility outcome: `"Ok"`, `"AcceptedPartial"`, `"Warn"`, `"Dispute"`,
    /// `"Dispatched"` (outbound 31006 sent), or `"Paid"` (outbound 31006 settled).
    pub outcome: String,
    /// Serialised plausibility findings.
    pub findings: serde_json::Value,
    /// Optional Zahlungsziel from INVOIC `DTM+92`.  `None` when no payment deadline is stated.
    pub pay_by: Option<time::OffsetDateTime>,
    /// Timestamp when this daemon received the CloudEvent.
    pub received_at: OffsetDateTime,
    /// Timestamp when the check completed.
    pub checked_at: OffsetDateTime,
    /// Timestamp when the REMADV/COMDIS command was dispatched (set separately).
    pub dispatched_at: Option<OffsetDateTime>,
    /// Operator-configured tenant identifier.
    pub tenant: String,
}

/// Insert or update a receipt row.
///
/// Uses `ON CONFLICT (process_id) DO UPDATE` — safe for re-delivered events.
/// `received_at` is never overwritten on conflict; all other fields are.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.  The caller must decide whether
/// to abort dispatch or proceed with a warning log.
pub async fn upsert_receipt(pool: &PgPool, row: &ReceiptRow) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO invoic_receipts
            (process_id, pid, direction, sender_mp_id, receiver_gln, rechnung, bo4e_version,
             outcome, findings, pay_by, received_at, checked_at, dispatched_at, tenant)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        ON CONFLICT (process_id) DO UPDATE SET
            outcome       = EXCLUDED.outcome,
            findings      = EXCLUDED.findings,
            pay_by        = EXCLUDED.pay_by,
            checked_at    = EXCLUDED.checked_at,
            dispatched_at = EXCLUDED.dispatched_at
        "#,
    )
    .bind(row.process_id)
    .bind(row.pid)
    .bind(&row.direction)
    .bind(&row.sender_mp_id)
    .bind(&row.receiver_gln)
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

/// Mark a receipt as dispatched.
///
/// Called after the REMADV/COMDIS command has been successfully submitted
/// to makod.  Sets `dispatched_at` to the provided timestamp.
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
