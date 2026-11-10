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
//!
//! The same table carries a second kind of obligation: a [`Command`], an HTTP
//! call one upstream service is owed (an ORDERS 17008 Abbestellung to `makod`,
//! say). [`enqueue_command`] writes it in the business transaction exactly like
//! an event, and a worker built with [`OutboxWorker::commands`] drains the rows
//! of one upstream under the same attempts, backoff and dead-letter policy.

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
    kind             TEXT        NOT NULL DEFAULT 'cloudevent'
                                 CHECK (kind IN ('cloudevent', 'command')),
    upstream         TEXT,
    ce_type          TEXT        NOT NULL,
    envelope         JSONB       NOT NULL,
    attempts         SMALLINT    NOT NULL DEFAULT 0,
    next_attempt_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at     TIMESTAMPTZ,
    dead_lettered_at TIMESTAMPTZ,
    last_error       TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A command names the upstream it is owed to. An event names one only
    -- when it goes to a receiver other than the service's default webhook.
    CONSTRAINT event_outbox_upstream_per_kind
        CHECK (kind = 'cloudevent' OR upstream IS NOT NULL)
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

/// [`enqueue`], for a receiver other than the service's default webhook.
///
/// `target` names the receiver; the worker built with
/// [`OutboxWorker::for_target`] under the same name drains it, and the default
/// worker never claims it.
///
/// # Errors
///
/// Returns `sqlx::Error` if the envelope cannot be encoded or the insert fails.
pub async fn enqueue_to(
    conn: &mut sqlx::PgConnection,
    target: &str,
    event: &CloudEvent,
) -> Result<(), sqlx::Error> {
    let envelope = serde_json::to_value(event).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT INTO event_outbox (event_id, upstream, ce_type, envelope) VALUES ($1, $2, $3, $4)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&event.id)
    .bind(target)
    .bind(&event.ce_type)
    .bind(envelope)
    .execute(conn)
    .await?;
    Ok(())
}

/// An HTTP call owed to one upstream service, persisted like an event.
///
/// `key` is the obligation's identity: it is the row's unique `event_id`, so a
/// retried business command enqueues it once, and it travels as the
/// `Idempotency-Key` header on every delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Command {
    /// The upstream this call is owed to — the name a worker built with
    /// [`OutboxWorker::commands`] drains.
    pub upstream: String,
    /// A short label for the obligation, listed beside a dead letter.
    pub label: String,
    /// `POST` or `PUT`.
    pub method: CommandMethod,
    /// Path below the upstream's base URL, starting with `/`.
    pub path: String,
    /// The JSON body.
    pub body: serde_json::Value,
    /// The obligation's identity and idempotency key.
    pub key: String,
    /// Statuses the upstream answers when the call already took effect, which
    /// therefore discharge the obligation rather than fail it.
    #[serde(default)]
    pub settled_statuses: Vec<u16>,
}

/// The HTTP method of a [`Command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CommandMethod {
    /// `POST`.
    Post,
    /// `PUT`.
    Put,
}

/// Persist a [`Command`] to the outbox **within the caller's transaction**.
///
/// The counterpart of [`enqueue`] for a call owed to an upstream service:
/// idempotent on [`Command::key`], and woken by the same trigger, so there is
/// nothing to call after the commit either.
///
/// # Errors
///
/// Returns `sqlx::Error` if the command cannot be encoded or the insert fails.
pub async fn enqueue_command(
    conn: &mut sqlx::PgConnection,
    command: &Command,
) -> Result<(), sqlx::Error> {
    let envelope = serde_json::to_value(command).map_err(|e| sqlx::Error::Encode(Box::new(e)))?;
    sqlx::query(
        "INSERT INTO event_outbox (event_id, kind, upstream, ce_type, envelope)
         VALUES ($1, 'command', $2, $3, $4)
         ON CONFLICT (event_id) DO NOTHING",
    )
    .bind(&command.key)
    .bind(&command.upstream)
    .bind(&command.label)
    .bind(envelope)
    .execute(conn)
    .await?;
    Ok(())
}

