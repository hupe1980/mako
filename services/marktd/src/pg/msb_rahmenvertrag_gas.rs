//! PostgreSQL repository for the Gas MSB-Rahmenvertrag registry
//! (`msb_rahmenvertraege_gas`).
//!
//! GeLi Gas 3.0 (BK7-24-01-009, Tenor Ziff. 13–16): the BNetzA-imposed
//! BK7-17-026 contract is revoked effective 01.10.2026 and replaced by the
//! market-developed KoV XV Anlage 8 in its jeweils gültige Fassung. This
//! registry tracks per-(GNB, MSB) conclusion state, including the migration
//! duty for legacy contracts.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

use mako_markt::error::MdmError;

/// Lifecycle state of a Gas MSB framework contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum MsbRvGasStatus {
    /// Published/offered by the GNB; not yet concluded.
    Angeboten,
    /// Concluded between GNB and MSB.
    Abgeschlossen,
    /// Legacy BK7-17-026 contract that must migrate to the KoV XV Fassung
    /// by 01.10.2026.
    AnpassungErforderlich,
    /// Ended.
    Beendet,
}

impl MsbRvGasStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Angeboten => "angeboten",
            Self::Abgeschlossen => "abgeschlossen",
            Self::AnpassungErforderlich => "anpassung_erforderlich",
            Self::Beendet => "beendet",
        }
    }
}

fn status_from_str(s: &str) -> Result<MsbRvGasStatus, sqlx::Error> {
    match s {
        "angeboten" => Ok(MsbRvGasStatus::Angeboten),
        "abgeschlossen" => Ok(MsbRvGasStatus::Abgeschlossen),
        "anpassung_erforderlich" => Ok(MsbRvGasStatus::AnpassungErforderlich),
        "beendet" => Ok(MsbRvGasStatus::Beendet),
        other => Err(sqlx::Error::Decode(
            format!("unknown status {other}").into(),
        )),
    }
}

/// A Gas MSB framework-contract record.
///
/// The natural key is `(tenant, gnb_mp_id, msb_mp_id, valid_from)` — upserts
/// are idempotent on it and keep the `id` stable.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MsbRahmenvertragGas {
    #[serde(default)]
    pub id: Uuid,
    #[serde(default)]
    pub tenant: String,
    pub gnb_mp_id: String,
    pub msb_mp_id: String,
    /// Contract text edition, e.g. `KoV XV Anlage 8` (legacy: `BK7-17-026`).
    #[serde(default = "default_fassung")]
    pub fassung: String,
    #[serde(default = "default_status")]
    pub status: MsbRvGasStatus,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub signed_at: Option<time::OffsetDateTime>,
    pub valid_from: Date,
    #[serde(default)]
    pub valid_to: Option<Date>,
    /// Full BO4E `Vertrag` payload (vertragsart `RAHMENVERTRAG`).
    #[serde(default)]
    pub vertrag: serde_json::Value,
    /// Optimistic-locking version. Incremented on every successful write; on
    /// PUT, supply the last-read version to guard against lost updates
    /// (absent/`0` skips the check).
    #[serde(default)]
    pub version: i64,
}

fn default_fassung() -> String {
    "KoV XV Anlage 8".to_owned()
}

const fn default_status() -> MsbRvGasStatus {
    MsbRvGasStatus::Angeboten
}

fn map_row(row: &PgRow) -> Result<MsbRahmenvertragGas, sqlx::Error> {
    Ok(MsbRahmenvertragGas {
        id: row.try_get("id")?,
        tenant: row.try_get("tenant")?,
        gnb_mp_id: row.try_get("gnb_mp_id")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        fassung: row.try_get("fassung")?,
        status: status_from_str(row.try_get::<String, _>("status")?.as_str())?,
        signed_at: row.try_get("signed_at").unwrap_or(None),
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to").unwrap_or(None),
        vertrag: row.try_get("vertrag")?,
        version: row.try_get("version")?,
    })
}

const SELECT_COLS: &str = "id, tenant, gnb_mp_id, msb_mp_id, fassung, status, signed_at, \
                           valid_from, valid_to, vertrag, version";

