//! PostgreSQL implementation of [`NeLoRepository`].

use mako_markt::{
    domain::Sparte,
    error::MdmError,
    repository::{NeLoRecord, NeLoRepository, PageResult},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use std::str::FromStr as _;

/// PostgreSQL-backed NeLo repository.
///
/// One row per `(nelo_id, tenant)`.
/// Writes use optimistic concurrency via `if_match` (ETag version).
#[derive(Clone, Debug)]
pub struct PgNeLoRepository {
    pool: PgPool,
}

impl PgNeLoRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<NeLoRecord, sqlx::Error> {
    let sparte_str: String = row.try_get("sparte")?;
    let sparte = Sparte::from_str(&sparte_str).map_err(|e| sqlx::Error::ColumnDecode {
        index: "sparte".into(),
        source: Box::new(std::io::Error::other(e)),
    })?;
    Ok(NeLoRecord {
        nelo_id: row.try_get("nelo_id")?,
        tenant: row.try_get("tenant")?,
        name: row.try_get("name")?,
        sparte,
        netzebene: row.try_get("netzebene")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        data: row.try_get("data")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

impl NeLoRepository for PgNeLoRepository {
    async fn upsert(&self, rec: NeLoRecord, if_match: Option<i64>) -> Result<i64, MdmError> {
        let rows_affected: u64 = if let Some(expected) = if_match {
            // Conditional update — only succeeds when version matches.
            sqlx::query(
                r#"UPDATE nelo
                   SET name       = $3,
                       sparte     = $4,
                       netzebene  = $5,
                       nb_mp_id     = $6,
                       data       = $7,
                       version    = version + 1,
                       updated_at = now()
                   WHERE nelo_id = $1 AND tenant = $2 AND version = $8"#,
            )
            .bind(&rec.nelo_id)
            .bind(&rec.tenant)
            .bind(&rec.name)
            .bind(rec.sparte.to_string())
            .bind(&rec.netzebene)
            .bind(&rec.nb_mp_id)
            .bind(&rec.data)
            .bind(expected)
            .execute(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .rows_affected()
        } else {
            // Blind upsert — insert or update unconditionally.
            sqlx::query(
                r#"INSERT INTO nelo
                   (nelo_id, tenant, name, sparte, netzebene, nb_mp_id, data, version, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now())
                   ON CONFLICT (nelo_id, tenant) DO UPDATE
                   SET name       = EXCLUDED.name,
                       sparte     = EXCLUDED.sparte,
                       netzebene  = EXCLUDED.netzebene,
                       nb_mp_id     = EXCLUDED.nb_mp_id,
                       data       = EXCLUDED.data,
                       version    = nelo.version + 1,
                       updated_at = now()"#,
            )
            .bind(&rec.nelo_id)
            .bind(&rec.tenant)
            .bind(&rec.name)
            .bind(rec.sparte.to_string())
            .bind(&rec.netzebene)
            .bind(&rec.nb_mp_id)
            .bind(&rec.data)
            .execute(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .rows_affected()
        };

        if rows_affected == 0 {
            Err(MdmError::VersionConflict {
                expected: if_match.map_or("new".into(), |v| v.to_string()),
                actual: "(concurrent update)".into(),
            })
        } else {
            // Return the new version: re-read is the simplest correct path for
            // both branches (the blind upsert may have incremented an existing row).
            let new_version: i64 =
                sqlx::query_scalar("SELECT version FROM nelo WHERE nelo_id = $1 AND tenant = $2")
                    .bind(&rec.nelo_id)
                    .bind(&rec.tenant)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| MdmError::Internal(e.to_string()))?;
            Ok(new_version)
        }
    }

    async fn find(&self, nelo_id: &str, tenant: &str) -> Result<Option<NeLoRecord>, MdmError> {
        let opt = sqlx::query("SELECT * FROM nelo WHERE nelo_id = $1 AND tenant = $2")
            .bind(nelo_id)
            .bind(tenant)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        opt.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError> {
        let offset = i64::from(page * size);
        let limit = i64::from(size);
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM nelo WHERE tenant = $1 AND nb_mp_id = $2")
                .bind(tenant)
                .bind(nb_mp_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let rows = sqlx::query(
            r#"SELECT * FROM nelo WHERE tenant = $1 AND nb_mp_id = $2
               ORDER BY nelo_id LIMIT $3 OFFSET $4"#,
        )
        .bind(tenant)
        .bind(nb_mp_id)
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

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<NeLoRecord>, MdmError> {
        let offset = i64::from(page * size);
        let limit = i64::from(size);
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nelo WHERE tenant = $1")
            .bind(tenant)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        let rows =
            sqlx::query("SELECT * FROM nelo WHERE tenant = $1 ORDER BY nelo_id LIMIT $2 OFFSET $3")
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
