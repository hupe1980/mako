//! PostgreSQL repository for the §20b EnWG Netzzugangsplattform request
//! registry (`netzzugang_antraege`).
//!
//! The stored row is the shared [`NetzzugangAntrag`] plus an optimistic-locking
//! `version` (incremented on every successful write, checked on status
//! transitions when the caller supplies its last-read version).

use mako_markt::{
    error::MdmError,
    repository::{NetzzugangAktion, NetzzugangAntrag, NetzzugangAntragTyp, NetzzugangStatus},
};
use serde::Serialize;
use sqlx::{PgPool, Row, postgres::PgRow};
use uuid::Uuid;

/// A stored §20b request: the shared [`NetzzugangAntrag`] plus the registry's
/// optimistic-locking `version`.
#[derive(Debug, Clone, Serialize)]
pub struct NetzzugangAntragRow {
    #[serde(flatten)]
    pub antrag: NetzzugangAntrag,
    /// Optimistic-locking version; incremented on every successful write.
    pub version: i64,
}

/// PostgreSQL-backed §20b request registry.
#[derive(Clone, Debug)]
pub struct PgNetzzugangRepository {
    pool: PgPool,
}

fn typ_from_str(s: &str) -> Result<NetzzugangAntragTyp, sqlx::Error> {
    match s {
        "zaehlpunktanordnung" => Ok(NetzzugangAntragTyp::Zaehlpunktanordnung),
        "verrechnungskonzept" => Ok(NetzzugangAntragTyp::Verrechnungskonzept),
        "energysharing_vereinbarung" => Ok(NetzzugangAntragTyp::EnergySharingVereinbarung),
        other => Err(sqlx::Error::Decode(
            format!("unknown antrag_typ {other}").into(),
        )),
    }
}

fn aktion_from_str(s: &str) -> Result<NetzzugangAktion, sqlx::Error> {
    match s {
        "bestellung" => Ok(NetzzugangAktion::Bestellung),
        "aenderung" => Ok(NetzzugangAktion::Aenderung),
        "abbestellung" => Ok(NetzzugangAktion::Abbestellung),
        "registrierung" => Ok(NetzzugangAktion::Registrierung),
        other => Err(sqlx::Error::Decode(
            format!("unknown aktion {other}").into(),
        )),
    }
}

fn status_from_str(s: &str) -> Result<NetzzugangStatus, sqlx::Error> {
    match s {
        "erfasst" => Ok(NetzzugangStatus::Erfasst),
        "uebermittelt" => Ok(NetzzugangStatus::Uebermittelt),
        "bestaetigt" => Ok(NetzzugangStatus::Bestaetigt),
        "abgelehnt" => Ok(NetzzugangStatus::Abgelehnt),
        "fehlgeschlagen" => Ok(NetzzugangStatus::Fehlgeschlagen),
        other => Err(sqlx::Error::Decode(
            format!("unknown status {other}").into(),
        )),
    }
}

fn map_antrag(row: &PgRow) -> Result<NetzzugangAntragRow, sqlx::Error> {
    Ok(NetzzugangAntragRow {
        antrag: NetzzugangAntrag {
            id: row.try_get("id")?,
            tenant: row.try_get("tenant")?,
            antrag_typ: typ_from_str(row.try_get::<String, _>("antrag_typ")?.as_str())?,
            aktion: aktion_from_str(row.try_get::<String, _>("aktion")?.as_str())?,
            netzanschluss_id: row.try_get("netzanschluss_id")?,
            nb_mp_id: row.try_get("nb_mp_id")?,
            antragsteller_ref: row.try_get("antragsteller_ref")?,
            status: status_from_str(row.try_get::<String, _>("status")?.as_str())?,
            payload: row.try_get("payload")?,
            platform_ref: row.try_get("platform_ref").unwrap_or(None),
            created_at: row.try_get("created_at")?,
            submitted_at: row.try_get("submitted_at").unwrap_or(None),
        },
        version: row.try_get("version")?,
    })
}

const SELECT_COLS: &str = "id, tenant, antrag_typ, aktion, netzanschluss_id, nb_mp_id, \
                           antragsteller_ref, status, payload, platform_ref, version, \
                           created_at, submitted_at";

