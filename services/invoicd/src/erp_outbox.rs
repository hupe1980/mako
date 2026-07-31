//! Background ERP outbox worker — retries failed `de.invoic.receipt.*` deliveries.
//!
//! # Design
//!
//! `invoicd` writes every validated INVOIC to `invoic_receipts` before dispatching
//! the REMADV to `makod`.  After dispatch, it attempts to notify the ERP webhook
//! inline.  If that attempt fails (transport error or HTTP 5xx), the row is marked
//! for retry via `erp_next_attempt_at`.
//!
//! This worker runs on a 30-second poll loop and retries any rows where:
//! - `erp_notified_at IS NULL` (not yet delivered)
//! - `erp_attempts < 5` (not dead-lettered)
//! - `erp_next_attempt_at <= now()` (backoff window elapsed)
//!
//! Backoff schedule:
//! | attempt | delay before next retry |
//! |---------|------------------------|
//! | 1       | 30 s                   |
//! | 2       | 5 min                  |
//! | 3       | 30 min                 |
//! | 4       | 2 h                    |
//! | 5       | dead-lettered          |
//!
//! HTTP status semantics:
//! - **2xx**: success → `erp_notified_at` set, row removed from pending index
//! - **4xx**: permanent failure → dead-lettered immediately (set `erp_attempts = 5`)
//! - **5xx / transport**: transient → increment `erp_attempts`, schedule next retry
//!
//! Uses `FOR UPDATE SKIP LOCKED` so multiple worker replicas (e.g. blue/green) can
//! run concurrently without double-delivery.

use secrecy::ExposeSecret as _;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Spawn the ERP outbox flush worker as a background Tokio task.
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
    info!("invoicd: ERP outbox worker started (poll interval 30 s)");
    let http = mako_service::http::default_client();
    let interval = tokio::time::interval(std::time::Duration::from_secs(30));
    tokio::pin!(interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if let Err(e) = flush(&pool, &tenant, &erp_webhook_url, erp_hmac_secret.as_ref(), &http).await {
                    warn!(error = %e, "invoicd: ERP outbox flush error");
                }
            }
            _ = shutdown.cancelled() => {
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
    hmac_secret: Option<&secrecy::SecretString>,
    http: &reqwest::Client,
) -> Result<(), sqlx::Error> {
    let tx = pool.begin().await?;

    let rows = crate::pg::receipts::fetch_erp_pending(pool, tenant, 50).await?;
    if rows.is_empty() {
        return Ok(());
    }

    debug!(
        count = rows.len(),
        "invoicd: ERP outbox flush — delivering pending notifications"
    );

    for row in rows {
        let ce_type = match row.outcome.as_str() {
            "Dispute" => mako_events::invoic::RECEIPT_DISPUTED,
            "Dispatched" => mako_events::invoic::RECEIPT_DISPATCHED,
            _ => mako_events::invoic::RECEIPT_SETTLED,
        };

        let pay_by_str = row.pay_by.map(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });

        let ce = mako_service::CloudEvent::new(
            mako_service::source("invoicd", tenant),
            ce_type,
            row.process_id.to_string(),
            serde_json::json!({
                "process_id":     row.process_id.to_string(),
                "pid":            row.pid,
                "direction":      row.direction,
                "sender_mp_id":   row.sender_mp_id,
                "outcome":        row.outcome,
                "pay_by":         pay_by_str,
                "findings_count": row.findings_count,
            }),
        );

        let secret = hmac_secret.map(|s| s.expose_secret().as_bytes());
        match mako_service::post_ce_with_retry(http, url, &ce, secret).await {
            Ok(()) => {
                debug!(
                    process_id = %row.process_id, ce_type,
                    attempt = row.erp_attempts + 1,
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
                // A 4xx — the ERP rejected these exact bytes (mis-addressed URL,
                // schema mismatch). Retrying wastes the full 2.5 h backoff window,
                // so dead-letter immediately with a diagnostic instead.
                warn!(
                    error = %e, process_id = %row.process_id,
                    "invoicd: ERP outbox permanent failure — dead-lettering (check ERP webhook config)"
                );
                let _ = crate::pg::receipts::dead_letter_erp(pool, row.process_id).await;
            }
            Err(e) => {
                warn!(
                    error = %e, process_id = %row.process_id,
                    attempt = row.erp_attempts + 1,
                    "invoicd: ERP outbox delivery failed — will retry"
                );
                let _ =
                    crate::pg::receipts::record_erp_failure(pool, row.process_id, row.erp_attempts)
                        .await;
            }
        }
    }

    tx.rollback().await.ok(); // tx was only used for SKIP LOCKED — no writes go through it
    Ok(())
}
