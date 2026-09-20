//! Transactional outbox for `CloudEvents` — *persist-before-dispatch*.
//!
//! The platform principle: a service must never lose a domain event because the
//! HTTP POST that carries it failed after the business row was already committed.
//! The fix is the classic transactional outbox — write the event to a table **in
//! the same SQL transaction as the business write**, then let a background worker
//! deliver it with at-least-once semantics and dead-letter what it can't.
//!
//! ```rust,no_run
//! # async fn ex(pool: sqlx::PgPool, ce: mako_service::CloudEvent) -> Result<(), sqlx::Error> {
//! // 1. Emit inside the business transaction (atomic with the domain write):
//! let mut tx = pool.begin().await?;
//! // ... the business INSERT/UPDATE on &mut *tx ...
//! mako_service::outbox::enqueue(&mut tx, &ce).await?;
//! tx.commit().await?;   // event and business row commit together, or not at all
//! # Ok(()) }
//! ```
//!
//! ```rust,no_run
//! # async fn ex(pool: sqlx::PgPool, url: String, secret: Option<secrecy::SecretString>, ct: tokio_util::sync::CancellationToken) {
//! // 2. Drain it from a background worker (one per service):
//! tokio::spawn(mako_service::outbox::OutboxWorker::new(pool, url, secret).run(ct));
//! # }
//! ```
//!
//! Delivery reuses [`crate::post_ce_with_retry`] (signing, `X-Idempotency-Key`,
//! transient-vs-permanent classification), so a receiver dedups the inevitable
//! at-least-once duplicates on the `CloudEvent` `id`. Dead-lettering is a status
//! column on the same row (it already holds the whole event) — inspect and
//! requeue via [`list_dead_letters`] / [`requeue`].

use std::time::Duration;

use secrecy::{ExposeSecret, SecretString};
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

use crate::cloudevent::CloudEvent;

/// DDL for the per-service `event_outbox` table. Idempotent (`IF NOT EXISTS`).
/// Fold it into the service's schema migration, or call [`ensure_schema`].
///
/// The `pending` partial index gates on `delivered_at IS NULL` (not on the
/// attempt count) so a delivery that succeeds on its final retry is never
/// mistaken for a dead-letter.
pub const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS event_outbox (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    event_id         TEXT        NOT NULL UNIQUE,
    ce_type          TEXT        NOT NULL,
    envelope         JSONB       NOT NULL,
    attempts         SMALLINT    NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at     TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    last_error       TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS event_outbox_pending ON event_outbox (next_attempt_at)
    WHERE delivered_at IS NULL AND dead_lettered_at IS NULL;
CREATE INDEX IF NOT EXISTS event_outbox_dead ON event_outbox (dead_lettered_at)
    WHERE dead_lettered_at IS NOT NULL;
-- Wake the drain worker the moment an enqueued event becomes visible.
--
-- The notification is raised by the database, not by the producer, and that is
-- the whole point: Postgres queues a NOTIFY until the raising transaction
-- COMMITs, so the worker cannot be woken onto a snapshot in which the row does
-- not exist yet. A producer-side, in-process hint has no way to promise that —
-- called inside the transaction it is spent on an invisible row, and the event
-- then waits out a whole poll interval with nothing logged.
--
-- FOR EACH STATEMENT with a transition table, so a batch enqueue raises one
-- notification rather than one per row, and an `ON CONFLICT DO NOTHING` that
-- inserted nothing raises none.
CREATE OR REPLACE FUNCTION event_outbox_notify() RETURNS trigger
    LANGUAGE plpgsql AS $fn$
BEGIN
    IF EXISTS (SELECT 1 FROM inserted) THEN
        PERFORM pg_notify('event_outbox', '');
    END IF;
    RETURN NULL;
END;
$fn$;
DROP TRIGGER IF EXISTS event_outbox_notify ON event_outbox;
CREATE TRIGGER event_outbox_notify
    AFTER INSERT ON event_outbox
    REFERENCING NEW TABLE AS inserted
    FOR EACH STATEMENT EXECUTE FUNCTION event_outbox_notify();";

/// The `LISTEN` channel [`SCHEMA`]'s trigger raises on.
///
/// Named for the table rather than the service: each service has its own
/// database, so the channel never has to be disambiguated.
pub const NOTIFY_CHANNEL: &str = "event_outbox";

/// Create the `event_outbox` table + indexes if absent. Idempotent.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn ensure_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Persist a `CloudEvent` to the outbox **within the caller's transaction**.
///
/// Pass the business transaction (`&mut tx`) so the event and the domain write
/// commit atomically — that atomicity is the whole point. The row is immediately
/// pending (`next_attempt_at` defaults to `now()`). Enqueue is idempotent on the
/// `CloudEvent` `id` (`ON CONFLICT DO NOTHING`), so a retried command cannot
/// double-enqueue.
///
/// **Nothing to call afterwards.** The `event_outbox_notify` trigger raises a
/// `NOTIFY` that Postgres holds until this transaction commits, and the worker
/// is listening, so delivery starts on the commit rather than at the next poll.
/// [`OutboxConfig::poll_interval`] remains what correctness rests on: a lost
/// notification delays delivery and never drops it.
///
/// # Errors
///
/// Returns `sqlx::Error` if the envelope cannot be encoded or the insert fails.
pub async fn enqueue(conn: &mut sqlx::PgConnection, event: &CloudEvent) -> Result<(), sqlx::Error> {
    let envelope = serde_json::to_value(event).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT INTO event_outbox (event_id, ce_type, envelope) VALUES ($1, $2, $3)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&event.id)
    .bind(&event.ce_type)
    .bind(envelope)
    .execute(conn)
    .await?;
    Ok(())
}

