//! Durable two-phase fan-out worker.
//!
//! marktd persists every produced event to the `event_log` outbox *before*
//! fan-out (see [`crate::outbox`]). This worker is the sole consumer and is
//! crash-safe end to end:
//!
//! **Phase 1 — fan-out.** Claim undelivered `event_log` rows
//! (`WHERE fanned_out_at IS NULL … FOR UPDATE SKIP LOCKED`). For each, resolve
//! the matching subscribers ([`SubscriptionRepository::list_matching`]) and, in
//! one transaction, insert an `event_delivery` row per subscriber
//! (`ON CONFLICT DO NOTHING`) and stamp `event_log.fanned_out_at = now()`. This
//! snapshots the subscriber set atomically and is idempotent under crash: a
//! crash before commit leaves the row still pending, so it is re-claimed.
//!
//! **Phase 2 — deliver.** Claim-with-lease due `event_delivery` rows (the same
//! `FOR UPDATE SKIP LOCKED` + push-`next_attempt_at`-forward pattern as
//! [`mako_service::outbox`]). Load the envelope, HMAC-sign per subscriber
//! secret, POST `application/cloudevents+json`. On 2xx mark `delivered_at`; on
//! failure back off, and after `max_attempts` set `dead_lettered_at` (the
//! status-column DLQ — § 147 AO / GoBD: events are never silently dropped).
//!
//! The [`SubscriptionRepository`] futures are `!Send` (AFIT), so the loop runs
//! on a dedicated thread with a current-thread runtime + `LocalSet`. Delivery
//! POSTs are awaited inline within a batch; concurrency across replicas/rows is
//! provided by `SKIP LOCKED`, not by spawning `Send` tasks.

use std::{sync::Arc, time::Duration};

use mako_markt::repository::SubscriptionRepository;
use mako_service::webhook::sign;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

/// Fan-out configuration.
#[derive(Debug, Clone)]
pub struct FanoutConfig {
    /// HTTP request timeout per delivery attempt.
    pub delivery_timeout: Duration,
    /// Delivery attempts before dead-lettering.
    pub max_attempts: i16,
    /// How often the worker polls (in addition to `notify` wake-ups).
    pub poll_interval: Duration,
    /// Max `event_log` rows fanned out per Phase-1 batch.
    pub fanout_batch: i64,
    /// Max `event_delivery` rows delivered per Phase-2 batch.
    pub deliver_batch: i64,
    /// How many deliveries run concurrently within a batch, so one slow/hung
    /// subscriber cannot stall the others (the futures are polled concurrently
    /// on the single worker thread — I/O-bound, so this is real concurrency).
    pub deliver_concurrency: usize,
    /// Back-off per prior attempt; the last entry repeats.
    pub backoff: Vec<Duration>,
    /// Claim lease: a claimed delivery's `next_attempt_at` is pushed this far
    /// ahead so a crash mid-delivery makes it due again after the lease.
    pub lease: Duration,
}

impl Default for FanoutConfig {
    fn default() -> Self {
        Self {
            delivery_timeout: Duration::from_secs(10),
            max_attempts: 5,
            poll_interval: Duration::from_secs(30),
            fanout_batch: 100,
            deliver_batch: 50,
            deliver_concurrency: 16,
            backoff: vec![
                Duration::from_secs(30),
                Duration::from_secs(300),
                Duration::from_secs(1800),
                Duration::from_secs(7200),
            ],
            lease: Duration::from_secs(120),
        }
    }
}

/// Spawn the durable fan-out worker on its own thread.
///
/// No receiver: the worker is driven entirely by the `event_log` /
/// `event_delivery` tables. `notify` is a low-latency wake-up hint from
/// [`crate::outbox::enqueue`]; the worker also polls every
/// [`FanoutConfig::poll_interval`], so a missed notification only delays work.
pub fn spawn<S>(
    pool: PgPool,
    sub_repo: S,
    http: reqwest::Client,
    config: FanoutConfig,
    notify: Arc<Notify>,
    shutdown: CancellationToken,
) where
    S: SubscriptionRepository + Clone + Send + Sync + 'static,
{
    // AFIT SubscriptionRepository futures are !Send, so drive the loop on a
    // dedicated blocking thread with its own current-thread runtime + LocalSet.
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("fanout: failed to build single-thread runtime");

        let local = tokio::task::LocalSet::new();
        local.block_on(&rt, async move {
            let worker = Worker {
                pool,
                sub_repo,
                http,
                config,
            };
            let mut interval = tokio::time::interval(worker.config.poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            info!("fanout: durable worker started");
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => {
                        debug!("fanout: shutdown signal received");
                        break;
                    }
                    _ = interval.tick() => worker.drain().await,
                    () = notify.notified() => worker.drain().await,
                }
            }
        });
    });
}

