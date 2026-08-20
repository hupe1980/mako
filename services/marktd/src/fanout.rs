//! Durable two-phase fan-out worker.
//!
//! marktd persists every produced event to the `event_log` outbox (see
//! [`crate::outbox`]) *before* fan-out. This worker is the sole consumer and is
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
//! [`mako_service::outbox`]). The claim returns the envelope and the
//! subscriber's signing secret in the same round trip, so a delivery is one
//! query plus one POST. On 2xx mark `delivered_at`; on failure back off with
//! jitter, and after `max_attempts` set `dead_lettered_at` (the status-column
//! DLQ — § 147 AO / GoBD: events are never silently dropped).
//!
//! # Ordering
//!
//! Deliveries are **ordered per aggregate**, by `event_log.seq`: a delivery is
//! held back while an earlier event about the same Marktlokation is still
//! outstanding to the same subscriber. Events about different MaLos, and events
//! tied to no MaLo, never wait for each other.
//!
//! One Marktlokation's supply lifecycle is the only sequence here whose order
//! carries meaning, so that is the scope the guarantee covers. Per-endpoint FIFO
//! would serialise the hub behind its slowest subscriber; unordered delivery
//! would let a retried `versorgung.changed` arrive after the transition that
//! superseded it. Head-of-line blocking is bounded — a dead-lettered row stops
//! blocking its key.

use std::{sync::Arc, time::Duration};

use mako_markt::repository::SubscriptionRepository;
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
    /// subscriber cannot stall the others.
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

