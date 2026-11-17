//! Fan-out and delivery: two phases over the service's `event_outbox`.
//!
//! **Fan-out.** Claim the CloudEvents not yet fanned out, in commit order, and
//! in one transaction insert a delivery per matching endpoint and stamp
//! `fanned_out_at`. A crash before the commit leaves the events unfanned, and
//! the next pass fans them again; the delivery rows are unique per (endpoint,
//! event), so nothing is delivered twice by it.
//!
//! **Delivery.** Claim due deliveries with a lease, count the attempt in the
//! claim (so the budget holds when the outcome write is lost), sign with every
//! active secret, POST, record the status. A delivery is held back while an
//! earlier event of its ordering key is still owed to the same endpoint — or
//! not yet fanned out; other keys and key-less events never wait. A
//! dead-lettered delivery stops blocking. `410 Gone` deactivates the endpoint.

use std::time::{Duration, Instant};

use sqlx::PgPool;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::Subscriptions;
use super::store::{ENDPOINT_COLS, EndpointRow};
use crate::outbox::{COMMITTED_BELOW, NOTIFY_CHANNEL};

#[derive(sqlx::FromRow)]
struct Unfanned {
    id: Uuid,
    txid: i64,
    seq: i64,
    event_id: String,
    ce_type: String,
    envelope: serde_json::Value,
    ordering_key: Option<String>,
}

#[derive(sqlx::FromRow)]
struct Claimed {
    id: Uuid,
    endpoint_id: Uuid,
    event_id: String,
    attempts: i16,
    envelope: serde_json::Value,
    url: String,
    owner: String,
    secret: Option<String>,
    previous_secret: Option<String>,
    previous_active: bool,
}

/// What one attempt came to.
enum Outcome {
    Delivered {
        status: u16,
        latency_ms: i32,
    },
    Gone {
        latency_ms: i32,
    },
    Failed {
        status: Option<u16>,
        latency_ms: Option<i32>,
        error: String,
    },
}

