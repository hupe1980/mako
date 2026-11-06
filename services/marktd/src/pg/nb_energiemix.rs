//! PostgreSQL storage: [`PgNbEnergiemixRepository`] — the annual grid-area
//! Energiemix a Netzbetreiber publishes (§ 42 EnWG), one row per
//! `(tenant, nb_mp_id, gueltig_fuer)`.

use mako_markt::error::MdmError;
use sqlx::{PgPool, Row, postgres::PgRow};
use time::OffsetDateTime;

/// One stored year of a Netzbetreiber's Energiemix.
#[derive(Debug, Clone, PartialEq)]
pub struct NbEnergiemixRecord {
    /// Tenant identifier — data-isolation key written to every database row.
    /// Typically the operator's BDEW- or DVGW-Codenummer, but any stable
    /// unique string is valid.
    pub tenant: String,
    /// The Netzbetreiber's MP-ID.
    pub nb_mp_id: String,
    /// Calendar year the mix is valid for.
    pub gueltig_fuer: i16,
    /// The BO4E `Energiemix` COM, canonical camelCase JSON as stored.
    pub energiemix: serde_json::Value,
    /// Total EEG feed-in into the grid area, kWh.
    pub eeg_einspeisung_kwh: Option<i64>,
    /// Total grid withdrawal (Gesamtentnahme), kWh.
    pub gesamtentnahme_kwh: Option<i64>,
    /// When the row was last written.
    pub updated_at: OffsetDateTime,
}

/// What a `PUT` writes for one year.
#[derive(Debug, Clone)]
pub struct NbEnergiemixWrite<'a> {
    /// Tenant identifier — data-isolation key written to every database row.
    pub tenant: &'a str,
    /// The Netzbetreiber's MP-ID.
    pub nb_mp_id: &'a str,
    /// Calendar year the mix is valid for.
    pub gueltig_fuer: i16,
    /// The BO4E `Energiemix` COM, canonical camelCase JSON.
    pub energiemix: &'a serde_json::Value,
    /// Total EEG feed-in, kWh. `None` keeps the stored figure.
    pub eeg_einspeisung_kwh: Option<i64>,
    /// Total grid withdrawal, kWh. `None` keeps the stored figure.
    pub gesamtentnahme_kwh: Option<i64>,
}

/// The `nb_energiemix` table.
#[derive(Clone, Debug)]
pub struct PgNbEnergiemixRepository {
    pool: PgPool,
}

fn map_row(row: &PgRow) -> Result<NbEnergiemixRecord, sqlx::Error> {
    Ok(NbEnergiemixRecord {
        tenant: row.try_get("tenant")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        gueltig_fuer: row.try_get("gueltig_fuer")?,
        energiemix: row.try_get("energiemix")?,
        eeg_einspeisung_kwh: row.try_get("eeg_einspeisung_kwh")?,
        gesamtentnahme_kwh: row.try_get("gesamtentnahme_kwh")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn internal(e: sqlx::Error) -> MdmError {
    MdmError::Internal(e.to_string())
}

impl PgNbEnergiemixRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Store one year's mix, replacing the stored mix for that year.
    ///
    /// A kWh figure the write leaves out keeps the stored one: the mix is
    /// republished more often than the annual totals are known.
    pub async fn upsert(&self, w: &NbEnergiemixWrite<'_>) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO nb_energiemix
                  (nb_mp_id, tenant, gueltig_fuer, energiemix,
                   eeg_einspeisung_kwh, gesamtentnahme_kwh, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, now())
              ON CONFLICT (tenant, nb_mp_id, gueltig_fuer) DO UPDATE
              SET energiemix          = EXCLUDED.energiemix,
                  eeg_einspeisung_kwh = COALESCE(EXCLUDED.eeg_einspeisung_kwh, nb_energiemix.eeg_einspeisung_kwh),
                  gesamtentnahme_kwh  = COALESCE(EXCLUDED.gesamtentnahme_kwh,  nb_energiemix.gesamtentnahme_kwh),
                  updated_at          = now()",
        )
        .bind(w.nb_mp_id)
        .bind(w.tenant)
        .bind(w.gueltig_fuer)
        .bind(w.energiemix)
        .bind(w.eeg_einspeisung_kwh)
        .bind(w.gesamtentnahme_kwh)
        .execute(&self.pool)
        .await
        .map_err(super::write_error)?;
        Ok(())
    }

    /// The mix for `year`, or the most recent year when `year` is `None`.
    pub async fn find(
        &self,
        tenant: &str,
        nb_mp_id: &str,
        year: Option<i16>,
    ) -> Result<Option<NbEnergiemixRecord>, MdmError> {
        // The two kWh figures are `NUMERIC(18, 0)`, which does not decode as
        // an `i64`; the cast is exact for eighteen digits.
        let row = sqlx::query(
            r"SELECT tenant, nb_mp_id, gueltig_fuer, energiemix,
                    eeg_einspeisung_kwh::int8 AS eeg_einspeisung_kwh,
                    gesamtentnahme_kwh::int8 AS gesamtentnahme_kwh, updated_at
              FROM nb_energiemix
              WHERE tenant = $1 AND nb_mp_id = $2
                AND ($3::smallint IS NULL OR gueltig_fuer = $3)
              ORDER BY gueltig_fuer DESC
              LIMIT 1",
        )
        .bind(tenant)
        .bind(nb_mp_id)
        .bind(year)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;
        row.as_ref().map(map_row).transpose().map_err(internal)
    }

    /// Every stored year, newest first.
    pub async fn history(
        &self,
        tenant: &str,
        nb_mp_id: &str,
    ) -> Result<Vec<NbEnergiemixRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tenant, nb_mp_id, gueltig_fuer, energiemix,
                    eeg_einspeisung_kwh::int8 AS eeg_einspeisung_kwh,
                    gesamtentnahme_kwh::int8 AS gesamtentnahme_kwh, updated_at
              FROM nb_energiemix
              WHERE tenant = $1 AND nb_mp_id = $2
              ORDER BY gueltig_fuer DESC",
        )
        .bind(tenant)
        .bind(nb_mp_id)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;
        rows.iter()
            .map(map_row)
            .collect::<Result<_, _>>()
            .map_err(internal)
    }
}
