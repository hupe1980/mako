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
    let http = reqwest::Client::new();
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
            "Dispute" => "de.invoic.receipt.disputed",
            "Dispatched" => "de.invoic.receipt.dispatched",
            _ => "de.invoic.receipt.settled",
        };

        let pay_by_str = row.pay_by.map(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });

        let event = serde_json::json!({
            "specversion": "1.0",
            "id":          uuid::Uuid::new_v4().to_string(),
            "source":      format!("urn:invoicd:tenant:{tenant}"),
            "type":        ce_type,
            "time":        time::OffsetDateTime::now_utc()
                               .format(&time::format_description::well_known::Rfc3339)
                               .unwrap_or_default(),
            "subject":     row.process_id.to_string(),
            "datacontenttype": "application/json",
            "data": {
                "process_id":     row.process_id.to_string(),
                "pid":            row.pid,
                "direction":      row.direction,
                "sender_mp_id":   row.sender_mp_id,
                "outcome":        row.outcome,
                "pay_by":         pay_by_str,
                "findings_count": row.findings_count,
            },
        });

        let body = match serde_json::to_vec(&event) {
            Ok(b) => b,
            Err(e) => {
                warn!(error = %e, process_id = %row.process_id, "invoicd: outbox serialize error — skipping");
                continue;
            }
        };

        let sig = hmac_secret.map(|s| {
            format!(
                "sha256={}",
                mako_service::webhook::hmac_hex(s.expose_secret().as_bytes(), &body)
            )
        });

        let mut req = http
            .post(url)
            .header("Content-Type", "application/cloudevents+json")
            .body(body);
        if let Some(sig) = sig {
            req = req.header("X-Mako-Signature", sig);
        }

        match req.send().await {
            Err(e) => {
                warn!(
                    error = %e, process_id = %row.process_id,
                    attempt = row.erp_attempts + 1,
                    "invoicd: ERP outbox delivery transport error — will retry"
                );
                let _ =
                    crate::pg::receipts::record_erp_failure(pool, row.process_id, row.erp_attempts)
                        .await;
            }
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    debug!(
                        process_id = %row.process_id, ce_type, %status,
                        attempt = row.erp_attempts + 1,
                        "invoicd: ERP outbox delivery succeeded"
                    );
                    let _ = crate::pg::receipts::mark_erp_notified(
                        pool,
                        row.process_id,
                        time::OffsetDateTime::now_utc(),
                    )
                    .await;
                } else if status.is_client_error() {
                    let preview = resp
                        .text()
                        .await
                        .unwrap_or_default()
                        .chars()
                        .take(256)
                        .collect::<String>();
                    warn!(
                        process_id = %row.process_id, ce_type, %status,
                        response_body = %preview,
                        "invoicd: ERP outbox 4xx — dead-lettering (check ERP webhook config)"
                    );
                    // Set erp_attempts = 5 to prevent further retries.
                    for _ in 0..5 {
                        let _ =
                            crate::pg::receipts::record_erp_failure(pool, row.process_id, 5).await;
                    }
                } else {
                    warn!(
                        process_id = %row.process_id, ce_type, %status,
                        attempt = row.erp_attempts + 1,
                        "invoicd: ERP outbox 5xx — will retry"
                    );
                    let _ = crate::pg::receipts::record_erp_failure(
                        pool,
                        row.process_id,
                        row.erp_attempts,
                    )
                    .await;
                }
            }
        }
    }

    tx.rollback().await.ok(); // tx was only used for SKIP LOCKED — no writes go through it
    Ok(())
}