impl Subscriptions {
    /// Fan out one batch of unfanned events. Returns how many were fanned.
    ///
    /// # Errors
    ///
    /// A database error; the batch is then left unfanned.
    pub async fn fan_out_once(&self) -> Result<usize, sqlx::Error> {
        let mut tx = self.inner.pool.begin().await?;
        let events: Vec<Unfanned> = sqlx::query_as(crate::sql::dynamic(format!(
            "SELECT id, txid, seq, event_id, ce_type, envelope, ordering_key
               FROM event_outbox
              WHERE fanned_out_at IS NULL AND kind = 'cloudevent' AND upstream IS NULL
                AND txid < {COMMITTED_BELOW}
              ORDER BY txid, seq
              LIMIT $1
              FOR UPDATE SKIP LOCKED"
        )))
        .bind(self.inner.policy.batch_size)
        .fetch_all(&mut *tx)
        .await?;
        if events.is_empty() {
            return Ok(0);
        }
        let endpoints: Vec<EndpointRow> = sqlx::query_as(crate::sql::dynamic(format!(
            "SELECT {ENDPOINT_COLS} FROM webhook_endpoints WHERE tenant = $1 AND active"
        )))
        .bind(&self.inner.tenant)
        .fetch_all(&mut *tx)
        .await?;
        for event in &events {
            for endpoint in endpoints.iter().filter(|e| e.matches(&event.envelope)) {
                sqlx::query(
                    "INSERT INTO webhook_deliveries
                         (tenant, endpoint_id, event_id, ce_type, event_txid, event_seq, ordering_key)
                     VALUES ($1, $2, $3, $4, $5, $6, $7)
                     ON CONFLICT (endpoint_id, event_id) DO NOTHING",
                )
                .bind(&self.inner.tenant)
                .bind(endpoint.id)
                .bind(&event.event_id)
                .bind(&event.ce_type)
                .bind(event.txid)
                .bind(event.seq)
                .bind(&event.ordering_key)
                .execute(&mut *tx)
                .await?;
            }
        }
        let ids: Vec<Uuid> = events.iter().map(|e| e.id).collect();
        sqlx::query("UPDATE event_outbox SET fanned_out_at = now() WHERE id = ANY($1)")
            .bind(&ids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(events.len())
    }

    /// Dead-letter a delivery whose budget is spent but whose outcome write
    /// was lost; on the healthy path the attempt itself dead-letters it.
    async fn sweep_exhausted(&self) -> Result<(), sqlx::Error> {
        let swept: Vec<(Uuid, String)> = sqlx::query_as(
            "UPDATE webhook_deliveries
                SET dead_lettered_at = now()
              WHERE tenant = $1 AND delivered_at IS NULL AND dead_lettered_at IS NULL
                AND attempts >= $2 AND next_attempt_at <= now()
              RETURNING id, event_id",
        )
        .bind(&self.inner.tenant)
        .bind(self.inner.policy.max_attempts)
        .fetch_all(&self.inner.pool)
        .await?;
        for (id, event_id) in swept {
            tracing::error!(delivery_id = %id, %event_id,
                "subscriptions: attempt budget spent with no recorded outcome — dead-lettering");
        }
        Ok(())
    }

    /// Claim and attempt one batch of due deliveries. Returns how many.
    ///
    /// # Errors
    ///
    /// A database error on the sweep or the claim; an attempt's own failure is
    /// recorded on its row.
    pub async fn deliver_once(&self) -> Result<usize, sqlx::Error> {
        self.sweep_exhausted().await?;
        let lease = i64::try_from(self.inner.policy.lease.as_secs()).unwrap_or(i64::MAX);
        let claimed: Vec<Claimed> = sqlx::query_as(
            "WITH due AS (
                 SELECT d.id FROM webhook_deliveries d
                   JOIN webhook_endpoints w ON w.id = d.endpoint_id AND w.active
                  WHERE d.tenant = $4
                    AND d.delivered_at IS NULL AND d.dead_lettered_at IS NULL
                    AND d.next_attempt_at <= now()
                    AND d.attempts < $3
                    AND (d.ordering_key IS NULL OR (
                         NOT EXISTS (
                             SELECT 1 FROM webhook_deliveries e
                              WHERE e.endpoint_id = d.endpoint_id
                                AND e.ordering_key = d.ordering_key
                                AND (e.event_txid, e.event_seq) < (d.event_txid, d.event_seq)
                                AND e.delivered_at IS NULL AND e.dead_lettered_at IS NULL)
                         AND NOT EXISTS (
                             SELECT 1 FROM event_outbox o
                              WHERE o.fanned_out_at IS NULL
                                AND o.kind = 'cloudevent' AND o.upstream IS NULL
                                AND o.ordering_key = d.ordering_key
                                AND (o.txid, o.seq) < (d.event_txid, d.event_seq))))
                  ORDER BY d.event_txid, d.event_seq
                  LIMIT $2
                  FOR UPDATE OF d SKIP LOCKED
             ), claimed AS (
                 UPDATE webhook_deliveries d
                    SET attempts = d.attempts + 1,
                        next_attempt_at = now() + ($1 * INTERVAL '1 second')
                   FROM due
                  WHERE d.id = due.id
                  RETURNING d.id, d.endpoint_id, d.event_id, d.attempts
             )
             SELECT c.id, c.endpoint_id, c.event_id, c.attempts, o.envelope, w.url, w.owner,
                    w.secret, w.previous_secret,
                    COALESCE(w.previous_secret_expires_at > now(), false) AS previous_active
               FROM claimed c
               JOIN webhook_endpoints w ON w.id = c.endpoint_id
               JOIN event_outbox o ON o.event_id = c.event_id",
        )
        .bind(lease)
        .bind(self.inner.policy.batch_size)
        .bind(self.inner.policy.max_attempts)
        .bind(&self.inner.tenant)
        .fetch_all(&self.inner.pool)
        .await?;
        let n = claimed.len();
        let mut set = tokio::task::JoinSet::new();
        for c in claimed {
            let this = self.clone();
            set.spawn(async move { this.attempt(c).await });
        }
        while set.join_next().await.is_some() {}
        Ok(n)
    }

