//! INVOIC receipt persistence — the `invoic_receipts` table.
//!
//! Every INVOIC `invoicd` handles is written here **before** the REMADV/COMDIS
//! answer is dispatched to `makod`. A received INVOIC is a received invoice — a
//! Buchungsbeleg — so § 147 Abs. 3 AO / § 14b UStG require 8-year retention,
//! complete and unaltered (GoBD).
//!
//! # Idempotency
//!
//! Inserts use `ON CONFLICT (tenant, process_id) DO UPDATE`, so a redelivered
//! CloudEvent is safe: the second delivery refreshes the check result and
//! leaves `received_at` alone. The conflict key is the **pair**: `process_id`
//! is read from the inbound event's `subject`, so on its own it would let one
//! sender's redelivery overwrite another tenant's receipt.

use sqlx::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

/// The BO4E schema version a `Rechnung` was read under.
///
/// Server-derived provenance, taken from the type that parsed it rather than
/// written as a literal: a hard-coded release string in four call sites keeps
/// claiming the old version for a whole release after `rubo4e` moves on, and
/// the column is what a later reader uses to decide how to interpret the JSONB.
#[must_use]
pub fn bo4e_version(rechnung: &rubo4e::current::Rechnung) -> &'static str {
    use rubo4e::Bo4eTyped as _;
    rechnung.schema_version()
}

/// A row in `invoic_receipts`.
#[derive(Debug)]
pub struct ReceiptRow {
    /// Workflow process ID from the CloudEvent `subject`.
    pub process_id: Uuid,
    /// EDIFACT INVOIC message reference (BGM 1004) — the key `makod` routes the
    /// answer command by.
    pub invoice_ref: String,
    /// Invoice number, when the document states one.
    pub rechnungsnummer: Option<String>,
    /// BDEW Prüfidentifikator.
    pub pid: i16,
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
    /// Zahlungsziel from INVOIC `SG8 DTM+265` (MIG Nr. 00033), whose DE 2379
    /// admits only `303`. The INVOIC MIG publishes no qualifier 92 — that is
    /// UTILMD's Vertragsbeginn, a different message.
    pub pay_by: Option<OffsetDateTime>,
    pub received_at: OffsetDateTime,
    pub checked_at: OffsetDateTime,
    /// Set once the answer command has been accepted by `makod`.
    pub dispatched_at: Option<OffsetDateTime>,
    pub tenant: String,
}

/// The receipt upsert, named so the conflict key is assertable without a
/// database — `invoic_receipts_tenant_process` is the index it relies on.
const UPSERT_SQL: &str = r"INSERT INTO invoic_receipts
            (process_id, invoice_ref, rechnungsnummer, pid, sender_mp_id,
             receiver_gln, malo_id, rechnung, bo4e_version, outcome, findings,
             pay_by, received_at, checked_at, dispatched_at, tenant)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
          ON CONFLICT (tenant, process_id) DO UPDATE SET
            outcome         = EXCLUDED.outcome,
            findings        = EXCLUDED.findings,
            pay_by          = EXCLUDED.pay_by,
            checked_at      = EXCLUDED.checked_at,
            bo4e_version    = EXCLUDED.bo4e_version,
            -- COALESCE, not EXCLUDED: a redelivery whose payload has lost the
            -- MaLo must not erase the one already recorded.
            malo_id         = COALESCE(EXCLUDED.malo_id, invoic_receipts.malo_id),
            rechnungsnummer = COALESCE(EXCLUDED.rechnungsnummer, invoic_receipts.rechnungsnummer)";

