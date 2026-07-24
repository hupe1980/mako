//! PostgreSQL implementation of [`MeloRepository`].
//!
//! # Single-write-path invariant (MaLo ↔ MeLo)
//!
//! The MeLo→MaLo parent relation is recorded twice:
//!
//! 1. `melo.malo_id` — a plain FK, the *derived convenience* for "current parent"
//! 2. `lokationszuordnungen` — the temporal graph, the *authoritative history*
//!
//! [`PgMeloRepository::upsert`] is the only writer of `melo.malo_id`, and it
//! maintains the corresponding `melo → malo` edge in `lokationszuordnungen`
//! **in the same transaction**: the graph is always a superset of the FK, and
//! the two can never contradict. When a PUT changes the parent, the previous
//! open edge is closed (`valid_to = today`) and a new one opened
//! (`valid_from = today`).

use mako_markt::{
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{MeloRecord, MeloRepository},
};
use sqlx::{PgPool, Row, postgres::PgRow};

/// PostgreSQL-backed MeLo repository.
///
/// Carries the deployment tenant so the MeLo write path can maintain the
/// tenant-scoped `lokationszuordnungen` graph (the `melo` table itself is not
/// tenant-scoped — marktd is a single-tenant deployment).
#[derive(Clone, Debug)]
pub struct PgMeloRepository {
    pool: PgPool,
    tenant: String,
}

impl PgMeloRepository {
    #[must_use]
    pub fn new(pool: PgPool, tenant: impl Into<String>) -> Self {
        Self {
            pool,
            tenant: tenant.into(),
        }
    }
}