    async fn attempt(&self, c: Claimed) {
        let mut secrets: Vec<&str> = c.secret.iter().map(String::as_str).collect();
        if c.previous_active
            && let Some(prev) = c.previous_secret.as_deref()
        {
            secrets.push(prev);
        }
        let outcome = self
            .post(
                &c.url,
                c.owner == "config",
                &c.event_id,
                &secrets,
                &c.envelope,
            )
            .await;
        if let Err(e) = self.record(&c, &outcome).await {
            tracing::error!(delivery_id = %c.id, endpoint_id = %c.endpoint_id, error = %e,
                "subscriptions: the outcome of an attempt could not be written; the claim \
                 counted it, so the delivery still dead-letters rather than retrying forever");
        }
    }

    /// POST `envelope` to `url`, signed per Standard Webhooks with every
    /// secret in `secrets` (`webhook-id` the CloudEvent id, the timestamp this
    /// attempt's).
    async fn post(
        &self,
        url: &str,
        config_owned: bool,
        event_id: &str,
        secrets: &[&str],
        envelope: &serde_json::Value,
    ) -> Outcome {
        let client = if config_owned {
            &self.inner.config_client
        } else {
            if let Err(e) = super::ssrf::check_url(url, self.inner.cfg.allow_loopback) {
                return Outcome::Failed {
                    status: None,
                    latency_ms: None,
                    error: format!("endpoint_url_not_allowed: {e}"),
                };
            }
            &self.inner.api_client
        };
        let body = match serde_json::to_vec(envelope) {
            Ok(b) => b,
            Err(e) => {
                return Outcome::Failed {
                    status: None,
                    latency_ms: None,
                    error: format!("unencodable envelope: {e}"),
                };
            }
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut req = client
            .post(url)
            .header("Content-Type", "application/cloudevents+json")
            .header(crate::webhook::ID_HEADER, event_id)
            .header(crate::webhook::TIMESTAMP_HEADER, now.to_string());
        if !secrets.is_empty() {
            let keys: Vec<Vec<u8>> = secrets
                .iter()
                .map(|s| crate::webhook::signing_key(s))
                .collect();
            req = req.header(
                crate::webhook::SIGNATURE_HEADER,
                crate::webhook::sign_all(&keys, event_id, now, &body),
            );
        }
        let started = Instant::now();
        let res = req.body(body).send().await;
        let latency_ms = i32::try_from(started.elapsed().as_millis()).unwrap_or(i32::MAX);
        match res {
            Ok(r) if r.status().is_success() => Outcome::Delivered {
                status: r.status().as_u16(),
                latency_ms,
            },
            Ok(r) if r.status() == reqwest::StatusCode::GONE => Outcome::Gone { latency_ms },
            Ok(r) => Outcome::Failed {
                status: Some(r.status().as_u16()),
                latency_ms: Some(latency_ms),
                error: format!("HTTP {}", r.status()),
            },
            Err(e) => {
                // The guarded resolver's refusal is the source of a connect error.
                let mut why = e.to_string();
                let mut source = std::error::Error::source(&e);
                while let Some(s) = source {
                    why = format!("{why}: {s}");
                    source = s.source();
                }
                Outcome::Failed {
                    status: None,
                    latency_ms: None,
                    error: why,
                }
            }
        }
    }

    async fn record(&self, c: &Claimed, outcome: &Outcome) -> Result<(), sqlx::Error> {
        let pool = &self.inner.pool;
        match outcome {
            Outcome::Delivered { status, latency_ms } => {
                sqlx::query(
                    "UPDATE webhook_deliveries
                        SET delivered_at = now(), last_status = $2, latency_ms = $3, last_error = NULL
                      WHERE id = $1",
                )
                .bind(c.id)
                .bind(i16::try_from(*status).unwrap_or(i16::MAX))
                .bind(latency_ms)
                .execute(pool)
                .await?;
            }
            Outcome::Gone { latency_ms } => {
                let mut tx = pool.begin().await?;
                sqlx::query(
                    "UPDATE webhook_deliveries
                        SET dead_lettered_at = now(), last_status = 410, latency_ms = $2,
                            last_error = 'the endpoint answered 410 Gone and was deactivated'
                      WHERE id = $1",
                )
                .bind(c.id)
                .bind(latency_ms)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE webhook_endpoints
                        SET active = false, deactivated_reason = 'gone',
                            version = version + 1, updated_at = now()
                      WHERE id = $1 AND tenant = $2",
                )
                .bind(c.endpoint_id)
                .bind(&self.inner.tenant)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                tracing::warn!(endpoint_id = %c.endpoint_id,
                    "subscriptions: endpoint answered 410 Gone — deactivated");
            }
            Outcome::Failed {
                status,
                latency_ms,
                error,
            } => {
                let status = status.map(|s| i16::try_from(s).unwrap_or(i16::MAX));
                if c.attempts >= self.inner.policy.max_attempts {
                    tracing::error!(delivery_id = %c.id, endpoint_id = %c.endpoint_id,
                        attempts = c.attempts, %error, "subscriptions: attempts spent — dead-lettering");
                    sqlx::query(
                        "UPDATE webhook_deliveries
                            SET dead_lettered_at = now(), last_status = $2, latency_ms = $3, last_error = $4
                          WHERE id = $1",
                    )
                    .bind(c.id)
                    .bind(status)
                    .bind(latency_ms)
                    .bind(error)
                    .execute(pool)
                    .await?;
                } else {
                    let delay = self.backoff(c.attempts, c.endpoint_id);
                    sqlx::query(
                        "UPDATE webhook_deliveries
                            SET next_attempt_at = now() + ($2 * INTERVAL '1 second'),
                                last_status = $3, latency_ms = $4, last_error = $5
                          WHERE id = $1",
                    )
                    .bind(c.id)
                    .bind(delay)
                    .bind(status)
                    .bind(latency_ms)
                    .bind(error)
                    .execute(pool)
                    .await?;
                }
            }
        }
        Ok(())
    }