/// Tuning for the [`OutboxWorker`]. [`Default`] mirrors the proven invoicd
/// schedule: poll every 30 s, batches of 50, 5 attempts, backoff 30 s → 5 m →
/// 30 m → 2 h then dead-letter.
#[derive(Debug, Clone)]
pub struct OutboxConfig {
    /// How often the worker polls for due events.
    pub poll_interval: Duration,
    /// Max events claimed per poll.
    pub batch_size: i64,
    /// Delivery attempts before dead-lettering.
    pub max_attempts: i16,
    /// Back-off per prior attempt; the last entry repeats.
    pub backoff: Vec<Duration>,
    /// Claim lease: a claimed event's `next_attempt_at` is pushed this far ahead,
    /// so a concurrent replica (or the next tick during slow delivery) skips it.
    /// If the worker crashes mid-delivery, the event becomes due again after the
    /// lease — that is the crash-recovery window.
    pub lease: Duration,
    /// Retention for **delivered** rows: the worker prunes them once past this age
    /// (hourly), so the table does not grow unbounded. Dead-lettered rows are
    /// never auto-pruned — they await manual requeue/discard.
    pub retention: Duration,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            batch_size: 50,
            max_attempts: 5,
            backoff: vec![
                Duration::from_secs(30),
                Duration::from_secs(300),
                Duration::from_secs(1800),
                Duration::from_secs(7200),
            ],
            lease: Duration::from_secs(120),
            retention: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// Await the next `event_outbox` notification.
///
/// Returns `true` when one arrived. On error the listener is dropped and the
/// worker falls back to its poll interval, so a database that never regains the
/// channel degrades to the behaviour it had without one. With no listener this
/// never resolves, leaving the other `select!` arms to drive the loop —
/// returning immediately would spin it.
async fn wait_for_notify(listener: &mut Option<sqlx::postgres::PgListener>) -> bool {
    match listener.as_mut() {
        // `recv` reconnects transparently and drops whatever was raised while
        // the connection was down. That is exactly the right trade for a hint:
        // the poll interval still finds those rows.
        Some(l) => match l.recv().await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(error = %e,
                    "outbox: listener lost — draining on the poll interval alone");
                *listener = None;
                false
            }
        },
        None => std::future::pending().await,
    }
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: uuid::Uuid,
    envelope: serde_json::Value,
    attempts: i16,
}

/// Background worker that drains the `event_outbox` to a single ERP/webhook URL.
///
/// Multi-replica safe: the claim is an atomic `UPDATE … FOR UPDATE SKIP LOCKED`,
/// so two replicas partition the pending rows instead of double-claiming. The
/// receiver still dedups the at-least-once duplicates via `X-Idempotency-Key`.
pub struct OutboxWorker {
    pool: PgPool,
    client: reqwest::Client,
    url: String,
    secret: Option<SecretString>,
    cfg: OutboxConfig,
}

impl OutboxWorker {
    /// Build a worker delivering to `url`, signing with `secret` when present.
    #[must_use]
    pub fn new(pool: PgPool, url: impl Into<String>, secret: Option<SecretString>) -> Self {
        Self {
            pool,
            client: crate::http::default_client(),
            url: url.into(),
            secret,
            cfg: OutboxConfig::default(),
        }
    }

    /// Override the default schedule.
    #[must_use]
    pub fn with_config(mut self, cfg: OutboxConfig) -> Self {
        self.cfg = cfg;
        self
    }