impl MeloRepository for PgMeloRepository {
    /// Upsert a MeLo and reconcile its `melo → malo` graph edge transactionally.
    ///
    /// See the module docs for the single-write-path invariant. Graph effects:
    /// - parent unchanged → graph untouched
    /// - parent changed   → previous open edge closed (`valid_to = today`),
    ///   edge to the new parent opened (`valid_from = today`) unless one is
    ///   already open
    /// - parent removed (`malo_id = None`) → all open `melo → malo` edges closed
    async fn upsert(
        &self,
        melo_id: &MeloId,
        malo_id: Option<&MaloId>,
        data: serde_json::Value,
        if_match: Option<i64>,
        bo4e_version: &str,
    ) -> Result<i64, MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        let current: Option<i64> =
            sqlx::query_scalar("SELECT version FROM melo WHERE melo_id = $1")
                .bind(melo_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;

        let new_version = match (current, if_match) {
            (Some(v), Some(expected)) if v != expected => {
                return Err(MdmError::VersionConflict {
                    expected: expected.to_string(),
                    actual: v.to_string(),
                });
            }
            (Some(v), _) => v + 1,
            (None, _) => 1,
        };

        let netzebene_messung = data
            .get("netzebeneMessung")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Extract `regelzone` from `standorteigenschaften.eigenschaftenStrom[0].regelzone`.
        // This maps the MeLo to the ÜNB responsible for Redispatch 2.0 Stammdaten routing.
        let regelzone = data
            .get("standorteigenschaften")
            .and_then(|s| s.get("eigenschaftenStrom"))
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|first| first.get("regelzone"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        // Extract full standorteigenschaften JSONB for Redispatch 2.0 and Gas billing zone.
        let standorteigenschaften = data.get("standorteigenschaften").cloned();

        // Extract `lokationsbuendelObjektcode` (BO4E Messlokation) as a typed column.
        let lokationsbuendel_objektcode = data
            .get("lokationsbuendelObjektcode")
            .and_then(|v| v.as_str())
            .map(|s| s.to_owned());

        sqlx::query(
            r#"INSERT INTO melo (melo_id, malo_id, netzebene_messung, regelzone, standorteigenschaften, lokationsbuendel_objektcode, version, data, bo4e_version, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
               ON CONFLICT (melo_id) DO UPDATE
               SET malo_id                = EXCLUDED.malo_id,
                   netzebene_messung      = EXCLUDED.netzebene_messung,
                   regelzone              = EXCLUDED.regelzone,
                   standorteigenschaften  = COALESCE(EXCLUDED.standorteigenschaften,
                                                     melo.standorteigenschaften),
                   lokationsbuendel_objektcode = EXCLUDED.lokationsbuendel_objektcode,
                   version                = EXCLUDED.version,
                   data                   = EXCLUDED.data,
                   bo4e_version           = EXCLUDED.bo4e_version,
                   updated_at             = now()"#,
        )
        .bind(melo_id)
        .bind(malo_id)
        .bind(&netzebene_messung)
        .bind(&regelzone)
        .bind(standorteigenschaften.as_ref())
        .bind(&lokationsbuendel_objektcode)
        .bind(new_version)
        .bind(&data)
        .bind(bo4e_version)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        // ── Graph reconciliation (single-write-path invariant) ────────────────
        let today = time::OffsetDateTime::now_utc().date();
        let melo_str = melo_id.to_string();
        let new_parent = malo_id.map(ToString::to_string);

        // Close open melo→malo edges that no longer match the FK parent.
        // (`nach_id IS DISTINCT FROM $4` also closes everything when the
        // parent is removed.)
        sqlx::query(
            r"UPDATE lokationszuordnungen
              SET valid_to = $3, updated_at = now()
              WHERE tenant = $1
                AND von_id = $2
                AND von_typ = 'melo'
                AND nach_typ = 'malo'
                AND valid_to IS NULL
                AND nach_id IS DISTINCT FROM $4",
        )
        .bind(&self.tenant)
        .bind(&melo_str)
        .bind(today)
        .bind(&new_parent)
        .execute(&mut *tx)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        if let Some(parent) = &new_parent {
            // Is there already an open edge to this parent (either open-ended
            // or dated)? If so the graph already agrees with the FK.
            let open_edge_exists: bool = sqlx::query_scalar(
                r"SELECT EXISTS (
                      SELECT 1 FROM lokationszuordnungen
                      WHERE tenant = $1
                        AND von_id = $2
                        AND nach_id = $3
                        AND von_typ = 'melo'
                        AND nach_typ = 'malo'
                        AND valid_to IS NULL
                  )",
            )
            .bind(&self.tenant)
            .bind(&melo_str)
            .bind(parent)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

            if !open_edge_exists {
                // Dated upsert path — matches the `lz_unique_dated` partial
                // index, so a same-day re-parent back reopens the closed edge.
                sqlx::query(
                    r"INSERT INTO lokationszuordnungen
                          (tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to, data, updated_at)
                      VALUES ($1, $2, 'melo', $3, 'malo', $4, NULL, '{}'::jsonb, now())
                      ON CONFLICT (tenant, von_id, nach_id, valid_from) WHERE valid_from IS NOT NULL
                      DO UPDATE SET valid_to = NULL, updated_at = now()",
                )
                .bind(&self.tenant)
                .bind(&melo_str)
                .bind(parent)
                .bind(today)
                .execute(&mut *tx)
                .await
                .map_err(|e| MdmError::Internal(e.to_string()))?;
            }
        }

        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(new_version)
    }

    async fn find(&self, melo_id: &MeloId) -> Result<Option<MeloRecord>, MdmError> {
        let row: Option<PgRow> = sqlx::query(
            "SELECT melo_id, malo_id, netzebene_messung, regelzone, standorteigenschaften, lokationsbuendel_objektcode, version, data, bo4e_version, updated_at FROM melo WHERE melo_id = $1",
        )
        .bind(melo_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| MeloRecord {
            melo_id: r.get("melo_id"),
            malo_id: r.get("malo_id"),
            netzebene_messung: r.try_get("netzebene_messung").unwrap_or(None),
            regelzone: r.try_get("regelzone").unwrap_or(None),
            standorteigenschaften: r.try_get("standorteigenschaften").unwrap_or(None),
            lokationsbuendel_objektcode: r.try_get("lokationsbuendel_objektcode").unwrap_or(None),
            version: r.get("version"),
            data: r.get("data"),
            updated_at: r.get("updated_at"),
            bo4e_version: r
                .try_get("bo4e_version")
                .unwrap_or_else(|_| "v202607.0.0".to_owned()),
        }))
    }
}