/// Tuning for the [`OutboxWorker`]. [`Default`] is the one policy every kind of
/// row is delivered under: poll every 30 s, batches of 50, 8 attempts, backoff
/// 30 s × 4ⁿ capped at 2 h (30 s → 2 min → 8 min → 32 min → 2 h …), so the
/// budget runs out about nine hours after the first attempt — inside the day an
/// IFTSTA or a Mahnung delivery is owed by — and then dead-letters.
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
            max_attempts: 8,
            backoff: vec![
                Duration::from_secs(30),
                Duration::from_mins(2),
                Duration::from_mins(8),
                Duration::from_mins(32),
                Duration::from_hours(2),
            ],
            lease: Duration::from_mins(2),
            retention: Duration::from_hours(720),
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
    target: Target,
    cfg: OutboxConfig,
    on_delivered: Option<OnDelivered>,
}

/// What a receiver answered to a delivered row, handed to the worker's
/// [`OutboxWorker::on_delivered`] handler.
#[derive(Debug)]
pub struct Delivered {
    /// The CloudEvent `type`, or a command's label.
    pub ce_type: String,
    /// The CloudEvent `subject`, or a command's key; `None` for an event
    /// without a subject.
    pub subject: Option<String>,
    /// The receiver's JSON answer, when it sent one.
    pub reply: Option<serde_json::Value>,
}

/// The handler type behind [`OutboxWorker::on_delivered`].
pub type OnDelivered = std::sync::Arc<
    dyn for<'c> Fn(
            &'c mut sqlx::PgConnection,
            &'c Delivered,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<(), sqlx::Error>> + Send + 'c>,
        > + Send
        + Sync,
>;

/// What one worker drains: the events, or the commands of one upstream.
enum Target {
    CloudEvents {
        name: Option<String>,
        url: String,
        secret: Option<SecretString>,
    },
    Commands {
        upstream: String,
        base_url: String,
        bearer: Option<SecretString>,
        /// Sign each command with Standard Webhooks, keyed with `bearer`.
        signed: bool,
    },
}

impl Target {
    const fn kind(&self) -> &'static str {
        match self {
            Self::CloudEvents { .. } => "cloudevent",
            Self::Commands { .. } => "command",
        }
    }

    fn upstream(&self) -> Option<&str> {
        match self {
            Self::CloudEvents { name, .. } => name.as_deref(),
            Self::Commands { upstream, .. } => Some(upstream),
        }
    }

    fn describe(&self) -> &str {
        match self {
            Self::CloudEvents { url, .. } => url,
            Self::Commands { base_url, .. } => base_url,
        }
    }
}

impl OutboxWorker {
    /// Build a worker delivering the events to `url`, signing with `secret`
    /// when present.
    #[must_use]
    pub fn new(pool: PgPool, url: impl Into<String>, secret: Option<SecretString>) -> Self {
        Self {
            pool,
            client: crate::http::default_client(),
            target: Target::CloudEvents {
                name: None,
                url: url.into(),
                secret,
            },
            cfg: OutboxConfig::default(),
            on_delivered: None,
        }
    }

    /// Build a worker delivering the events enqueued with [`enqueue_to`] under
    /// `target` to `url`, signing with `secret` when present.
    #[must_use]
    pub fn for_target(
        pool: PgPool,
        target: impl Into<String>,
        url: impl Into<String>,
        secret: Option<SecretString>,
    ) -> Self {
        let mut worker = Self::new(pool, url, secret);
        if let Target::CloudEvents { name, .. } = &mut worker.target {
            *name = Some(target.into());
        }
        worker
    }

    /// Build a worker delivering the [`Command`]s owed to `upstream`, each to
    /// `base_url` + its path, with `bearer` as the `Authorization` token when
    /// present.
    #[must_use]
    pub fn commands(
        pool: PgPool,
        upstream: impl Into<String>,
        base_url: impl Into<String>,
        bearer: Option<SecretString>,
    ) -> Self {
        Self {
            pool,
            client: crate::http::default_client(),
            target: Target::Commands {
                upstream: upstream.into(),
                base_url: base_url.into(),
                bearer,
                signed: false,
            },
            cfg: OutboxConfig::default(),
            on_delivered: None,
        }
    }

