//! Background worker that emits `de.invoic.payment.overdue` CloudEvents.
//!
//! # Regulatory basis
//!
//! § 147 AO / GoBD / §41 EnWG require a durable payment audit trail.  `invoicd`
//! dispatches REMADV but has no feedback path from the banking system.
//! This worker closes the gap: when `pay_by` has passed and
//! `payment_confirmed_at IS NULL`, the ERP has not called
//! `POST /api/v1/receipts/{id}/confirm-payment` — the invoice is overdue.
//!
//! # Behaviour
//!
//! - Polls every 6 hours (configurable via the constant below).
//! - Queries `invoic_receipts WHERE pay_by < now() AND payment_confirmed_at IS
//!   NULL AND dispatched_at IS NOT NULL AND outcome IN ('Ok','AcceptedPartial','Warn')`.
//! - For each overdue receipt, POSTs a `de.invoic.payment.overdue` CloudEvent 1.0
//!   to `erp_webhook_url`.
//! - The CloudEvent is fire-and-forget: a failed delivery is logged as a warning
//!   but does NOT increment `erp_attempts` (that counter belongs to the receipt
//!   delivery path, not the dunning path).
//! - Stops cleanly on `shutdown` signal.

use secrecy::ExposeSecret as _;
use time::format_description::well_known::Rfc3339;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Poll interval for the overdue-payment check.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// Spawn the payment-overdue worker as a background Tokio task.
///
/// No-op when `erp_webhook_url` is `None` (ERP integration not configured).
pub fn spawn(
    pool: sqlx::PgPool,
    tenant: String,
    erp_webhook_url: String,
    erp_hmac_secret: Option<secrecy::SecretString>,
    shutdown: CancellationToken,
) {
    tokio::spawn(run(
        pool,
        tenant,
        erp_webhook_url,
        erp_hmac_secret,
        shutdown,
    ));
}

async fn run(
    pool: sqlx::PgPool,
    tenant: String,
    erp_webhook_url: String,
    erp_hmac_secret: Option<secrecy::SecretString>,
    shutdown: CancellationToken,
) {
    let client = reqwest::Client::new();

    loop {
        tokio::select! {
            _ = tokio::time::sleep(POLL_INTERVAL) => {}
            _ = shutdown.cancelled() => {
                info!("invoicd: payment_overdue worker shutting down");
                return;
            }
        }

        match check_and_emit(&pool, &tenant, &erp_webhook_url, &erp_hmac_secret, &client).await {
            Ok(n) if n > 0 => {
                info!(
                    count = n,
                    "invoicd: emitted de.invoic.payment.overdue events"
                );
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "invoicd: payment_overdue check failed");
            }
        }
    }
}

async fn check_and_emit(
    pool: &sqlx::PgPool,
    tenant: &str,
    erp_webhook_url: &str,
    erp_hmac_secret: &Option<secrecy::SecretString>,
    client: &reqwest::Client,
) -> anyhow::Result<usize> {
    use sqlx::Row as _;

    let rows = sqlx::query(
        r"SELECT id::TEXT, process_id::TEXT, pid, sender_mp_id, pay_by
          FROM invoic_receipts
          WHERE tenant = $1
            AND pay_by < now()
            AND payment_confirmed_at IS NULL
            AND dispatched_at IS NOT NULL
            AND outcome IN ('Ok', 'AcceptedPartial', 'Warn')
          ORDER BY pay_by ASC
          LIMIT 200",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await?;

    let mut emitted = 0usize;

    for row in &rows {
        let id: String = row.try_get("id")?;
        let process_id: String = row.try_get("process_id")?;
        let pid: i16 = row.try_get("pid")?;
        let sender_mp_id: String = row.try_get("sender_mp_id")?;
        let pay_by: time::OffsetDateTime = row.try_get("pay_by")?;

        let event = serde_json::json!({
            "specversion": "1.0",
            "type":        "de.invoic.payment.overdue",
            "source":      format!("urn:invoicd:tenant:{tenant}"),
            "subject":     process_id,
            "id":          uuid::Uuid::new_v4().to_string(),
            "time":        time::OffsetDateTime::now_utc()
                               .format(&Rfc3339)
                               .unwrap_or_default(),
            "data": {
                "receipt_id":   id,
                "pid":          pid,
                "sender_mp_id": sender_mp_id,
                "pay_by":       pay_by.format(&Rfc3339).unwrap_or_default(),
                "tenant":       tenant,
            }
        });

        let body = serde_json::to_string(&event)?;

        let mut req = client
            .post(erp_webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .body(body.clone());

        if let Some(secret) = erp_hmac_secret {
            let sig = format!(
                "sha256={}",
                mako_service::webhook::hmac_hex(secret.expose_secret().as_bytes(), body.as_bytes())
            );
            req = req.header("X-Mako-Signature", sig);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                emitted += 1;
            }
            Ok(resp) => {
                warn!(
                    receipt_id = %id,
                    status = %resp.status(),
                    "invoicd: payment.overdue delivery failed (non-2xx)"
                );
            }
            Err(e) => {
                warn!(receipt_id = %id, error = %e, "invoicd: payment.overdue delivery error");
            }
        }
    }

    Ok(emitted)
}
