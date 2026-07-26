//! PostgreSQL implementation of [`TrancheRepository`].
//!
//! A Tranche is a share of a Marktlokation's energy assigned to a distinct
//! balancing responsibility (GPKE Teil 4 „Daten der Tranche"). One row per
//! `(tranche_id, tenant)`.

use mako_markt::{
    error::MdmError,
    repository::{PageResult, TrancheRecord, TrancheRepository, TrancheStammdatenPatch},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed Tranche repository.
#[derive(Clone, Debug)]
pub struct PgTrancheRepository {
    pool: PgPool,
}

impl PgTrancheRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<TrancheRecord, sqlx::Error> {
    Ok(TrancheRecord {
        tranche_id: row.try_get("tranche_id")?,
        tenant: row.try_get("tenant")?,
        malo_id: row.try_get("malo_id")?,
        bilanzierungsgebiet: row.try_get("bilanzierungsgebiet")?,
        netzebene: row.try_get("netzebene")?,
        energierichtung: row.try_get("energierichtung")?,
        data: row.try_get("data")?,
        version: row.try_get("version")?,
        updated_at: row.try_get("updated_at")?,
    })
}

impl TrancheRepository for PgTrancheRepository {
    async fn upsert(&self, rec: TrancheRecord, if_match: Option<i64>) -> Result<i64, MdmError> {
        let rows_affected: u64 = if let Some(expected) = if_match {
            sqlx::query(
                r"UPDATE tranche
                   SET malo_id             = $3,
                       bilanzierungsgebiet = $4,
                       netzebene           = $5,
                       energierichtung     = $6,
                       data                = $7,
                       version             = version + 1,
                       updated_at          = now()
                   WHERE tranche_id = $1 AND tenant = $2 AND version = $8",
            )
            .bind(&rec.tranche_id)
            .bind(&rec.tenant)
            .bind(&rec.malo_id)
            .bind(&rec.bilanzierungsgebiet)
            .bind(&rec.netzebene)
            .bind(&rec.energierichtung)
            .bind(&rec.data)
            .bind(expected)
            .execute(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
            .rows_affected()
        } else {
            sqlx::query(
                r"INSERT INTO tranche
                   (tranche_id, tenant, malo_id, bilanzierungsgebiet, netzebene,
                    energierichtung, data, version, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now())
                   ON CONFLICT (tranche_id, tenant) DO UPDATE
                   SET malo_id             = EXCLUDED.malo_id,
                       bilanzierungsgebiet = EXCLUDED.bilanzierungsgebiet,
                       netzebene           = EXCLUDED.netzebene,
                       energierichtung     = EXCLUDED.energierichtung,
                       data                = EXCLUDED.data,
                       version             = tranche.version + 1,
                       updated_at          = now()",
            )
            .bind(&rec.tranche_id)
            .bind(&rec.tenant)
            .bind(&rec.malo_id)
            .bind(&rec.bilanzierungsgebiet)
            .bind(&rec.netzebene)
            .bind(&rec.energierichtung)
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
            let new_version: i64 = sqlx::query_scalar(
                "SELECT version FROM tranche WHERE tranche_id = $1 AND tenant = $2",
            )
            .bind(&rec.tranche_id)
            .bind(&rec.tenant)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
            Ok(new_version)
        }
    }

    async fn find(
        &self,
        tranche_id: &str,
        tenant: &str,
    ) -> Result<Option<TrancheRecord>, MdmError> {
        let opt = sqlx::query("SELECT * FROM tranche WHERE tranche_id = $1 AND tenant = $2")
            .bind(tranche_id)
            .bind(tenant)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        opt.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn list_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<TrancheRecord>, MdmError> {
        let offset = i64::from(page * size);
        let limit = i64::from(size);
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM tranche WHERE tenant = $1 AND malo_id = $2")
                .bind(tenant)
                .bind(malo_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let rows = sqlx::query(
            r"SELECT * FROM tranche WHERE tenant = $1 AND malo_id = $2
               ORDER BY tranche_id LIMIT $3 OFFSET $4",
        )
        .bind(tenant)
        .bind(malo_id)
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

    async fn patch_stammdaten(
        &self,
        tranche_id: &str,
        tenant: &str,
        patch: &TrancheStammdatenPatch,
    ) -> Result<bool, MdmError> {
        if patch.is_empty() {
            return Ok(false);
        }
        // COALESCE per column; JSONB payload and version are untouched.
        let affected = sqlx::query(
            r"UPDATE tranche
               SET bilanzierungsgebiet = COALESCE($3, bilanzierungsgebiet),
                   netzebene           = COALESCE($4, netzebene),
                   energierichtung     = COALESCE($5, energierichtung),
                   updated_at          = now()
               WHERE tranche_id = $1 AND tenant = $2",
        )
        .bind(tranche_id)
        .bind(tenant)
        .bind(&patch.bilanzierungsgebiet)
        .bind(&patch.netzebene)
        .bind(&patch.energierichtung)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }
}
