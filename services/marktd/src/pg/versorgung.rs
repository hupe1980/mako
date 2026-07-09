//! PostgreSQL implementation of [`VersorgungsStatusRepository`].

use mako_markt::{
    domain::MaloId,
    error::MdmError,
    repository::{
        LieferStatus, PageResult, VersorgungsStatusHistoryRecord, VersorgungsStatusRecord,
        VersorgungsStatusRepository,
    },
};
use sqlx::{PgPool, Row, postgres::PgRow};
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
        lf_gln_next: row.try_get("lf_gln_next")?,
        lieferbeginn: row.try_get("lieferbeginn")?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
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
        lf_gln_next: row.try_get("lf_gln_next")?,
        lieferbeginn: row.try_get("lieferbeginn")?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        last_process_id: row.try_get("last_process_id")?,
        version: row.try_get("version")?,
        valid_from: row.try_get("valid_from")?,
    })
}

/// Reconstruct a [`VersorgungsStatusRecord`] from a history row.
///
/// `updated_at` is set to `valid_from` (the instant the snapshot was recorded).
fn history_to_current(h: VersorgungsStatusHistoryRecord) -> VersorgungsStatusRecord {
    VersorgungsStatusRecord {
        malo_id: h.malo_id,
        tenant: h.tenant,
        lieferstatus: h.lieferstatus,
        lf_mp_id: h.lf_mp_id,
        lf_gln_next: h.lf_gln_next,
        lieferbeginn: h.lieferbeginn,
        lieferende: h.lieferende,
        msb_mp_id: h.msb_mp_id,
        nb_mp_id: h.nb_mp_id,
        last_process_id: h.last_process_id,
        updated_at: h.valid_from,
        version: h.version,
    }
}

impl VersorgungsStatusRepository for PgVersorgungsStatusRepository {
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let new_version = if_version.map_or(1, |v| v + 1);

        // Upsert into versorgungsstatus + atomically append to history in one
        // transaction.  Both writes share the same `now()` so the history
        // `valid_from` is identical to `versorgungsstatus.updated_at`.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        // On first insert (if_version = None) we do a blind INSERT ... ON CONFLICT UPDATE
        // guarded only by the version = 1 expectation.
        // On subsequent updates we add a WHERE version = $expected clause via a CTE.
        let rows_affected: u64 = if let Some(expected) = if_version {
            sqlx::query(
                r#"WITH cte AS (
                    SELECT 1 FROM versorgungsstatus
                    WHERE malo_id = $1 AND tenant = $2 AND version = $3
                )
                UPDATE versorgungsstatus
                SET lieferstatus     = $4,
                    lf_mp_id           = $5,
                    lf_gln_next      = $6,
                    lieferbeginn     = $7,
                    lieferende       = $8,
                    msb_mp_id          = $9,
                    nb_mp_id           = $10,
                    last_process_id  = $11,
                    updated_at       = now(),
                    version          = $12
                WHERE malo_id = $1 AND tenant = $2 AND EXISTS (SELECT 1 FROM cte)"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(expected)
            .bind(rec.lieferstatus.to_string())
            .bind(&rec.lf_mp_id)
            .bind(&rec.lf_gln_next)
            .bind(rec.lieferbeginn)
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .bind(new_version)
            .execute(&mut *tx)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .rows_affected()
        } else {
            sqlx::query(
                r#"INSERT INTO versorgungsstatus
                   (malo_id, tenant, lieferstatus, lf_mp_id, lf_gln_next,
                    lieferbeginn, lieferende, msb_mp_id, nb_mp_id,
                    last_process_id, updated_at, version)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now(), 1)
                   ON CONFLICT (malo_id, tenant) DO UPDATE
                   SET lieferstatus    = EXCLUDED.lieferstatus,
                       lf_mp_id         = EXCLUDED.lf_mp_id,
                       lf_gln_next    = EXCLUDED.lf_gln_next,
                       lieferbeginn   = EXCLUDED.lieferbeginn,
                       lieferende     = EXCLUDED.lieferende,
                       msb_mp_id        = EXCLUDED.msb_mp_id,
                       nb_mp_id         = EXCLUDED.nb_mp_id,
                       last_process_id = EXCLUDED.last_process_id,
                       updated_at     = now(),
                       version        = versorgungsstatus.version + 1"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(rec.lieferstatus.to_string())
            .bind(&rec.lf_mp_id)
            .bind(&rec.lf_gln_next)
            .bind(rec.lieferbeginn)
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .rows_affected()
        };

        if rows_affected == 0 {
            tx.rollback()
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;
            return Err(MdmError::VersionConflict {
                expected: if_version.map_or("new".into(), |v| v.to_string()),
                actual: "(concurrent update)".into(),
            });
        }

        // Append history snapshot atomically.
        sqlx::query(
            r#"INSERT INTO versorgungsstatus_history
               (malo_id, tenant, lieferstatus, lf_mp_id, lf_gln_next,
                lieferbeginn, lieferende, msb_mp_id, nb_mp_id,
                last_process_id, version, valid_from)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, now())"#,
        )
        .bind(&rec.malo_id)
        .bind(&rec.tenant)
        .bind(rec.lieferstatus.to_string())
        .bind(&rec.lf_mp_id)
        .bind(&rec.lf_gln_next)
        .bind(rec.lieferbeginn)
        .bind(rec.lieferende)
        .bind(&rec.msb_mp_id)
        .bind(&rec.nb_mp_id)
        .bind(rec.last_process_id)
        .bind(new_version)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

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
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        opt.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
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
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        opt.as_ref()
            .map(|row| map_history_row(row).map(history_to_current))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn find_history(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusHistoryRecord>, MdmError> {
        let offset = i64::from(page * size);
        let limit = i64::from(size);

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM versorgungsstatus_history WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(malo_id)
        .bind(tenant)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

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
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let items = rows
            .iter()
            .map(map_history_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))?;

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
        let offset = i64::from(page * size);
        let limit = i64::from(size);
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM versorgungsstatus WHERE tenant = $1")
                .bind(tenant)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let rows = sqlx::query(
            "SELECT * FROM versorgungsstatus WHERE tenant = $1 ORDER BY malo_id LIMIT $2 OFFSET $3",
        )
        .bind(tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let items = rows
            .iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(PageResult {
            items,
            total: total as u64,
            page,
            size,
        })
    }
}