/// Insert or refresh a receipt, keyed on `(tenant, process_id)`.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure. The caller must **not** dispatch
/// the market answer when this fails: an answered invoice missing from the
/// audit trail is the failure this table exists to prevent.
pub async fn upsert_receipt(pool: &PgPool, row: &ReceiptRow) -> Result<(), sqlx::Error> {
    sqlx::query(UPSERT_SQL)
        .bind(row.process_id)
        .bind(&row.invoice_ref)
        .bind(row.rechnungsnummer.as_deref())
        .bind(row.pid)
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
/// The `outcome` recorded for an invoice this tenant answered, by its
/// **Rechnungsnummer**.
///
/// `E_0267` Prüfschritte 70/80 turn on how the original was answered: a Storno
/// of an invoice the ESA paid is confirmed, one of an invoice it *refused* — or
/// never answered — needs **no answer at all**. A Stornorechnung names its
/// predecessor by Rechnungsnummer and nothing else, so that is the key.
///
/// `Ok(None)` means „never answered", which is Prüfschritt 80's second branch
/// and not an error.
/// **Prüfschritt 50** — has this Rechnungssteller used this Rechnungsnummer
/// before?
///
/// Every invoice tree in `mako_pruefung::rechnung` asks it (`A05`), and it is
/// answerable from this service's own receipt store: § 14 Abs. 4 Nr. 4 UStG
/// makes the Rechnungsnummer einmalig **per Rechnungssteller**, so the pair
/// (`sender_mp_id`, `rechnungsnummer`) is the key.
///
/// `process_id` is the invoice being checked and is **excluded**: a redelivery
/// of the same process must not refuse itself. Duplicate detection across
/// processes is what the Prüfschritt is for.
///
/// Returns `Ok(false)` when nothing matches. A query error propagates — a
/// transport failure is not evidence of absence, and answering `A05` on a
/// database blip refuses a correct invoice.
pub async fn rechnungsnummer_bereits_verwendet(
    pool: &PgPool,
    tenant: &str,
    sender_mp_id: &str,
    rechnungsnummer: &str,
    process_id: Uuid,
) -> Result<bool, sqlx::Error> {
    // No cancellation filter: `outcome` has no stornierte state, and a Storno
    // arrives as its own receipt rather than mutating the original's. So a
    // number whose invoice was later cancelled still counts as used, which is
    // the § 14 Abs. 4 Nr. 4 UStG reading anyway: the Rechnungsnummer is
    // einmalig vergeben, not einmalig *wirksam*.
    let found: Option<(bool,)> = sqlx::query_as(
        r"SELECT true
            FROM invoic_receipts
           WHERE tenant          = $1
             AND sender_mp_id    = $2
             AND rechnungsnummer = $3
             AND process_id     <> $4
           LIMIT 1",
    )
    .bind(tenant)
    .bind(sender_mp_id)
    .bind(rechnungsnummer)
    .bind(process_id)
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

pub async fn receipt_outcome(
    pool: &PgPool,
    tenant: &str,
    rechnungsnummer: &str,
) -> Result<Option<String>, sqlx::Error> {
    // Newest first: a Rechnungsnummer is unique per Rechnungssteller, but a
    // redelivery can leave more than one row and the latest verdict is the one
    // that went out.
    sqlx::query_scalar(
        "SELECT outcome FROM invoic_receipts \
         WHERE tenant = $1 AND rechnungsnummer = $2 \
         ORDER BY checked_at DESC NULLS LAST LIMIT 1",
    )
    .bind(tenant)
    .bind(rechnungsnummer)
    .fetch_optional(pool)
    .await
}

/// Conclude a checked receipt: stamp `dispatched_at` when the market answer
/// went out, and enqueue the receipt's CloudEvent for the ERP, in one
/// transaction.
///
/// The `de.invoic.receipt.*` event states whether the answer went out, so it
/// cannot commit with the receipt itself; it commits with the stamp that
/// records the answer, and the `mako_service::outbox` worker delivers it with
/// retry. A failure before this commit leaves the inbound event unanswered, and
/// makod redelivers it.
///
/// # Errors
///
/// Returns `sqlx::Error` when the update, the enqueue or the commit fails.
pub async fn conclude(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
    dispatched_at: Option<OffsetDateTime>,
    event: Option<&mako_service::CloudEvent>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    if let Some(at) = dispatched_at {
        mark_dispatched(&mut *tx, tenant, process_id, at).await?;
    }
    if let Some(ce) = event {
        mako_service::outbox::enqueue(&mut tx, ce).await?;
    }
    tx.commit().await
}

pub async fn mark_dispatched(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    process_id: Uuid,
    dispatched_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE invoic_receipts SET dispatched_at = $1 \
         WHERE tenant = $3 AND process_id = $2",
    )
    .bind(dispatched_at)
    .bind(process_id)
    .bind(tenant)
    .execute(executor)
    .await?;
    Ok(())
}

/// Record the answer payload a receipt's market answer carries, before it is
/// sent — the one a re-dispatch sends again.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn store_antwort(
    pool: &PgPool,
    tenant: &str,
    process_id: Uuid,
    antwort: &serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE invoic_receipts SET antwort = $1 WHERE tenant = $2 AND process_id = $3")
        .bind(antwort)
        .bind(tenant)
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
    /// The answer payload recorded before the first attempt.
    pub antwort: Option<serde_json::Value>,
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
    /// `(process_id, pid, outcome, invoice_ref, dispatched_at, antwort)`
    type Row = (
        Uuid,
        i16,
        String,
        Option<String>,
        Option<OffsetDateTime>,
        Option<serde_json::Value>,
    );
    let row: Option<Row> = sqlx::query_as(
        r"SELECT process_id, pid, outcome, invoice_ref, dispatched_at, antwort
          FROM invoic_receipts WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(process_id, pid, outcome, invoice_ref, dispatched_at, antwort)| DispatchTarget {
            process_id,
            pid,
            outcome,
            invoice_ref,
            already_dispatched: dispatched_at.is_some(),
            antwort,
        },
    ))
}

/// Close a dispute after operator negotiation, recording who closed it.
///
/// `resolved_by` is the verified caller: a resolution settles what the
/// Lieferant pays, and the § 147 AO record of it names the person.
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
    resolved_by: &str,
    note: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        r"UPDATE invoic_receipts
          SET outcome = 'Resolved',
              dispute_resolved_at = now(),
              dispute_resolved_by = $3,
              dispute_resolution_note = $4
          WHERE id = $1 AND tenant = $2 AND outcome = 'Dispute'",
    )
    .bind(id)
    .bind(tenant)
    .bind(resolved_by)
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

#[cfg(test)]
mod tests {
    use super::UPSERT_SQL;

    /// The receipt upsert conflicts on the pair, never on `process_id` alone.
    ///
    /// `process_id` is read from the inbound CloudEvent's `subject` — a value a
    /// sender chooses. Keyed globally, a redelivery from one market partner
    /// rewrites the outcome, the findings and the Zahlungsziel of another
    /// tenant's Buchungsbeleg.
    #[test]
    fn the_receipt_upsert_conflicts_on_tenant_and_process() {
        assert!(
            UPSERT_SQL.contains("ON CONFLICT (tenant, process_id)"),
            "the conflict key must be the pair: {UPSERT_SQL}"
        );
    }

    /// The schema's uniqueness must be the same pair the upsert names, or the
    /// statement fails at runtime for want of a matching unique index.
    #[test]
    fn the_schema_declares_that_pair_unique() {
        let schema = include_str!("../../migrations/0001_schema.sql");
        assert!(
            schema.contains("ON invoic_receipts (tenant, process_id)"),
            "no unique index backs `ON CONFLICT (tenant, process_id)`"
        );
        assert!(
            !schema.contains("process_id              UUID        NOT NULL UNIQUE"),
            "a globally unique process_id lets one tenant's event reach another's receipt"
        );
    }
}
