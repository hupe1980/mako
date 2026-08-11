//! PostgreSQL implementation of [`VersorgungsStatusRepository`].

use mako_markt::{
    domain::MaloId,
    error::MdmError,
    repository::{
        LieferStatus, PageResult, VersorgungsStatusHistoryRecord, VersorgungsStatusRecord,
        VersorgungsStatusRepository,
    },
};
use sqlx::{PgConnection, PgPool, Row, postgres::PgRow};
use std::str::FromStr as _;
use time::Date;

/// PostgreSQL-backed VersorgungsStatus repository.
///
/// One row per `(malo_id, tenant)`.  All writes use optimistic concurrency —
/// `upsert` with `if_version = Some(v)` issues `WHERE version = v` and returns
/// `MdmError::Conflict` on 0-row update.
///
/// Every successful `upsert` atomically appends a row to
/// `versorgungsstatus_history`, enabling `find_at` point-in-time queries.
///
/// Each mutation is also available as an inherent `*_tx` function taking a
/// `&mut PgConnection`, so callers (event ingest, PUT handlers) can commit the
/// state change atomically with their idempotency marker and outbox enqueue.
#[derive(Clone, Debug)]
pub struct PgVersorgungsStatusRepository {
    pool: PgPool,
}

impl PgVersorgungsStatusRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn internal(e: impl std::fmt::Display) -> MdmError {
    MdmError::Internal(e.to_string())
}

fn map_row(row: &PgRow) -> Result<VersorgungsStatusRecord, sqlx::Error> {
    let status_str: String = row.try_get("lieferstatus")?;
    let lieferstatus =
        LieferStatus::from_str(&status_str).map_err(|e| sqlx::Error::ColumnDecode {
            index: "lieferstatus".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    let malo_id_str: String = row.try_get("malo_id")?;
    let malo_id = malo_id_str
        .parse::<mako_markt::domain::MaloId>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "malo_id".into(),
            source: Box::new(std::io::Error::other(e.to_string())),
        })?;
    Ok(VersorgungsStatusRecord {
        malo_id,
        lieferstatus,
        lf_mp_id: row.try_get("lf_mp_id")?,
        lf_mp_id_next: row.try_get("lf_mp_id_next")?,
        lf_next_lieferbeginn: row.try_get("lf_next_lieferbeginn")?,
        lieferbeginn: row.try_get("lieferbeginn")?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        eog_seit: row.try_get("eog_seit")?,
        last_process_id: row.try_get("last_process_id")?,
        updated_at: row.try_get("updated_at")?,
        tenant: row.try_get("tenant")?,
        version: row.try_get("version")?,
    })
}

fn map_history_row(row: &PgRow) -> Result<VersorgungsStatusHistoryRecord, sqlx::Error> {
    let status_str: String = row.try_get("lieferstatus")?;
    let lieferstatus =
        LieferStatus::from_str(&status_str).map_err(|e| sqlx::Error::ColumnDecode {
            index: "lieferstatus".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    let malo_id_str: String = row.try_get("malo_id")?;
    let malo_id = malo_id_str
        .parse::<mako_markt::domain::MaloId>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "malo_id".into(),
            source: Box::new(std::io::Error::other(e.to_string())),
        })?;
    Ok(VersorgungsStatusHistoryRecord {
        id: row.try_get("id")?,
        malo_id,
        tenant: row.try_get("tenant")?,
        lieferstatus,
        lf_mp_id: row.try_get("lf_mp_id")?,
        lf_mp_id_next: row.try_get("lf_mp_id_next")?,
        lf_next_lieferbeginn: row.try_get("lf_next_lieferbeginn")?,
        lieferbeginn: row.try_get("lieferbeginn")?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        last_process_id: row.try_get("last_process_id")?,
        version: row.try_get("version")?,
        valid_from: row.try_get("valid_from")?,
    })
}

