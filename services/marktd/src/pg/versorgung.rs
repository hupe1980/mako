//! PostgreSQL implementation of [`VersorgungsStatusRepository`].

use mako_markt::{
    domain::MaloId,
    error::MdmError,
    repository::{
        LfZuordnung, LieferStatus, PageResult, VersorgungsStatusHistoryRecord,
        VersorgungsStatusRecord, VersorgungsStatusRepository,
    },
};
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgPool, Row, postgres::PgRow, types::Json};
use std::str::FromStr as _;
use time::Date;

/// PostgreSQL-backed VersorgungsStatus repository.
///
/// The scalar state lives in `versorgungsstatus`, one row per
/// `(malo_id, tenant)`; **who supplies the Marktlokation** lives in
/// `lf_zuordnung`, one row per assignment, because a tranchierte Marktlokation
/// has several at once. Reads assemble the two in one query, so there is one
/// round trip and one consistent view.
///
/// All full-row writes use optimistic concurrency — `upsert` with
/// `if_version = Some(v)` issues `WHERE version = v` and returns
/// `MdmError::Conflict` on a 0-row update.
///
/// Every successful write appends a row to `versorgungsstatus_history`,
/// carrying the assignment list as a JSONB snapshot, which is what `find_at`
/// resolves a point-in-time query against.
///
/// Each mutation is also available as an inherent `*_tx` function taking a
/// `&mut PgConnection`, so callers (event ingest, PUT handlers) can commit the
/// state change atomically with their idempotency marker and outbox enqueue.
#[derive(Clone, Debug)]
pub struct PgVersorgungsStatusRepository {
    pool: PgPool,
}

impl PgVersorgungsStatusRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn internal(e: impl std::fmt::Display) -> MdmError {
    MdmError::Internal(e.to_string())
}

/// The assignment list of the `versorgungsstatus` row aliased `v`, as a JSONB
/// array shaped like [`LfZuordnung`].
///
/// A correlated subquery rather than a `LEFT JOIN`: the parent row must come
/// back exactly once whether the Marktlokation carries no assignment, one, or
/// the several a tranchierte one has, and a join would fan it out.
///
/// `prozent` is cast to `text` because a `Decimal` deserialises from its exact
/// decimal string — the same reason the column is `NUMERIC` and not a float.
const ZUORDNUNGEN_JSON: &str = r#"COALESCE((
        SELECT jsonb_agg(jsonb_build_object(
                   'lf_mp_id',         z.lf_mp_id,
                   'prozent',          z.prozent::text,
                   'tranche_id',       z.tranche_id,
                   'status',           z.status,
                   'zuordnungsbeginn', to_char(z.zuordnungsbeginn, 'YYYY-MM-DD'),
                   'zuordnungsende',   to_char(z.zuordnungsende, 'YYYY-MM-DD'),
                   'process_id',       z.process_id
               ) ORDER BY z.status, z.lf_mp_id)
        FROM lf_zuordnung z
        WHERE z.malo_id = v.malo_id AND z.tenant = v.tenant
    ), '[]'::jsonb) AS zuordnungen"#;

fn decode<E: std::fmt::Display>(index: &'static str, e: E) -> sqlx::Error {
    sqlx::Error::ColumnDecode {
        index: index.into(),
        source: Box::new(std::io::Error::other(e.to_string())),
    }
}

fn row_malo_id(row: &PgRow) -> Result<MaloId, sqlx::Error> {
    row.try_get::<String, _>("malo_id")?
        .parse::<MaloId>()
        .map_err(|e| decode("malo_id", e))
}

fn row_lieferstatus(row: &PgRow) -> Result<LieferStatus, sqlx::Error> {
    LieferStatus::from_str(&row.try_get::<String, _>("lieferstatus")?)
        .map_err(|e| decode("lieferstatus", e))
}

fn row_zuordnungen(row: &PgRow) -> Result<Vec<LfZuordnung>, sqlx::Error> {
    Ok(row.try_get::<Json<Vec<LfZuordnung>>, _>("zuordnungen")?.0)
}

