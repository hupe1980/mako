//! PostgreSQL implementations of [`SteuerbareRessourceRepository`],
//! [`TechnischeRessourceRepository`], and [`DeviceRepository`].

use mako_markt::{
    error::MdmError,
    repository::{
        DeviceRepository, GeraetKonfiguration, GeraetRecord, SteuerbareRessourceRecord,
        SteuerbareRessourceRepository, TechnischeRessourceRecord, TechnischeRessourceRepository,
        ZaehlerRecord,
    },
};
use sqlx::{PgPool, Row};

// ── SteuerbareRessource ───────────────────────────────────────────────────────

/// PostgreSQL-backed [`SteuerbareRessourceRepository`].
#[derive(Clone, Debug)]
pub struct PgSteuerbareRessourceRepository {
    pool: PgPool,
}

impl PgSteuerbareRessourceRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SteuerbareRessourceRepository for PgSteuerbareRessourceRepository {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_sr(
        &self,
        sr_id: &str,
        tenant: &str,
        malo_id: Option<&str>,
        melo_id: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
        konfigurationsprodukte: Option<serde_json::Value>,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO steuerbare_ressourcen
                  (sr_id, tenant, malo_id, melo_id, data, bo4e_version, konfigurationsprodukte, version, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now())
              ON CONFLICT (sr_id, tenant) DO UPDATE
              SET malo_id                 = COALESCE(EXCLUDED.malo_id, steuerbare_ressourcen.malo_id),
                  melo_id                 = COALESCE(EXCLUDED.melo_id, steuerbare_ressourcen.melo_id),
                  data                    = EXCLUDED.data,
                  bo4e_version            = EXCLUDED.bo4e_version,
                  konfigurationsprodukte  = COALESCE(EXCLUDED.konfigurationsprodukte,
                                                     steuerbare_ressourcen.konfigurationsprodukte),
                  version                 = steuerbare_ressourcen.version + 1,
                  updated_at              = now()",
        )
        .bind(sr_id)
        .bind(tenant)
        .bind(malo_id)
        .bind(melo_id)
        .bind(&data)
        .bind(bo4e_version)
        .bind(konfigurationsprodukte.as_ref())
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_sr(
        &self,
        sr_id: &str,
        tenant: &str,
    ) -> Result<Option<SteuerbareRessourceRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT sr_id, tenant, malo_id, melo_id, data, konfigurationsprodukte, bo4e_version, version, updated_at
              FROM steuerbare_ressourcen
              WHERE sr_id = $1 AND tenant = $2",
        )
        .bind(sr_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| SteuerbareRessourceRecord {
            sr_id: r.get("sr_id"),
            tenant: r.get("tenant"),
            malo_id: r.try_get("malo_id").unwrap_or(None),
            melo_id: r.try_get("melo_id").unwrap_or(None),
            data: r.get("data"),
            konfigurationsprodukte: r.try_get("konfigurationsprodukte").unwrap_or(None),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
            version: r.try_get("version").unwrap_or(1),
            updated_at: r.get("updated_at"),
        }))
    }

    async fn list_sr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<SteuerbareRessourceRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT sr_id, tenant, malo_id, melo_id, data, konfigurationsprodukte, bo4e_version, version, updated_at
              FROM steuerbare_ressourcen
              WHERE malo_id = $1 AND tenant = $2
              ORDER BY updated_at DESC",
        )
        .bind(malo_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| SteuerbareRessourceRecord {
                sr_id: r.get("sr_id"),
                tenant: r.get("tenant"),
                malo_id: r.try_get("malo_id").unwrap_or(None),
                melo_id: r.try_get("melo_id").unwrap_or(None),
                data: r.get("data"),
                konfigurationsprodukte: r.try_get("konfigurationsprodukte").unwrap_or(None),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    async fn replace_sr_konfigurationsprodukte(
        &self,
        sr_id: &str,
        tenant: &str,
        konfigurationsprodukte: serde_json::Value,
    ) -> Result<bool, MdmError> {
        let updated = sqlx::query_scalar::<_, i64>(
            r"UPDATE steuerbare_ressourcen
              SET konfigurationsprodukte = $3,
                  version                = version + 1,
                  updated_at             = now()
              WHERE sr_id = $1 AND tenant = $2
              RETURNING 1",
        )
        .bind(sr_id)
        .bind(tenant)
        .bind(&konfigurationsprodukte)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(updated.is_some())
    }
}

// ── Device registry (Zaehler + Geraete) ──────────────────────────────────────

/// PostgreSQL-backed [`DeviceRepository`].
#[derive(Clone, Debug)]
pub struct PgDeviceRepository {
    pool: PgPool,
}

impl PgDeviceRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl DeviceRepository for PgDeviceRepository {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
        melo_id: &str,
        zaehler_typ: Option<&str>,
        eichung_bis: Option<time::Date>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO zaehler
                  (zaehler_id, tenant, melo_id, zaehler_typ, eichung_bis, data, bo4e_version, version, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, 1, now())
              ON CONFLICT (zaehler_id, tenant) DO UPDATE
              SET melo_id      = EXCLUDED.melo_id,
                  zaehler_typ  = COALESCE(EXCLUDED.zaehler_typ, zaehler.zaehler_typ),
                  eichung_bis  = COALESCE(EXCLUDED.eichung_bis, zaehler.eichung_bis),
                  data         = EXCLUDED.data,
                  bo4e_version = EXCLUDED.bo4e_version,
                  version      = zaehler.version + 1,
                  updated_at   = now()",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .bind(melo_id)
        .bind(zaehler_typ)
        .bind(eichung_bis)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_zaehler_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<ZaehlerRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT zaehler_id, tenant, melo_id, zaehler_typ, eichung_bis, data, bo4e_version, version, updated_at
              FROM zaehler
              WHERE melo_id = $1 AND tenant = $2
              ORDER BY updated_at DESC",
        )
        .bind(melo_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_zaehler).collect())
    }

    async fn find_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Option<ZaehlerRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT zaehler_id, tenant, melo_id, zaehler_typ, eichung_bis, data, bo4e_version, version, updated_at
              FROM zaehler
              WHERE zaehler_id = $1 AND tenant = $2",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_zaehler))
    }

    async fn upsert_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
        zaehler_id: &str,
        geraet_typ: Option<&str>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO geraete
                  (geraet_id, tenant, zaehler_id, geraet_typ, data, bo4e_version, version, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, 1, now())
              ON CONFLICT (geraet_id, tenant) DO UPDATE
              SET zaehler_id   = EXCLUDED.zaehler_id,
                  geraet_typ   = COALESCE(EXCLUDED.geraet_typ, geraete.geraet_typ),
                  data         = EXCLUDED.data,
                  bo4e_version = EXCLUDED.bo4e_version,
                  version      = geraete.version + 1,
                  updated_at   = now()",
        )
        .bind(geraet_id)
        .bind(tenant)
        .bind(zaehler_id)
        .bind(geraet_typ)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn list_geraete_by_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
    ) -> Result<Vec<GeraetRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT geraet_id, tenant, zaehler_id, geraet_typ, data, geraet_konfigurationen, bo4e_version, version, updated_at
              FROM geraete
              WHERE zaehler_id = $1 AND tenant = $2
              ORDER BY updated_at DESC",
        )
        .bind(zaehler_id)
        .bind(tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows.into_iter().map(row_to_geraet).collect())
    }

    async fn find_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
    ) -> Result<Option<GeraetRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT geraet_id, tenant, zaehler_id, geraet_typ, data, geraet_konfigurationen, bo4e_version, version, updated_at
              FROM geraete
              WHERE geraet_id = $1 AND tenant = $2",
        )
        .bind(geraet_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_geraet))
    }

    async fn upsert_geraet_konfigurationen(
        &self,
        geraet_id: &str,
        tenant: &str,
        konfigurationen: Vec<GeraetKonfiguration>,
    ) -> Result<bool, MdmError> {
        let now = time::OffsetDateTime::now_utc();
        // Set server-side updated_at on every entry (caller value is ignored).
        let timestamped: Vec<GeraetKonfiguration> = konfigurationen
            .into_iter()
            .map(|mut k| {
                k.updated_at = now;
                k
            })
            .collect();
        let json = serde_json::to_value(&timestamped)
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        let updated = sqlx::query(
            r"UPDATE geraete
              SET geraet_konfigurationen = $1,
                  version    = version + 1,
                  updated_at = now()
              WHERE geraet_id = $2 AND tenant = $3
              RETURNING geraet_id",
        )
        .bind(&json)
        .bind(geraet_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(updated.is_some())
    }
}

