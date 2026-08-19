//! PostgreSQL implementation of `MabisZpRepository`.

use mako_markt::{
    error::MdmError,
    repository::{MabisZpRecord, MabisZpRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

#[derive(Clone, Debug)]
pub struct PgMabisZpRepository {
    pool: PgPool,
}

impl PgMabisZpRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<MabisZpRecord, sqlx::Error> {
    Ok(MabisZpRecord {
        bilanzierungsgebiet: row.try_get("bilanzierungsgebiet")?,
        mabis_zp_id: row.try_get("mabis_zp_id")?,
        source: row.try_get("source")?,
        tenant: row.try_get("tenant")?,
        updated_at: row.try_get("updated_at")?,
    })
}

impl MabisZpRepository for PgMabisZpRepository {
    async fn upsert(&self, rec: MabisZpRecord) -> Result<(), MdmError> {
        sqlx::query(
            r#"
            INSERT INTO mabis_zaehlpunkte
                (bilanzierungsgebiet, tenant, mabis_zp_id, source, updated_at)
            VALUES ($1, $2, $3, $4, now())
            ON CONFLICT (bilanzierungsgebiet, tenant)
            DO UPDATE SET
                mabis_zp_id = EXCLUDED.mabis_zp_id,
                source      = EXCLUDED.source,
                updated_at  = now()
            "#,
        )
        .bind(&rec.bilanzierungsgebiet)
        .bind(&rec.tenant)
        .bind(&rec.mabis_zp_id)
        .bind(&rec.source)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find(
        &self,
        bilanzierungsgebiet: &str,
        tenant: &str,
    ) -> Result<Option<MabisZpRecord>, MdmError> {
        let row = sqlx::query(
            r#"
            SELECT bilanzierungsgebiet, tenant, mabis_zp_id, source, updated_at
            FROM mabis_zaehlpunkte
            WHERE bilanzierungsgebiet = $1 AND tenant = $2
            "#,
        )
        .bind(bilanzierungsgebiet)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        row.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn list(&self, tenant: &str) -> Result<Vec<MabisZpRecord>, MdmError> {
        let rows = sqlx::query(
            r#"
            SELECT bilanzierungsgebiet, tenant, mabis_zp_id, source, updated_at
            FROM mabis_zaehlpunkte
            WHERE tenant = $1
            ORDER BY bilanzierungsgebiet
            "#,
        )
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }
}
