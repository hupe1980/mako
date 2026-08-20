//! Background ERP outbox worker — retries `de.invoic.receipt.*` deliveries.
//!
//! # Design
//!
//! Every receipt starts life due for ERP notification (`erp_notified_at IS
//! NULL`, `erp_next_attempt_at = now()`). The handler makes the first attempt
//! inline and clears the row on success; this worker picks up whatever the
//! inline attempt did not deliver — a failed POST, an ERP that was down, or a
//! receipt written by a path that never got as far as notifying.
//!
//! ## Backoff
//!
//! | attempt | delay before the next |
//! |---|---|
//! | 1 | 30 s |
//! | 2 | 5 min |
//! | 3 | 30 min |
//! | 4 | 2 h |
//! | 5 | dead-lettered |
//!
//! ## HTTP status semantics
//!
//! - **2xx** — delivered; the row leaves the pending index.
//! - **4xx** — permanent; dead-lettered at once. The ERP rejected these exact
//!   bytes, so spending the full 2.5 h budget on them buys nothing.
//! - **5xx / transport** — transient; counted and rescheduled.
//!
//! ## Concurrency
//!
//! The batch is claimed with an `UPDATE … RETURNING` that leases the rows by
//! pushing `erp_next_attempt_at` forward — see
//! [`crate::pg::receipts::claim_erp_pending`] for why a pooled `SELECT … FOR
//! UPDATE SKIP LOCKED` did not hold across the deliveries and let two replicas
//! send everything twice.

use secrecy::ExposeSecret as _;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Poll interval, and the lease a claimed batch holds.
const POLL: std::time::Duration = std::time::Duration::from_secs(30);
/// Receipts claimed per tick.
const BATCH: i64 = 50;

/// Spawn the worker. Call only when an ERP webhook is configured — nothing else
/// drains the outbox, and rows would accumulate without a consumer.
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
    info!(
        poll_secs = POLL.as_secs(),
        "invoicd: ERP outbox worker started"
    );
    let mut interval = tokio::time::interval(POLL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = flush(&pool, &tenant, &url, secret.as_ref(), &http).await {
                    warn!(error = %e, "invoicd: ERP outbox flush error");
                }
            }
            () = shutdown.cancelled() => {
                info!("invoicd: ERP outbox worker shutting down");
                return;
            }
        }
    }
}

async fn flush(
    pool: &sqlx::PgPool,
    tenant: &str,
    url: &str,
    secret: Option<&secrecy::SecretString>,
    http: &reqwest::Client,
) -> Result<(), sqlx::Error> {
    let rows =
        crate::pg::receipts::claim_erp_pending(pool, tenant, BATCH, POLL.as_secs() as i64).await?;
    if rows.is_empty() {
        return Ok(());
    }
    debug!(count = rows.len(), "invoicd: ERP outbox flush");

    for row in rows {
        let pay_by = row.pay_by.and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        });
        let ce = mako_service::CloudEvent::new(
            mako_service::source("invoicd", tenant),
            crate::handler::ce_type_for(&row.outcome),
            row.process_id.to_string(),
            serde_json::json!({
                "process_id":     row.process_id.to_string(),
                "pid":            row.pid,
                "direction":      row.direction,
                "sender_mp_id":   row.sender_mp_id,
                "outcome":        row.outcome,
                "pay_by":         pay_by,
                "findings_count": row.findings_count,
                "dispatched":     row.dispatched,
            }),
        );

        let bytes = secret.map(|s| s.expose_secret().as_bytes());
        match mako_service::post_ce_with_retry(http, url, &ce, bytes).await {
            Ok(()) => {
                debug!(
                    process_id = %row.process_id, attempt = row.erp_attempts + 1,
                    "invoicd: ERP outbox delivery succeeded"
                );
                let _ = crate::pg::receipts::mark_erp_notified(
                    pool,
                    row.process_id,
                    time::OffsetDateTime::now_utc(),
                )
                .await;
            }
            Err(e) if e.is_permanent() => {
                warn!(
                    error = %e, process_id = %row.process_id,
                    "invoicd: ERP outbox permanent failure — dead-lettering (check ERP webhook config)"
                );
                let _ = crate::pg::receipts::dead_letter_erp(pool, row.process_id).await;
            }
            Err(e) => {
                warn!(
                    error = %e, process_id = %row.process_id, attempt = row.erp_attempts + 1,
                    "invoicd: ERP outbox delivery failed — will retry"
                );
                let _ =
                    crate::pg::receipts::record_erp_failure(pool, row.process_id, row.erp_attempts)
                        .await;
            }
        }
    }
    Ok(())
}
