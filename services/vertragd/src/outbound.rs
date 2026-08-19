//! Durable outbound tasks — everything `vertragd` owes another service.
//!
//! A Lieferbeginn is an obligation: the customer has a contract and the NB is
//! waiting for the UTILMD. Dispatching it from a detached `tokio::spawn` meant a
//! restart between the contract insert and the `processd` call dropped the
//! registration in silence, leaving the component in `ANGELEGT` with nothing to
//! retry it — and the same held for the Schlussablesung, the tariff assignment
//! and the billing account.
//!
//! So the *intent* is written in the same transaction as the contract change
//! ([`enqueue`]), and [`OutboundWorker`] performs it afterwards with exponential
//! backoff and a dead-letter. A crash at any point loses nothing; it only
//! delays.
//!
//! ```text
//! handler ─┬─ contract write ──┐
//!          └─ enqueue task  ───┴─ COMMIT ─→ worker ─→ processd / edmd / tarifbd / accountingd
//! ```

use std::sync::Arc;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::Date;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::config::VertragdConfig;

/// How many times a task is attempted before it is dead-lettered.
const MAX_ATTEMPTS: i32 = 8;
/// Base of the exponential backoff, in seconds: 30s, 1m, 2m, … capped at 1h.
const BACKOFF_BASE_SECS: u64 = 30;
const BACKOFF_CAP_SECS: u64 = 3600;

/// What the worker has to do. The variant decides the target service, the HTTP
/// shape and the follow-up write, so callers name the obligation rather than
/// assembling a URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskKind {
    /// `processd` UTILMD Anmeldung (GPKE 55001 / GeLi Gas 44001).
    Lieferbeginn,
    /// `processd` UTILMD Abmeldung.
    Lieferende,
    /// `edmd` GPKE Beginnablesung.
    AblesungBeginn,
    /// `edmd` GPKE Schlussablesung — the basis of the Schlussrechnung.
    AblesungEnde,
    /// `accountingd` billing account for a contract that went AKTIV.
    Abrechnungskonto,
}

impl TaskKind {
    #[must_use]
    pub const fn as_db(self) -> &'static str {
        match self {
            Self::Lieferbeginn => "LIEFERBEGINN",
            Self::Lieferende => "LIEFERENDE",
            Self::AblesungBeginn => "ABLESUNG_BEGINN",
            Self::AblesungEnde => "ABLESUNG_ENDE",
            Self::Abrechnungskonto => "ABRECHNUNGSKONTO",
        }
    }

    fn from_db(s: &str) -> Option<Self> {
        Some(match s {
            "LIEFERBEGINN" => Self::Lieferbeginn,
            "LIEFERENDE" => Self::Lieferende,
            "ABLESUNG_BEGINN" => Self::AblesungBeginn,
            "ABLESUNG_ENDE" => Self::AblesungEnde,
            "ABRECHNUNGSKONTO" => Self::Abrechnungskonto,
            _ => None?,
        })
    }
}

/// A task ready to be enqueued, with the deduplication key that makes the
/// enqueue exactly-once.
#[derive(Debug, Clone)]
pub struct Task {
    pub kind: TaskKind,
    pub komp_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub dedupe_key: String,
}

// ── Task constructors ─────────────────────────────────────────────────────────

/// The `processd` Lieferbeginn task for one component.
///
/// # Errors
///
/// Refuses a gas component without a Messlokation: `start-supply-gas` requires
/// the Zählpunktbezeichnung (RFF+Z13, mandatory per the BK7-24-01-009 AHB), and
/// a MaLo-ID is not one — an 11-digit MaLo in a 33-character Zählpunkt field is
/// a malformed UTILMD, which the NB rejects after the fact instead of the API
/// rejecting it now.
pub fn lieferbeginn(
    komp_id: Uuid,
    sparte: &str,
    malo_id: &str,
    melo_id: Option<&str>,
    nb_mp_id: &str,
    lf_mp_id: &str,
    lieferbeginn: Date,
) -> Result<Task> {
    let payload = lieferbeginn_body(sparte, malo_id, melo_id, nb_mp_id, lf_mp_id, lieferbeginn)?;
    Ok(Task {
        kind: TaskKind::Lieferbeginn,
        komp_id: Some(komp_id),
        payload,
        dedupe_key: format!("LIEFERBEGINN:{komp_id}"),
    })
}