fn map_row(row: &PgRow) -> Result<VersorgungsStatusRecord, sqlx::Error> {
    Ok(VersorgungsStatusRecord {
        malo_id: row_malo_id(row)?,
        lieferstatus: row_lieferstatus(row)?,
        zuordnungen: row_zuordnungen(row)?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        eog_seit: row.try_get("eog_seit")?,
        last_process_id: row.try_get("last_process_id")?,
        updated_at: row.try_get("updated_at")?,
        tenant: row.try_get("tenant")?,
        version: row.try_get("version")?,
    })
}

fn map_history_row(row: &PgRow) -> Result<VersorgungsStatusHistoryRecord, sqlx::Error> {
    Ok(VersorgungsStatusHistoryRecord {
        id: row.try_get("id")?,
        malo_id: row_malo_id(row)?,
        tenant: row.try_get("tenant")?,
        lieferstatus: row_lieferstatus(row)?,
        zuordnungen: row_zuordnungen(row)?,
        lieferende: row.try_get("lieferende")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        nb_mp_id: row.try_get("nb_mp_id")?,
        last_process_id: row.try_get("last_process_id")?,
        version: row.try_get("version")?,
        valid_from: row.try_get("valid_from")?,
    })
}

/// Reconstruct a [`VersorgungsStatusRecord`] from a history row, including the
/// snapshotted `eog_seit`.  `updated_at` is set to `valid_from` (the instant
/// the snapshot was recorded).
fn map_history_row_as_current(row: &PgRow) -> Result<VersorgungsStatusRecord, sqlx::Error> {
    let h = map_history_row(row)?;
    Ok(VersorgungsStatusRecord {
        malo_id: h.malo_id,
        tenant: h.tenant,
        lieferstatus: h.lieferstatus,
        zuordnungen: h.zuordnungen,
        lieferende: h.lieferende,
        msb_mp_id: h.msb_mp_id,
        nb_mp_id: h.nb_mp_id,
        eog_seit: row.try_get("eog_seit")?,
        last_process_id: h.last_process_id,
        updated_at: h.valid_from,
        version: h.version,
    })
}