    /// The delay before the next attempt: the policy's backoff for this
    /// attempt, spread over ±12.5 % by the endpoint so a recovered endpoint's
    /// backlog does not arrive as one burst.
    fn backoff(&self, attempts: i16, endpoint: Uuid) -> i64 {
        let backoff = &self.inner.policy.backoff;
        let idx = usize::try_from(attempts.saturating_sub(1)).unwrap_or(0);
        let base = backoff
            .get(idx)
            .or_else(|| backoff.last())
            .map_or(30, Duration::as_secs);
        if base == 0 {
            return 0;
        }
        let spread = (base / 4).max(1);
        let hash = endpoint
            .as_bytes()
            .iter()
            .fold(u64::from(attempts.unsigned_abs()), |acc, b| {
                acc.wrapping_mul(31).wrapping_add(u64::from(*b))
            });
        i64::try_from(base.saturating_sub(spread / 2) + hash % spread).unwrap_or(i64::MAX)
    }

    /// Delete what has left the retention window: terminal deliveries, and
    /// CloudEvents fanned out with nothing still owed. Returns the events
    /// pruned.
    ///
    /// An event the [`crate::outbox::OutboxWorker`] still has to deliver
    /// (attempted and neither delivered nor dead-lettered) is kept.
    ///
    /// # Errors
    ///
    /// A database error.
    pub async fn prune(&self) -> Result<u64, sqlx::Error> {
        let secs = i64::from(self.inner.cfg.retention_days) * 86_400;
        let mut tx = self.inner.pool.begin().await?;
        sqlx::query(
            "DELETE FROM webhook_deliveries
              WHERE tenant = $1 AND created_at < now() - ($2 * INTERVAL '1 second')
                AND (delivered_at IS NOT NULL OR dead_lettered_at IS NOT NULL)",
        )
        .bind(&self.inner.tenant)
        .bind(secs)
        .execute(&mut *tx)
        .await?;
        let pruned = sqlx::query(
            "DELETE FROM event_outbox o
              WHERE o.kind = 'cloudevent' AND o.upstream IS NULL
                AND o.fanned_out_at IS NOT NULL
                AND o.created_at < now() - ($1 * INTERVAL '1 second')
                AND (o.delivered_at IS NOT NULL OR o.dead_lettered_at IS NOT NULL OR o.attempts = 0)
                AND NOT EXISTS (
                    SELECT 1 FROM webhook_deliveries d
                     WHERE d.event_id = o.event_id
                       AND d.delivered_at IS NULL AND d.dead_lettered_at IS NULL)",
        )
        .bind(secs)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(pruned)
    }