// ── Row helpers ───────────────────────────────────────────────────────────────

fn row_to_zaehler(r: sqlx::postgres::PgRow) -> ZaehlerRecord {
    ZaehlerRecord {
        zaehler_id: r.get("zaehler_id"),
        tenant: r.get("tenant"),
        melo_id: r.get("melo_id"),
        zaehler_typ: r.try_get("zaehler_typ").unwrap_or(None),
        eichung_bis: r.try_get("eichung_bis").unwrap_or(None),
        data: r.get("data"),
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| "v202607.0.0".to_owned()),
        version: r.try_get("version").unwrap_or(1),
        updated_at: r.get("updated_at"),
    }
}

fn row_to_geraet(r: sqlx::postgres::PgRow) -> GeraetRecord {
    // Deserialize the JSONB konfigurationen column.  Falls back to empty vec on
    // schema drift so legacy rows without the column don't break the API.
    let konfigurationen: Vec<GeraetKonfiguration> = r
        .try_get::<serde_json::Value, _>("geraet_konfigurationen")
        .ok()
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    GeraetRecord {
        geraet_id: r.get("geraet_id"),
        tenant: r.get("tenant"),
        zaehler_id: r.get("zaehler_id"),
        geraet_typ: r.try_get("geraet_typ").unwrap_or(None),
        data: r.get("data"),
        konfigurationen,
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| "v202607.0.0".to_owned()),
        version: r.try_get("version").unwrap_or(1),
        updated_at: r.get("updated_at"),
    }
}

