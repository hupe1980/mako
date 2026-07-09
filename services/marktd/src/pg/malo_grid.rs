//! PostgreSQL implementation of `MaloGridRepository`.

use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{MaloGridRecord, MaloGridRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

#[derive(Clone, Debug)]
pub struct PgMaloGridRepository {
    pool: PgPool,
}

impl PgMaloGridRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<MaloGridRecord, sqlx::Error> {
    let malo_id_str: String = row.try_get("malo_id")?;
    let malo_id = malo_id_str
        .parse::<MaloId>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "malo_id".into(),
            source: Box::new(std::io::Error::other(e.to_string())),
        })?;
    let sparte_str: String = row.try_get("sparte")?;
    let sparte = sparte_str
        .parse::<Sparte>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "sparte".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    Ok(MaloGridRecord {
        malo_id,
        nb_mp_id: row.try_get("nb_mp_id")?,
        bilanzierungsgebiet: row.try_get("bilanzierungsgebiet")?,
        netzgebiet: row.try_get("netzgebiet")?,
        sparte,
        source: row.try_get("source")?,
        updated_at: row.try_get("updated_at")?,
        tenant: row.try_get("tenant")?,
    })
}

impl MaloGridRepository for PgMaloGridRepository {
    async fn upsert(&self, rec: MaloGridRecord) -> Result<(), MdmError> {
        let sparte = rec.sparte.to_string();
        sqlx::query(
            r#"
            INSERT INTO malo_grid
                (malo_id, tenant, nb_mp_id, bilanzierungsgebiet, netzgebiet, sparte, source, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (malo_id, tenant)
            DO UPDATE SET
                nb_mp_id               = EXCLUDED.nb_mp_id,
                bilanzierungsgebiet  = EXCLUDED.bilanzierungsgebiet,
                netzgebiet           = EXCLUDED.netzgebiet,
                sparte               = EXCLUDED.sparte,
                source               = EXCLUDED.source,
                updated_at           = EXCLUDED.updated_at
            "#,
        )
        .bind(&rec.malo_id)
        .bind(&rec.tenant)
        .bind(&rec.nb_mp_id)
        .bind(&rec.bilanzierungsgebiet)
        .bind(&rec.netzgebiet)
        .bind(&sparte)
        .bind(&rec.source)
        .bind(rec.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<MaloGridRecord>, MdmError> {
        let opt = sqlx::query(
            "SELECT malo_id, tenant, nb_mp_id, bilanzierungsgebiet, netzgebiet, sparte, source, updated_at FROM malo_grid WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(malo_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        opt.map(|r| map_row(&r).map_err(|e| MdmError::Internal(e.to_string())))
            .transpose()
    }

    async fn list_by_nb(
        &self,
        nb_mp_id: &str,
        tenant: &str,
    ) -> Result<Vec<MaloGridRecord>, MdmError> {
        let rows = sqlx::query(
            "SELECT malo_id, tenant, nb_mp_id, bilanzierungsgebiet, netzgebiet, sparte, source, updated_at FROM malo_grid WHERE nb_mp_id = $1 AND tenant = $2 ORDER BY malo_id ASC",
        )
        .bind(nb_mp_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter()
            .map(|r| map_row(r).map_err(|e| MdmError::Internal(e.to_string())))
            .collect()
    }

    async fn delete(&self, malo_id: &MaloId, tenant: &str) -> Result<(), MdmError> {
        sqlx::query("DELETE FROM malo_grid WHERE malo_id = $1 AND tenant = $2")
            .bind(malo_id)
            .bind(tenant)
            .execute(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }
}
