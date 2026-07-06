//! PostgreSQL implementation of [`MeloRepository`].

use mako_mdm::{
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{MeloRecord, MeloRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed MeLo repository.
#[derive(Clone, Debug)]
pub struct PgMeloRepository {
    pool: PgPool,
}

impl PgMeloRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl MeloRepository for PgMeloRepository {
    async fn upsert(
        &self,
        melo_id: &MeloId,
        malo_id: Option<&MaloId>,
        data: serde_json::Value,
        if_match: Option<i64>,
    ) -> Result<i64, MdmError> {
        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM melo WHERE melo_id = $1")
                .bind(melo_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = match (current, if_match) {
            (Some(v), Some(expected)) if v != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: v.to_string(),
                });
            }
            (Some(v), _) => v + 1,
            (None, _) => 1,
        };

        sqlx::query(
            r#"INSERT INTO melo (melo_id, malo_id, version, data, updated_at)
               VALUES ($1, $2, $3, $4, now())
               ON CONFLICT (melo_id) DO UPDATE
               SET malo_id = EXCLUDED.malo_id,
                   version = EXCLUDED.version,
                   data = EXCLUDED.data,
                   updated_at = now()"#,
        )
        .bind(melo_id)
        .bind(malo_id)
        .bind(new_version)
        .bind(&data)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            "SELECT melo_id, malo_id, version, data, updated_at FROM melo WHERE melo_id = $1",
        )
        .bind(melo_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| MeloRecord {
            melo_id: r.get("melo_id"),
            malo_id: r.get("malo_id"),
            version: r.get("version"),
            data: r.get("data"),
            updated_at: r.get("updated_at"),
        }))
    }
}