    /// Fan out and deliver until neither phase finds work.
    pub async fn drain(&self) {
        loop {
            let fanned = self.fan_out_once().await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "subscriptions: fan-out pass failed");
                0
            });
            let delivered = self.deliver_once().await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "subscriptions: delivery pass failed");
                0
            });
            if fanned == 0 && delivered == 0 {
                return;
            }
        }
    }

    /// The worker loop: woken by the outbox's commit-coupled `NOTIFY`, polling
    /// on the policy's interval, pruning hourly.
    pub(crate) async fn run(self, shutdown: CancellationToken) {
        let mut interval = tokio::time::interval(self.inner.policy.poll_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut prune = tokio::time::interval(Duration::from_hours(1));
        prune.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        prune.reset();
        let mut listener = open_listener(&self.inner.pool).await;
        tracing::info!(
            listening = listener.is_some(),
            "subscriptions: worker started"
        );
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                notified = wait_for_notify(&mut listener) => {
                    if notified {
                        self.drain().await;
                    }
                }
                _ = prune.tick() => match self.prune().await {
                    Ok(n) if n > 0 => tracing::debug!(pruned = n, "subscriptions: pruned events"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "subscriptions: prune failed"),
                },
                _ = interval.tick() => self.drain().await,
            }
        }
    }
}

async fn open_listener(pool: &PgPool) -> Option<sqlx::postgres::PgListener> {
    match crate::outbox::own_listener(pool).await {
        Ok(mut l) => match l.listen(NOTIFY_CHANNEL).await {
            Ok(()) => Some(l),
            Err(e) => {
                tracing::warn!(error = %e,
                    "subscriptions: LISTEN failed — draining on the poll interval alone");
                None
            }
        },
        Err(e) => {
            tracing::warn!(error = %e,
                "subscriptions: listener connection failed — draining on the poll interval alone");
            None
        }
    }
}

/// `true` when a notification arrived; never resolves without a listener.
async fn wait_for_notify(listener: &mut Option<sqlx::postgres::PgListener>) -> bool {
    match listener.as_mut() {
        Some(l) => match l.recv().await {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(error = %e,
                    "subscriptions: listener lost — draining on the poll interval alone");
                *listener = None;
                false
            }
        },
        None => std::future::pending().await,
    }
}

/// The test event to one endpoint, sent at once and recorded nowhere.
pub(crate) async fn send_test(
    subs: &Subscriptions,
    endpoint: &EndpointRow,
    secrets: &[&str],
) -> super::routes::TestDelivery {
    let payload = super::WebhookTest {
        endpoint_id: endpoint.id.to_string(),
        message: "Testereignis".to_owned(),
    };
    let event = match crate::CloudEvent::typed(
        crate::source(&subs.inner.service, &subs.inner.tenant),
        format!("webhooks/endpoints/{}", endpoint.id),
        &payload,
    ) {
        Ok(e) => e,
        Err(e) => {
            return super::routes::TestDelivery {
                delivered: false,
                status: None,
                latency_ms: None,
                error: Some(format!("unencodable test event: {e}")),
            };
        }
    };
    let envelope = serde_json::to_value(&event).unwrap_or_default();
    let outcome = subs
        .post(
            &endpoint.url,
            endpoint.owner == "config",
            &event.id,
            secrets,
            &envelope,
        )
        .await;
    match outcome {
        Outcome::Delivered { status, latency_ms } => super::routes::TestDelivery {
            delivered: true,
            status: Some(status),
            latency_ms: Some(latency_ms),
            error: None,
        },
        Outcome::Gone { latency_ms } => super::routes::TestDelivery {
            delivered: false,
            status: Some(410),
            latency_ms: Some(latency_ms),
            error: Some("HTTP 410 Gone".to_owned()),
        },
        Outcome::Failed {
            status,
            latency_ms,
            error,
        } => super::routes::TestDelivery {
            delivered: false,
            status,
            latency_ms,
            error: Some(error),
        },
    }
}