/// Reconstruct a [`VersorgungsStatusRecord`] from a history row, including the
/// snapshotted `eog_seit`.  `updated_at` is set to `valid_from` (the instant
/// the snapshot was recorded).
fn map_history_row_as_current(row: &PgRow) -> Result<VersorgungsStatusRecord, sqlx::Error> {
    let h = map_history_row(row)?;
    Ok(VersorgungsStatusRecord {
        malo_id: h.malo_id,
        tenant: h.tenant,
        lieferstatus: h.lieferstatus,
        lf_mp_id: h.lf_mp_id,
        lf_mp_id_next: h.lf_mp_id_next,
        lf_next_lieferbeginn: h.lf_next_lieferbeginn,
        lieferbeginn: h.lieferbeginn,
        lieferende: h.lieferende,
        msb_mp_id: h.msb_mp_id,
        nb_mp_id: h.nb_mp_id,
        eog_seit: row.try_get("eog_seit")?,
        last_process_id: h.last_process_id,
        updated_at: h.valid_from,
        version: h.version,
    })
}

/// Snapshot the current `versorgungsstatus` row into the history table.
async fn append_history_snapshot(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
) -> Result<(), MdmError> {
    sqlx::query(
        r#"INSERT INTO versorgungsstatus_history
           (malo_id, tenant, lieferstatus, lf_mp_id, lf_mp_id_next,
            lf_next_lieferbeginn, lieferbeginn, lieferende,
            msb_mp_id, nb_mp_id, eog_seit, last_process_id, version, valid_from)
           SELECT malo_id, tenant, lieferstatus, lf_mp_id, lf_mp_id_next,
                  lf_next_lieferbeginn, lieferbeginn, lieferende,
                  msb_mp_id, nb_mp_id, eog_seit, last_process_id, version, now()
           FROM versorgungsstatus
           WHERE malo_id = $1 AND tenant = $2"#,
    )
    .bind(malo_id)
    .bind(tenant)
    .execute(conn)
    .await
    .map_err(internal)?;
    Ok(())
}

