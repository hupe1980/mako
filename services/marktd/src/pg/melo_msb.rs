//! PostgreSQL implementation of `MeloMsbRepository` — the per-Messlokation
//! dated MSB timeline (WiM Teil 2 UC 4.1.1 historical Werteanfrage routing).

use mako_markt::{
    error::MdmError,
    repository::{MeloMsbRepository, MeloMsbZuordnung},
};
use sqlx::{PgPool, Row, postgres::PgRow};
use time::Date;

#[derive(Clone, Debug)]
pub struct PgMeloMsbRepository {
    pool: PgPool,
}

impl PgMeloMsbRepository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn map_row(row: &PgRow) -> Result<MeloMsbZuordnung, sqlx::Error> {
    Ok(MeloMsbZuordnung {
        melo_id: row.try_get("melo_id")?,
        msb_mp_id: row.try_get("msb_mp_id")?,
        valid_from: row.try_get("valid_from")?,
        valid_to: row.try_get("valid_to")?,
        tenant: row.try_get("tenant")?,
    })
}

impl PgMeloMsbRepository {
    /// [`MeloMsbRepository::assign_msb`] on a caller-supplied connection.
    ///
    /// The WiM Zuordnung is derived inside `event_ingest`'s transaction,
    /// alongside the idempotency marker and the durable outbox rows: a
    /// Zuordnung that committed while the marker rolled back would be applied
    /// twice on redelivery, and one that rolled back while the marker
    /// committed would never be applied at all.
    ///
    /// # Errors
    ///
    /// [`MdmError::Internal`] on any SQL failure.
    pub async fn assign_msb_tx(
        conn: &mut sqlx::PgConnection,
        tenant: &str,
        melo_id: &str,
        msb_mp_id: &str,
        valid_from: Date,
    ) -> Result<(), MdmError> {
        // Close the currently-open assignment at the new start date. Guarded on
        // `valid_from < $new` so a same-day overwrite (handled by the upsert
        // below) does not create a zero-length closed row.
        sqlx::query(
            r"UPDATE melo_msb_zuordnungen
              SET valid_to = $3, updated_at = now()
              WHERE tenant = $1 AND melo_id = $2
                AND valid_to IS NULL AND valid_from < $3",
        )
        .bind(tenant)
        .bind(melo_id)
        .bind(valid_from)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        // A backdated correction must not leave two open assignments: the new
        // row ends where the next one starts (half-open `[from, to)`), so the
        // later assignment stays the open one.
        let next_start: Option<Date> = sqlx::query_scalar(
            r"SELECT MIN(valid_from) FROM melo_msb_zuordnungen
              WHERE tenant = $1 AND melo_id = $2 AND valid_from > $3",
        )
        .bind(tenant)
        .bind(melo_id)
        .bind(valid_from)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        sqlx::query(
            r"INSERT INTO melo_msb_zuordnungen (tenant, melo_id, msb_mp_id, valid_from, valid_to)
              VALUES ($1, $2, $3, $4, $5)
              ON CONFLICT (tenant, melo_id, valid_from) DO UPDATE
              SET msb_mp_id = EXCLUDED.msb_mp_id,
                  valid_to = EXCLUDED.valid_to,
                  updated_at = now()",
        )
        .bind(tenant)
        .bind(melo_id)
        .bind(msb_mp_id)
        .bind(valid_from)
        .bind(next_start)
        .execute(&mut *conn)
        .await
        .map_err(super::write_error)?;

        Ok(())
    }
}

impl MeloMsbRepository for PgMeloMsbRepository {
    async fn assign_msb(
        &self,
        tenant: &str,
        melo_id: &str,
        msb_mp_id: &str,
        valid_from: Date,
    ) -> Result<(), MdmError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Self::assign_msb_tx(&mut tx, tenant, melo_id, msb_mp_id, valid_from).await?;
        tx.commit()
            .await
            .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_msb_at(
        &self,
        tenant: &str,
        melo_id: &str,
        at: Date,
    ) -> Result<Option<String>, MdmError> {
        let row: Option<(String,)> = sqlx::query_as(
            r"SELECT msb_mp_id FROM melo_msb_zuordnungen
              WHERE tenant = $1 AND melo_id = $2
                AND valid_from <= $3 AND (valid_to IS NULL OR valid_to > $3)
              ORDER BY valid_from DESC
              LIMIT 1",
        )
        .bind(tenant)
        .bind(melo_id)
        .bind(at)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(row.map(|(m,)| m))
    }

    async fn history(
        &self,
        tenant: &str,
        melo_id: &str,
    ) -> Result<Vec<MeloMsbZuordnung>, MdmError> {
        let rows = sqlx::query(
            r"SELECT tenant, melo_id, msb_mp_id, valid_from, valid_to
              FROM melo_msb_zuordnungen
              WHERE tenant = $1 AND melo_id = $2
              ORDER BY valid_from DESC",
        )
        .bind(tenant)
        .bind(melo_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        rows.iter()
            .map(map_row)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MdmError::Internal(e.to_string()))
    }
}