/// PostgreSQL-backed Gas MSB framework-contract registry.
#[derive(Clone, Debug)]
pub struct PgMsbRahmenvertragGasRepository {
    pool: PgPool,
}

impl PgMsbRahmenvertragGasRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or update a record by its natural key
    /// `(tenant, gnb_mp_id, msb_mp_id, valid_from)` — re-submitting the same
    /// business key is an idempotent update and keeps the `id` stable.
    ///
    /// Optimistic locking (same contract as the MeLo repository): when the
    /// caller supplies a non-zero `version`, the write only proceeds if it
    /// matches the stored version and fails with
    /// [`MdmError::VersionConflict`] otherwise. The version increments on
    /// every successful write.
    ///
    /// Returns the stable id and the new version.
    pub async fn upsert(&self, rec: &MsbRahmenvertragGas) -> Result<(Uuid, i64), MdmError> {
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM msb_rahmenvertraege_gas \
             WHERE tenant = $1 AND gnb_mp_id = $2 AND msb_mp_id = $3 AND valid_from = $4",
        )
        .bind(&rec.tenant)
        .bind(&rec.gnb_mp_id)
        .bind(&rec.msb_mp_id)
        .bind(rec.valid_from)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        let expected = (rec.version > 0).then_some(rec.version);
        let new_version = match (current, expected) {
            (Some(actual), Some(expected)) if actual != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
            (Some(actual), _) => actual + 1,
            (None, _) => 1,
        };

        let insert_id = if rec.id == Uuid::nil() {
            Uuid::new_v4()
        } else {
            rec.id
        };
        let row = sqlx::query(
            "INSERT INTO msb_rahmenvertraege_gas \
               (id, tenant, gnb_mp_id, msb_mp_id, fassung, status, signed_at, \
                valid_from, valid_to, vertrag, version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
             ON CONFLICT (tenant, gnb_mp_id, msb_mp_id, valid_from) DO UPDATE SET \
               fassung    = EXCLUDED.fassung, \
               status     = EXCLUDED.status, \
               signed_at  = COALESCE(EXCLUDED.signed_at, msb_rahmenvertraege_gas.signed_at), \
               valid_to   = EXCLUDED.valid_to, \
               vertrag    = EXCLUDED.vertrag, \
               version    = EXCLUDED.version, \
               updated_at = now() \
             RETURNING id, version",
        )
        .bind(insert_id)
        .bind(&rec.tenant)
        .bind(&rec.gnb_mp_id)
        .bind(&rec.msb_mp_id)
        .bind(&rec.fassung)
        .bind(rec.status.as_str())
        .bind(rec.signed_at)
        .bind(rec.valid_from)
        .bind(rec.valid_to)
        .bind(&rec.vertrag)
        .bind(new_version)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        let id: Uuid = row
            .try_get("id")
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        let version: i64 = row
            .try_get("version")
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok((id, version))
    }

    /// Fetch by id (tenant-scoped).
    pub async fn get(
        &self,
        tenant: &str,
        id: Uuid,
    ) -> Result<Option<MsbRahmenvertragGas>, MdmError> {
        let row = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM msb_rahmenvertraege_gas WHERE tenant = $1 AND id = $2"
        ))
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        row.map(|r| map_row(&r))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// List records, optionally filtered by MSB and/or status.
    pub async fn list(
        &self,
        tenant: &str,
        msb_mp_id: Option<&str>,
        status: Option<MsbRvGasStatus>,
    ) -> Result<Vec<MsbRahmenvertragGas>, MdmError> {
        let rows = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM msb_rahmenvertraege_gas \
             WHERE tenant = $1 \
               AND ($2::text IS NULL OR msb_mp_id = $2) \
               AND ($3::text IS NULL OR status = $3) \
             ORDER BY valid_from DESC \
             LIMIT 500"
        ))
        .bind(tenant)
        .bind(msb_mp_id)
        .bind(status.map(MsbRvGasStatus::as_str))
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(|r| map_row(r).map_err(|e| MdmError::Internal(e.to_string())))
            .collect()
    }
}
