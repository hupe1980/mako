//! Fan-out worker — delivers `MarktEvent`s to matching webhook subscribers.
//!
//! For each event received from the MPSC channel:
//! 1. Query the subscription repository (in the worker task)
//! 2. Collect the matching subscriber URLs and secrets
//! 3. For each subscriber, spawn a separate `Send` delivery task using reqwest
//! 4. On final retry failure, write the event to the `fanout_dlq` table — no
//!    events are silently dropped (§ 147 AO / GoBD compliance)
//!
//! The channel carries `serde_json::Value` (CloudEvent envelopes) so the
//! worker is decoupled from the typed `MarktEvent` struct.  This also means
//! [`mako_service::event_bus::WebhookBus`] can enqueue events directly without
//! deserialising back to `MarktEvent`.

use std::{sync::Arc, time::Duration};

use mako_markt::{
    cloudevents::compute_signature,
    repository::{Subscription, SubscriptionRepository},
};
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Fan-out configuration.
#[derive(Debug, Clone)]
pub struct FanoutConfig {
    pub delivery_timeout: Duration,
    pub max_retry_attempts: u32,
}

/// Spawn the fan-out background task.
///
/// Uses `mpsc::UnboundedReceiver` — unlike `broadcast`, this never silently
/// drops events when the receiver falls behind.
///
/// `dlq_pool`: PostgreSQL pool used to persist events that exhaust all retry
/// attempts into the `fanout_dlq` table.  On a DLQ write failure the entry
/// is still logged at `error` level so it can be recovered from application
/// logs, but the operational guarantee is best-effort for the DLQ write itself.
pub fn spawn<S>(
    mut rx: mpsc::UnboundedReceiver<Value>,
    sub_repo: S,
    http: reqwest::Client,
    config: FanoutConfig,
    dlq_pool: PgPool,
    shutdown: CancellationToken,
) where
    S: SubscriptionRepository + Clone + Send + Sync + 'static,
{
    // The worker loop is NOT spawned with tokio::spawn because AFIT futures
    // are not Send.  Instead it runs as a local task in the tokio current-thread
    // context.  We use tokio::task::spawn_local inside a LocalSet-based runner.
    // Since main.rs uses tokio::main (multi-thread), we drive this loop via a
    // dedicated blocking thread with its own single-thread runtime.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fanout: failed to build single-thread runtime");

        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            loop {
                tokio::select! {
                    recv = rx.recv() => {
                        match recv {
                            Some(event) => {
                                let subs = collect_subscribers(&sub_repo, &event).await;
                                let body = match serde_json::to_vec(&event) {
                                    Ok(b) => Arc::new(b),
                                    Err(e) => {
                                        warn!(error = %e, "fanout: serialize failed");
                                        continue;
                                    }
                                };
                                let event_type = event
                                    .get("type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                                    .to_owned();
                                for sub in subs {
                                    deliver(
                                        sub,
                                        Arc::clone(&body),
                                        event.clone(),
                                        event_type.clone(),
                                        http.clone(),
                                        config.clone(),
                                        dlq_pool.clone(),
                                    );
                                }
                            }
                            None => {
                                debug!("fanout: channel closed, exiting");
                                break;
                            }
                        }
                    }
                    _ = shutdown.cancelled() => {
                        debug!("fanout: shutdown signal received");
                        break;
                    }
                }
            }
        });
    });
}

/// Query subscriptions matching the event.  Non-Send (AFIT) — runs in LocalSet.
async fn collect_subscribers<S>(sub_repo: &S, event: &Value) -> Vec<Subscription>
where
    S: SubscriptionRepository,
{
    let role = event
        .get("marktrole")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let event_type = event
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    match sub_repo.list_matching(event_type, role, None).await {
        Ok(subs) => subs,
        Err(e) => {
            warn!(error = %e, "fanout: list_matching failed");
            vec![]
        }
    }
}

/// Spawn a `Send + 'static` delivery task.  Only reqwest is used here — no repo calls.
///
/// On final retry exhaustion the event is written to `fanout_dlq` via `dlq_pool`
/// so no events are silently dropped (§ 147 AO / GoBD compliance).
fn deliver(
    sub: Subscription,
    body: Arc<Vec<u8>>,
    event: Value,
    event_type: String,
    http: reqwest::Client,
    config: FanoutConfig,
    dlq_pool: PgPool,
) {
    tokio::task::spawn_local(async move {
        let sig = sub
            .webhook_secret
            .as_deref()
            .map(|s| compute_signature(s.as_bytes(), &body));

        let mut attempt = 0u32;
        loop {
            let mut req = http
                .post(&sub.webhook_url)
                .header("Content-Type", "application/cloudevents+json")
                .timeout(config.delivery_timeout)
                .body((*body).clone());

            if let Some(sig) = &sig {
                req = req.header("X-Mako-Signature", sig);
            }

            // `last_error` is the failure description for this attempt.
            // The success arm returns, so the `!` type coerces to String.
            let last_error: String = match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    debug!(subscriber_id = %sub.subscriber_id, attempt, "fanout: delivered");
                    return;
                }
                Ok(resp) => {
                    warn!(subscriber_id = %sub.subscriber_id, status = resp.status().as_u16(), attempt, "fanout: non-2xx");
                    format!("HTTP {}", resp.status().as_u16())
                }
                Err(e) => {
                    warn!(subscriber_id = %sub.subscriber_id, error = %e, attempt, "fanout: error");
                    e.to_string()
                }
            };

            attempt += 1;
            if attempt >= config.max_retry_attempts {
                // Write to DLQ instead of silently dropping — § 147 AO / GoBD compliance.
                error!(
                    subscriber_id = %sub.subscriber_id,
                    webhook_url   = %sub.webhook_url,
                    event_type    = %event_type,
                    attempts      = attempt,
                    last_error    = %last_error,
                    "fanout: max retries exhausted — writing to fanout_dlq",
                );
                let event_json = serde_json::to_value(&event).unwrap_or(event.clone());
                if let Err(e) = sqlx::query(
                    r#"INSERT INTO fanout_dlq
                       (subscriber_id, webhook_url, event_type, event_body, attempts, last_error)
                       VALUES ($1, $2, $3, $4, $5, $6)"#,
                )
                .bind(&sub.subscriber_id)
                .bind(&sub.webhook_url)
                .bind(&event_type)
                .bind(&event_json)
                .bind(attempt as i32)
                .bind(&last_error)
                .execute(&dlq_pool)
                .await
                {
                    // DLQ write failed — log at error level so it can be
                    // recovered from application logs / log-aggregation.
                    error!(
                        subscriber_id = %sub.subscriber_id,
                        event_type    = %event_type,
                        dlq_error     = %e,
                        "fanout: DLQ write failed — event data follows for manual recovery",
                    );
                    error!(
                        subscriber_id = %sub.subscriber_id,
                        event_body    = ?event,
                        "fanout: undelivered event body",
                    );
                }
                return;
            }

            let delay = Duration::from_secs(1 << attempt.min(6));
            info!(subscriber_id = %sub.subscriber_id, delay_secs = delay.as_secs(), "fanout: retrying");
            tokio::time::sleep(delay).await;
        }
    });
}
