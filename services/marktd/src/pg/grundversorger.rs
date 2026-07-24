//! PostgreSQL implementation of `GrundversorgerRepository`.

use mako_markt::{
    domain::Sparte,
    error::MdmError,
    repository::{GrundversorgerRecord, GrundversorgerRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

#[derive(Clone, Debug)]
pub struct PgGrundversorgerRepository {
    pool: PgPool,
}

impl PgGrundversorgerRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<GrundversorgerRecord, sqlx::Error> {
    let sparte_str: String = row.try_get("sparte")?;
    let sparte = sparte_str
        .parse::<Sparte>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "sparte".into(),
            source: Box::new(std::io::Error::other(e)),
        })?;
    Ok(GrundversorgerRecord {
        nb_mp_id: row.try_get("nb_mp_id")?,
        sparte,
        gv_mp_id: row.try_get("gv_mp_id")?,
        festgestellt_am: row.try_get("festgestellt_am")?,
        updated_at: row.try_get("updated_at")?,
        tenant: row.try_get("tenant")?,
    })
}

impl GrundversorgerRepository for PgGrundversorgerRepository {
    async fn find(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        sparte: Sparte,
    ) -> Result<Option<GrundversorgerRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT tenant, nb_mp_id, sparte, gv_mp_id, festgestellt_am, updated_at
              FROM grundversorger
              WHERE tenant = $1 AND nb_mp_id = $2 AND sparte = $3",
        )
        .bind(tenant)
        .bind(nb_mp_id)
        .bind(sparte.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        row.as_ref()
            .map(map_row)
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    async fn upsert(&self, rec: &GrundversorgerRecord) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO grundversorger
                  (tenant, nb_mp_id, sparte, gv_mp_id, festgestellt_am, updated_at)
              VALUES ($1, $2, $3, $4, $5, now())
              ON CONFLICT (tenant, nb_mp_id, sparte)
              DO UPDATE SET
                  gv_mp_id        = EXCLUDED.gv_mp_id,
                  festgestellt_am = EXCLUDED.festgestellt_am,
                  updated_at      = now()",
        )
        .bind(&rec.tenant)
        .bind(&rec.nb_mp_id)
        .bind(rec.sparte.to_string())
        .bind(&rec.gv_mp_id)
        .bind(rec.festgestellt_am)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }
}
