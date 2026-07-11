//! PostgreSQL implementation of [`MaloRepository`].

use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{Lokationszuordnung, MaloFilter, MaloRecord, MaloRepository, PageResult},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::Date;

/// PostgreSQL-backed MaLo repository.
#[derive(Clone, Debug)]
pub struct PgMaloRepository {
    pool: PgPool,
}

impl PgMaloRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl MaloRepository for PgMaloRepository {
    async fn upsert(
        &self,
        malo_id: &MaloId,
        sparte: Sparte,
        data: serde_json::Value,
        lokationszuordnung: Vec<Lokationszuordnung>,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        let current_version: Option<i64> =
            sqlx::query_scalar("SELECT version FROM malo WHERE malo_id = $1")
                .bind(malo_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = match (current_version, if_match) {
            (Some(v), Some(expected)) if v != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: v.to_string(),
                });
            }
            (Some(v), _) => v + 1,
            (None, _) => 1,
        };

        let sparte_str = sparte.to_string();
        // Extract typed fields from the BO4E Marktlokation JSONB payload.
        // These are stored as indexed columns for efficient SQL queries.
        // All are optional — missing fields in the JSONB produce NULL.
        let netzebene = data
            .get("netzebene")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let bilanzierungsgebiet = data
            .get("bilanzierungsgebiet")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let gasqualitaet = data
            .get("gasqualitaet")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let energierichtung = data
            .get("energierichtung")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let bilanzierungsmethode = data
            .get("bilanzierungsmethode")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let regelzone = data
            .get("regelzone")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());
        let fallgruppe = data
            .get("fallgruppenzuordnung")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        sqlx::query(
            r#"INSERT INTO malo (malo_id, sparte, netzebene, bilanzierungsgebiet, gasqualitaet, energierichtung, bilanzierungsmethode, regelzone, fallgruppe, version, data, bo4e_version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())
               ON CONFLICT (malo_id) DO UPDATE
               SET sparte               = EXCLUDED.sparte,
                   netzebene            = EXCLUDED.netzebene,
                   bilanzierungsgebiet  = EXCLUDED.bilanzierungsgebiet,
                   gasqualitaet         = EXCLUDED.gasqualitaet,
                   energierichtung      = EXCLUDED.energierichtung,
                   bilanzierungsmethode = EXCLUDED.bilanzierungsmethode,
                   regelzone            = EXCLUDED.regelzone,
                   fallgruppe           = EXCLUDED.fallgruppe,
                   version              = EXCLUDED.version,
                   data                 = EXCLUDED.data,
                   bo4e_version         = EXCLUDED.bo4e_version,
                   updated_at           = now()"#,
        )
        .bind(malo_id)
        .bind(&sparte_str)
        .bind(&netzebene)
        .bind(&bilanzierungsgebiet)
        .bind(&gasqualitaet)
        .bind(&energierichtung)
        .bind(&bilanzierungsmethode)
        .bind(&regelzone)
        .bind(&fallgruppe)
        .bind(new_version)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        sqlx::query("DELETE FROM lokationszuordnung WHERE malo_id = $1")
            .bind(malo_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        // Bulk-insert all new entries in a single round-trip using UNNEST.
        // An empty lokationszuordnung vec is a no-op (unnest of empty arrays = 0 rows).
        if !lokationszuordnung.is_empty() {
            let zuordnungstypen: Vec<&str> = lokationszuordnung
                .iter()
                .map(|lz| lz.zuordnungstyp.as_str())
                .collect();
            let rollencodenummern: Vec<&str> = lokationszuordnung
                .iter()
                .map(|lz| lz.rollencodenummer.as_str())
                .collect();
            let valid_froms: Vec<Date> =
                lokationszuordnung.iter().map(|lz| lz.valid_from).collect();
            let valid_tos: Vec<Option<Date>> =
                lokationszuordnung.iter().map(|lz| lz.valid_to).collect();

            sqlx::query(
                r#"INSERT INTO lokationszuordnung
                       (malo_id, zuordnungstyp, rollencodenummer, valid_from, valid_to)
                   SELECT $1, unnest($2::text[]), unnest($3::text[]),
                          unnest($4::date[]), unnest($5::date[])"#,
            )
            .bind(malo_id)
            .bind(zuordnungstypen)
            .bind(rollencodenummern)
            .bind(valid_froms)
            .bind(valid_tos)
            .execute(&mut *tx)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        }

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
                      ) AS lokationszuordnung
               FROM malo m
               LEFT JOIN lokationszuordnung lz
                     ON  lz.malo_id   = m.malo_id
                     AND lz.valid_from <= $2
                     AND (lz.valid_to IS NULL OR lz.valid_to >= $2)
               WHERE m.malo_id = $1
               GROUP BY m.malo_id, m.sparte, m.netzebene, m.bilanzierungsgebiet, m.gasqualitaet, m.energierichtung, m.bilanzierungsmethode, m.regelzone, m.fallgruppe, m.version, m.data, m.bo4e_version, m.updated_at"#,
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
                      ) AS lokationszuordnung,
                      COUNT(*) OVER () AS total_count
               FROM malo m
               LEFT JOIN lokationszuordnung lz
                     ON  lz.malo_id   = m.malo_id
                     AND lz.valid_from <= $1
                     AND (lz.valid_to IS NULL OR lz.valid_to >= $1)
               WHERE ($2::text IS NULL OR m.sparte = $2)
                 AND ($3::text IS NULL OR lz.zuordnungstyp    = $3)
                 AND ($4::text IS NULL OR lz.rollencodenummer = $4)
               GROUP BY m.malo_id, m.sparte, m.netzebene, m.bilanzierungsgebiet, m.gasqualitaet, m.energierichtung, m.bilanzierungsmethode, m.regelzone, m.fallgruppe, m.version, m.data, m.bo4e_version, m.updated_at
               ORDER BY m.malo_id
               LIMIT $5 OFFSET $6"#,
        )
        .bind(at)
        .bind(&sparte_str)
        .bind(&filter.zuordnungstyp)
        .bind(&filter.rollencodenummer)
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
}

fn row_to_malo(r: PgRow) -> MaloRecord {
    let sparte_str: String = r.get("sparte");
    let lz_json: serde_json::Value = r.get("lokationszuordnung");
    let lokationszuordnung: Vec<Lokationszuordnung> =
        serde_json::from_value(lz_json).unwrap_or_default();
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
        version: r.get("version"),
        data: r.get("data"),
        lokationszuordnung,
        updated_at: r.get("updated_at"),
        bo4e_version: r
            .try_get("bo4e_version")
            .unwrap_or_else(|_| "v202607.0.0".to_owned()),
    }
}
