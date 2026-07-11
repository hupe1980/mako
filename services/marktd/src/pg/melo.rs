//! PostgreSQL implementation of [`MeloRepository`].

use mako_markt::{
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
        bo4e_version: &str,
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

        let netzebene_messung = data
            .get("netzebeneMessung")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Extract `regelzone` from `standorteigenschaften.eigenschaftenStrom[0].regelzone`.
        // This maps the MeLo to the \u00dcNB responsible for Redispatch 2.0 Stammdaten routing.
        let regelzone = data
            .get("standorteigenschaften")
            .and_then(|s| s.get("eigenschaftenStrom"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("regelzone"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Extract full standorteigenschaften JSONB for Redispatch 2.0 and Gas billing zone.
        let standorteigenschaften = data.get("standorteigenschaften").cloned();

        sqlx::query(
            r#"INSERT INTO melo (melo_id, malo_id, netzebene_messung, regelzone, standorteigenschaften, version, data, bo4e_version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
               ON CONFLICT (melo_id) DO UPDATE
               SET malo_id                = EXCLUDED.malo_id,
                   netzebene_messung      = EXCLUDED.netzebene_messung,
                   regelzone              = EXCLUDED.regelzone,
                   standorteigenschaften  = COALESCE(EXCLUDED.standorteigenschaften,
                                                     melo.standorteigenschaften),
                   version                = EXCLUDED.version,
                   data                   = EXCLUDED.data,
                   bo4e_version           = EXCLUDED.bo4e_version,
                   updated_at             = now()"#,
        )
        .bind(melo_id)
        .bind(malo_id)
        .bind(&netzebene_messung)
        .bind(&regelzone)
        .bind(standorteigenschaften.as_ref())
        .bind(new_version)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            "SELECT melo_id, malo_id, netzebene_messung, regelzone, standorteigenschaften, version, data, bo4e_version, updated_at FROM melo WHERE melo_id = $1",
        )
        .bind(melo_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| MeloRecord {
            melo_id: r.get("melo_id"),
            malo_id: r.get("malo_id"),
            netzebene_messung: r.try_get("netzebene_messung").unwrap_or(None),
            regelzone: r.try_get("regelzone").unwrap_or(None),
            standorteigenschaften: r.try_get("standorteigenschaften").unwrap_or(None),
            version: r.get("version"),
            data: r.get("data"),
            updated_at: r.get("updated_at"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
        }))
    }
}