    /// Run the drain loop until `shutdown` is cancelled.
    ///
    /// [`OutboxConfig::poll_interval`] is what delivery rests on;
    /// [`NOTIFY_CHANNEL`] is the latency hint, raised by the `event_outbox`
    /// trigger rather than by the producer, so it arrives on the producer's
    /// COMMIT and never on a snapshot without the row.
    ///
    /// The listener holds one pooled connection for the worker's life. If it
    /// cannot be opened the worker says so once and runs on the poll alone —
    /// slower, never wrong.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.cfg.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // Prune delivered rows hourly so the table stays small. Skip the immediate
        // first tick — nothing is old enough at startup.
        let mut prune = tokio::time::interval(Duration::from_secs(3600));
        prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        prune.reset();
        let mut listener = match sqlx::postgres::PgListener::connect_with(&self.pool).await {
            Ok(mut l) => match l.listen(NOTIFY_CHANNEL).await {
                Ok(()) => Some(l),
                Err(e) => {
                    tracing::warn!(error = %e, channel = NOTIFY_CHANNEL,
                        "outbox: LISTEN failed — draining on the poll interval alone");
                    None
                }
            },
            Err(e) => {
                tracing::warn!(error = %e,
                    "outbox: listener connection failed — draining on the poll interval alone");
                None
            }
        };
        tracing::info!(
            url = %self.url,
            listening = listener.is_some(),
            "outbox worker started"
        );
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("outbox worker: shutdown");
                    return;
                }
                notified = wait_for_notify(&mut listener) => {
                    if notified {
                        match self.flush_once().await {
                            Ok(n) if n > 0 => tracing::debug!(
                                delivered_or_retried = n, "outbox flush (notified)"),
                            Ok(_) => {}
                            Err(e) => tracing::warn!(error = %e, "outbox flush cycle failed"),
                        }
                    }
                }
                _ = prune.tick() => {
                    match prune_delivered(&self.pool, self.cfg.retention).await {
                        Ok(n) if n > 0 => tracing::debug!(pruned = n, "outbox pruned delivered rows"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "outbox prune failed"),
                    }
                }
                _ = interval.tick() => {
                    match self.flush_once().await {
                        Ok(n) if n > 0 => tracing::debug!(delivered_or_retried = n, "outbox flush"),
                        Ok(_) => {}
                        Err(e) => tracing::warn!(error = %e, "outbox flush cycle failed"),
                    }
                }
            }
        }
    }

    /// Claim and process one batch of due events. Returns how many were handled.
    /// Public so a service can trigger an immediate drain (e.g. right after an
    /// enqueue) or an admin endpoint can flush on demand.
    ///
    /// # Errors
    ///
    /// Returns `sqlx::Error` if the claim query fails. A per-row delivery
    /// failure is recorded on the row rather than returned — the batch carries
    /// on. A failure to *record* it cannot be recorded anywhere, so it is
    /// logged: it leaves the row claimed and the event is posted again after the
    /// lease expires, which is the one outcome nothing downstream can see.
    pub async fn flush_once(&self) -> Result<usize, sqlx::Error> {
        let lease_secs = i64::try_from(self.cfg.lease.as_secs()).unwrap_or(i64::MAX);
        // Atomically claim + lease a batch (push next_attempt_at forward) so no
        // long-held row locks and no double-claim across replicas.
        let rows: Vec<Claimed> = sqlx::query_as(
            "UPDATE event_outbox
                SET next_attempt_at = now() + ($1 * INTERVAL '1 second')
              WHERE id IN (
                  SELECT id FROM event_outbox
                   WHERE delivered_at IS NULL AND dead_lettered_at IS NULL
                     AND next_attempt_at <= now()
                   ORDER BY next_attempt_at
                   LIMIT $2
                   FOR UPDATE SKIP LOCKED
              )
              RETURNING id, envelope, attempts",
        )
        .bind(lease_secs)
        .bind(self.cfg.batch_size)
        .fetch_all(&self.pool)
        .await?;

        let n = rows.len();
        // Expose the secret once per batch (not per row).
        let secret = self
            .secret
            .as_ref()
            .map(|s| s.expose_secret().as_bytes().to_vec());
        for row in rows {
            let ce: CloudEvent = match serde_json::from_value(row.envelope) {
                Ok(ce) => ce,
                Err(e) => {
                    // A stored envelope that no longer decodes can never succeed.
                    if let Err(db) = self
                        .dead_letter(row.id, &format!("undecodable envelope: {e}"))
                        .await
                    {
                        tracing::error!(
                            outbox_id = %row.id, error = %db,
                            "outbox: undecodable envelope could not be dead-lettered; \
                             it stays in the queue"
                        );
                    }
                    continue;
                }
            };
            match crate::post_ce_with_retry(&self.client, &self.url, &ce, secret.as_deref()).await {
                Ok(()) => {
                    if let Err(db) = self.mark_delivered(row.id).await {
                        // The consumer accepted the event and the row still says
                        // it did not, so the next flush posts it again — and
                        // keeps posting it. Nothing downstream can tell the
                        // difference; only this line can.
                        tracing::error!(
                            outbox_id = %row.id, error = %db,
                            "outbox: delivered event could not be marked delivered; \
                             it will be redelivered"
                        );
                    }
                }
                Err(e) if e.is_permanent() => {
                    if let Err(db) = self.dead_letter(row.id, &e.to_string()).await {
                        tracing::error!(
                            outbox_id = %row.id, error = %db,
                            "outbox: permanent delivery failure could not be \
                             dead-lettered; the row stays in the queue"
                        );
                    }
                }
                Err(e) => {
                    if let Err(db) = self
                        .record_failure(row.id, row.attempts, &e.to_string())
                        .await
                    {
                        tracing::warn!(
                            outbox_id = %row.id, error = %db,
                            "outbox: retry bookkeeping failed; the lease still \
                             expires and the event retries"
                        );
                    }
                }
            }
        }
        Ok(n)
    }

    async fn mark_delivered(&self, id: uuid::Uuid) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE event_outbox SET delivered_at = now(), last_error = NULL WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn dead_letter(&self, id: uuid::Uuid, err: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE event_outbox SET dead_lettered_at = now(), last_error = $2 WHERE id = $1",
        )
        .bind(id)
        .bind(err)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn record_failure(
        &self,
        id: uuid::Uuid,
        attempts: i16,
        err: &str,
    ) -> Result<(), sqlx::Error> {
        let new_attempts = attempts + 1;
        if new_attempts >= self.cfg.max_attempts {
            return self.dead_letter(id, err).await;
        }
        let idx = usize::try_from(attempts)
            .unwrap_or(0)
            .min(self.cfg.backoff.len() - 1);
        let delay = i64::try_from(self.cfg.backoff[idx].as_secs()).unwrap_or(i64::MAX);
        sqlx::query(
            "UPDATE event_outbox
                SET attempts = $2,
                    next_attempt_at = now() + ($3 * INTERVAL '1 second'),
                    last_error = $4
              WHERE id = $1",
        )
        .bind(id)
        .bind(new_attempts)
        .bind(delay)
        .bind(err)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// A dead-lettered outbox row, for admin inspection.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct DeadLetter {
    pub id: uuid::Uuid,
    pub event_id: String,
    pub ce_type: String,
    pub attempts: i16,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub dead_lettered_at: time::OffsetDateTime,
}

