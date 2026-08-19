//! PostgreSQL implementation of [`MaloRepository`].

use mako_markt::{
    bo4e::MaloShadowColumns,
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{MaloFilter, MaloRecord, MaloRepository, PageResult, Rollenzuordnung},
};
use rubo4e::current::Marktlokation;
use sqlx::{PgConnection, PgPool, Row, postgres::PgRow};
use time::Date;

/// PostgreSQL-backed MaLo repository.
///
/// The mutations are also available as inherent `*_tx` functions taking a
/// `&mut PgConnection`, so callers (the PUT handler, event ingest) can commit
/// the write atomically with their outbox enqueue / idempotency marker.
#[derive(Clone, Debug)]
pub struct PgMaloRepository {
    pool: PgPool,
}

impl PgMaloRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Full-row upsert. Returns the actual new version (`RETURNING version`).
    ///
    /// With `if_match` the UPDATE is guarded on `version = $expected`; 0 rows —
    /// a concurrent write, or no row at all — is a [`MdmError::VersionConflict`].
    pub async fn upsert_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        sparte: Sparte,
        data: &Marktlokation,
        rollenzuordnung: Vec<Rollenzuordnung>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError> {
        let sparte_str = sparte.to_string();
        // Typed columns, derived from the validated BO rather than from string
        // lookups on its JSON. Every value is a BO4E wire value by construction.
        let cols = MaloShadowColumns::from_marktlokation(data);
        let payload = serde_json::to_value(data)
            .map_err(|e| MdmError::Internal(format!("Marktlokation is not serialisable: {e}")))?;

        // `fallgruppe` and `fernsteuerbar` are deliberately not in this list:
        // neither is a `Marktlokation` field (the GaBi Fallgruppe belongs to
        // `Bilanzierung`, the §14a Fernsteuerbarkeit to no BO at all), so both
        // are owned by `patch_stammdaten` / `patch_typenmerkmal`.
        let new_version: Option<i64> = if let Some(expected) = if_match {
            sqlx::query_scalar(
                r#"UPDATE malo
                   SET sparte               = $2,
                       netzebene            = $3,
                       bilanzierungsgebiet  = $4,
                       gasqualitaet         = $5,
                       energierichtung      = $6,
                       bilanzierungsmethode = $7,
                       regelzone            = $8,
                       lokationsbuendel_objektcode = $9,
                       data                 = $10,
                       bo4e_version         = $11,
                       version              = version + 1,
                       updated_at           = now()
                   WHERE malo_id = $1 AND version = $12
                   RETURNING version"#,
            )
            .bind(malo_id)
            .bind(&sparte_str)
            .bind(cols.netzebene)
            .bind(&cols.bilanzierungsgebiet)
            .bind(cols.gasqualitaet)
            .bind(cols.energierichtung)
            .bind(cols.bilanzierungsmethode)
            .bind(&cols.regelzone)
            .bind(&cols.lokationsbuendel_objektcode)
            .bind(&payload)
            .bind(bo4e_version)
            .bind(expected)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
        } else {
            sqlx::query_scalar(
                r#"INSERT INTO malo (malo_id, sparte, netzebene, bilanzierungsgebiet, gasqualitaet, energierichtung, bilanzierungsmethode, regelzone, lokationsbuendel_objektcode, version, data, bo4e_version, updated_at)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 1, $10, $11, now())
                   ON CONFLICT (malo_id) DO UPDATE
                   SET sparte               = EXCLUDED.sparte,
                       netzebene            = EXCLUDED.netzebene,
                       bilanzierungsgebiet  = EXCLUDED.bilanzierungsgebiet,
                       gasqualitaet         = EXCLUDED.gasqualitaet,
                       energierichtung      = EXCLUDED.energierichtung,
                       bilanzierungsmethode = EXCLUDED.bilanzierungsmethode,
                       regelzone            = EXCLUDED.regelzone,
                       lokationsbuendel_objektcode = EXCLUDED.lokationsbuendel_objektcode,
                       version              = malo.version + 1,
                       data                 = EXCLUDED.data,
                       bo4e_version         = EXCLUDED.bo4e_version,
                       updated_at           = now()
                   RETURNING version"#,
            )
            .bind(malo_id)
            .bind(&sparte_str)
            .bind(cols.netzebene)
            .bind(&cols.bilanzierungsgebiet)
            .bind(cols.gasqualitaet)
            .bind(cols.energierichtung)
            .bind(cols.bilanzierungsmethode)
            .bind(&cols.regelzone)
            .bind(&cols.lokationsbuendel_objektcode)
            .bind(&payload)
            .bind(bo4e_version)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
        };

        let Some(new_version) = new_version else {
            return Err(MdmError::VersionConflict {
                expected: if_match.map_or_else(|| "new".into(), |v| v.to_string()),
                actual: "(concurrent update)".into(),
            });
        };

        sqlx::query("DELETE FROM rollenzuordnungen WHERE malo_id = $1")
            .bind(malo_id)
            .execute(&mut *conn)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        // Bulk-insert all new entries in a single round-trip using UNNEST.
        // An empty rollenzuordnung vec is a no-op (unnest of empty arrays = 0 rows).
        if !rollenzuordnung.is_empty() {
            let zuordnungstypen: Vec<&str> = rollenzuordnung
                .iter()
                .map(|lz| lz.zuordnungstyp.as_str())
                .collect();
            let rollencodenummern: Vec<&str> = rollenzuordnung
                .iter()
                .map(|lz| lz.rollencodenummer.as_str())
                .collect();
            let valid_froms: Vec<Date> = rollenzuordnung.iter().map(|lz| lz.valid_from).collect();
            let valid_tos: Vec<Option<Date>> =
                rollenzuordnung.iter().map(|lz| lz.valid_to).collect();

            sqlx::query(
                r#"INSERT INTO rollenzuordnungen
                       (malo_id, zuordnungstyp, rollencodenummer, valid_from, valid_to)
                   SELECT $1, unnest($2::text[]), unnest($3::text[]),
                          unnest($4::date[]), unnest($5::date[])"#,
            )
            .bind(malo_id)
            .bind(zuordnungstypen)
            .bind(rollencodenummern)
            .bind(valid_froms)
            .bind(valid_tos)
            .execute(&mut *conn)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        }

        Ok(new_version)
    }

    /// Patch the derived Typenmerkmale columns on the caller's transaction.
    /// A `None` argument leaves the existing value unchanged.
    pub async fn patch_typenmerkmal_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        bilanzierungsmethode: Option<&str>,
        fallgruppe: Option<&str>,
    ) -> Result<(), MdmError> {
        if bilanzierungsmethode.is_none() && fallgruppe.is_none() {
            return Ok(());
        }
        // Patch only the typed columns — do NOT touch data JSONB or version.
        sqlx::query(
            r#"UPDATE malo
               SET bilanzierungsmethode = COALESCE($2, bilanzierungsmethode),
                   fallgruppe           = COALESCE($3, fallgruppe),
                   updated_at           = now()
               WHERE malo_id = $1"#,
        )
        .bind(malo_id)
        .bind(bilanzierungsmethode)
        .bind(fallgruppe)
        .execute(conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }
}

