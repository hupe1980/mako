//! PostgreSQL storage: [`PgLokationszuordnungRepository`].
//!
//! Uses a recursive CTE for graph traversal (`find_graph`), enabling a single
//! query to return the full MaLo → MeLo → NeLo → SR/TR location graph.

use std::str::FromStr;

use mako_markt::domain::Lokationstyp;
use mako_markt::{
    error::MdmError,
    repository::{Lokationsbuendel, LokationszuordnungEdge},
};
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Decode a [`Lokationstyp`] from the wire value stored in a `TEXT` column.
///
/// The column holds the enum's wire string; its strict `FromStr` reads it back
/// and refuses `"UNKNOWN"` and any value the schema does not define.
fn typ_from_str(s: String) -> Result<Lokationstyp, sqlx::Error> {
    Lokationstyp::from_str(&s).map_err(|e| sqlx::Error::ColumnDecode {
        index: "typ".into(),
        source: Box::new(std::io::Error::other(format!(
            "invalid Lokationstyp {s:?}: {e}"
        ))),
    })
}

/// Persistent store for the `Lokationszuordnung` location graph.
///
/// Enables single-query recursive traversal of the full MaLo → MeLo → NeLo →
/// SR/TR graph for topology-dependent operations (Redispatch 2.0, iMS E-mobility
/// Steuerungsauftrag routing, MSB Stammdaten hierarchy).
///
/// # Single-write-path invariant (MaLo ↔ MeLo)
///
/// The `melo → malo` edges of this graph are ALSO maintained by the MeLo write
/// path (`PgMeloRepository::upsert` in marktd's PG implementation) in the same
/// transaction that sets the `melo.malo_id` FK: the FK is a derived convenience
/// for "current parent", the graph is the authoritative temporal history, and
/// the two never contradict. Writers other than the MeLo PUT and the graph API
/// must not touch `melo.malo_id` directly.
#[derive(Clone, Debug)]
pub struct PgLokationszuordnungRepository {
    pool: PgPool,
}

impl PgLokationszuordnungRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_edge(
    row: &sqlx::postgres::PgRow,
    depth: i32,
) -> Result<LokationszuordnungEdge, sqlx::Error> {
    let id_str: String = row.try_get::<String, _>("id")?;
    let id = id_str
        .parse::<Uuid>()
        .map_err(|e| sqlx::Error::ColumnDecode {
            index: "id".into(),
            source: Box::new(std::io::Error::other(e.to_string())),
        })?;
    Ok(LokationszuordnungEdge {
        id,
        tenant: row.try_get("tenant")?,
        von_id: row.try_get("von_id")?,
        von_typ: typ_from_str(row.try_get("von_typ")?)?,
        nach_id: row.try_get("nach_id")?,
        nach_typ: typ_from_str(row.try_get("nach_typ")?)?,
        valid_from: row.try_get("valid_from").unwrap_or(None),
        valid_to: row.try_get("valid_to").unwrap_or(None),
        lokationsbuendelcode: row.try_get("lokationsbuendelcode").unwrap_or(None),
        data: row.try_get("data")?,
        depth,
    })
}

