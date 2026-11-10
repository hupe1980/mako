//! PostgreSQL storage: [`PgSteuerbareRessourceRepository`],
//! [`PgTechnischeRessourceRepository`], and [`PgDeviceRepository`].

use mako_markt::{
    bo4e::ZaehlerShadowColumns,
    error::MdmError,
    repository::{
        GeraetKonfiguration, GeraetRecord, SteuerbareRessourceRecord, TechnischeRessourceRecord,
        ZaehlerRecord,
    },
};
use sqlx::{PgPool, Row};

// ── SteuerbareRessource ───────────────────────────────────────────────────────

/// Persistent store for `SteuerbareRessource` registrations.
///
/// Populated by the WiM iMS Steuerungsauftrag process (PID 55168)
/// and by operator REST uploads.
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

impl PgSteuerbareRessourceRepository {
    /// Upsert a `SteuerbareRessource` for the given `sr_id` + tenant.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
    pub async fn upsert_sr(
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

    /// Return the `SteuerbareRessource` for `sr_id`, or `None` if not found.
    pub async fn find_sr(
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
                .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
            version: r.try_get("version").unwrap_or(1),
            updated_at: r.get("updated_at"),
        }))
    }

    /// Return all `SteuerbareRessource` records for a MaLo.
    pub async fn list_sr_by_malo(
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
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// Lock an SR's row on the caller's transaction and return its
    /// `konfigurationsprodukte`: `None` when the SR does not exist, `Some(None)`
    /// when it carries no list.
    pub async fn lock_konfigurationsprodukte_tx(
        conn: &mut sqlx::PgConnection,
        sr_id: &str,
        tenant: &str,
    ) -> Result<Option<Option<serde_json::Value>>, MdmError> {
        sqlx::query_scalar(
            r"SELECT konfigurationsprodukte FROM steuerbare_ressourcen
              WHERE sr_id = $1 AND tenant = $2
              FOR UPDATE",
        )
        .bind(sr_id)
        .bind(tenant)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// Replace the `konfigurationsprodukte` array for an existing SR.
    ///
    /// Returns `Ok(true)` when the SR was found and updated,
    /// `Ok(false)` when the SR does not exist (caller should return 404).
    pub async fn replace_sr_konfigurationsprodukte(
        &self,
        sr_id: &str,
        tenant: &str,
        konfigurationsprodukte: serde_json::Value,
    ) -> Result<bool, MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::replace_sr_konfigurationsprodukte_tx(&mut conn, sr_id, tenant, konfigurationsprodukte)
            .await
    }

    /// [`Self::replace_sr_konfigurationsprodukte`] on the caller's transaction.
    pub async fn replace_sr_konfigurationsprodukte_tx(
        conn: &mut sqlx::PgConnection,
        sr_id: &str,
        tenant: &str,
        konfigurationsprodukte: serde_json::Value,
    ) -> Result<bool, MdmError> {
        // **The document moves with the column.** `konfigurationsprodukte` is a
        // field `SteuerbareRessource` declares, so this column indexes the
        // stored document rather than standing beside it: writing only the
        // column would leave a contracted product absent from the
        // `SteuerbareRessource` a `GET` returns.
        //
        // `jsonb_set` with `create_missing = true`, so a document that has
        // never carried the key gains it.
        let affected = sqlx::query(
            r"UPDATE steuerbare_ressourcen
              SET konfigurationsprodukte = $3,
                  data                   = jsonb_set(
                                               data, '{konfigurationsprodukte}', $3::jsonb, true),
                  version                = version + 1,
                  updated_at             = now()
              WHERE sr_id = $1 AND tenant = $2",
        )
        .bind(sr_id)
        .bind(tenant)
        .bind(&konfigurationsprodukte)
        .execute(&mut *conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }
}

// ── Device registry (Zaehler + Geraete) ──────────────────────────────────────

/// Persistent store for Zähler (meters) and Geräte (devices).
///
/// Populated by WiM MSB/NB device handover processes (ORDERS PIDs 17001, 17002, 17009)
/// and operator REST uploads.
///
/// Source: WiM AHB BK6-24-174; BO4E Zaehler/Geraet schemas.
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

impl PgDeviceRepository {
    /// Upsert a `Zaehler` record.
    /// `zaehler_typ` and `eichung_bis` are **derived from `data`**
    /// ([`ZaehlerShadowColumns`]), not passed
    /// alongside it: `Zaehler` declares both, and asking for them twice let the
    /// column contradict the document it shadows.
    pub async fn upsert_zaehler(
        &self,
        zaehler_id: &str,
        tenant: &str,
        melo_id: &str,
        data: &rubo4e::current::Zaehler,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        let cols = ZaehlerShadowColumns::from_zaehler(data);
        let payload = serde_json::to_value(data)
            .map_err(|e| MdmError::Internal(format!("Zaehler is not serialisable: {e}")))?;
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
        .bind(cols.zaehler_typ)
        .bind(cols.eichung_bis)
        .bind(&payload)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Return all `Zaehler` for a given MeLo-ID.
    pub async fn list_zaehler_by_melo(
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

    /// Return the `Zaehler` for a given `zaehler_id`, or `None` if not found.
    pub async fn find_zaehler(
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

    /// Upsert a `Geraet` record.
    /// `geraet_typ` is derived from `data.geraetetyp`, for the same reason
    /// [`upsert_zaehler`](Self::upsert_zaehler) derives its columns.
    pub async fn upsert_geraet(
        &self,
        geraet_id: &str,
        tenant: &str,
        zaehler_id: &str,
        data: &rubo4e::current::Geraet,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        let typ = mako_markt::bo4e::geraet_typ(data);
        let payload = serde_json::to_value(data)
            .map_err(|e| MdmError::Internal(format!("Geraet is not serialisable: {e}")))?;
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
        .bind(typ)
        .bind(&payload)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    /// Return all `Geraete` for a given `zaehler_id`.
    pub async fn list_geraete_by_zaehler(
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

    /// Return a single `Geraet` by its `geraet_id`, or `None` if not found.
    pub async fn find_geraet(
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

    /// Atomically replace all `GeraetKonfiguration` entries for a `Geraet`.
    ///
    /// Returns `true` if the Geraet was found and updated, `false` if not found.
    ///
    /// The `updated_at` timestamp on each entry is set server-side; callers
    /// should not set it in the request (it is overwritten).
    ///
    /// Emits `de.markt.geraet.konfiguration.updated` via the durable fan-out.
    pub async fn upsert_geraet_konfigurationen(
        &self,
        geraet_id: &str,
        tenant: &str,
        konfigurationen: Vec<GeraetKonfiguration>,
    ) -> Result<bool, MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::upsert_geraet_konfigurationen_tx(&mut conn, geraet_id, tenant, konfigurationen).await
    }

    /// [`Self::upsert_geraet_konfigurationen`] on the caller's transaction.
    pub async fn upsert_geraet_konfigurationen_tx(
        conn: &mut sqlx::PgConnection,
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
        let json =
            serde_json::to_value(&timestamped).map_err(|e| MdmError::Internal(e.to_string()))?;

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
        .fetch_optional(&mut *conn)
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
            .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
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
            .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
        version: r.try_get("version").unwrap_or(1),
        updated_at: r.get("updated_at"),
    }
}

// ── TechnischeRessource (B9) ─────────────────────────────────────────────────

/// Persistent store for `TechnischeRessource` registrations.
///
/// Populated by Redispatch 2.0 registration processes and by operator REST
/// uploads.  Used by iMS E-mobility `Steuerungsauftrag` routing and flex-market
/// clearing.
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

impl PgTechnischeRessourceRepository {
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
    pub async fn upsert_tr(
        &self,
        tr_id: &str,
        tenant: &str,
        malo_id: Option<&str>,
        melo_id: Option<&str>,
        nutzung: Option<&str>,
        verbrauchsart: Option<&str>,
        ist_fernschaltbar: Option<bool>,
        data: serde_json::Value,
        bo4e_version: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r"INSERT INTO technische_ressourcen
                  (tr_id, tenant, malo_id, melo_id, nutzung, verbrauchsart, ist_fernschaltbar,
                   data, bo4e_version, version, updated_at)
              VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, now())
              ON CONFLICT (tr_id, tenant) DO UPDATE
              SET malo_id           = COALESCE(EXCLUDED.malo_id, technische_ressourcen.malo_id),
                  melo_id           = COALESCE(EXCLUDED.melo_id, technische_ressourcen.melo_id),
                  nutzung           = COALESCE(EXCLUDED.nutzung, technische_ressourcen.nutzung),
                  verbrauchsart     = COALESCE(EXCLUDED.verbrauchsart, technische_ressourcen.verbrauchsart),
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
        .bind(nutzung)
        .bind(verbrauchsart)
        .bind(ist_fernschaltbar)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    pub async fn find_tr(
        &self,
        tr_id: &str,
        tenant: &str,
    ) -> Result<Option<TechnischeRessourceRecord>, MdmError> {
        let row = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, nutzung, verbrauchsart, ist_fernschaltbar,
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
            nutzung: r.try_get("nutzung").unwrap_or(None),
            verbrauchsart: r.try_get("verbrauchsart").unwrap_or(None),
            ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
            data: r.get("data"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
            version: r.try_get("version").unwrap_or(1),
            updated_at: r.get("updated_at"),
        }))
    }

    /// Return all `TechnischeRessource` records for a `MaLo`.
    pub async fn list_tr_by_malo(
        &self,
        malo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, nutzung, verbrauchsart, ist_fernschaltbar,
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
                nutzung: r.try_get("nutzung").unwrap_or(None),
                verbrauchsart: r.try_get("verbrauchsart").unwrap_or(None),
                ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// Return all `TechnischeRessource` records for a `MeLo`.
    pub async fn list_tr_by_melo(
        &self,
        melo_id: &str,
        tenant: &str,
    ) -> Result<Vec<TechnischeRessourceRecord>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tr_id, tenant, malo_id, melo_id, nutzung, verbrauchsart, ist_fernschaltbar,
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
                nutzung: r.try_get("nutzung").unwrap_or(None),
                verbrauchsart: r.try_get("verbrauchsart").unwrap_or(None),
                ist_fernschaltbar: r.try_get("ist_fernschaltbar").unwrap_or(None),
                data: r.get("data"),
                bo4e_version: r
                    .try_get("bo4e_version")
                    .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
                version: r.try_get("version").unwrap_or(1),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }

    /// Apply a UTILMD Stammdatenänderung (`LOC+Z20`) to the typed TR columns.
    ///
    /// `COALESCE` per column; the JSONB payload and `version` are untouched.
    /// Returns `true` when a row was updated, `false` when the TR is unknown.
    pub async fn patch_stammdaten(
        &self,
        tr_id: &str,
        tenant: &str,
        patch: &mako_markt::repository::TechnischeRessourceStammdatenPatch,
    ) -> Result<bool, MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::patch_stammdaten_tx(&mut conn, tr_id, tenant, patch).await
    }

    /// [`Self::patch_stammdaten`] on the caller's transaction.
    pub async fn patch_stammdaten_tx(
        conn: &mut sqlx::PgConnection,
        tr_id: &str,
        tenant: &str,
        patch: &mako_markt::repository::TechnischeRessourceStammdatenPatch,
    ) -> Result<bool, MdmError> {
        if patch.is_empty() {
            return Ok(false);
        }

        // **The document moves with the columns.** All three are fields
        // `TechnischeRessource` declares, so the columns index the stored
        // document rather than standing beside it. Leaving the JSONB untouched
        // would make a `GET` return the superseded document while §14a steering
        // reads the current `ist_fernschaltbar` — same fact, two answers.
        //
        // The merge is `data || patch` — JSONB concatenation replaces the
        // top-level keys it names and keeps every other key, which is exactly
        // the semantics of a partial Stammdatenänderung. Keys are the BO4E
        // camelCase spellings, because that is what the document is written in.
        let mut doc_patch = serde_json::Map::new();
        if let Some(n) = patch.nutzung.as_deref() {
            doc_patch.insert("technischeRessourceNutzung".into(), n.into());
        }
        if let Some(v) = patch.verbrauchsart.as_deref() {
            doc_patch.insert("technischeRessourceVerbrauchsart".into(), v.into());
        }
        if let Some(f) = patch.ist_fernschaltbar {
            doc_patch.insert("istFernschaltbar".into(), f.into());
        }
        let doc_patch = serde_json::Value::Object(doc_patch);

        let affected = sqlx::query(
            r"UPDATE technische_ressourcen
               SET nutzung           = COALESCE($3, nutzung),
                   verbrauchsart     = COALESCE($4, verbrauchsart),
                   ist_fernschaltbar = COALESCE($5, ist_fernschaltbar),
                   data              = data || $6::jsonb,
                   updated_at        = now()
               WHERE tr_id = $1 AND tenant = $2",
        )
        .bind(tr_id)
        .bind(tenant)
        .bind(patch.nutzung.as_deref())
        .bind(patch.verbrauchsart.as_deref())
        .bind(patch.ist_fernschaltbar)
        .bind(&doc_patch)
        .execute(&mut *conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }
}
