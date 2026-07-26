//! PostgreSQL implementation of `BilanzierungRepository` — the first-class,
//! temporal BO4E `Bilanzierung` resource (BO #3).

use mako_markt::{
    error::MdmError,
    repository::{BilanzierungRecord, BilanzierungRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;

#[derive(Clone, Debug)]
pub struct PgBilanzierungRepository {
    pool: PgPool,
}

impl PgBilanzierungRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<BilanzierungRecord, sqlx::Error> {
    Ok(BilanzierungRecord {
        malo_id: row.try_get("malo_id")?,
        bilanzierungsbeginn: row.try_get("bilanzierungsbeginn")?,
        bilanzierungsende: row.try_get("bilanzierungsende")?,
        bilanzkreis: row.try_get("bilanzkreis")?,
        aggregationsverantwortung: row.try_get("aggregationsverantwortung")?,
        prognosegrundlage: row.try_get("prognosegrundlage")?,
        fallgruppenzuordnung: row.try_get("fallgruppenzuordnung")?,
        data: row.try_get("data")?,
        bo4e_version: row.try_get("bo4e_version")?,
        tenant: row.try_get("tenant")?,
        updated_at: row.try_get("updated_at")?,
    })
}

const SELECT_COLS: &str = "tenant, malo_id, bilanzierungsbeginn, bilanzierungsende, \
     bilanzkreis, aggregationsverantwortung, prognosegrundlage, fallgruppenzuordnung, \
     data, bo4e_version, updated_at";

impl BilanzierungRepository for PgBilanzierungRepository {
    async fn upsert(&self, rec: &BilanzierungRecord) -> Result<(), MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        sqlx::query(
            r"INSERT INTO bilanzierungen
                  (tenant, malo_id, bilanzierungsbeginn, bilanzierungsende, bilanzkreis,
                   aggregationsverantwortung, prognosegrundlage, fallgruppenzuordnung,
                   data, bo4e_version, updated_at)
              VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10, now())
              ON CONFLICT (tenant, malo_id, bilanzierungsbeginn) DO UPDATE
              SET bilanzierungsende         = EXCLUDED.bilanzierungsende,
                  bilanzkreis               = EXCLUDED.bilanzkreis,
                  aggregationsverantwortung = EXCLUDED.aggregationsverantwortung,
                  prognosegrundlage         = EXCLUDED.prognosegrundlage,
                  fallgruppenzuordnung      = EXCLUDED.fallgruppenzuordnung,
                  data                      = EXCLUDED.data,
                  bo4e_version              = EXCLUDED.bo4e_version,
                  updated_at                = now()",
        )
        .bind(&rec.tenant)
        .bind(&rec.malo_id)
        .bind(rec.bilanzierungsbeginn)
        .bind(rec.bilanzierungsende)
        .bind(&rec.bilanzkreis)
        .bind(&rec.aggregationsverantwortung)
        .bind(&rec.prognosegrundlage)
        .bind(&rec.fallgruppenzuordnung)
        .bind(&rec.data)
        .bind(&rec.bo4e_version)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        // Derive the denormalised `malo.fallgruppe` current-value when this
        // Bilanzierung is the one effective *now* (BO4E: Fallgruppe is a
        // Bilanzierung field, not a Marktlokation field — the resource is
        // authoritative and the MaLo column is derived from it). No-op when the
        // MaLo row does not exist.
        let is_current = rec.bilanzierungsbeginn <= OffsetDateTime::now_utc()
            && rec
                .bilanzierungsende
                .is_none_or(|e| e > OffsetDateTime::now_utc());
        if is_current {
            sqlx::query("UPDATE malo SET fallgruppe = $1, updated_at = now() WHERE malo_id = $2")
                .bind(&rec.fallgruppenzuordnung)
                .bind(&rec.malo_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;
        }

        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_at(
        &self,
        tenant: &str,
        malo_id: &str,
        at: OffsetDateTime,
    ) -> Result<Option<BilanzierungRecord>, MdmError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM bilanzierungen \
             WHERE tenant = $1 AND malo_id = $2 \
               AND bilanzierungsbeginn <= $3 \
               AND (bilanzierungsende IS NULL OR bilanzierungsende > $3) \
             ORDER BY bilanzierungsbeginn DESC LIMIT 1"
        );
        let row = sqlx::query(&sql)
            .bind(tenant)
            .bind(malo_id)
            .bind(at)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        row.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn history(
        &self,
        tenant: &str,
        malo_id: &str,
    ) -> Result<Vec<BilanzierungRecord>, MdmError> {
        let sql = format!(
            "SELECT {SELECT_COLS} FROM bilanzierungen \
             WHERE tenant = $1 AND malo_id = $2 ORDER BY bilanzierungsbeginn DESC"
        );
        let rows = sqlx::query(&sql)
            .bind(tenant)
            .bind(malo_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }
}
