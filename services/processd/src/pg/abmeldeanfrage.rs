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
/// Spelled out because both queries select the same seven columns and a tuple
/// of that width is unreadable at the call site.
type PendingRow = (
    String,
    String,
    Vec<String>,
    i32,
    serde_json::Value,
    serde_json::Value,
    OffsetDateTime,
);

/// The same seven columns with the primary key in front, for the listing query.
type KeyedPendingRow = (
    Uuid,
    String,
    String,
    Vec<String>,
    i32,
    serde_json::Value,
    serde_json::Value,
    OffsetDateTime,
);

/// What recording a waiting Anmeldung found.
///
/// The row is written **before** the 55010 goes out — an LFA answering within
/// milliseconds must find something to resume — so the two failure orders are
/// not symmetric, and a redelivery has to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// Nothing was waiting; the Anfrage has to be sent.
    Recorded,
    /// A row was already there and its Anfrage never reached `makod`. Nothing
    /// registered the LFA's 09:00 window, so nothing will ever resolve this
    /// Anmeldung: the redelivery has to send the Anfrage after all.
    Unsent,
    /// A row was already there and its Anfrage is out (or the decision has
    /// since been resolved). The redelivery is a duplicate and ends here — a
    /// second 55010 would ask the LFA twice.
    AlreadySent,
}

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
    /// The serialised Meldepflicht context — what phase two needs to tell the
    /// LFA its Zuordnung ends (55037 / 44037) once the Anmeldung is confirmed.
    /// Not derivable from `anfrage`, which names no incumbent.
    pub meldung: serde_json::Value,
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

    /// Record that an Anmeldung is waiting on the LFA, and say what was found.
    ///
    /// `ON CONFLICT DO NOTHING` on the primary key: the event fan-out is
    /// at-least-once, and a redelivered Anmeldung must not ask the LFA twice.
    /// But „a row exists" is not the same as „the Anfrage went out": the row is
    /// written first, deliberately, so an answer arriving in milliseconds finds
    /// something to resume. A dispatch that then fails leaves a row nothing will
    /// ever resolve — no 55010, so no 09:00 window, so no lapse — and the
    /// Anmeldung silently misses its own 11:00 Frist.
    ///
    /// [`Waiting`] separates the two, and only [`Waiting::AlreadySent`] ends the
    /// handling. Stamp a successful dispatch with
    /// [`mark_anfrage_sent`](Self::mark_anfrage_sent).
    ///
    /// # Errors
    ///
    /// Propagates any `sqlx` failure.
    pub async fn record(&self, rec: &AbmeldeanfrageRecord) -> Result<Waiting, sqlx::Error> {
        let res = sqlx::query(
            "INSERT INTO abmeldeanfragen \
             (anmeldung_process_id, tenant, malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, meldung, received_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
             ON CONFLICT (anmeldung_process_id, tenant) DO NOTHING",
        )
        .bind(rec.anmeldung_process_id)
        .bind(&rec.tenant)
        .bind(&rec.malo_id)
        .bind(&rec.lfn_mp_id)
        .bind(&rec.lfa_mp_ids)
        .bind(rec.pid)
        .bind(&rec.anfrage)
        .bind(&rec.meldung)
        .bind(rec.received_at)
        .execute(&self.pool)
        .await?;
        if res.rows_affected() > 0 {
            return Ok(Waiting::Recorded);
        }
        // A row was already there. Whether the redelivery has work to do turns
        // on whether its Anfrage ever reached `makod`. A row that has since been
        // resolved answers `None` and is `AlreadySent`: the decision is made.
        let sent: Option<(Option<OffsetDateTime>,)> = sqlx::query_as(
            "SELECT anfrage_gesendet_at FROM abmeldeanfragen \
                 WHERE anmeldung_process_id = $1 AND tenant = $2 AND resolved_at IS NULL",
        )
        .bind(rec.anmeldung_process_id)
        .bind(&rec.tenant)
        .fetch_optional(&self.pool)
        .await?;
        Ok(match sent {
            Some((None,)) => Waiting::Unsent,
            _ => Waiting::AlreadySent,
        })
    }

    /// Stamp the moment the 55010 reached `makod`.
    ///
    /// Called once every Anfrage of the Vorgang has been accepted — with several
    /// LFA at Geschäftsvorfall 3, a partial dispatch is an unsent Anfrage, and
    /// re-sending the accepted ones replays their idempotency key rather than
    /// asking twice.
    ///
    /// # Errors
    ///
    /// Propagates any `sqlx` failure.
    pub async fn mark_anfrage_sent(
        &self,
        anmeldung_process_id: Uuid,
        tenant: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE abmeldeanfragen SET anfrage_gesendet_at = now() \
                 WHERE anmeldung_process_id = $1 AND tenant = $2 \
                   AND anfrage_gesendet_at IS NULL",
        )
        .bind(anmeldung_process_id)
        .bind(tenant)
        .execute(&self.pool)
        .await?;
        Ok(())
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
                 RETURNING malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, meldung, received_at",
        )
        .bind(anmeldung_process_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(
            |(malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, meldung, received_at)| {
                AbmeldeanfrageRecord {
                    anmeldung_process_id,
                    malo_id,
                    lfn_mp_id,
                    lfa_mp_ids,
                    pid,
                    anfrage,
                    meldung,
                    received_at,
                    tenant: tenant.to_owned(),
                }
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
                 meldung, received_at FROM abmeldeanfragen \
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
                |(id, malo_id, lfn_mp_id, lfa_mp_ids, pid, anfrage, meldung, received_at)| {
                    AbmeldeanfrageRecord {
                        anmeldung_process_id: id,
                        malo_id,
                        lfn_mp_id,
                        lfa_mp_ids,
                        pid,
                        anfrage,
                        meldung,
                        received_at,
                        tenant: tenant.to_owned(),
                    }
                },
            )
            .collect())
    }
}