/// Snapshot the current `versorgungsstatus` row — assignment list included —
/// into the history table.
async fn append_history_snapshot(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
) -> Result<(), MdmError> {
    sqlx::query(&format!(
        r#"INSERT INTO versorgungsstatus_history
           (malo_id, tenant, lieferstatus, zuordnungen, lieferende,
            msb_mp_id, nb_mp_id, eog_seit, last_process_id, version, valid_from)
           SELECT v.malo_id, v.tenant, v.lieferstatus, {ZUORDNUNGEN_JSON}, v.lieferende,
                  v.msb_mp_id, v.nb_mp_id, v.eog_seit, v.last_process_id, v.version, now()
           FROM versorgungsstatus v
           WHERE v.malo_id = $1 AND v.tenant = $2"#
    ))
    .bind(malo_id)
    .bind(tenant)
    .execute(conn)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Bump the parent row's version and touch its provenance.
///
/// The assignment tables carry no version of their own: `versorgungsstatus` is
/// the aggregate root, so one version guards the Marktlokation's whole supply
/// state and an ETag stays meaningful across an assignment-only change.
async fn touch(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
    process_id: Option<uuid::Uuid>,
) -> Result<(), MdmError> {
    sqlx::query(
        r#"UPDATE versorgungsstatus
           SET last_process_id = $3, updated_at = now(), version = version + 1
           WHERE malo_id = $1 AND tenant = $2"#,
    )
    .bind(malo_id)
    .bind(tenant)
    .bind(process_id)
    .execute(conn)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Insert the parent row as `Unbeliefert` if the Marktlokation is not in the
/// projection yet, so an assignment always has a root to hang off.
///
/// `lf_zuordnung` carries a foreign key to it: an assignment for a
/// Marktlokation with no supply state is not a state this projection can hold.
async fn ensure_root(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
    nb_mp_id: &str,
) -> Result<(), MdmError> {
    sqlx::query(
        r#"INSERT INTO versorgungsstatus
           (malo_id, tenant, lieferstatus, nb_mp_id, updated_at, version)
           VALUES ($1, $2, 'Unbeliefert', $3, now(), 0)
           ON CONFLICT (malo_id, tenant) DO NOTHING"#,
    )
    .bind(malo_id)
    .bind(tenant)
    .bind(nb_mp_id)
    .execute(conn)
    .await
    .map_err(internal)?;
    Ok(())
}

/// Set `lieferstatus` from what is left in `lf_zuordnung`.
///
/// A Marktlokation is `Beliefert` while **any** assignment runs. One LFA
/// leaving a tranchierte Marktlokation does not make it unsupplied, and
/// treating it as if it did would open a §38 EnWG Ersatzversorgung against a
/// Marktlokation that still has suppliers.
///
/// Only ever moves between `Beliefert` and `Unbeliefert`: `Ruhend`,
/// `Stillgelegt` and the two fallback states are decisions of their own
/// processes, not consequences of an assignment count.
async fn resync_lieferstatus(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
) -> Result<(), MdmError> {
    sqlx::query(
        r#"UPDATE versorgungsstatus v
           SET lieferstatus = CASE
                   WHEN EXISTS (
                       SELECT 1 FROM lf_zuordnung z
                       WHERE z.malo_id = v.malo_id AND z.tenant = v.tenant
                         AND z.status = 'Aktiv'
                   ) THEN 'Beliefert' ELSE 'Unbeliefert' END,
               eog_seit = NULL
           WHERE v.malo_id = $1 AND v.tenant = $2
             AND v.lieferstatus IN ('Beliefert', 'Unbeliefert',
                                    'Ersatzversorgung', 'Grundversorgung')"#,
    )
    .bind(malo_id)
    .bind(tenant)
    .execute(conn)
    .await
    .map_err(internal)?;
    Ok(())
}

// ── Transactional building blocks ─────────────────────────────────────────────
//
// Each function performs one supply-state transition on a caller-provided
// connection/transaction and returns the error instead of swallowing it, so
// the caller can roll back its idempotency marker and force a redelivery.
impl PgVersorgungsStatusRepository {
    /// Full-row upsert, assignment list included.
    ///
    /// Returns the actual new row version (`RETURNING version`), which the
    /// caller must use for the ETag and emitted events. The assignment list is
    /// replaced wholesale — this is the „write the record I read" path, and a
    /// merge would silently keep an assignment the caller had removed.
    pub async fn upsert_tx(
        conn: &mut PgConnection,
        rec: &VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let new_version: Option<i64> = if let Some(expected) = if_version {
            sqlx::query_scalar(
                r#"UPDATE versorgungsstatus
                   SET lieferstatus    = $4,
                       lieferende      = $5,
                       msb_mp_id       = $6,
                       nb_mp_id        = $7,
                       last_process_id = $8,
                       eog_seit        = $9,
                       updated_at      = now(),
                       version         = version + 1
                   WHERE malo_id = $1 AND tenant = $2 AND version = $3
                   RETURNING version"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(expected)
            .bind(rec.lieferstatus.to_string())
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .bind(rec.eog_seit)
            .fetch_optional(&mut *conn)
            .await
            .map_err(internal)?
        } else {
            sqlx::query_scalar(
                r#"INSERT INTO versorgungsstatus
                   (malo_id, tenant, lieferstatus, lieferende,
                    msb_mp_id, nb_mp_id, last_process_id, eog_seit, updated_at, version)
                   VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), 1)
                   ON CONFLICT (malo_id, tenant) DO UPDATE
                   SET lieferstatus    = EXCLUDED.lieferstatus,
                       lieferende      = EXCLUDED.lieferende,
                       msb_mp_id       = EXCLUDED.msb_mp_id,
                       nb_mp_id        = EXCLUDED.nb_mp_id,
                       last_process_id = EXCLUDED.last_process_id,
                       eog_seit        = EXCLUDED.eog_seit,
                       updated_at      = now(),
                       version         = versorgungsstatus.version + 1
                   RETURNING version"#,
            )
            .bind(&rec.malo_id)
            .bind(&rec.tenant)
            .bind(rec.lieferstatus.to_string())
            .bind(rec.lieferende)
            .bind(&rec.msb_mp_id)
            .bind(&rec.nb_mp_id)
            .bind(rec.last_process_id)
            .bind(rec.eog_seit)
            .fetch_optional(&mut *conn)
            .await
            .map_err(internal)?
        };

        let Some(new_version) = new_version else {
            return Err(MdmError::VersionConflict {
                expected: if_version.map_or_else(|| "new".into(), |v| v.to_string()),
                actual: "(concurrent update)".into(),
            });
        };

        replace_zuordnungen(conn, &rec.malo_id, &rec.tenant, &rec.zuordnungen).await?;
        append_history_snapshot(conn, &rec.malo_id, &rec.tenant).await?;
        Ok(new_version)
    }

    /// NB received a Lieferbeginn-Anfrage — record the announced assignment.
    ///
    /// **Several may be pending at once.** A second Anmeldung by a different
    /// supplier is what `E_0622` Prüfschritt 70 refuses with `A06` „Andere
    /// Anmeldung in Bearbeitung", and 55038 / 44038 „Aufhebung einer
    /// zukünftigen Zuordnung" addresses such an LFZ — both decisions need the
    /// competing announcement to exist.
    ///
    /// Re-announcing the same `(lf_mp_id, tranche_id)` updates in place, so an
    /// at-least-once redelivery is idempotent rather than cumulative.
    ///
    /// Returns `true` when a row actually changed.
    #[allow(clippy::too_many_arguments)] // one assignment carries its full identity
    pub async fn announce_lf_next_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<Date>,
        prozent: Decimal,
        tranche_id: Option<&str>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<bool, MdmError> {
        ensure_root(&mut *conn, malo_id, tenant, nb_mp_id).await?;
        let r = sqlx::query(
            r#"INSERT INTO lf_zuordnung
               (malo_id, tenant, lf_mp_id, prozent, tranche_id, status,
                zuordnungsbeginn, process_id, updated_at)
               VALUES ($1, $2, $3, $4, $5, 'Angekuendigt', $6, $7, now())
               ON CONFLICT (tenant, malo_id, lf_mp_id, tranche_id, status) DO UPDATE
               SET prozent          = EXCLUDED.prozent,
                   zuordnungsbeginn = EXCLUDED.zuordnungsbeginn,
                   process_id       = EXCLUDED.process_id,
                   updated_at       = now()"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(lf_mp_id_next)
        .bind(prozent)
        .bind(tranche_id)
        .bind(lf_next_lieferbeginn)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        if r.rows_affected() == 0 {
            return Ok(false);
        }
        touch(&mut *conn, malo_id, tenant, process_id).await?;
        append_history_snapshot(conn, malo_id, tenant).await?;
        Ok(true)
    }

    /// Promote `lf_mp_id`'s announcement to a running assignment.
    ///
    /// `lf_mp_id` names which announcement is confirmed. `None` means „the one
    /// that is pending" — well defined exactly while there is one, which is
    /// what a Bestätigung payload naming no supplier can mean. With several
    /// pending it resolves to none: picking one would assign the Marktlokation
    /// to a supplier the message never mentioned.
    ///
    /// The assignment being confirmed displaces the running one **for the same
    /// Tranche**, and only that one: an Anmeldung for a 25 % Tranche leaves the
    /// LFA holding the other 75 % where it is.
    ///
    /// No-op (no version bump, no history row) when nothing resolves, which is
    /// what makes an idempotent re-delivery free.
    pub async fn confirm_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<bool, MdmError> {
        // Resolve „the pending one" here rather than in SQL, so the ambiguous
        // case is a `None` the caller can log instead of a query that silently
        // matches several rows.
        let lf_mp_id: Option<String> = match lf_mp_id {
            Some(lf) => Some(lf.to_owned()),
            None => {
                let pending: Vec<String> = sqlx::query_scalar(
                    r#"SELECT DISTINCT lf_mp_id FROM lf_zuordnung
                       WHERE malo_id = $1 AND tenant = $2 AND status = 'Angekuendigt'"#,
                )
                .bind(malo_id)
                .bind(tenant)
                .fetch_all(&mut *conn)
                .await
                .map_err(internal)?;
                match <[String; 1]>::try_from(pending) {
                    Ok([only]) => Some(only),
                    Err(_) => None,
                }
            }
        };
        let Some(lf_mp_id) = lf_mp_id else {
            return Ok(false);
        };

        // The displaced assignment is the running one on the same Tranche —
        // resolved from the announcement rather than passed in, so the two
        // cannot disagree.
        let displaced = sqlx::query(
            r#"DELETE FROM lf_zuordnung a
               WHERE a.malo_id = $1 AND a.tenant = $2 AND a.status = 'Aktiv'
                 AND EXISTS (
                     SELECT 1 FROM lf_zuordnung n
                     WHERE n.malo_id = a.malo_id AND n.tenant = a.tenant
                       AND n.status = 'Angekuendigt' AND n.lf_mp_id = $3
                       AND n.tranche_id IS NOT DISTINCT FROM a.tranche_id
                 )"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(&lf_mp_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        let promoted = sqlx::query(
            r#"UPDATE lf_zuordnung
               SET status = 'Aktiv', process_id = $4, updated_at = now()
               WHERE malo_id = $1 AND tenant = $2
                 AND lf_mp_id = $3 AND status = 'Angekuendigt'"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(&lf_mp_id)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        if promoted.rows_affected() == 0 && displaced.rows_affected() == 0 {
            return Ok(false);
        }
        resync_lieferstatus(&mut *conn, malo_id, tenant).await?;
        touch(&mut *conn, malo_id, tenant, process_id).await?;
        append_history_snapshot(conn, malo_id, tenant).await?;
        Ok(true)
    }

    /// End a running assignment — `lf_mp_id = None` ends every one of them,
    /// which is what an untranchierte Marktlokation's Abmeldung means.
    ///
    /// Announced assignments are preserved, so a pending supplier switch
    /// survives the outgoing supplier's departure. `lieferende` defaults to
    /// today (Berlin civil date) when the process carries no date.
    pub async fn end_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        nb_mp_id: &str,
        lieferende: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<bool, MdmError> {
        let lieferende = lieferende.unwrap_or_else(crate::handlers::malo::today_berlin);
        ensure_root(&mut *conn, malo_id, tenant, nb_mp_id).await?;
        sqlx::query(
            r#"DELETE FROM lf_zuordnung
               WHERE malo_id = $1 AND tenant = $2 AND status = 'Aktiv'
                 AND ($3::text IS NULL OR lf_mp_id = $3)"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(lf_mp_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        resync_lieferstatus(&mut *conn, malo_id, tenant).await?;
        sqlx::query(
            r#"UPDATE versorgungsstatus
               SET lieferende = $3, nb_mp_id = $4, last_process_id = $5,
                   updated_at = now(), version = version + 1
               WHERE malo_id = $1 AND tenant = $2"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(lieferende)
        .bind(nb_mp_id)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        append_history_snapshot(conn, malo_id, tenant).await?;
        Ok(true)
    }

    /// The E/G becomes the sole supplier of record.
    ///
    /// Every announced assignment is preserved — its confirmation is what ends
    /// the fallback supply.
    #[allow(clippy::too_many_arguments)]
    pub async fn begin_eog_supply_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<bool, MdmError> {
        if !matches!(
            eog_status,
            LieferStatus::Ersatzversorgung | LieferStatus::Grundversorgung
        ) {
            return Err(MdmError::Unprocessable {
                reason: "begin_eog_supply requires Ersatzversorgung or Grundversorgung".into(),
            });
        }
        // The CHECK constraint requires eog_seit while the fallback runs;
        // default a missing start date to today in German local time — the
        // §38 Abs. 4 clock runs on the Berlin civil calendar.
        let eog_seit = eog_seit.unwrap_or_else(crate::handlers::malo::today_berlin);

        ensure_root(&mut *conn, malo_id, tenant, nb_mp_id).await?;
        sqlx::query(
            "DELETE FROM lf_zuordnung WHERE malo_id = $1 AND tenant = $2 AND status = 'Aktiv'",
        )
        .bind(malo_id)
        .bind(tenant)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;
        sqlx::query(
            r#"INSERT INTO lf_zuordnung
               (malo_id, tenant, lf_mp_id, prozent, tranche_id, status,
                zuordnungsbeginn, process_id, updated_at)
               VALUES ($1, $2, $3, 100, NULL, 'Aktiv', $4, $5, now())"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(gv_mp_id)
        .bind(eog_seit)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        sqlx::query(
            r#"UPDATE versorgungsstatus
               SET lieferstatus = $3, nb_mp_id = $4, eog_seit = $5,
                   last_process_id = $6, updated_at = now(), version = version + 1
               WHERE malo_id = $1 AND tenant = $2"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(eog_status.to_string())
        .bind(nb_mp_id)
        .bind(eog_seit)
        .bind(process_id)
        .execute(&mut *conn)
        .await
        .map_err(internal)?;

        append_history_snapshot(conn, malo_id, tenant).await?;
        Ok(true)
    }

    /// Drop a pending announcement — `lf_mp_id = None` drops every one.
    ///
    /// Also the write behind 55038 / 44038 „Aufhebung einer zukünftigen
    /// Zuordnung", which is this operation addressed at an LFZ rather than at
    /// the sender. Only touches announced assignments, so a duplicate
    /// cancellation is a genuine no-op (no version bump, no history row).
    pub async fn clear_lf_next_tx(
        conn: &mut PgConnection,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<bool, MdmError> {
        let cleared = sqlx::query(
            r#"DELETE FROM lf_zuordnung
               WHERE malo_id = $1 AND tenant = $2 AND status = 'Angekuendigt'
                 AND ($3::text IS NULL OR lf_mp_id = $3)"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(lf_mp_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        if cleared.rows_affected() == 0 {
            return Ok(false);
        }
        touch(&mut *conn, malo_id, tenant, process_id).await?;
        append_history_snapshot(conn, malo_id, tenant).await?;
        Ok(true)
    }
}

/// Replace the whole assignment list of one Marktlokation.
async fn replace_zuordnungen(
    conn: &mut PgConnection,
    malo_id: &MaloId,
    tenant: &str,
    zuordnungen: &[LfZuordnung],
) -> Result<(), MdmError> {
    sqlx::query("DELETE FROM lf_zuordnung WHERE malo_id = $1 AND tenant = $2")
        .bind(malo_id)
        .bind(tenant)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;
    for z in zuordnungen {
        sqlx::query(
            r#"INSERT INTO lf_zuordnung
               (malo_id, tenant, lf_mp_id, prozent, tranche_id, status,
                zuordnungsbeginn, zuordnungsende, process_id, updated_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(&z.lf_mp_id)
        .bind(z.prozent)
        .bind(z.tranche_id.as_deref())
        .bind(z.status.as_str())
        .bind(z.zuordnungsbeginn)
        .bind(z.zuordnungsende)
        .bind(z.process_id)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;
    }
    Ok(())
}

impl VersorgungsStatusRepository for PgVersorgungsStatusRepository {
    async fn upsert(
        &self,
        rec: VersorgungsStatusRecord,
        if_version: Option<i64>,
    ) -> Result<i64, MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        let new_version = Self::upsert_tx(&mut tx, &rec, if_version).await?;
        tx.commit().await.map_err(internal)?;
        Ok(new_version)
    }

    async fn find(
        &self,
        malo_id: &MaloId,
        tenant: &str,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        let opt = sqlx::query(&format!(
            "SELECT v.*, {ZUORDNUNGEN_JSON} FROM versorgungsstatus v
             WHERE v.malo_id = $1 AND v.tenant = $2"
        ))
        .bind(malo_id)
        .bind(tenant)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;

        opt.as_ref().map(map_row).transpose().map_err(internal)
    }

    async fn find_at(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        at: Date,
    ) -> Result<Option<VersorgungsStatusRecord>, MdmError> {
        // Find the most recent history entry whose `valid_from`, expressed in
        // German local time (CET/CEST via 'Europe/Berlin'), falls on or before `at`.
        let opt = sqlx::query(
            r#"SELECT *
               FROM versorgungsstatus_history
               WHERE malo_id = $1 AND tenant = $2
                 AND (valid_from AT TIME ZONE 'Europe/Berlin')::date <= $3
               ORDER BY valid_from DESC
               LIMIT 1"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(at)
        .fetch_optional(&self.pool)
        .await
        .map_err(internal)?;

        opt.as_ref()
            .map(map_history_row_as_current)
            .transpose()
            .map_err(internal)
    }

    async fn find_history(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusHistoryRecord>, MdmError> {
        let size = size.min(500);
        let limit = i64::from(size);
        let offset = i64::from(page) * limit;

        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM versorgungsstatus_history WHERE malo_id = $1 AND tenant = $2",
        )
        .bind(malo_id)
        .bind(tenant)
        .fetch_one(&self.pool)
        .await
        .map_err(internal)?;

        let rows = sqlx::query(
            r#"SELECT *
               FROM versorgungsstatus_history
               WHERE malo_id = $1 AND tenant = $2
               ORDER BY valid_from DESC
               LIMIT $3 OFFSET $4"#,
        )
        .bind(malo_id)
        .bind(tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;

        let items = rows
            .iter()
            .map(map_history_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        Ok(PageResult {
            items,
            total: total as u64,
            page,
            size,
        })
    }

    async fn list_by_tenant(
        &self,
        tenant: &str,
        page: u32,
        size: u32,
    ) -> Result<PageResult<VersorgungsStatusRecord>, MdmError> {
        let size = size.min(500);
        let limit = i64::from(size);
        let offset = i64::from(page) * limit;
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM versorgungsstatus WHERE tenant = $1")
                .bind(tenant)
                .fetch_one(&self.pool)
                .await
                .map_err(internal)?;

        let rows = sqlx::query(&format!(
            "SELECT v.*, {ZUORDNUNGEN_JSON} FROM versorgungsstatus v
             WHERE v.tenant = $1 ORDER BY v.malo_id LIMIT $2 OFFSET $3"
        ))
        .bind(tenant)
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(internal)?;

        let items = rows
            .iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(internal)?;

        Ok(PageResult {
            items,
            total: total as u64,
            page,
            size,
        })
    }

    async fn announce_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id_next: &str,
        lf_next_lieferbeginn: Option<Date>,
        prozent: Decimal,
        tranche_id: Option<&str>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::announce_lf_next_tx(
            &mut tx,
            malo_id,
            tenant,
            lf_mp_id_next,
            lf_next_lieferbeginn,
            prozent,
            tranche_id,
            nb_mp_id,
            process_id,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    async fn confirm_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::confirm_supply_tx(&mut tx, malo_id, tenant, lf_mp_id, process_id).await?;
        tx.commit().await.map_err(internal)
    }

    async fn end_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        nb_mp_id: &str,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::end_supply_tx(
            &mut tx, malo_id, tenant, lf_mp_id, nb_mp_id, None, process_id,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    #[allow(clippy::too_many_arguments)]
    async fn begin_eog_supply(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        gv_mp_id: &str,
        nb_mp_id: &str,
        eog_status: LieferStatus,
        eog_seit: Option<Date>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::begin_eog_supply_tx(
            &mut tx, malo_id, tenant, gv_mp_id, nb_mp_id, eog_status, eog_seit, process_id,
        )
        .await?;
        tx.commit().await.map_err(internal)
    }

    async fn clear_lf_next(
        &self,
        malo_id: &MaloId,
        tenant: &str,
        lf_mp_id: Option<&str>,
        process_id: Option<uuid::Uuid>,
    ) -> Result<(), MdmError> {
        let mut tx = self.pool.begin().await.map_err(internal)?;
        Self::clear_lf_next_tx(&mut tx, malo_id, tenant, lf_mp_id, process_id).await?;
        tx.commit().await.map_err(internal)
    }
}
