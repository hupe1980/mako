//! PostgreSQL implementation of [`PreisblattRepository`].

use mako_markt::{
    error::MdmError,
    repository::{PreisblattRecord, PreisblattRepository, PreisblattSource},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed `PreisblattNetznutzung` repository.
///
/// Upserts by `(nb_mp_id, valid_from)`: re-publishing the same NB GLN + validity
/// start date overwrites the previous entry in-place, subject to source rules
/// (see [`PreisblattSource`] for the Api-override-protection semantics).
#[derive(Clone, Debug)]
pub struct PgPreisblattRepository {
    pool: PgPool,
}

impl PgPreisblattRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl PreisblattRepository for PgPreisblattRepository {
    async fn upsert(
        &self,
        nb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        // Extract valid_from / valid_to from the JSONB blob so the index column
        // stays consistent without requiring application-level date parsing.
        //
        // Api-override protection: if an existing row has source='api' and the
        // caller is source='mako', skip the overwrite.  The operator's manual
        // override is preserved until explicitly replaced via an 'api' call.
        sqlx::query(
            r#"INSERT INTO preisblaetter
                   (nb_mp_id, valid_from, valid_to, data, bo4e_version, source, updated_at)
               VALUES (
                   $1,
                   ($2->'gueltigkeit'->>'startdatum')::date,
                   ($2->'gueltigkeit'->>'enddatum')::date,
                   $2,
                   $3,
                   $4,
                   now()
               )
               ON CONFLICT (nb_mp_id, valid_from) DO UPDATE
               SET valid_to      = EXCLUDED.valid_to,
                   data          = EXCLUDED.data,
                   bo4e_version  = EXCLUDED.bo4e_version,
                   source        = EXCLUDED.source,
                   updated_at    = now()
               -- Never silently overwrite an operator API upload with a mako ingest.
               WHERE preisblaetter.source <> 'api' OR EXCLUDED.source = 'api'"#,
        )
        .bind(nb_mp_id)
        .bind(&data)
        .bind(bo4e_version)
        .bind(source.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(())
    }

    async fn find_for_date(
        &self,
        nb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            r#"SELECT nb_mp_id, data, bo4e_version, source, created_at, updated_at
               FROM preisblaetter
               WHERE nb_mp_id = $1
                 AND (valid_from IS NULL OR valid_from <= $2::date)
                 AND (valid_to   IS NULL OR valid_to   >  $2::date)
               ORDER BY valid_from DESC NULLS LAST
               LIMIT 1"#,
        )
        .bind(nb_mp_id)
        .bind(billing_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| {
            let source_str: String = r.try_get("source").unwrap_or_else(|_| "api".to_owned());
            let source = source_str
                .parse::<PreisblattSource>()
                .unwrap_or(PreisblattSource::Api);
            PreisblattRecord {
                nb_mp_id: r.get("nb_mp_id"),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202501.0.0".to_owned()),
                source,
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}