// ── TechnischeRessource (B9) ─────────────────────────────────────────────────

/// PostgreSQL-backed [`TechnischeRessourceRepository`].
#[derive(Clone, Debug)]
pub struct PgTechnischeRessourceRepository {
    pool: PgPool,
}

impl PgTechnischeRessourceRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl TechnischeRessourceRepository for PgTechnischeRessourceRepository {
    #[allow(clippy::too_many_arguments)]
    async fn upsert_tr(
        &self,
        tr_id: &str,
        tenant: &str,
        malo_id: Option<&str>,
        melo_id: Option<&str>,
        tr_typ: Option<&str>,
        ist_fernschaltbar: Option<bool>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO technische_ressourcen
                  (tr_id, tenant, malo_id, melo_id, tr_typ, ist_fernschaltbar,
                   data, bo4e_version, version, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 1, now())
              ON CONFLICT (tr_id, tenant) DO UPDATE
              SET malo_id           = COALESCE(EXCLUDED.malo_id, technische_ressourcen.malo_id),
                  melo_id           = COALESCE(EXCLUDED.melo_id, technische_ressourcen.melo_id),
                  tr_typ            = COALESCE(EXCLUDED.tr_typ, technische_ressourcen.tr_typ),
                  ist_fernschaltbar = COALESCE(EXCLUDED.ist_fernschaltbar, technische_ressourcen.ist_fernschaltbar),
                  data              = EXCLUDED.data,
                  bo4e_version      = EXCLUDED.bo4e_version,
                  version           = technische_ressourcen.version + 1,
                  updated_at        = now()",
        )
        .bind(tr_id)
        .bind(tenant)
        .bind(malo_id)
        .bind(melo_id)
        .bind(tr_typ)
        .bind(ist_fernschaltbar)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_tr(
        &self,
        tr_id: &str,
        tenant: &str,
    ) -> Result<Option<TechnischeRessourceRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, tr_typ, ist_fernschaltbar,
                     data, bo4e_version, version, updated_at
              FROM technische_ressourcen
              WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(tr_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| TechnischeRessourceRecord {
            tr_id: r.get("tr_id"),
            tenant: r.get("tenant"),
            malo_id: r.try_get("malo_id").unwrap_or(None),
            melo_id: r.try_get("melo_id").unwrap_or(None),
            tr_typ: r.try_get("tr_typ").unwrap_or(None),
            ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
            data: r.get("data"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
            version: r.try_get("version").unwrap_or(1),
            updated_at: r.get("updated_at"),
        }))
    }

    async fn list_tr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, tr_typ, ist_fernschaltbar,
                     data, bo4e_version, version, updated_at
              FROM technische_ressourcen
              WHERE tenant = $1 AND malo_id = $2",
        )
        .bind(tenant)
        .bind(malo_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| TechnischeRessourceRecord {
                tr_id: r.get("tr_id"),
                tenant: r.get("tenant"),
                malo_id: r.try_get("malo_id").unwrap_or(None),
                melo_id: r.try_get("melo_id").unwrap_or(None),
                tr_typ: r.try_get("tr_typ").unwrap_or(None),
                ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    async fn list_tr_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, tr_typ, ist_fernschaltbar,
                     data, bo4e_version, version, updated_at
              FROM technische_ressourcen
              WHERE tenant = $1 AND melo_id = $2",
        )
        .bind(tenant)
        .bind(melo_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| TechnischeRessourceRecord {
                tr_id: r.get("tr_id"),
                tenant: r.get("tenant"),
                malo_id: r.try_get("malo_id").unwrap_or(None),
                melo_id: r.try_get("melo_id").unwrap_or(None),
                tr_typ: r.try_get("tr_typ").unwrap_or(None),
                ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| "v202607.0.0".to_owned()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }
}