impl PgLokationszuordnungRepository {
    /// Insert or replace a directed edge.
    ///
    /// For open-ended edges (`valid_from = None`), only one edge per
    /// `(tenant, von_id, nach_id)` pair is kept.  Dated edges
    /// (`valid_from = Some(date)`) allow temporal succession.
    #[expect(
        clippy::too_many_arguments,
        reason = "each argument is a separate input of this operation; a parameter struct would exist for this one call"
    )]
    pub async fn upsert_edge(
        &self,
        tenant: &str,
        von_id: &str,
        von_typ: Lokationstyp,
        nach_id: &str,
        nach_typ: Lokationstyp,
        valid_from: Option<time::Date>,
        valid_to: Option<time::Date>,
        data: serde_json::Value,
    ) -> Result<Uuid, MdmError> {
        // Bridge the typed enum to its canonical BO4E code for the TEXT column.
        let von_typ: &'static str = von_typ.into();
        let nach_typ: &'static str = nach_typ.into();
        // Use two separate upsert paths matching the two partial unique indexes:
        // - open-ended edges (valid_from IS NULL): ON CONFLICT (tenant, von_id, nach_id) WHERE valid_from IS NULL
        // - dated edges: ON CONFLICT (tenant, von_id, nach_id, valid_from) WHERE valid_from IS NOT NULL
        //
        // `lokationsbuendelcode` is extracted from the BO4E Lokationszuordnung
        // payload into a typed column (JSONB-fidelity + typed-column pattern).
        let lokationsbuendelcode = data
            .get("lokationsbuendelcode")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned);
        let id = if valid_from.is_none() {
            sqlx::query_scalar::<_, String>(
                r"INSERT INTO lokationszuordnungen
                      (tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to, lokationsbuendelcode, data, updated_at)
                  VALUES ($1, $2, $3, $4, $5, NULL, $6, $7, $8, now())
                  ON CONFLICT (tenant, von_id, nach_id) WHERE valid_from IS NULL
                  DO UPDATE SET
                      von_typ    = EXCLUDED.von_typ,
                      nach_typ   = EXCLUDED.nach_typ,
                      valid_to   = EXCLUDED.valid_to,
                      lokationsbuendelcode = EXCLUDED.lokationsbuendelcode,
                      data       = EXCLUDED.data,
                      updated_at = now()
                  RETURNING id::TEXT",
            )
            .bind(tenant)
            .bind(von_id)
            .bind(von_typ)
            .bind(nach_id)
            .bind(nach_typ)
            .bind(valid_to)
            .bind(&lokationsbuendelcode)
            .bind(&data)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
        } else {
            sqlx::query_scalar::<_, String>(
                r"INSERT INTO lokationszuordnungen
                      (tenant, von_id, von_typ, nach_id, nach_typ, valid_from, valid_to, lokationsbuendelcode, data, updated_at)
                  VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
                  ON CONFLICT (tenant, von_id, nach_id, valid_from) WHERE valid_from IS NOT NULL
                  DO UPDATE SET
                      von_typ    = EXCLUDED.von_typ,
                      nach_typ   = EXCLUDED.nach_typ,
                      valid_to   = EXCLUDED.valid_to,
                      lokationsbuendelcode = EXCLUDED.lokationsbuendelcode,
                      data       = EXCLUDED.data,
                      updated_at = now()
                  RETURNING id::TEXT",
            )
            .bind(tenant)
            .bind(von_id)
            .bind(von_typ)
            .bind(nach_id)
            .bind(nach_typ)
            .bind(valid_from)
            .bind(valid_to)
            .bind(&lokationsbuendelcode)
            .bind(&data)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?
        };

        id.parse::<Uuid>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// Recursively traverse the full location graph reachable from `root_id`.
    ///
    /// Returns all edges BFS-ordered by depth (depth 0 = direct edges from root).
    /// Pass `at_date = None` to return all edges regardless of validity.
    /// Pass `at_date = Some(d)` to filter to edges valid on date `d`.
    ///
    /// Traversal is capped at depth 8 to prevent runaway queries on malformed data.
    pub async fn find_graph(
        &self,
        tenant: &str,
        root_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<LokationszuordnungEdge>, MdmError> {
        // Recursive CTE: BFS traversal from root_id, depth-capped at 8.
        // The `at_date` filter: edge valid if valid_from ≤ at_date AND (valid_to IS NULL OR valid_to ≥ at_date).
        // When at_date IS NULL, all edges are returned regardless of validity.
        let rows = sqlx::query(
            r"WITH RECURSIVE graph(id, tenant, von_id, von_typ, nach_id, nach_typ,
                                    valid_from, valid_to, lokationsbuendelcode, data, depth) AS (
                -- Seed: direct edges from root
                SELECT id::TEXT, tenant, von_id, von_typ, nach_id, nach_typ,
                       valid_from, valid_to, lokationsbuendelcode, data, 0
                FROM lokationszuordnungen
                WHERE tenant = $1
                  AND von_id = $2
                  AND ($3::DATE IS NULL OR (valid_from IS NULL OR valid_from <= $3))
                  AND ($3::DATE IS NULL OR (valid_to   IS NULL OR valid_to   >  $3))
                UNION ALL
                -- Recursive: follow edges from each reached node, depth < 8
                SELECT lz.id::TEXT, lz.tenant, lz.von_id, lz.von_typ, lz.nach_id,
                       lz.nach_typ, lz.valid_from, lz.valid_to, lz.lokationsbuendelcode,
                       lz.data, g.depth + 1
                FROM lokationszuordnungen lz
                JOIN graph g ON lz.von_id = g.nach_id AND lz.tenant = g.tenant
                WHERE g.depth < 8
                  AND ($3::DATE IS NULL OR (lz.valid_from IS NULL OR lz.valid_from <= $3))
                  AND ($3::DATE IS NULL OR (lz.valid_to   IS NULL OR lz.valid_to   >  $3))
            )
            SELECT * FROM graph ORDER BY depth, von_id, nach_id",
        )
        .bind(tenant)
        .bind(root_id)
        .bind(at_date)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter()
            .map(|r| {
                let depth: i32 = r.try_get("depth").unwrap_or(0);
                map_edge(r, depth)
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// Return direct (depth-0) edges FROM a given node, optionally filtered by date.
    pub async fn list_edges_from(
        &self,
        tenant: &str,
        von_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Vec<LokationszuordnungEdge>, MdmError> {
        let rows = sqlx::query(
            r"SELECT id::TEXT, tenant, von_id, von_typ, nach_id, nach_typ,
                     valid_from, valid_to, lokationsbuendelcode, data
              FROM lokationszuordnungen
              WHERE tenant = $1
                AND von_id = $2
                AND ($3::DATE IS NULL OR (valid_from IS NULL OR valid_from <= $3))
                AND ($3::DATE IS NULL OR (valid_to   IS NULL OR valid_to   >  $3))
              ORDER BY nach_typ, nach_id",
        )
        .bind(tenant)
        .bind(von_id)
        .bind(at_date)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter()
            .map(|r| map_edge(r, 0))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }

    /// Hard-delete an edge by `(tenant, von_id, nach_id)`.
    ///
    /// Removes all temporal variants of the edge pair.
    /// Returns `true` if at least one row was deleted.
    pub async fn delete_edge(
        &self,
        tenant: &str,
        von_id: &str,
        nach_id: &str,
    ) -> Result<bool, MdmError> {
        let rows_affected = sqlx::query(
            "DELETE FROM lokationszuordnungen WHERE tenant = $1 AND von_id = $2 AND nach_id = $3",
        )
        .bind(tenant)
        .bind(von_id)
        .bind(nach_id)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?
        .rows_affected();
        Ok(rows_affected > 0)
    }

    /// Load the [`Lokationsbuendel`] rooted at `malo_id`, projected from the
    /// typed edge graph (validity-filtered by `at_date`).
    ///
    /// Built on [`find_graph`](Self::find_graph).
    pub async fn load_buendel(
        &self,
        tenant: &str,
        malo_id: &str,
        at_date: Option<time::Date>,
    ) -> Result<Lokationsbuendel, MdmError> {
        let edges = self.find_graph(tenant, malo_id, at_date).await?;
        Ok(Lokationsbuendel::from_graph(malo_id, &edges))
    }
}