    /// Sign every command this worker sends with [Standard Webhooks]
    /// (`webhook-id` the command's key, keyed with the bearer token), for a
    /// receiver that must tell this service's push from anything else that
    /// reached its URL. No effect on an event worker, which signs with its own
    /// secret, or on a command worker without a bearer.
    ///
    /// [Standard Webhooks]: https://www.standardwebhooks.com/
    #[must_use]
    pub fn signed(mut self) -> Self {
        if let Target::Commands { signed, .. } = &mut self.target {
            *signed = true;
        }
        self
    }

    /// Run `handler` for every delivered row, in the transaction that marks it
    /// delivered.
    ///
    /// The business follow-up of a delivery — a reference the receiver
    /// assigned, written back onto the row that owed the delivery — then lands
    /// with the delivery or not at all: a handler that fails rolls the mark
    /// back, and the row is delivered again, so the receiver must answer a
    /// redelivery idempotently.
    #[must_use]
    pub fn on_delivered(mut self, handler: OnDelivered) -> Self {
        self.on_delivered = Some(handler);
        self
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
        let mut prune = tokio::time::interval(Duration::from_hours(1));
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
            target = %self.target.describe(),
            kind = self.target.kind(),
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
    /// Returns `sqlx::Error` if the sweep or the claim query fails. A per-row delivery
    /// failure is recorded on the row rather than returned — the batch carries
    /// on. A failure to *record* it cannot be recorded anywhere, so it is
    /// logged: it leaves the row claimed and the event is posted again after the
    /// lease expires, which is the one outcome nothing downstream can see.
    pub async fn flush_once(&self) -> Result<usize, sqlx::Error> {
        let lease_secs = i64::try_from(self.cfg.lease.as_secs()).unwrap_or(i64::MAX);
        sweep(
            &self.pool,
            self.cfg.max_attempts,
            Some((self.target.kind(), self.target.upstream())),
        )
        .await?;
        // Atomically claim, lease and count a batch: `attempts` is advanced here
        // and nowhere else, so the budget survives a worker killed between the
        // POST and the outcome write. RETURNING yields the post-increment value,
        // the number of this attempt. A spent budget is never claimed again —
        // only the sweep above takes it.
        let rows: Vec<Claimed> = sqlx::query_as(
            "UPDATE event_outbox
                SET next_attempt_at = now() + ($1 * INTERVAL '1 second'),
                    attempts = attempts + 1
              WHERE id IN (
                  SELECT id FROM event_outbox
                   WHERE delivered_at IS NULL AND dead_lettered_at IS NULL
                     AND next_attempt_at <= now()
                     AND attempts < $3
                     AND kind = $4
                     AND upstream IS NOT DISTINCT FROM $5
                   ORDER BY next_attempt_at
                   LIMIT $2
                   FOR UPDATE SKIP LOCKED
              )
              RETURNING id, envelope, attempts",
        )
        .bind(lease_secs)
        .bind(self.cfg.batch_size)
        .bind(self.cfg.max_attempts)
        .bind(self.target.kind())
        .bind(self.target.upstream())
        .fetch_all(&self.pool)
        .await?;

        let n = rows.len();
        // Expose the secret once per batch (not per row).
        let secret = match &self.target {
            Target::CloudEvents { secret, .. } => secret
                .as_ref()
                .map(|s| s.expose_secret().as_bytes().to_vec()),
            Target::Commands { .. } => None,
        };
        for row in rows {
            let outcome = self.deliver(row.envelope, secret.as_deref()).await;
            match outcome {
                Ok(delivered) => {
                    if let Err(db) = self.mark_delivered(row.id, &delivered).await {
                        // The receiver accepted the delivery and the row still
                        // says it did not, so the next flush sends it again — and
                        // keeps sending it. Nothing downstream can tell the
                        // difference; only this line can.
                        tracing::error!(
                            outbox_id = %row.id, error = %db,
                            "outbox: delivered row could not be marked delivered; \
                             it will be redelivered"
                        );
                    }
                }
                Err((true, e)) => {
                    if let Err(db) = self.dead_letter(row.id, &e).await {
                        tracing::error!(
                            outbox_id = %row.id, error = %db,
                            "outbox: permanent delivery failure could not be \
                             dead-lettered; the row stays in the queue"
                        );
                    }
                }
                Err((false, e)) => {
                    if let Err(db) = self.record_failure(row.id, row.attempts, &e).await {
                        tracing::warn!(
                            outbox_id = %row.id, error = %db,
                            "outbox: retry bookkeeping failed; the lease still \
                             expires and the row retries"
                        );
                    }
                }
            }
        }
        Ok(n)
    }

    /// Deliver one claimed row's envelope to this worker's target.
    /// `Err((permanent, reason))` on failure.
    async fn deliver(
        &self,
        envelope: serde_json::Value,
        secret: Option<&[u8]>,
    ) -> Result<Delivered, (bool, String)> {
        match &self.target {
            Target::CloudEvents { url, .. } => {
                match serde_json::from_value::<CloudEvent>(envelope) {
                    Ok(ce) => crate::post_ce_for_reply(&self.client, url, &ce, secret)
                        .await
                        .map(|reply| Delivered {
                            ce_type: ce.ce_type.clone(),
                            subject: ce.subject.clone(),
                            reply,
                        })
                        .map_err(|e| (e.is_permanent(), e.to_string())),
                    Err(e) => Err((true, format!("undecodable envelope: {e}"))),
                }
            }
            Target::Commands {
                base_url,
                bearer,
                signed,
                ..
            } => match serde_json::from_value::<Command>(envelope) {
                Ok(cmd) => self
                    .send_command(base_url, bearer.as_ref(), *signed, &cmd)
                    .await
                    .map(|reply| Delivered {
                        ce_type: cmd.label.clone(),
                        subject: Some(cmd.key.clone()),
                        reply,
                    }),
                Err(e) => Err((true, format!("undecodable command: {e}"))),
            },
        }
    }

    /// Send one [`Command`]. `Err((permanent, reason))` on failure: a 408, a
    /// 429, a 5xx or a transport error is transient, any other non-2xx status that is
    /// not one of the command's settled statuses is permanent.
    async fn send_command(
        &self,
        base_url: &str,
        bearer: Option<&SecretString>,
        signed: bool,
        cmd: &Command,
    ) -> Result<Option<serde_json::Value>, (bool, String)> {
        let url = if cmd.path.is_empty() {
            base_url.to_owned()
        } else {
            format!("{}{}", base_url.trim_end_matches('/'), cmd.path)
        };
        let payload =
            serde_json::to_vec(&cmd.body).map_err(|e| (true, format!("unencodable body: {e}")))?;
        let mut req = match cmd.method {
            CommandMethod::Post => self.client.post(&url),
            CommandMethod::Put => self.client.put(&url),
        }
        .header("Idempotency-Key", &cmd.key)
        .header("Content-Type", "application/json");
        if let Some(b) = bearer {
            req = req.bearer_auth(b.expose_secret());
            if signed {
                for (name, value) in crate::webhook::headers(
                    b.expose_secret().as_bytes(),
                    &cmd.key,
                    time::OffsetDateTime::now_utc().unix_timestamp(),
                    &payload,
                ) {
                    req = req.header(name, value);
                }
            }
        }
        match req.body(payload).send().await {
            Ok(r) if r.status().is_success() => Ok(r.json().await.ok()),
            Ok(r) if cmd.settled_statuses.contains(&r.status().as_u16()) => {
                tracing::debug!(
                    key = %cmd.key, status = r.status().as_u16(),
                    "outbox: command answered with a settled status — discharged"
                );
                Ok(None)
            }
            Ok(r) => {
                let status = r.status();
                let transient = crate::cloudevent::is_transient_status(status);
                let body = r.text().await.unwrap_or_default();
                Err((!transient, format!("HTTP {status}: {body}")))
            }
            Err(e) => Err((false, e.to_string())),
        }
    }

    async fn mark_delivered(
        &self,
        id: uuid::Uuid,
        delivered: &Delivered,
    ) -> Result<(), sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "UPDATE event_outbox SET delivered_at = now(), last_error = NULL WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?;
        if let Some(handler) = &self.on_delivered {
            handler(&mut tx, delivered).await?;
        }
        tx.commit().await
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
        // `attempts` was counted at the claim: it is the number of this attempt.
        if attempts >= self.cfg.max_attempts {
            return self.dead_letter(id, err).await;
        }
        let idx = usize::try_from(attempts - 1)
            .unwrap_or(0)
            .min(self.cfg.backoff.len() - 1);
        let delay = i64::try_from(self.cfg.backoff[idx].as_secs()).unwrap_or(i64::MAX);
        sqlx::query(
            "UPDATE event_outbox
                SET next_attempt_at = now() + ($2 * INTERVAL '1 second'),
                    last_error = $3
              WHERE id = $1",
        )
        .bind(id)
        .bind(delay)
        .bind(err)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

/// Dead-letter every row whose attempt budget is spent but which is still in
/// the queue. Returns the number of rows swept.
///
/// On the healthy path the worker dead-letters a row itself when its last
/// attempt fails, so this sweeps nothing. It is the backstop for the path where
/// that outcome write never landed — the worker was killed between the POST
/// and the write, or the write hit a database error. Because the attempt is
/// counted at the claim, such a row still spends its budget; without the sweep
/// it would sit due and never reach [`list_dead_letters`].
///
/// `next_attempt_at <= now()` excludes rows whose lease is still running, so a
/// delivery in flight is never dead-lettered. `last_error` keeps the diagnostic
/// of the last attempt that recorded one.
///
/// # Errors
///
/// Returns `sqlx::Error` on database failure.
pub async fn sweep_exhausted(pool: &PgPool, max_attempts: i16) -> Result<u64, sqlx::Error> {
    sweep(pool, max_attempts, None).await
}

/// [`sweep_exhausted`], narrowed to one worker's rows when `scope` names its
/// kind and upstream — a worker sweeps only what its own budget governs.
async fn sweep(
    pool: &PgPool,
    max_attempts: i16,
    scope: Option<(&str, Option<&str>)>,
) -> Result<u64, sqlx::Error> {
    let (kind, upstream) = scope.unwrap_or(("", None));
    let swept: Vec<(uuid::Uuid, String, i16)> = sqlx::query_as(
        "UPDATE event_outbox
            SET dead_lettered_at = now()
          WHERE delivered_at IS NULL AND dead_lettered_at IS NULL
            AND attempts >= $1
            AND next_attempt_at <= now()
            AND ($2 = '' OR (kind = $2 AND upstream IS NOT DISTINCT FROM $3))
          RETURNING id, event_id, attempts",
    )
    .bind(max_attempts)
    .bind(kind)
    .bind(upstream)
    .fetch_all(pool)
    .await?;
    for (id, event_id, attempts) in &swept {
        tracing::error!(
            outbox_id = %id, %event_id, attempts,
            "outbox: attempt budget spent with no recorded outcome — dead-lettering"
        );
    }
    Ok(swept.len() as u64)
}

/// A dead-lettered outbox row, for admin inspection.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct DeadLetter {
    /// The outbox row's id.
    pub id: uuid::Uuid,
    /// The CloudEvent `id`, or a command's key.
    pub event_id: String,
    /// `cloudevent` or `command`.
    pub kind: String,
    /// The upstream a command is owed to; `None` for an event.
    pub upstream: Option<String>,
    /// The CloudEvent `type`, or a command's label.
    pub ce_type: String,
    /// Delivery attempts made before the row was dead-lettered.
    pub attempts: i16,
    /// The last attempt's failure, if one was recorded.
    pub last_error: Option<String>,
    /// When the row was dead-lettered.
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
        "SELECT id, event_id, kind, upstream, ce_type, attempts, last_error, dead_lettered_at
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
    fn the_default_policy_is_30s_times_four_capped_at_two_hours() {
        let c = OutboxConfig::default();
        assert_eq!(c.max_attempts, 8);
        assert_eq!(
            c.backoff,
            vec![
                Duration::from_secs(30),
                Duration::from_mins(2),
                Duration::from_mins(8),
                Duration::from_mins(32),
                Duration::from_hours(2),
            ]
        );
    }
}