/// Spawn the durable fan-out worker.
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
    tokio::spawn(async move {
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
        let rows: Vec<PendingEvent> = sqlx::query_as(
            "SELECT event_id, seq, ce_type, marktrole, sparte, ordering_key
               FROM event_log
              WHERE fanned_out_at IS NULL
              ORDER BY seq
              LIMIT $1",
        )
        .bind(self.config.fanout_batch)
        .fetch_all(&self.pool)
        .await?;

        if rows.is_empty() {
            return Ok(0);
        }

        // Resolve subscriber sets *before* opening the transaction. Doing it
        // inside meant a second pool connection was taken while the first held
        // `FOR UPDATE SKIP LOCKED` on up to `fanout_batch` rows, so a small pool
        // could deadlock against itself. A subscription changing in the gap is
        // harmless: the claim below re-checks that the row is still pending.
        let mut resolved = Vec::with_capacity(rows.len());
        for row in rows {
            let role = row.marktrole.as_deref().unwrap_or("");
            match self
                .sub_repo
                .list_matching(&row.ce_type, role, row.sparte.as_deref())
                .await
            {
                Ok(subs) => resolved.push((row, subs)),
                // Leave the row pending (do NOT stamp fanned_out_at) so it is
                // retried on the next cycle rather than fanned out to nobody.
                Err(e) => {
                    warn!(event_id = %row.event_id, error = %e, "fanout: list_matching failed; leaving pending");
                }
            }
        }

        let mut tx = self.pool.begin().await?;
        let mut fanned = 0usize;
        for (row, subs) in resolved {
            // Re-claim under the lock; another replica may have taken it since
            // the unlocked read above.
            let claimed: Option<String> = sqlx::query_scalar(
                "SELECT event_id FROM event_log
                  WHERE event_id = $1 AND fanned_out_at IS NULL
                  FOR UPDATE SKIP LOCKED",
            )
            .bind(&row.event_id)
            .fetch_optional(&mut *tx)
            .await?;
            if claimed.is_none() {
                continue;
            }

            for sub in subs {
                sqlx::query(
                    "INSERT INTO event_delivery
                         (event_id, subscriber_id, webhook_url, seq, ordering_key)
                     VALUES ($1, $2, $3, $4, $5)
                     ON CONFLICT (event_id, subscriber_id) DO NOTHING",
                )
                .bind(&row.event_id)
                .bind(&sub.subscriber_id)
                .bind(&sub.webhook_url)
                .bind(row.seq)
                .bind(&row.ordering_key)
                .execute(&mut *tx)
                .await?;
            }

            sqlx::query("UPDATE event_log SET fanned_out_at = now() WHERE event_id = $1")
                .bind(&row.event_id)
                .execute(&mut *tx)
                .await?;
            fanned += 1;
        }

        tx.commit().await?;
        Ok(fanned)
    }

    // ── Phase 2: deliver ──────────────────────────────────────────────────────

    /// Claim-with-lease due deliveries and POST them. Returns rows processed.
    async fn deliver_phase(&self) -> Result<usize, sqlx::Error> {
        let lease_secs = i64::try_from(self.config.lease.as_secs()).unwrap_or(i64::MAX);
        // The claim carries the envelope and the signing secret out with it.
        // Loading them per delivery instead cost two extra round trips each, and
        // the secret lookup was allowed to fail silently — which downgraded a
        // delivery to *unsigned* on a transient database error, exactly when a
        // subscriber's integrity check matters most.
        let claimed: Vec<ClaimedDelivery> = sqlx::query_as(
            "WITH due AS (
                 SELECT d.event_id, d.subscriber_id FROM event_delivery d
                  WHERE d.delivered_at IS NULL AND d.dead_lettered_at IS NULL
                    AND d.next_attempt_at <= now()
                    -- Per-aggregate FIFO: hold this delivery back while an
                    -- earlier event about the same Marktlokation is still
                    -- outstanding to the same subscriber. Dead-lettered rows do
                    -- not block, bounding the stall by max_attempts.
                    AND (d.ordering_key IS NULL OR NOT EXISTS (
                          SELECT 1 FROM event_delivery e
                           WHERE e.subscriber_id = d.subscriber_id
                             AND e.ordering_key  = d.ordering_key
                             AND e.seq           < d.seq
                             AND e.delivered_at     IS NULL
                             AND e.dead_lettered_at IS NULL))
                  ORDER BY d.seq
                  LIMIT $2
                  FOR UPDATE SKIP LOCKED
             ), claimed AS (
                 UPDATE event_delivery d
                    SET next_attempt_at = now() + ($1 * INTERVAL '1 second')
                   FROM due
                  WHERE d.event_id = due.event_id AND d.subscriber_id = due.subscriber_id
                  RETURNING d.event_id, d.subscriber_id, d.webhook_url, d.attempts
             )
             SELECT c.event_id, c.subscriber_id, c.webhook_url, c.attempts,
                    l.envelope, s.webhook_secret
               FROM claimed c
               JOIN event_log l ON l.event_id = c.event_id
               LEFT JOIN subscriptions s ON s.subscriber_id = c.subscriber_id",
        )
        .bind(lease_secs)
        .bind(self.config.deliver_batch)
        .fetch_all(&self.pool)
        .await?;

        let n = claimed.len();
        // Deliver concurrently (bounded) so a hung subscriber cannot stall the
        // batch.
        use futures::StreamExt as _;
        futures::stream::iter(claimed)
            .for_each_concurrent(self.config.deliver_concurrency, |d| self.deliver_one(d))
            .await;
        Ok(n)
    }

    async fn deliver_one(&self, d: ClaimedDelivery) {
        let body = match serde_json::to_vec(&d.envelope) {
            Ok(b) => b,
            Err(e) => {
                let _ = self
                    .record_failure(&d, &format!("serialize envelope: {e}"))
                    .await;
                return;
            }
        };

        // Standard Webhooks: the **event id is stable across retries** and the
        // **timestamp is per attempt**. Both halves matter here and they pull in
        // opposite directions — a subscriber deduplicates on `webhook-id`, so a
        // redelivery must reuse it; but a retry can land hours after the first
        // attempt, so re-using its timestamp would be refused as stale by the
        // receiver's own freshness check.
        //
        // (`post_ce_with_retry` stamps once instead, because its three attempts
        // are sub-second and identical bytes are the simpler guarantee there.)
        let webhook_id = d.event_id.to_string();
        let signed = d.webhook_secret.as_deref().map(|s| {
            mako_service::webhook::headers(
                s.as_bytes(),
                &webhook_id,
                time::OffsetDateTime::now_utc().unix_timestamp(),
                &body,
            )
        });

        let mut req = self
            .http
            .post(&d.webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .timeout(self.config.delivery_timeout)
            .body(body);
        match signed {
            Some(ref h) => {
                for (name, value) in h {
                    req = req.header(*name, value);
                }
            }
            // Unsigned subscribers still receive the id, so their deduplication
            // does not depend on whether they configured a secret.
            None => req = req.header(mako_service::webhook::ID_HEADER, &webhook_id),
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
        let base = self
            .config
            .backoff
            .get(idx)
            .or_else(|| self.config.backoff.last())
            .map_or_else(
                || self.config.poll_interval.as_secs(),
                std::time::Duration::as_secs,
            );
        let delay =
            i64::try_from(jittered(base, &d.subscriber_id, new_attempts)).unwrap_or(i64::MAX);
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

/// Spread retries over ±12.5 % of the base delay.
///
/// A subscriber that went down takes its whole backlog with it, and without
/// jitter every one of those deliveries becomes due in the same second and
/// arrives as one burst the moment it comes back — the thundering herd that
/// knocks it over again. The offset is derived from the subscriber ID and the
/// attempt number rather than from a random source so the schedule is
/// reproducible in a test.
fn jittered(base_secs: u64, subscriber_id: &str, attempt: i16) -> u64 {
    if base_secs == 0 {
        return 0;
    }
    let spread = (base_secs / 4).max(1);
    let hash = subscriber_id
        .bytes()
        .fold(u64::from(attempt.unsigned_abs()), |acc, b| {
            acc.wrapping_mul(31).wrapping_add(u64::from(b))
        });
    base_secs.saturating_sub(spread / 2) + (hash % spread)
}

#[derive(sqlx::FromRow)]
struct PendingEvent {
    event_id: String,
    seq: i64,
    ce_type: String,
    marktrole: Option<String>,
    sparte: Option<String>,
    ordering_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct ClaimedDelivery {
    event_id: String,
    subscriber_id: String,
    webhook_url: String,
    attempts: i16,
    envelope: Value,
    webhook_secret: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::jittered;

    #[test]
    fn jitter_stays_within_an_eighth_of_the_base_delay() {
        let base = 300;
        for subscriber in ["erp", "processd", "invoicd", "edmd", "obsd"] {
            for attempt in 1..5 {
                let d = jittered(base, subscriber, attempt);
                assert!(
                    (base - base / 8..=base + base / 8).contains(&d),
                    "{subscriber}/{attempt} produced {d}, outside ±12.5 % of {base}"
                );
            }
        }
    }

    #[test]
    fn different_subscribers_do_not_all_become_due_together() {
        let delays: std::collections::BTreeSet<u64> = ["a", "b", "c", "d", "e", "f", "g", "h"]
            .into_iter()
            .map(|s| jittered(300, s, 1))
            .collect();
        assert!(
            delays.len() > 1,
            "every subscriber got the same retry instant — the herd is not spread"
        );
    }

    #[test]
    fn a_zero_base_delay_stays_zero() {
        assert_eq!(jittered(0, "erp", 1), 0);
    }
}
