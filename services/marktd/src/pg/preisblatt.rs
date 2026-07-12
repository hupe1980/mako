//! PostgreSQL implementations of [`PreisblattRepository`], [`PreisblattMessungRepository`],
//! [`PreisblattKaRepository`], [`PreisblattDienstleistungRepository`], and
//! [`PreisblattHardwareRepository`].

use mako_markt::{
    error::MdmError,
    repository::{
        PreisblattDienstleistungRecord, PreisblattDienstleistungRepository,
        PreisblattHardwareRecord, PreisblattHardwareRepository, PreisblattKaRecord,
        PreisblattKaRepository, PreisblattMessungRecord, PreisblattMessungRepository,
        PreisblattRecord, PreisblattRepository, PreisblattSource,
    },
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
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                source,
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}

// ── PreisblattMessung (MSB metering price sheets — B5) ───────────────────────

/// PostgreSQL-backed `PreisblattMessung` repository.
///
/// Upserts by `(msb_mp_id, valid_from)`: same source-override protection as
/// [`PgPreisblattRepository`].
#[derive(Clone, Debug)]
pub struct PgPreisblattMessungRepository {
    pool: sqlx::PgPool,
}

impl PgPreisblattMessungRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub fn pool(&self) -> &sqlx::PgPool {
        &self.pool
    }
}