// ── Transactional building blocks ─────────────────────────────────────────────
//
// Each function performs one supply-state transition on a caller-provided
// connection/transaction and returns the error instead of swallowing it, so
// the caller can roll back its idempotency marker and force a redelivery.
impl PgVersorgungsStatusRepository {
    /// Full-row upsert.  Returns the actual new row version (`RETURNING
    /// version`), which the caller must use for the ETag and emitted events.
    pub async fn upsert_tx(
        conn: &mut PgConnection,
        rec: &VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let new_version: Option<i64> = if let Some(expected) = if_version {
            sqlx::query_scalar(
                r#"UPDATE versorgungsstatus
                   SET lieferstatus         = $4,
                       lf_mp_id             = $5,
                       lf_mp_id_next        = $6,
                       lf_next_lieferbeginn = $7,
                       lieferbeginn         = $8,
                       lieferende           = $9,
                       msb_mp_id            = $10,
                       nb_mp_id             = $11,
                       last_process_id      = $12,
                       eog_seit             = $13,
                       updated_at           = now(),
                       version              = version + 1
                   WHERE malo_id = $1 AND tenant = $2 AND version = $3
                   RETURNING version"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(expected)
            .bind(rec.lieferstatus.to_string())
            .bind(&rec.lf_mp_id)
            .bind(&rec.lf_mp_id_next)
            .bind(rec.lf_next_lieferbeginn)
            .bind(rec.lieferbeginn)
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .bind(rec.eog_seit)
            .fetch_optional(&mut *conn)
            .await
            .map_err(internal)?
        } else {
            sqlx::query_scalar(
                r#"INSERT INTO versorgungsstatus
                   (malo_id, tenant, lieferstatus, lf_mp_id, lf_mp_id_next,
                    lf_next_lieferbeginn, lieferbeginn, lieferende,
                    msb_mp_id, nb_mp_id, last_process_id, eog_seit, updated_at, version)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), 1)
                   ON CONFLICT (malo_id, tenant) DO UPDATE
                   SET lieferstatus    = EXCLUDED.lieferstatus,
                       lf_mp_id         = EXCLUDED.lf_mp_id,
                       lf_mp_id_next    = EXCLUDED.lf_mp_id_next,
                       lf_next_lieferbeginn = EXCLUDED.lf_next_lieferbeginn,
                       lieferbeginn   = EXCLUDED.lieferbeginn,
                       lieferende     = EXCLUDED.lieferende,
                       msb_mp_id        = EXCLUDED.msb_mp_id,
                       nb_mp_id         = EXCLUDED.nb_mp_id,
                       last_process_id = EXCLUDED.last_process_id,
                       eog_seit       = EXCLUDED.eog_seit,
                       updated_at     = now(),
                       version        = versorgungsstatus.version + 1
                   RETURNING version"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(rec.lieferstatus.to_string())
            .bind(&rec.lf_mp_id)
            .bind(&rec.lf_mp_id_next)
            .bind(rec.lf_next_lieferbeginn)
            .bind(rec.lieferbeginn)
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .bind(rec.eog_seit)
            .fetch_optional(&mut *conn)
            .await
            .map_err(internal)?
        };

        let Some(new_version) = new_version else {
            return Err(MdmError::VersionConflict {
                expected: if_version.map_or_else(|| "new".into(), |v| v.to_string()),
                actual: "(concurrent update)".into(),
            });
        };

        // Append history snapshot with the ACTUAL row version.
        sqlx::query(
            r#"INSERT INTO versorgungsstatus_history
               (malo_id, tenant, lieferstatus, lf_mp_id, lf_mp_id_next,
                lf_next_lieferbeginn, lieferbeginn, lieferende,
                msb_mp_id, nb_mp_id, eog_seit, last_process_id, version, valid_from)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, now())"#,
        )
        .bind(&rec.malo_id)
        .bind(&rec.tenant)
        .bind(rec.lieferstatus.to_string())
        .bind(&rec.lf_mp_id)
        .bind(&rec.lf_mp_id_next)
        .bind(rec.lf_next_lieferbeginn)
        .bind(rec.lieferbeginn)
        .bind(rec.lieferende)
        .bind(&rec.msb_mp_id)
        .bind(&rec.nb_mp_id)
        .bind(rec.eog_seit)
        .bind(rec.last_process_id)
        .bind(new_version)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        Ok(new_version)
    }

    /// NB received Lieferbeginn Anfrage — record the pending transition.
    pub async fn announce_lf_next_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<Date>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        // Partial upsert: insert as Unbeliefert if new, otherwise only update
        // the announcement fields — never overwrite lieferstatus / lf_mp_id.
        sqlx::query(
            r#"INSERT INTO versorgungsstatus
               (malo_id, tenant, lieferstatus, nb_mp_id,
                lf_mp_id_next, lf_next_lieferbeginn, last_process_id, updated_at, version)
               VALUES ($1, $2, 'Unbeliefert', $3, $4, $5, $6, now(), 1)
               ON CONFLICT (malo_id, tenant) DO UPDATE
               SET lf_mp_id_next          = EXCLUDED.lf_mp_id_next,
                   lf_next_lieferbeginn = EXCLUDED.lf_next_lieferbeginn,
                   last_process_id      = EXCLUDED.last_process_id,
                   updated_at           = now(),
                   version              = versorgungsstatus.version + 1"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(nb_mp_id)
        .bind(lf_mp_id_next)
        .bind(lf_next_lieferbeginn)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        append_history_snapshot(conn, malo_id, tenant).await
    }

    /// Atomic SQL promotion: `lf_mp_id_next` → `lf_mp_id`.  No-op (no version
    /// bump, no history row) if no announcement is pending (idempotent
    /// re-delivery).
    pub async fn confirm_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let updated = sqlx::query(
            r#"UPDATE versorgungsstatus
               SET lieferstatus         = 'Beliefert',
                   lf_mp_id             = lf_mp_id_next,
                   lieferbeginn         = lf_next_lieferbeginn,
                   lf_mp_id_next          = NULL,
                   lf_next_lieferbeginn = NULL,
                   eog_seit             = NULL,
                   last_process_id      = $3,
                   updated_at           = now(),
                   version              = version + 1
               WHERE malo_id = $1 AND tenant = $2 AND lf_mp_id_next IS NOT NULL"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        if updated.rows_affected() > 0 {
            append_history_snapshot(conn, malo_id, tenant).await?;
        }
        Ok(())
    }

    /// Clear active LF fields; preserve `lf_mp_id_next` / `lf_next_lieferbeginn`
    /// so a pending future Lieferant announcement is not lost.  `lieferende`
    /// defaults to today (Berlin civil date) when the process carries no date.
    pub async fn end_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        nb_mp_id: &str,
        lieferende: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let lieferende = lieferende.unwrap_or_else(crate::handlers::malo::today_berlin);
        sqlx::query(
            r#"INSERT INTO versorgungsstatus
               (malo_id, tenant, lieferstatus, nb_mp_id, lieferende, last_process_id, updated_at, version)
               VALUES ($1, $2, 'Unbeliefert', $3, $4, $5, now(), 1)
               ON CONFLICT (malo_id, tenant) DO UPDATE
               SET lieferstatus    = 'Unbeliefert',
                   lf_mp_id        = NULL,
                   lieferbeginn    = NULL,
                   lieferende      = EXCLUDED.lieferende,
                   eog_seit        = NULL,
                   nb_mp_id        = $3,
                   last_process_id = $5,
                   updated_at      = now(),
                   version         = versorgungsstatus.version + 1"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(nb_mp_id)
        .bind(lieferende)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        append_history_snapshot(conn, malo_id, tenant).await
    }

    /// The E/G becomes the supplier of record; preserve a pending regular
    /// switch (`lf_mp_id_next` / `lf_next_lieferbeginn`) — its confirmation
    /// ends the fallback supply.
    #[allow(clippy::too_many_arguments)]
    pub async fn begin_eog_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        if !matches!(
            eog_status,
            LieferStatus::Ersatzversorgung | LieferStatus::Grundversorgung
        ) {
            return Err(MdmError::Unprocessable {
                reason: "begin_eog_supply requires Ersatzversorgung or Grundversorgung".into(),
            });
        }
        // The CHECK constraint requires eog_seit while the fallback runs;
        // default a missing start date to today in German local time — the
        // §38 Abs. 2 clock runs on the Berlin civil calendar.
        let eog_seit = eog_seit.unwrap_or_else(crate::handlers::malo::today_berlin);

        sqlx::query(
            r#"INSERT INTO versorgungsstatus
               (malo_id, tenant, lieferstatus, lf_mp_id, lieferbeginn,
                nb_mp_id, eog_seit, last_process_id, updated_at, version)
               VALUES ($1, $2, $3, $4, $5, $6, $5, $7, now(), 1)
               ON CONFLICT (malo_id, tenant) DO UPDATE
               SET lieferstatus    = EXCLUDED.lieferstatus,
                   lf_mp_id        = EXCLUDED.lf_mp_id,
                   lieferbeginn    = EXCLUDED.lieferbeginn,
                   nb_mp_id        = EXCLUDED.nb_mp_id,
                   eog_seit        = EXCLUDED.eog_seit,
                   last_process_id = EXCLUDED.last_process_id,
                   updated_at      = now(),
                   version         = versorgungsstatus.version + 1"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(eog_status.to_string())
        .bind(gv_mp_id)
        .bind(eog_seit)
        .bind(nb_mp_id)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        append_history_snapshot(conn, malo_id, tenant).await
    }

    /// Drop the announced future Lieferant.  Only touches rows that actually
    /// carry a pending announcement, so a duplicate cancellation is a genuine
    /// no-op (no version bump, no history row).
    pub async fn clear_lf_next_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let updated = sqlx::query(
            r#"UPDATE versorgungsstatus
               SET lf_mp_id_next       = NULL,
                   lf_next_lieferbeginn = NULL,
                   last_process_id      = $3,
                   updated_at           = now(),
                   version              = version + 1
               WHERE malo_id = $1 AND tenant = $2
                 AND lf_mp_id_next IS NOT NULL"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        if updated.rows_affected() > 0 {
            append_history_snapshot(conn, malo_id, tenant).await?;
        }
        Ok(())
    }
}