impl PgNetzzugangRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Insert or update a request by id (tenant-scoped).
    ///
    /// A caller-supplied `created_at` is persisted on insert (the serde
    /// epoch default falls back to `now()`); on update it stays untouched.
    /// Returns the id and the new optimistic-locking version.
    pub async fn upsert(&self, rec: &NetzzugangAntrag) -> Result<(Uuid, i64), MdmError> {
        let id = if rec.id == Uuid::nil() {
            Uuid::new_v4()
        } else {
            rec.id
        };
        // `NetzzugangAntrag::created_at` serde-defaults to the Unix epoch when
        // the caller omits it — treat that sentinel as "no value supplied".
        let created_at = if rec.created_at == time::OffsetDateTime::UNIX_EPOCH {
            time::OffsetDateTime::now_utc()
        } else {
            rec.created_at
        };
        let row = sqlx::query(
            "INSERT INTO netzzugang_antraege \
               (id, tenant, antrag_typ, aktion, netzanschluss_id, nb_mp_id, \
                antragsteller_ref, status, payload, platform_ref, created_at, \
                submitted_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (id) DO UPDATE SET \
               status       = EXCLUDED.status, \
               payload      = EXCLUDED.payload, \
               platform_ref = COALESCE(EXCLUDED.platform_ref, netzzugang_antraege.platform_ref), \
               submitted_at = COALESCE(EXCLUDED.submitted_at, netzzugang_antraege.submitted_at), \
               version      = netzzugang_antraege.version + 1, \
               updated_at   = now() \
             RETURNING id, version",
        )
        .bind(id)
        .bind(&rec.tenant)
        .bind(rec.antrag_typ.as_str())
        .bind(rec.aktion.as_str())
        .bind(&rec.netzanschluss_id)
        .bind(&rec.nb_mp_id)
        .bind(&rec.antragsteller_ref)
        .bind(rec.status.as_str())
        .bind(&rec.payload)
        .bind(&rec.platform_ref)
        .bind(created_at)
        .bind(rec.submitted_at)
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

    /// Fetch a request by id (tenant-scoped).
    pub async fn get(
        &self,
        tenant: &str,
        id: Uuid,
    ) -> Result<Option<NetzzugangAntragRow>, MdmError> {
        let row = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM netzzugang_antraege WHERE tenant = $1 AND id = $2"
        ))
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        row.map(|r| map_antrag(&r))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// List requests, optionally filtered by status and/or Netzanschluss.
    pub async fn list(
        &self,
        tenant: &str,
        status: Option<NetzzugangStatus>,
        netzanschluss_id: Option<&str>,
    ) -> Result<Vec<NetzzugangAntragRow>, MdmError> {
        let rows = sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM netzzugang_antraege \
             WHERE tenant = $1 \
               AND ($2::text IS NULL OR status = $2) \
               AND ($3::text IS NULL OR netzanschluss_id = $3) \
             ORDER BY created_at DESC \
             LIMIT 500"
        ))
        .bind(tenant)
        .bind(status.map(NetzzugangStatus::as_str))
        .bind(netzanschluss_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        rows.iter()
            .map(|r| map_antrag(r).map_err(|e| MdmError::Internal(e.to_string())))
            .collect()
    }

    /// Update lifecycle state (and optionally the platform reference).
    ///
    /// When `expected_version` is supplied the update only proceeds if it
    /// matches the stored version ([`MdmError::VersionConflict`] otherwise).
    /// Returns the updated record when it existed.
    pub async fn set_status(
        &self,
        tenant: &str,
        id: Uuid,
        status: NetzzugangStatus,
        platform_ref: Option<String>,
        expected_version: Option<i64>,
    ) -> Result<Option<NetzzugangAntragRow>, MdmError> {
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version FROM netzzugang_antraege WHERE tenant = $1 AND id = $2",
        )
        .bind(tenant)
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        match (current, expected_version) {
            (Some(actual), Some(expected)) if actual != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: actual.to_string(),
                });
            }
            (None, _) => return Ok(None),
            (Some(_), _) => {}
        }

        let row = sqlx::query(&format!(
            "UPDATE netzzugang_antraege SET \
               status       = $3, \
               platform_ref = COALESCE($4, platform_ref), \
               submitted_at = CASE WHEN $3 = 'uebermittelt' \
                                   THEN COALESCE(submitted_at, now()) \
                                   ELSE submitted_at END, \
               version      = version + 1, \
               updated_at   = now() \
             WHERE tenant = $1 AND id = $2 \
             RETURNING {SELECT_COLS}"
        ))
        .bind(tenant)
        .bind(id)
        .bind(status.as_str())
        .bind(platform_ref)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        row.map(|r| map_antrag(&r))
            .transpose()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }
}