impl PreisblattMessungRepository for PgPreisblattMessungRepository {
    async fn upsert_messung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO preisblaetter_messung
                   (msb_mp_id, valid_from, valid_to, data, bo4e_version, source, updated_at)
               VALUES (
                   $1,
                   ($2->'gueltigkeit'->>'startdatum')::date,
                   ($2->'gueltigkeit'->>'enddatum')::date,
                   $2,
                   $3,
                   $4,
                   now()
               )
               ON CONFLICT (msb_mp_id, valid_from) DO UPDATE
               SET valid_to      = EXCLUDED.valid_to,
                   data          = EXCLUDED.data,
                   bo4e_version  = EXCLUDED.bo4e_version,
                   source        = EXCLUDED.source,
                   updated_at    = now()
               WHERE preisblaetter_messung.source <> 'api' OR EXCLUDED.source = 'api'"#,
        )
        .bind(msb_mp_id)
        .bind(&data)
        .bind(bo4e_version)
        .bind(source.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(())
    }

    async fn find_messung_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattMessungRecord>, MdmError> {
        let row: Option<sqlx::postgres::PgRow> = sqlx::query(
            r#"SELECT msb_mp_id, data, bo4e_version, source, created_at, updated_at
               FROM preisblaetter_messung
               WHERE msb_mp_id = $1
                 AND (valid_from IS NULL OR valid_from <= $2::date)
                 AND (valid_to   IS NULL OR valid_to   >  $2::date)
               ORDER BY valid_from DESC NULLS LAST
               LIMIT 1"#,
        )
        .bind(msb_mp_id)
        .bind(billing_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        use sqlx::Row as _;
        Ok(row.map(|r| {
            let source_str: String = r.try_get("source").unwrap_or_else(|_| "api".to_owned());
            let source = source_str
                .parse::<PreisblattSource>()
                .unwrap_or(PreisblattSource::Api);
            PreisblattMessungRecord {
                msb_mp_id: r.get("msb_mp_id"),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                source,
                auf_abschlaege: r
                    .try_get::<serde_json::Value, _>("auf_abschlaege")
                    .ok()
                    .and_then(|v| v.as_array().cloned())
                    .unwrap_or_default(),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}

// ── PreisblattKonzessionsabgabe (B3) ─────────────────────────────────────────

/// PostgreSQL-backed [`PreisblattKaRepository`].
#[derive(Clone, Debug)]
pub struct PgPreisblattKaRepository {
    pool: sqlx::PgPool,
}

impl PgPreisblattKaRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl PreisblattKaRepository for PgPreisblattKaRepository {
    async fn upsert_ka(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO preisblaetter_konzessionsabgabe
                   (nb_mp_id, sparte, kundengruppe_ka, valid_from, valid_to, data, bo4e_version, source, updated_at)
               VALUES (
                   $1, $2, $3,
                   ($4->'gueltigkeit'->>'startdatum')::date,
                   ($4->'gueltigkeit'->>'enddatum')::date,
                   $4, $5, $6, now()
               )
               ON CONFLICT (nb_mp_id, sparte, kundengruppe_ka, valid_from) DO UPDATE
               SET valid_to      = EXCLUDED.valid_to,
                   data          = EXCLUDED.data,
                   bo4e_version  = EXCLUDED.bo4e_version,
                   source        = EXCLUDED.source,
                   updated_at    = now()
               WHERE preisblaetter_konzessionsabgabe.source <> 'api' OR EXCLUDED.source = 'api'"#,
        )
        .bind(nb_mp_id)
        .bind(sparte)
        .bind(kundengruppe_ka)
        .bind(&data)
        .bind(bo4e_version)
        .bind(source.to_string())
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_ka_for_date(
        &self,
        nb_mp_id: &str,
        sparte: &str,
        kundengruppe_ka: Option<&str>,
        billing_date: &str,
    ) -> Result<Option<PreisblattKaRecord>, MdmError> {
        use sqlx::Row as _;
        let row = sqlx::query(
            r#"SELECT nb_mp_id, sparte, kundengruppe_ka, data, bo4e_version, source, created_at, updated_at
               FROM preisblaetter_konzessionsabgabe
               WHERE nb_mp_id = $1
                 AND sparte   = $2
                 AND ($3::text IS NULL OR kundengruppe_ka = $3 OR kundengruppe_ka IS NULL)
                 AND (valid_from IS NULL OR valid_from <= $4::date)
                 AND (valid_to   IS NULL OR valid_to   >  $4::date)
               ORDER BY kundengruppe_ka NULLS LAST, valid_from DESC NULLS LAST
               LIMIT 1"#,
        )
        .bind(nb_mp_id)
        .bind(sparte)
        .bind(kundengruppe_ka)
        .bind(billing_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| {
            let source_str: String = r.try_get("source").unwrap_or_else(|_| "api".to_owned());
            PreisblattKaRecord {
                nb_mp_id: r.get("nb_mp_id"),
                sparte: r.get("sparte"),
                kundengruppe_ka: r.try_get("kundengruppe_ka").unwrap_or(None),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                source: source_str.parse().unwrap_or(PreisblattSource::Api),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}

// ── PreisblattDienstleistung ──────────────────────────────────────────────────

/// PostgreSQL-backed [`PreisblattDienstleistungRepository`].
#[derive(Clone, Debug)]
pub struct PgPreisblattDienstleistungRepository {
    pool: sqlx::PgPool,
}

impl PgPreisblattDienstleistungRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl PreisblattDienstleistungRepository for PgPreisblattDienstleistungRepository {
    async fn upsert_dienstleistung(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO preisblaetter_dienstleistung
                   (msb_mp_id, valid_from, valid_to, data, bo4e_version, source, updated_at)
               VALUES ($1, ($2->'gueltigkeit'->>'startdatum')::date, ($2->'gueltigkeit'->>'enddatum')::date, $2, $3, $4, now())
               ON CONFLICT (msb_mp_id, valid_from) DO UPDATE
               SET valid_to=EXCLUDED.valid_to, data=EXCLUDED.data, bo4e_version=EXCLUDED.bo4e_version,
                   source=EXCLUDED.source, updated_at=now()
               WHERE preisblaetter_dienstleistung.source<>'api' OR EXCLUDED.source='api'"#,
        )
        .bind(msb_mp_id).bind(&data).bind(bo4e_version).bind(source.to_string())
        .execute(&self.pool).await.map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_dienstleistung_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattDienstleistungRecord>, MdmError> {
        use sqlx::Row as _;
        let row = sqlx::query(
            r#"SELECT msb_mp_id, data, bo4e_version, source, created_at, updated_at
               FROM preisblaetter_dienstleistung
               WHERE msb_mp_id=$1 AND (valid_from IS NULL OR valid_from<=$2::date)
                 AND (valid_to IS NULL OR valid_to>$2::date)
               ORDER BY valid_from DESC NULLS LAST LIMIT 1"#,
        )
        .bind(msb_mp_id)
        .bind(billing_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(row.map(|r| PreisblattDienstleistungRecord {
            msb_mp_id: r.get("msb_mp_id"),
            data: r.get("data"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
            source: r
                .try_get::<String, _>("source")
                .unwrap_or_else(|_| "api".to_owned())
                .parse()
                .unwrap_or(PreisblattSource::Api),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }
}

// ── PreisblattHardware ────────────────────────────────────────────────────────

/// PostgreSQL-backed [`PreisblattHardwareRepository`].
#[derive(Clone, Debug)]
pub struct PgPreisblattHardwareRepository {
    pool: sqlx::PgPool,
}

impl PgPreisblattHardwareRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl PreisblattHardwareRepository for PgPreisblattHardwareRepository {
    async fn upsert_hardware(
        &self,
        msb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO preisblaetter_hardware
                   (msb_mp_id, valid_from, valid_to, data, bo4e_version, source, updated_at)
               VALUES ($1, ($2->'gueltigkeit'->>'startdatum')::date, ($2->'gueltigkeit'->>'enddatum')::date, $2, $3, $4, now())
               ON CONFLICT (msb_mp_id, valid_from) DO UPDATE
               SET valid_to=EXCLUDED.valid_to, data=EXCLUDED.data, bo4e_version=EXCLUDED.bo4e_version,
                   source=EXCLUDED.source, updated_at=now()
               WHERE preisblaetter_hardware.source<>'api' OR EXCLUDED.source='api'"#,
        )
        .bind(msb_mp_id).bind(&data).bind(bo4e_version).bind(source.to_string())
        .execute(&self.pool).await.map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_hardware_for_date(
        &self,
        msb_mp_id: &str,
        billing_date: &str,
    ) -> Result<Option<PreisblattHardwareRecord>, MdmError> {
        use sqlx::Row as _;
        let row = sqlx::query(
            r#"SELECT msb_mp_id, data, bo4e_version, source, created_at, updated_at
               FROM preisblaetter_hardware
               WHERE msb_mp_id=$1 AND (valid_from IS NULL OR valid_from<=$2::date)
                 AND (valid_to IS NULL OR valid_to>$2::date)
               ORDER BY valid_from DESC NULLS LAST LIMIT 1"#,
        )
        .bind(msb_mp_id)
        .bind(billing_date)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(row.map(|r| PreisblattHardwareRecord {
            msb_mp_id: r.get("msb_mp_id"),
            data: r.get("data"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
            source: r
                .try_get::<String, _>("source")
                .unwrap_or_else(|_| "api".to_owned())
                .parse()
                .unwrap_or(PreisblattSource::Api),
            created_at: r.get("created_at"),
            updated_at: r.get("updated_at"),
        }))
    }
}