/// Build the `processd` Lieferbeginn request body for the commodity.
///
/// The contract differs by commodity (verified against
/// `services/processd/src/server.rs`):
/// - `start-supply` (Strom et al.) takes `lieferbeginn_datum` (ISO-8601);
/// - `start-supply-gas` takes `zaehlpunkt` (the Messlokation) and
///   `process_date` (YYYYMMDD).
///
/// Pure, so the field-name contract is unit-tested without a live `processd`.
///
/// # Errors
///
/// A gas supply point with no Messlokation.
pub fn lieferbeginn_body(
    sparte: &str,
    malo_id: &str,
    melo_id: Option<&str>,
    nb_mp_id: &str,
    lf_mp_id: &str,
    lieferbeginn: Date,
) -> Result<serde_json::Value> {
    if sparte == "GAS" {
        let zaehlpunkt = melo_id.filter(|m| !m.is_empty()).ok_or_else(|| {
            anyhow::anyhow!(
                "gas supply point {malo_id} has no melo_id; \
                 start-supply-gas requires the Zählpunktbezeichnung (RFF+Z13)"
            )
        })?;
        return Ok(serde_json::json!({
            "endpoint": "start-supply-gas",
            "malo_id": malo_id,
            "zaehlpunkt": zaehlpunkt,
            "nb_mp_id": nb_mp_id,
            "lf_mp_id": lf_mp_id,
            "process_date": lieferbeginn
                .format(time::macros::format_description!("[year][month][day]"))
                .unwrap_or_else(|_| lieferbeginn.to_string()),
        }));
    }
    Ok(serde_json::json!({
        "endpoint": "start-supply",
        "malo_id": malo_id,
        "nb_mp_id": nb_mp_id,
        "lf_mp_id": lf_mp_id,
        "lieferbeginn_datum": lieferbeginn.to_string(),
    }))
}

/// The `processd` Lieferende task for one component.
#[must_use]
pub fn lieferende(
    komp_id: Uuid,
    sparte: &str,
    malo_id: &str,
    nb_mp_id: &str,
    lf_mp_id: &str,
    lieferende: Date,
) -> Task {
    let endpoint = if sparte == "GAS" {
        "end-supply-gas"
    } else {
        "end-supply"
    };
    Task {
        kind: TaskKind::Lieferende,
        komp_id: Some(komp_id),
        payload: serde_json::json!({
            "endpoint": endpoint,
            "malo_id": malo_id,
            "nb_mp_id": nb_mp_id,
            "lf_mp_id": lf_mp_id,
            "lieferende_datum": lieferende.to_string(),
        }),
        dedupe_key: format!("LIEFERENDE:{komp_id}"),
    }
}

/// An `edmd` reading order (GPKE Beginn-/Schlussablesung).
///
/// The Schlussablesung is the LF's own obligation and does not depend on the
/// Lieferende UTILMD reaching the NB, so it is a task of its own rather than a
/// step of one — a `processd` outage must not suppress the reading the
/// Schlussrechnung is built from.
#[must_use]
pub fn ablesung(komp_id: Uuid, malo_id: &str, ende: bool, geplant_am: Date) -> Task {
    let (kind, anlass) = if ende {
        (TaskKind::AblesungEnde, "LIEFERENDE")
    } else {
        (TaskKind::AblesungBeginn, "LIEFERBEGINN")
    };
    Task {
        kind,
        komp_id: Some(komp_id),
        payload: serde_json::json!({
            "malo_id": malo_id,
            "anlass": anlass,
            "auftraggeber_rolle": "LF",
            "geplant_am": geplant_am.to_string(),
            "auftrag_position_id": komp_id,
        }),
        dedupe_key: format!("{}:{komp_id}", kind.as_db()),
    }
}

/// An `accountingd` billing account for a contract that reached AKTIV.
#[must_use]
pub fn abrechnungskonto(komp_id: Uuid, malo_id: &str, lf_mp_id: &str) -> Task {
    Task {
        kind: TaskKind::Abrechnungskonto,
        komp_id: Some(komp_id),
        payload: serde_json::json!({ "malo_id": malo_id, "lf_mp_id": lf_mp_id }),
        dedupe_key: format!("ABRECHNUNGSKONTO:{malo_id}"),
    }
}

// ── Enqueue ───────────────────────────────────────────────────────────────────