struct Worker<S> {
    pool: PgPool,
    sub_repo: S,
    http: reqwest::Client,
    config: FanoutConfig,
}

impl<S> Worker<S>
where
    S: SubscriptionRepository,
{
    /// Run Phase 1 then Phase 2 repeatedly until neither makes progress, so a
    /// backlog is drained promptly on a single wake-up.
    async fn drain(&self) {
        loop {
            let fanned = match self.fanout_phase().await {
                Ok(n) => n,
                Err(e) => {
                    warn!(error = %e, "fanout: phase-1 (fan-out) cycle failed");
                    0
                }
            };
            let delivered = match self.deliver_phase().await {
                Ok(n) => n,
                Err(e) => {
                    warn!(error = %e, "fanout: phase-2 (deliver) cycle failed");
                    0
                }
            };
            if fanned == 0 && delivered == 0 {
                break;
            }
        }
    }

    // ── Phase 1: fan-out ──────────────────────────────────────────────────────

    /// Claim pending `event_log` rows, snapshot their subscriber sets into
    /// `event_delivery`, and stamp `fanned_out_at`. Returns rows fanned out.
    async fn fanout_phase(&self) -> Result<usize, sqlx::Error> {
        let mut tx = self.pool.begin().await?;

        let rows: Vec<PendingEvent> = sqlx::query_as(
            "SELECT event_id, ce_type, marktrole, sparte
               FROM event_log
              WHERE fanned_out_at IS NULL
              ORDER BY received_at
              LIMIT $1
              FOR UPDATE SKIP LOCKED",
        )
        .bind(self.config.fanout_batch)
        .fetch_all(&mut *tx)
        .await?;

        let n = rows.len();
        for row in &rows {
            let role = row.marktrole.as_deref().unwrap_or("");
            let subs = match self
                .sub_repo
                .list_matching(&row.ce_type, role, row.sparte.as_deref())
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    // Leave the row pending (do NOT stamp fanned_out_at) so it is
                    // retried on the next cycle rather than fanned out to nobody.
                    warn!(event_id = %row.event_id, error = %e, "fanout: list_matching failed; leaving pending");
                    continue;
                }
            };

            for sub in subs {
                sqlx::query(
                    "INSERT INTO event_delivery (event_id, subscriber_id, webhook_url)
                     VALUES ($1, $2, $3)
                     ON CONFLICT (event_id, subscriber_id) DO NOTHING",
                )
                .bind(&row.event_id)
                .bind(&sub.subscriber_id)
                .bind(&sub.webhook_url)
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query("UPDATE event_log SET fanned_out_at = now() WHERE event_id = $1")
                .bind(&row.event_id)
                .execute(&mut *tx)
                .await?;
        }

        tx.commit().await?;
        Ok(n)
    }

    // ── Phase 2: deliver ──────────────────────────────────────────────────────

    /// Claim-with-lease due deliveries and POST them. Returns rows processed.
    async fn deliver_phase(&self) -> Result<usize, sqlx::Error> {
        let lease_secs = i64::try_from(self.config.lease.as_secs()).unwrap_or(i64::MAX);
        let claimed: Vec<ClaimedDelivery> = sqlx::query_as(
            "UPDATE event_delivery
                SET next_attempt_at = now() + ($1 * INTERVAL '1 second')
              WHERE (event_id, subscriber_id) IN (
                  SELECT event_id, subscriber_id FROM event_delivery
                   WHERE delivered_at IS NULL AND dead_lettered_at IS NULL
                     AND next_attempt_at <= now()
                   ORDER BY next_attempt_at
                   LIMIT $2
                   FOR UPDATE SKIP LOCKED
              )
              RETURNING event_id, subscriber_id, webhook_url, attempts",
        )
        .bind(lease_secs)
        .bind(self.config.deliver_batch)
        .fetch_all(&self.pool)
        .await?;

        let n = claimed.len();
        // Deliver concurrently (bounded) so a hung subscriber cannot stall the
        // batch. The futures borrow `&self` and are polled concurrently within
        // this one task — no spawning, so the `!Send` repository stays fine.
        use futures::StreamExt as _;
        futures::stream::iter(claimed)
            .for_each_concurrent(self.config.deliver_concurrency, |d| self.deliver_one(d))
            .await;
        Ok(n)
    }

    async fn deliver_one(&self, d: ClaimedDelivery) {
        // Load the full envelope from the durable outbox.
        let envelope: Option<Value> =
            match sqlx::query_scalar("SELECT envelope FROM event_log WHERE event_id = $1")
                .bind(&d.event_id)
                .fetch_optional(&self.pool)
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    warn!(event_id = %d.event_id, error = %e, "fanout: envelope load failed");
                    return;
                }
            };
        let Some(envelope) = envelope else {
            // event_log row gone (should not happen — FK CASCADE) — dead-letter.
            let _ = self.record_failure(&d, "event_log envelope missing").await;
            return;
        };

        let body = match serde_json::to_vec(&envelope) {
            Ok(b) => b,
            Err(e) => {
                let _ = self
                    .record_failure(&d, &format!("serialize envelope: {e}"))
                    .await;
                return;
            }
        };

        // Per-subscriber HMAC signing — look up the current secret.
        let secret = self
            .sub_repo
            .find(&d.subscriber_id)
            .await
            .ok()
            .flatten()
            .and_then(|s| s.webhook_secret);
        let sig = secret.as_deref().map(|s| sign(s.as_bytes(), &body));

        let mut req = self
            .http
            .post(&d.webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .timeout(self.config.delivery_timeout)
            .body(body);
        if let Some(sig) = &sig {
            req = req.header("X-Mako-Signature", sig);
        }

        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                debug!(event_id = %d.event_id, subscriber_id = %d.subscriber_id, "fanout: delivered");
                let _ = self.mark_delivered(&d).await;
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                warn!(event_id = %d.event_id, subscriber_id = %d.subscriber_id, status, "fanout: non-2xx");
                let _ = self.record_failure(&d, &format!("HTTP {status}")).await;
            }
            Err(e) => {
                warn!(event_id = %d.event_id, subscriber_id = %d.subscriber_id, error = %e, "fanout: transport error");
                let _ = self.record_failure(&d, &e.to_string()).await;
            }
        }
    }

    async fn mark_delivered(&self, d: &ClaimedDelivery) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE event_delivery SET delivered_at = now(), last_error = NULL
              WHERE event_id = $1 AND subscriber_id = $2",
        )
        .bind(&d.event_id)
        .bind(&d.subscriber_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a failed attempt: back off, or dead-letter once `max_attempts` is
    /// reached (status-column DLQ).
    async fn record_failure(&self, d: &ClaimedDelivery, err: &str) -> Result<(), sqlx::Error> {
        let new_attempts = d.attempts + 1;
        if new_attempts >= self.config.max_attempts {
            error!(
                event_id = %d.event_id,
                subscriber_id = %d.subscriber_id,
                webhook_url = %d.webhook_url,
                attempts = new_attempts,
                last_error = %err,
                "fanout: max attempts exhausted — dead-lettering (§147 AO / GoBD)",
            );
            sqlx::query(
                "UPDATE event_delivery
                    SET attempts = $3, dead_lettered_at = now(), last_error = $4
                  WHERE event_id = $1 AND subscriber_id = $2",
            )
            .bind(&d.event_id)
            .bind(&d.subscriber_id)
            .bind(new_attempts)
            .bind(err)
            .execute(&self.pool)
            .await?;
            return Ok(());
        }

        // The last configured entry repeats; an empty backoff falls back to the
        // poll interval rather than underflowing the index.
        let idx = usize::try_from(d.attempts).unwrap_or(0);
        let delay = self
            .config
            .backoff
            .get(idx)
            .or_else(|| self.config.backoff.last())
            .map_or_else(
                || self.config.poll_interval.as_secs(),
                std::time::Duration::as_secs,
            );
        let delay = i64::try_from(delay).unwrap_or(i64::MAX);
        sqlx::query(
            "UPDATE event_delivery
                SET attempts = $3,
                    next_attempt_at = now() + ($4 * INTERVAL '1 second'),
                    last_error = $5
              WHERE event_id = $1 AND subscriber_id = $2",
        )
        .bind(&d.event_id)
        .bind(&d.subscriber_id)
        .bind(new_attempts)
        .bind(delay)
        .bind(err)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct PendingEvent {
    event_id: String,
    ce_type: String,
    marktrole: Option<String>,
    sparte: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ClaimedDelivery {
    event_id: String,
    subscriber_id: String,
    webhook_url: String,
    attempts: i16,
}
