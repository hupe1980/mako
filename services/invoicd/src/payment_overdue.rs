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
//! - Emits one `de.invoic.payment.overdue` per receipt, and stamps
//!   `overdue_notified_at` so a receipt is announced once rather than every six
//!   hours until someone acts on it.
//! - Delivery failure is logged and retried on the next pass. It deliberately
//!   does **not** touch `erp_attempts`: that budget belongs to the receipt
//!   notification path, and spending it here would dead-letter a receipt's
//!   settlement event because a dunning reminder could not be delivered.

use secrecy::ExposeSecret as _;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Poll interval for the overdue check.
const POLL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
/// Receipts announced per pass.
const BATCH: i64 = 200;

/// Spawn the worker. Call only when an ERP webhook is configured.
pub fn spawn(
    pool: sqlx::PgPool,
    tenant: String,
    erp_webhook_url: String,
    erp_hmac_secret: Option<secrecy::SecretString>,
    http: reqwest::Client,
    shutdown: CancellationToken,
) {
    tokio::spawn(run(
        pool,
        tenant,
        erp_webhook_url,
        erp_hmac_secret,
        http,
        shutdown,
    ));
}

async fn run(
    pool: sqlx::PgPool,
    tenant: String,
    url: String,
    secret: Option<secrecy::SecretString>,
    http: reqwest::Client,
    shutdown: CancellationToken,
) {
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
        match check_and_emit(&pool, &tenant, &url, secret.as_ref(), &http).await {
            Ok(n) if n > 0 => info!(count = n, "invoicd: emitted de.invoic.payment.overdue"),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "invoicd: payment_overdue check failed"),
        }
    }
}

async fn check_and_emit(
    pool: &sqlx::PgPool,
    tenant: &str,
    url: &str,
    secret: Option<&secrecy::SecretString>,
    http: &reqwest::Client,
) -> anyhow::Result<usize> {
    type Row = (uuid::Uuid, uuid::Uuid, i16, String, time::OffsetDateTime);
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
          LIMIT $2",
    )
    .bind(tenant)
    .bind(BATCH)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0usize;
    for (id, process_id, pid, sender_mp_id, pay_by) in rows {
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
        let bytes = secret.map(|s| s.expose_secret().as_bytes());
        match mako_service::post_ce_with_retry(http, url, &ce, bytes).await {
            Ok(()) => {
                // Stamped only after delivery, so a failed announcement is
                // retried rather than marked as made.
                if let Err(e) = sqlx::query(
                    "UPDATE invoic_receipts SET overdue_notified_at = now() WHERE id = $1",
                )
                .bind(id)
                .execute(pool)
                .await
                {
                    warn!(receipt_id = %id, error = %e, "invoicd: overdue notice not recorded — it will repeat");
                }
                emitted += 1;
            }
            Err(e) => {
                warn!(receipt_id = %id, error = %e, "invoicd: payment.overdue delivery failed — retrying next pass");
            }
        }
    }
    Ok(emitted)
}