/// Persist a task in the caller's transaction. `false` when an identical task
/// was already enqueued — the exactly-once guarantee, in the database rather
/// than in a caller's memory.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn enqueue(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    task: &Task,
) -> Result<bool> {
    let n = sqlx::query(
        "INSERT INTO outbound_tasks (tenant, kind, komp_id, payload, dedupe_key)
         VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (tenant, dedupe_key) DO NOTHING",
    )
    .bind(tenant)
    .bind(task.kind.as_db())
    .bind(task.komp_id)
    .bind(&task.payload)
    .bind(&task.dedupe_key)
    .execute(executor)
    .await?
    .rows_affected();
    Ok(n > 0)
}

/// Tasks that exhausted their retries — the operator's work queue.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_dead_lettered(pool: &PgPool, tenant: &str, limit: i64) -> Result<Vec<DeadTask>> {
    Ok(sqlx::query_as::<_, DeadTask>(
        "SELECT id, kind, komp_id, payload, attempts, last_error, dead_lettered_at
           FROM outbound_tasks
          WHERE tenant = $1 AND dead_lettered_at IS NOT NULL
          ORDER BY dead_lettered_at DESC
          LIMIT $2",
    )
    .bind(tenant)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// A task the worker gave up on.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct DeadTask {
    pub id: Uuid,
    pub kind: String,
    pub komp_id: Option<Uuid>,
    pub payload: serde_json::Value,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub dead_lettered_at: Option<time::OffsetDateTime>,
}

/// Put a dead-lettered task back in the queue after the operator fixed the
/// cause. `false` when no such dead task exists.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn retry_dead_lettered(pool: &PgPool, tenant: &str, id: Uuid) -> Result<bool> {
    let n = sqlx::query(
        "UPDATE outbound_tasks
            SET dead_lettered_at = NULL, attempts = 0,
                next_attempt_at = now(), last_error = NULL
          WHERE id = $1 AND tenant = $2 AND dead_lettered_at IS NOT NULL",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await?
    .rows_affected();
    Ok(n > 0)
}

// ── Worker ────────────────────────────────────────────────────────────────────

/// Drains [`outbound_tasks`](self) — one at a time, oldest due first.
pub struct OutboundWorker {
    pool: PgPool,
    cfg: Arc<VertragdConfig>,
    http: reqwest::Client,
}

impl OutboundWorker {
    #[must_use]
    pub fn new(pool: PgPool, cfg: Arc<VertragdConfig>, http: reqwest::Client) -> Self {
        Self { pool, cfg, http }
    }