/// List dead-lettered events, newest first — back an admin endpoint with this.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn list_dead_letters(pool: &PgPool, limit: i64) -> Result<Vec<DeadLetter>, sqlx::Error> {
    sqlx::query_as(
        "SELECT id, event_id, ce_type, attempts, last_error, dead_lettered_at
           FROM event_outbox
          WHERE dead_lettered_at IS NOT NULL
          ORDER BY dead_lettered_at DESC
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// Delete **delivered** rows older than `retention` — table-bloat control, run
/// hourly by [`OutboxWorker`]. Dead-lettered and pending rows are never touched.
/// Returns the number of rows pruned.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn prune_delivered(pool: &PgPool, retention: Duration) -> Result<u64, sqlx::Error> {
    let secs = i64::try_from(retention.as_secs()).unwrap_or(i64::MAX);
    let r = sqlx::query(
        "DELETE FROM event_outbox
          WHERE delivered_at IS NOT NULL
            AND delivered_at < now() - ($1 * INTERVAL '1 second')",
    )
    .bind(secs)
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// Requeue an event (typically a dead-letter) for immediate redelivery: clears
/// the delivered/dead-letter marks and resets the attempt counter. Returns
/// `true` if a row matched.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn requeue(pool: &PgPool, id: uuid::Uuid) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "UPDATE event_outbox
            SET dead_lettered_at = NULL, delivered_at = NULL,
                attempts = 0, next_attempt_at = now(), last_error = NULL
          WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_matches_the_proven_schedule() {
        let c = OutboxConfig::default();
        assert_eq!(c.max_attempts, 5);
        assert_eq!(
            c.backoff,
            vec![
                Duration::from_secs(30),
                Duration::from_secs(300),
                Duration::from_secs(1800),
                Duration::from_secs(7200),
            ]
        );
    }
}
