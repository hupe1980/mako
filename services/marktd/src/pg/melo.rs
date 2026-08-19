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

use mako_markt::bo4e::MeloShadowColumns;
use mako_markt::{
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{MeloRecord, MeloRepository},
};
use rubo4e::current::Messlokation;
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
        data: &Messlokation,
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

        // Typed columns, derived from the validated BO. The Regelzone comes off
        // the parsed `Standorteigenschaften` rather than a chain of JSON
        // lookups, so a malformed EIC is a rejected write, not a bad row.
        let cols =
            MeloShadowColumns::from_messlokation(data).map_err(|e| MdmError::Unprocessable {
                reason: e.to_string(),
            })?;
        let payload = serde_json::to_value(data)
            .map_err(|e| MdmError::Internal(format!("Messlokation is not serialisable: {e}")))?;

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
        .bind(cols.netzebene_messung)
        .bind(&cols.regelzone)
        .bind(cols.standorteigenschaften.as_ref())
        .bind(&cols.lokationsbuendel_objektcode)
        .bind(new_version)
        .bind(&payload)
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
                AND von_typ = 'MELO'
                AND nach_typ = 'MALO'
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
                        AND von_typ = 'MELO'
                        AND nach_typ = 'MALO'
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
                      VALUES ($1, $2, 'MELO', $3, 'MALO', $4, NULL, '{}'::jsonb, now())
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
                .unwrap_or_else(|_| mako_markt::bo4e::schema_version()),
        }))
    }

    async fn patch_stammdaten(
        &self,
        melo_id: &MeloId,
        patch: &mako_markt::repository::MeloStammdatenPatch,
    ) -> Result<bool, MdmError> {
        if patch.is_empty() {
            return Ok(false);
        }
        // COALESCE per column — a NULL argument leaves the existing value
        // unchanged. The JSONB payload (data, standorteigenschaften) and the
        // version are intentionally untouched.
        let affected = sqlx::query(
            r#"UPDATE melo
               SET netzebene_messung = COALESCE($2, netzebene_messung),
                   regelzone         = COALESCE($3, regelzone),
                   updated_at        = now()
               WHERE melo_id = $1"#,
        )
        .bind(melo_id.as_ref())
        .bind(&patch.netzebene_messung)
        .bind(&patch.regelzone)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(affected > 0)
    }
}
