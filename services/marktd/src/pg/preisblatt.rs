//! PostgreSQL storage: [`PgPreisblattRepository`], [`PgPreisblattMessungRepository`],
//! [`PgPreisblattKaRepository`], [`PgPreisblattDienstleistungRepository`], and
//! [`PgPreisblattHardwareRepository`].

use mako_markt::{
    error::MdmError,
    repository::{
        PreisblattDienstleistungRecord, PreisblattHardwareRecord, PreisblattKaRecord,
        PreisblattMessungRecord, PreisblattRecord, PreisblattSource,
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

    /// Upsert on the caller's transaction, so the durable price sheet, its
    /// PRICAT version snapshot and the `pricat.published` outbox row commit
    /// together.
    pub async fn upsert_tx(
        conn: &mut sqlx::PgConnection,
        nb_mp_id: &str,
        data: &serde_json::Value,
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
        .bind(data)
        .bind(bo4e_version)
        .bind(source.to_string())
        .execute(conn)
        .await
        .map_err(super::write_error)?;

        Ok(())
    }
}

impl PgPreisblattRepository {
    /// Upsert a `PreisblattNetznutzung` for the given NB GLN.
    ///
    /// Multiple records per GLN are stored; they are distinguished by the
    /// `gueltigkeit.startdatum` inside `data`.
    ///
    /// `source` tracks how the record entered the system: `Api` for operator
    /// REST uploads, `Mako` for engine-ingested PRICAT 27003 messages.
    /// An `Api`-sourced sheet is never overwritten by a `Mako` ingest unless
    /// `force = true`.
    pub async fn upsert(
        &self,
        nb_mp_id: &str,
        data: serde_json::Value,
        bo4e_version: &str,
        source: PreisblattSource,
    ) -> Result<(), MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::upsert_tx(&mut conn, nb_mp_id, &data, bo4e_version, source).await
    }

    /// Return the price sheet for `nb_mp_id` that was valid on `billing_date`
    /// (ISO 8601 date string, e.g. `"2025-06-15"`).
    ///
    /// Returns `None` when no matching entry is found.
    pub async fn find_for_date(
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
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
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

impl PgPreisblattMessungRepository {
    /// Upsert a `PreisblattMessung` for the given MSB MP-ID.
    ///
    /// Conflicts on `(msb_mp_id, valid_from)` perform an in-place update.
    /// An `Api`-sourced sheet is never overwritten by a `Mako` ingest.
    pub async fn upsert_messung(
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
        .map_err(super::write_error)?;

        Ok(())
    }

    /// Return the `PreisblattMessung` for `msb_mp_id` valid on `billing_date`
    /// (ISO 8601 date string, e.g. `"2025-06-15"`).
    ///
    /// Returns `None` when no matching entry is found.
    pub async fn find_messung_for_date(
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
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
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

/// Read/write access to `PreisblattKonzessionsabgabe` records.
///
/// Used by `netzbilanzd` for INVOIC 31001/31002 KA tariff positions.
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

impl PgPreisblattKaRepository {
    /// Upsert a `PreisblattKonzessionsabgabe` for the given NB MP-ID.
    ///
    /// Conflicts on `(nb_mp_id, sparte, kundengruppe_ka, valid_from)` are updated in-place.
    /// `Api`-sourced sheets are never overwritten by `Mako` ingests.
    pub async fn upsert_ka(
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
        .map_err(super::write_error)?;
        Ok(())
    }

    /// Return the `PreisblattKonzessionsabgabe` valid on `billing_date` for the NB.
    ///
    /// Returns `None` when no matching entry is found.
    pub async fn find_ka_for_date(
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
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
                source: source_str.parse().unwrap_or(PreisblattSource::Api),
                created_at: r.get("created_at"),
                updated_at: r.get("updated_at"),
            }
        }))
    }
}

// ── PreisblattDienstleistung ──────────────────────────────────────────────────

/// Read/write access to MSB service price sheets.
///
/// Used by `invoic-checker` for INVOIC 31009 service position validation
/// and by `mako-wim` REQOTE/QUOTES (PIDs 35001/35002/35004/35005).
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

impl PgPreisblattDienstleistungRepository {
    pub async fn upsert_dienstleistung(
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
        .execute(&self.pool).await.map_err(super::write_error)?;
        Ok(())
    }

    pub async fn find_dienstleistung_for_date(
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
                .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
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

/// Read/write access to MSB hardware rental price sheets.
///
/// Required for NB → MSB settlement INVOIC 31009 hardware positions.
/// `invoic-checker` check 5 cannot validate hardware positions without it.
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

impl PgPreisblattHardwareRepository {
    pub async fn upsert_hardware(
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
        .execute(&self.pool).await.map_err(super::write_error)?;
        Ok(())
    }

    pub async fn find_hardware_for_date(
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
                .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
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