impl VersorgungsStatusRepository for PgVersorgungsStatusRepository {
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let new_version = Self::upsert_tx(&mut tx, &rec, if_version).await?;
        tx.commit().await.map_err(internal)?;
        Ok(new_version)
    }

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        let opt = sqlx::query("SELECT * FROM versorgungsstatus WHERE malo_id = $1 AND tenant = $2")
            .bind(malo_id)
            .bind(tenant)
            .fetch_optional(&self.pool)
            .await
            .map_err(internal)?;

        opt.as_ref().map(map_row).transpose().map_err(internal)
    }

    async fn find_at(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        at: Date,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        // Find the most recent history entry whose `valid_from`, expressed in
        // German local time (CET/CEST via 'Europe/Berlin'), falls on or before `at`.
        let opt = sqlx::query(
            r#"SELECT *
               FROM versorgungsstatus_history
               WHERE malo_id = $1 AND tenant = $2
                 AND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= $3
               ORDER BY valid_from DESC
               LIMIT 1"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(at)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;

        opt.as_ref()
            .map(map_history_row_as_current)
            .transpose()
            .map_err(internal)
    }

    async fn find_history(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusHistoryRecord>, MdmError> {
        let size = size.min(500);
        let limit = i64::from(size);
        let offset = i64::from(page) * limit;

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM versorgungsstatus_history WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(malo_id)
        .bind(tenant)
        .fetch_one(&self.pool)
        .await
        .map_err(internal)?;

        let rows = sqlx::query(
            r#"SELECT *
               FROM versorgungsstatus_history
               WHERE malo_id = $1 AND tenant = $2
               ORDER BY valid_from DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;

        let items = rows
            .iter()
            .map(map_history_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        Ok(PageResult {
            items,
            total: total as u64,
            page,
            size,
        })
    }

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusRecord>, MdmError> {
        let size = size.min(500);
        let limit = i64::from(size);
        let offset = i64::from(page) * limit;
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM versorgungsstatus WHERE tenant = $1")
                .bind(tenant)
                .fetch_one(&self.pool)
                .await
                .map_err(internal)?;

        let rows = sqlx::query(
            "SELECT * FROM versorgungsstatus WHERE tenant = $1 ORDER BY malo_id LIMIT $2 OFFSET $3",
        )
        .bind(tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;

        let items = rows
            .iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        Ok(PageResult {
            items,
            total: total as u64,
            page,
            size,
        })
    }

    async fn announce_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<Date>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::announce_lf_next_tx(
            &mut tx,
            malo_id,
            tenant,
            lf_mp_id_next,
            lf_next_lieferbeginn,
            nb_mp_id,
            process_id,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    async fn confirm_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::confirm_supply_tx(&mut tx, malo_id, tenant, process_id).await?;
        tx.commit().await.map_err(internal)
    }

    async fn end_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::end_supply_tx(&mut tx, malo_id, tenant, nb_mp_id, None, process_id).await?;
        tx.commit().await.map_err(internal)
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin_eog_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::begin_eog_supply_tx(
            &mut tx, malo_id, tenant, gv_mp_id, nb_mp_id, eog_status, eog_seit, process_id,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    async fn clear_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::clear_lf_next_tx(&mut tx, malo_id, tenant, process_id).await?;
        tx.commit().await.map_err(internal)
    }
}