impl MaloRepository for PgMaloRepository {
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: &Marktlokation,
        rollenzuordnung: Vec<Rollenzuordnung>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        let new_version = Self::upsert_tx(
            &mut tx,
            malo_id,
            sparte,
            data,
            rollenzuordnung,
            if_match,
            bo4e_version,
        )
        .await?;
        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(new_version)
    }

    async fn find(&self, malo_id: &MaloId, at: Date) -> Result<Option<MaloRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            r#"SELECT m.malo_id,
                      m.sparte,
                      m.netzebene,
                      m.bilanzierungsgebiet,
                      m.gasqualitaet,
                      m.energierichtung,
                      m.bilanzierungsmethode,
                      m.regelzone,
                      m.fallgruppe,
                      m.lokationsbuendel_objektcode,
                      m.fernsteuerbar,
                      m.version,
                      m.data,
                      m.bo4e_version,
                      m.updated_at,
                      COALESCE(
                          json_agg(
                              json_build_object(
                                  'zuordnungstyp',    lz.zuordnungstyp,
                                  'rollencodenummer', lz.rollencodenummer,
                                  'valid_from',       to_char(lz.valid_from, 'YYYY-MM-DD'),
                                  'valid_to',         to_char(lz.valid_to,   'YYYY-MM-DD')
                              ) ORDER BY lz.zuordnungstyp, lz.valid_from
                          ) FILTER (WHERE lz.zuordnungstyp IS NOT NULL),
                          '[]'::json
                      ) AS rollenzuordnung
               FROM malo m
               LEFT JOIN rollenzuordnungen lz
                     ON  lz.malo_id   = m.malo_id
                     AND lz.valid_from <= $2
                     AND (lz.valid_to IS NULL OR lz.valid_to > $2)
               WHERE m.malo_id = $1
               GROUP BY m.malo_id, m.sparte, m.netzebene, m.bilanzierungsgebiet, m.gasqualitaet, m.energierichtung, m.bilanzierungsmethode, m.regelzone, m.fallgruppe, m.lokationsbuendel_objektcode, m.fernsteuerbar, m.version, m.data, m.bo4e_version, m.updated_at"#,
        )
        .bind(malo_id)
        .bind(at)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(row_to_malo))
    }

    async fn list(&self, filter: MaloFilter, at: Date) -> Result<PageResult<MaloRecord>, MdmError> {
        let sparte_str = filter.sparte.map(|s| s.to_string());
        let size = filter.size.clamp(1, 500);
        let limit = i64::from(size);
        let offset = i64::from(filter.page) * limit;

        // Single query: COUNT(*) OVER() window function returns the total matching
        // rows alongside each page row, avoiding a separate COUNT + SELECT round-trip
        // and eliminating the TOCTOU race between the two queries.
        let rows: Vec<PgRow> = sqlx::query(
            r#"SELECT m.malo_id,
                      m.sparte,
                      m.netzebene,
                      m.bilanzierungsgebiet,
                      m.gasqualitaet,
                      m.energierichtung,
                      m.bilanzierungsmethode,
                      m.regelzone,
                      m.fallgruppe,
                      m.lokationsbuendel_objektcode,
                      m.fernsteuerbar,
                      m.version,
                      m.data,
                      m.bo4e_version,
                      m.updated_at,
                      COALESCE(
                          json_agg(
                              json_build_object(
                                  'zuordnungstyp',    lz.zuordnungstyp,
                                  'rollencodenummer', lz.rollencodenummer,
                                  'valid_from',       to_char(lz.valid_from, 'YYYY-MM-DD'),
                                  'valid_to',         to_char(lz.valid_to,   'YYYY-MM-DD')
                              ) ORDER BY lz.zuordnungstyp, lz.valid_from
                          ) FILTER (WHERE lz.zuordnungstyp IS NOT NULL),
                          '[]'::json
                      ) AS rollenzuordnung,
                      COUNT(*) OVER () AS total_count
               FROM malo m
               LEFT JOIN rollenzuordnungen lz
                     ON  lz.malo_id   = m.malo_id
                     AND lz.valid_from <= $1
                     AND (lz.valid_to IS NULL OR lz.valid_to > $1)
               WHERE ($2::text IS NULL OR m.sparte = $2)
                 AND ($3::text IS NULL OR lz.zuordnungstyp    = $3)
                 AND ($4::text IS NULL OR lz.rollencodenummer = $4)
                 AND ($5::text IS NULL OR m.fallgruppe        = $5)
                 AND ($6::text IS NULL OR m.bilanzierungsmethode = $6)
                 AND ($7::text IS NULL OR m.regelzone         = $7)
               GROUP BY m.malo_id, m.sparte, m.netzebene, m.bilanzierungsgebiet, m.gasqualitaet, m.energierichtung, m.bilanzierungsmethode, m.regelzone, m.fallgruppe, m.lokationsbuendel_objektcode, m.fernsteuerbar, m.version, m.data, m.bo4e_version, m.updated_at
               ORDER BY m.malo_id
               LIMIT $8 OFFSET $9"#,
        )
        .bind(at)
        .bind(&sparte_str)
        .bind(&filter.zuordnungstyp)
        .bind(&filter.rollencodenummer)
        .bind(&filter.fallgruppe)
        .bind(&filter.bilanzierungsmethode)
        .bind(&filter.regelzone)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        // Read total_count from the first row before consuming the Vec.
        let total = rows
            .first()
            .and_then(|r| r.try_get::<i64, _>("total_count").ok())
            .unwrap_or(0) as u64;

        Ok(PageResult {
            items: rows.into_iter().map(row_to_malo).collect(),
            total,
            page: filter.page,
            size,
        })
    }

    async fn patch_typenmerkmal(
        &self,
        malo_id: &mako_markt::domain::MaloId,
        bilanzierungsmethode: Option<&str>,
        fallgruppe: Option<&str>,
    ) -> Result<(), mako_markt::error::MdmError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::patch_typenmerkmal_tx(&mut conn, malo_id, bilanzierungsmethode, fallgruppe).await
    }

    async fn patch_stammdaten(
        &self,
        malo_id: &mako_markt::domain::MaloId,
        patch: &mako_markt::repository::MaloStammdatenPatch,
    ) -> Result<bool, mako_markt::error::MdmError> {
        if patch.is_empty() {
            return Ok(false);
        }
        // COALESCE per column — a NULL argument leaves the existing value
        // unchanged. JSONB payload and version are intentionally untouched.
        let affected = sqlx::query(
            r#"UPDATE malo
               SET netzebene            = COALESCE($2, netzebene),
                   bilanzierungsgebiet  = COALESCE($3, bilanzierungsgebiet),
                   gasqualitaet         = COALESCE($4, gasqualitaet),
                   energierichtung      = COALESCE($5, energierichtung),
                   bilanzierungsmethode = COALESCE($6, bilanzierungsmethode),
                   regelzone            = COALESCE($7, regelzone),
                   fallgruppe           = COALESCE($8, fallgruppe),
                   fernsteuerbar        = COALESCE($9, fernsteuerbar),
                   updated_at           = now()
               WHERE malo_id = $1"#,
        )
        .bind(malo_id.to_string())
        .bind(&patch.netzebene)
        .bind(&patch.bilanzierungsgebiet)
        .bind(&patch.gasqualitaet)
        .bind(&patch.energierichtung)
        .bind(&patch.bilanzierungsmethode)
        .bind(&patch.regelzone)
        .bind(&patch.fallgruppe)
        .bind(patch.fernsteuerbar)
        .execute(&self.pool)
        .await
        .map_err(|e| mako_markt::error::MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }
}