    /// Run until cancelled.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut idle = tokio::time::interval(std::time::Duration::from_secs(5));
        idle.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => {
                    tracing::info!("vertragd: outbound worker stopping");
                    return;
                }
                _ = idle.tick() => {}
            }
            // Drain the backlog in one wake-up rather than one task per tick;
            // a burst of contract creations otherwise took minutes to register.
            for _ in 0..64 {
                match self.step().await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        tracing::error!(error = %e, "vertragd: outbound worker step failed");
                        break;
                    }
                }
            }
        }
    }

    /// Claim and perform one task. `false` when the queue is empty.
    async fn step(&self) -> Result<bool> {
        // Claim under FOR UPDATE SKIP LOCKED so several replicas can drain the
        // same queue without performing a task twice.
        let mut tx = self.pool.begin().await?;
        let Some(row) = sqlx::query(
            "SELECT id, tenant, kind, komp_id, payload, attempts
               FROM outbound_tasks
              WHERE completed_at IS NULL AND dead_lettered_at IS NULL
                AND next_attempt_at <= now()
              ORDER BY next_attempt_at
              LIMIT 1
              FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await?
        else {
            tx.commit().await?;
            return Ok(false);
        };

        let id: Uuid = row.try_get("id")?;
        let kind_db: String = row.try_get("kind")?;
        let komp_id: Option<Uuid> = row.try_get("komp_id")?;
        let payload: serde_json::Value = row.try_get("payload")?;
        let attempts: i32 = row.try_get("attempts")?;
        let Some(kind) = TaskKind::from_db(&kind_db) else {
            // An unknown kind can only come from a downgrade; parking it is
            // better than retrying it for ever.
            sqlx::query(
                "UPDATE outbound_tasks
                    SET dead_lettered_at = now(), last_error = 'unknown task kind'
                  WHERE id = $1",
            )
            .bind(id)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            return Ok(true);
        };

        match self.perform(kind, komp_id, &payload).await {
            Ok(()) => {
                sqlx::query(
                    "UPDATE outbound_tasks
                        SET completed_at = now(), attempts = attempts + 1, last_error = NULL
                      WHERE id = $1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
            Err(e) => {
                let attempts = attempts + 1;
                let detail = format!("{e:#}");
                if attempts >= MAX_ATTEMPTS {
                    tracing::error!(
                        task = %kind_db, %id, ?komp_id, attempts, error = %detail,
                        "vertragd: outbound task dead-lettered — the obligation is now the operator's"
                    );
                } else {
                    tracing::warn!(
                        task = %kind_db, %id, attempts, retry_in_s = backoff_secs(attempts),
                        error = %detail, "vertragd: outbound task failed — retrying"
                    );
                }
                record_failure(&mut tx, id, attempts, &detail).await?;
            }
        }
        tx.commit().await?;
        Ok(true)
    }
}

/// Book a failed attempt: schedule the next one, or give up.
///
/// Separate from the worker so the SQL — an interval computed from a bind
/// parameter, and the boundary at which a task stops being retried — is
/// exercised by a test rather than only in production.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn record_failure(
    conn: &mut sqlx::PgConnection,
    id: Uuid,
    attempts: i32,
    detail: &str,
) -> Result<()> {
    if attempts >= MAX_ATTEMPTS {
        sqlx::query(
            "UPDATE outbound_tasks
                SET dead_lettered_at = now(), attempts = $2, last_error = $3
              WHERE id = $1",
        )
        .bind(id)
        .bind(attempts)
        .bind(detail)
        .execute(&mut *conn)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "UPDATE outbound_tasks
            SET attempts = $2, last_error = $3,
                next_attempt_at = now() + make_interval(secs => $4)
          WHERE id = $1",
    )
    .bind(id)
    .bind(attempts)
    .bind(detail)
    .bind(f64::from(
        u32::try_from(backoff_secs(attempts)).unwrap_or(u32::MAX),
    ))
    .execute(&mut *conn)
    .await?;
    Ok(())
}

impl OutboundWorker {
    /// Perform one task's side effect and its follow-up write.
    async fn perform(
        &self,
        kind: TaskKind,
        komp_id: Option<Uuid>,
        payload: &serde_json::Value,
    ) -> Result<()> {
        match kind {
            TaskKind::Lieferbeginn => {
                let endpoint = payload
                    .get("endpoint")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("start-supply");
                let body = self
                    .post_json(
                        &self.cfg.processd_url,
                        endpoint,
                        self.cfg.processd_api_key.as_deref(),
                        payload,
                    )
                    .await?;
                let process_id = body
                    .get("process_id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned);
                if let Some(komp_id) = komp_id {
                    crate::pg::update_komponente_status(
                        &self.pool,
                        komp_id,
                        "ANGEMELDET",
                        process_id.as_deref(),
                        None,
                        None,
                        None,
                    )
                    .await?;
                }
                Ok(())
            }
            TaskKind::Lieferende => {
                let endpoint = payload
                    .get("endpoint")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("end-supply");
                self.post_json(
                    &self.cfg.processd_url,
                    endpoint,
                    self.cfg.processd_api_key.as_deref(),
                    payload,
                )
                .await?;
                Ok(())
            }
            TaskKind::AblesungBeginn | TaskKind::AblesungEnde => {
                let body = self
                    .post_json(
                        &self.cfg.edmd_url,
                        "reading-orders",
                        self.cfg.edmd_api_key.as_deref(),
                        payload,
                    )
                    .await?;
                // Keep the trail from a Schlussrechnung back to the reading it
                // was built on. edmd's POST is not idempotent, so the id is
                // recorded here and the dedupe key stops a second order.
                if let (Some(komp_id), Some(order_id)) = (
                    komp_id,
                    body.get("id")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|s| s.parse::<Uuid>().ok()),
                ) {
                    sqlx::query(
                        "UPDATE vertragskomponenten
                            SET ablese_auftrag_id = $2, updated_at = now()
                          WHERE id = $1 AND ablese_auftrag_id IS NULL",
                    )
                    .bind(komp_id)
                    .bind(order_id)
                    .execute(&self.pool)
                    .await?;
                }
                Ok(())
            }
            TaskKind::Abrechnungskonto => {
                self.post_json(
                    &self.cfg.accountingd_url,
                    "accounts",
                    self.cfg.accountingd_api_key.as_deref(),
                    payload,
                )
                .await?;
                Ok(())
            }
        }
    }

    async fn post_json(
        &self,
        base: &str,
        path: &str,
        key: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.send(self.http.post(url(base, path)), key, body).await
    }

    async fn send(
        &self,
        req: reqwest::RequestBuilder,
        key: Option<&str>,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let req = match key {
            Some(k) => req.bearer_auth(k),
            None => req,
        };
        let resp = req.json(body).send().await.context("request failed")?;
        let status = resp.status();
        if !status.is_success() {
            let detail = resp.text().await.unwrap_or_default();
            anyhow::bail!("upstream returned {status}: {}", detail.trim());
        }
        Ok(resp.json().await.unwrap_or(serde_json::Value::Null))
    }
}

fn url(base: &str, path: &str) -> String {
    format!("{}/api/v1/{path}", base.trim_end_matches('/'))
}

/// Exponential backoff, capped so a long outage does not push the next attempt
/// past the point where the obligation still matters.
fn backoff_secs(attempts: i32) -> u64 {
    let exp = u32::try_from(attempts.max(1) - 1).unwrap_or(0).min(16);
    BACKOFF_BASE_SECS
        .saturating_mul(1u64 << exp)
        .min(BACKOFF_CAP_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::date;

    #[test]
    fn strom_carries_lieferbeginn_datum() {
        let b = lieferbeginn_body(
            "STROM",
            "51238696012",
            None,
            "9900000000001",
            "9900357000004",
            date!(2026 - 04 - 01),
        )
        .unwrap();
        assert_eq!(b["endpoint"], "start-supply");
        assert_eq!(b["lieferbeginn_datum"], "2026-04-01");
        assert!(b.get("zaehlpunkt").is_none());
    }

    #[test]
    fn gas_carries_the_messlokation_as_zaehlpunkt_and_a_compact_date() {
        let b = lieferbeginn_body(
            "GAS",
            "51238696012",
            Some("DE0001112223334445556667778889"),
            "9900000000001",
            "9900357000004",
            date!(2026 - 04 - 01),
        )
        .unwrap();
        assert_eq!(b["endpoint"], "start-supply-gas");
        assert_eq!(b["zaehlpunkt"], "DE0001112223334445556667778889");
        assert_eq!(b["process_date"], "20260401");
    }

    #[test]
    fn gas_without_a_messlokation_is_refused_rather_than_sent_with_the_malo() {
        // An 11-digit MaLo is not a 33-character Zählpunktbezeichnung; sending
        // it produced a malformed UTILMD the NB rejected days later.
        let err = lieferbeginn_body(
            "GAS",
            "51238696012",
            None,
            "9900000000001",
            "9900357000004",
            date!(2026 - 04 - 01),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Zählpunktbezeichnung"));
    }

    #[test]
    fn an_empty_melo_counts_as_absent() {
        assert!(
            lieferbeginn_body(
                "GAS",
                "51238696012",
                Some(""),
                "9900000000001",
                "9900357000004",
                date!(2026 - 04 - 01)
            )
            .is_err()
        );
    }

    #[test]
    fn a_one_shot_registration_keeps_a_single_dedupe_key() {
        // A replay of the same contract must not enqueue a second UTILMD, so
        // the key does not vary by anything a replay could change.
        let komp = Uuid::nil();
        let x = lieferende(
            komp,
            "STROM",
            "51238696012",
            "NB",
            "LF",
            date!(2026 - 12 - 31),
        );
        let y = lieferende(
            komp,
            "STROM",
            "51238696012",
            "NB",
            "LF",
            date!(2027 - 01 - 31),
        );
        assert_eq!(x.dedupe_key, y.dedupe_key);
    }

    #[test]
    fn the_backoff_grows_and_then_stops_growing() {
        assert_eq!(backoff_secs(1), 30);
        assert_eq!(backoff_secs(2), 60);
        assert_eq!(backoff_secs(3), 120);
        assert_eq!(backoff_secs(30), BACKOFF_CAP_SECS);
    }
}
