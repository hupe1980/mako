//! Background worker that emits `de.invoic.payment.overdue`.
//!
//! # Why it exists
//!
//! `invoicd` answers an INVOIC and records the Zahlungsziel, but has no
//! feedback path from the bank: the only thing that says an invoice was paid is
//! the ERP calling `POST /api/v1/receipts/{id}/confirm-payment`. When the
//! Zahlungsziel has passed and that call never came, nobody would notice. This
//! worker is the notice.
//!
//! # Behaviour
//!
//! - Polls every 6 hours.
//! - Selects receipts past `pay_by` with no `payment_confirmed_at`, whose
//!   answer *did* go out, and whose outcome accepted the invoice. A disputed
//!   invoice is not overdue — it is disputed.
//! - Enqueues one `de.invoic.payment.overdue` per receipt and stamps
//!   `overdue_notified_at` in the same transaction, so a receipt is announced
//!   once rather than every six hours until someone acts on it, and an
//!   announcement is never stamped without its event. The webhook module
//!   delivers it — to the ERP, the endpoint `default`, and every other
//!   matching endpoint — with retry.

use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Poll interval for the overdue check.
const POLL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// Receipts announced per pass.
const BATCH: i64 = 200;

/// Spawn the worker.
pub fn spawn(pool: sqlx::PgPool, tenant: String, shutdown: CancellationToken) {
    tokio::spawn(run(pool, tenant, shutdown));
}

async fn run(pool: sqlx::PgPool, tenant: String, shutdown: CancellationToken) {
    let mut interval = tokio::time::interval(POLL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick fires immediately; a restart should not have to wait six
    // hours before noticing an invoice that went overdue while it was down.
    loop {
        tokio::select! {
            _ = interval.tick() => {}
            () = shutdown.cancelled() => {
                info!("invoicd: payment_overdue worker shutting down");
                return;
            }
        }
        match check_and_enqueue(&pool, &tenant).await {
            Ok(n) if n > 0 => info!(count = n, "invoicd: enqueued de.invoic.payment.overdue"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "invoicd: payment_overdue check failed"),
        }
    }
}

/// Enqueue a `de.invoic.payment.overdue` for every receipt past its
/// Zahlungsziel not yet announced, stamping each in the same transaction.
///
/// # Errors
///
/// A database error; nothing of the batch is then stamped or enqueued.
pub async fn check_and_enqueue(pool: &sqlx::PgPool, tenant: &str) -> anyhow::Result<usize> {
    type Row = (uuid::Uuid, uuid::Uuid, i16, String, time::OffsetDateTime);
    let mut tx = pool.begin().await?;
    let rows: Vec<Row> = sqlx::query_as(
        r"SELECT id, process_id, pid, sender_mp_id, pay_by
          FROM invoic_receipts
          WHERE tenant = $1
            AND pay_by < now()
            AND payment_confirmed_at IS NULL
            AND overdue_notified_at IS NULL
            AND dispatched_at IS NOT NULL
            AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
          ORDER BY pay_by ASC
          LIMIT $2
          FOR UPDATE SKIP LOCKED",
    )
    .bind(tenant)
    .bind(BATCH)
    .fetch_all(&mut *tx)
    .await?;

    for (id, process_id, pid, sender_mp_id, pay_by) in &rows {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("invoicd", tenant),
            mako_events::invoic::PAYMENT_OVERDUE,
            process_id.to_string(),
            serde_json::json!({
                "receipt_id":   id.to_string(),
                "process_id":   process_id.to_string(),
                "pid":          pid,
                "sender_mp_id": sender_mp_id,
                "pay_by":       pay_by.format(&Rfc3339).unwrap_or_default(),
                "tenant":       tenant,
            }),
        );
        mako_service::outbox::enqueue(&mut tx, &ce).await?;
        sqlx::query("UPDATE invoic_receipts SET overdue_notified_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(rows.len())
}