fn row_to_malo(r: PgRow) -> MaloRecord {
    let sparte_str: String = r.get("sparte");
    let lz_json: serde_json::Value = r.get("rollenzuordnung");
    let rollenzuordnung: Vec<Rollenzuordnung> = serde_json::from_value(lz_json).unwrap_or_default();
    MaloRecord {
        malo_id: r.get("malo_id"),
        sparte: sparte_str
            .parse::<Sparte>()
            .expect("DB has CHECK constraint on sparte"),
        netzebene: r.try_get("netzebene").unwrap_or(None),
        bilanzierungsgebiet: r.try_get("bilanzierungsgebiet").unwrap_or(None),
        gasqualitaet: r.try_get("gasqualitaet").unwrap_or(None),
        energierichtung: r.try_get("energierichtung").unwrap_or(None),
        bilanzierungsmethode: r.try_get("bilanzierungsmethode").unwrap_or(None),
        regelzone: r.try_get("regelzone").unwrap_or(None),
        fallgruppe: r.try_get("fallgruppe").unwrap_or(None),
        lokationsbuendel_objektcode: r.try_get("lokationsbuendel_objektcode").unwrap_or(None),
        fernsteuerbar: r.try_get("fernsteuerbar").unwrap_or(None),
        version: r.get("version"),
        data: r.get("data"),
        rollenzuordnung,
        updated_at: r.get("updated_at"),
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
    }
}
