//! The NB Anmeldung decisions waiting on an LFA's answer to a 55010.
//!
//! GPKE Teil 2 § 2.1.2 SD Lieferbeginn Nr. 1 **Prüfschritt 4** makes the NB's
//! answer two-phase whenever the Marktlokation is already assigned at the
//! Zuordnungsbeginn: ask the incumbent LFA to release it (Nr. 3), then decide
//! (Nr. 5/6) once the LFA answers or its 09:00 window lapses. `E_0623`
//! Prüfschritte 20–50 read that answer, so phase one cannot produce a
//! Bestätigung and phase two needs everything phase one knew.
//!
//! What is stored is the **replayable `AnmeldungAnfrage`**, not a set of derived
//! columns: phase two runs the same pure evaluation with one more fact, and a
//! second copy of the Anwendungsfall in SQL is a copy that drifts.

use time::OffsetDateTime;
use uuid::Uuid;

/// One `abmeldeanfragen` row as the driver returns it, before it is named.
///
/// Spelled out because both queries select the same six columns and a tuple of
/// that width is unreadable at the call site.
type PendingRow = (
    String,
    String,
    Vec<String>,
    i32,
    serde_json::Value,
    OffsetDateTime,
);

/// The same six columns with the primary key in front, for the listing query.
type KeyedPendingRow = (
    Uuid,
    String,
    String,
    Vec<String>,
    i32,
    serde_json::Value,
    OffsetDateTime,
);

/// A waiting Anmeldung and the Anfrage that is holding it.
#[derive(Debug, Clone)]
pub struct AbmeldeanfrageRecord {
    /// The Anmeldung's `process_id` — the key the answer names.
    pub anmeldung_process_id: Uuid,
    /// Marktlokations-ID, or the MaLo-ID of the Tranche.
    pub malo_id: String,
    /// The LFN whose Anmeldung is waiting.
    pub lfn_mp_id: String,
    /// Every LFA the Anfrage went to.
    pub lfa_mp_ids: Vec<String>,
    /// The inbound Anmeldung PID (55001 / 55077 / 44001).
    pub pid: i32,
    /// The serialised `mako_pruefung::AnmeldungAnfrage`.
    pub anfrage: serde_json::Value,
    /// When the Anmeldung arrived — the anchor of its **own** 11:00 window,
    /// which is not the Anfrage's 09:00 one.
    pub received_at: OffsetDateTime,
    /// Tenant.
    pub tenant: String,
}

/// PostgreSQL-backed store for waiting Anmeldung decisions.
#[derive(Clone)]
pub struct PgAbmeldeanfrageRepository {
    pool: sqlx::PgPool,
}

impl PgAbmeldeanfrageRepository {
    /// Wrap a connection pool.
    #[must_use]
    pub const fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Record that an Anmeldung is waiting on the LFA.
    ///
    /// `ON CONFLICT DO NOTHING` on the primary key: the event fan-out is
    /// at-least-once, and a redelivered Anmeldung must not restart a decision
    /// that is already waiting — the Anfrage has gone out and its window is
    /// running.
    ///
    /// Returns `true` when a row was written.
    ///
    /// # Errors
    ///
    /// Propagates any `sqlx` failure.
    pub async fn insert(&self, rec: &AbmeldeanfrageRecord) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO abmeldeanfragen \
             (anmeldung_process_id, tenant, malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, received_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (anmeldung_process_id, tenant) DO NOTHING",
        )
        .bind(rec.anmeldung_process_id)
        .bind(&rec.tenant)
        .bind(&rec.malo_id)
        .bind(&rec.lfn_mp_id)
        .bind(&rec.lfa_mp_ids)
        .bind(rec.pid)
        .bind(&rec.anfrage)
        .bind(rec.received_at)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected() > 0)
    }

    /// Take the waiting decision for `anmeldung_process_id`, marking it resolved.
    ///
    /// One statement, not a read followed by an update: the LFA's answer and the
    /// 09:00 lapse race by design, and both resume the same decision.
    /// `WHERE resolved_at IS NULL` makes the loser of that race see `None` and
    /// do nothing, so the Anmeldung is answered exactly once.
    ///
    /// # Errors
    ///
    /// Propagates any `sqlx` failure.
    pub async fn take(
        &self,
        anmeldung_process_id: Uuid,
        tenant: &str,
    ) -> Result<Option<AbmeldeanfrageRecord>, sqlx::Error> {
        let row: Option<PendingRow> = sqlx::query_as(
            "UPDATE abmeldeanfragen SET resolved_at = now() \
                 WHERE anmeldung_process_id = $1 AND tenant = $2 AND resolved_at IS NULL \
                 RETURNING malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, received_at",
        )
        .bind(anmeldung_process_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, received_at)| AbmeldeanfrageRecord {
                anmeldung_process_id,
                malo_id,
                lfn_mp_id,
                lfa_mp_ids,
                pid,
                anfrage,
                received_at,
                tenant: tenant.to_owned(),
            },
        ))
    }

    /// Every Anmeldung still waiting on an LFA, newest first.
    ///
    /// The operator view: a row here past its 11:00 window is an Anmeldung the
    /// NB has not answered, and the reason is a counterparty that has neither
    /// answered nor timed out — which the scheduler should have resolved.
    ///
    /// # Errors
    ///
    /// Propagates any `sqlx` failure.
    pub async fn list_pending(
        &self,
        tenant: &str,
        limit: i64,
    ) -> Result<Vec<AbmeldeanfrageRecord>, sqlx::Error> {
        let rows: Vec<KeyedPendingRow> = sqlx::query_as(
            "SELECT anmeldung_process_id, malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, \
                 received_at FROM abmeldeanfragen \
                 WHERE tenant = $1 AND resolved_at IS NULL \
                 ORDER BY received_at DESC LIMIT $2",
        )
        .bind(tenant)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, received_at)| {
                    AbmeldeanfrageRecord {
                        anmeldung_process_id: id,
                        malo_id,
                        lfn_mp_id,
                        lfa_mp_ids,
                        pid,
                        anfrage,
                        received_at,
                        tenant: tenant.to_owned(),
                    }
                },
            )
            .collect())
    }
}
