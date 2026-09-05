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
//!
//! That same statement is what advances `erp_attempts`, so the budget in the
//! table above is spent by the attempt being *made*. Nothing below depends on
//! the outcome write landing: a receipt whose failure or dead-lettering was
//! never recorded still runs out of attempts and leaves the outbox.

use secrecy::ExposeSecret as _;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

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
                // The ERP has the event; what is at stake is only whether we
                // remember that. A lost stamp leaves the row selectable, so the
                // next tick sends the ERP the same receipt again — an
                // at-least-once duplicate caused by our own database, not by
                // the ERP. Nothing here can undo the POST, so this is logged at
                // `error!` with the process_id rather than propagated: the
                // remaining rows in the batch still deserve their deliveries.
                if let Err(e) = crate::pg::receipts::mark_erp_notified(
                    pool,
                    row.process_id,
                    time::OffsetDateTime::now_utc(),
                )
                .await
                {
                    error!(
                        error = %e, process_id = %row.process_id,
                        "invoicd: ERP delivery succeeded but erp_notified_at was not stamped — \
                         the ERP will be sent this receipt again"
                    );
                }
            }
            Err(e) if e.is_permanent() => {
                warn!(
                    error = %e, process_id = %row.process_id,
                    "invoicd: ERP outbox permanent failure — dead-lettering (check ERP webhook config)"
                );
                // The attempt is already counted by the claim, so losing this
                // write no longer means an endless retry — it only means the
                // row spends its remaining budget re-POSTing bytes the ERP has
                // already refused before it dead-letters itself. Visible, but
                // wasteful and wrong, so it is logged rather than discarded.
                if let Err(db) = crate::pg::receipts::dead_letter_erp(pool, row.process_id).await {
                    error!(
                        error = %db, process_id = %row.process_id,
                        "invoicd: could not dead-letter a permanently rejected ERP notification — \
                         it will keep retrying until the attempt cap stops it"
                    );
                }
            }
            Err(e) => {
                warn!(
                    error = %e, process_id = %row.process_id, attempt = row.erp_attempts + 1,
                    "invoicd: ERP outbox delivery failed — will retry"
                );
                // Only the back-off schedule is lost here (the claim owns the
                // counter), so the row simply retries at the lease interval
                // instead of its backoff — a hot retry against an ERP that is
                // probably already struggling, which is worth knowing about.
                if let Err(db) =
                    crate::pg::receipts::record_erp_failure(pool, row.process_id, row.erp_attempts)
                        .await
                {
                    warn!(
                        error = %db, process_id = %row.process_id,
                        "invoicd: could not schedule the ERP retry back-off — the receipt retries \
                         at the lease interval instead"
                    );
                }
            }
        }
    }
    Ok(())
}
