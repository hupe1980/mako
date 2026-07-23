//! PostgreSQL implementation of [`Typ2Repository`] — the ESA "Werte nach Typ 2"
//! store (`esa_typ2_reads`).
//!
//! Deliberately minimal and completely separate from
//! [`super::timeseries::PgTimeSeriesRepository`]: Typ-2 data is non-authoritative
//! (Codeliste 1.4 Kap. 4.6, WiM Strom Teil 2 §4) and must never reach a billing
//! path. There is no audit CTE, no billing-period cache, no partition
//! management, and no correction/substitution machinery — a Typ-2 value is
//! stored as delivered and read back verbatim.

use sqlx::{PgPool, Row};
use time::OffsetDateTime;

use mako_edm::{
    domain::{TimeSeriesQuery, Typ2DeliveryPath, Typ2Read},
    error::EdmError,
    repository::Typ2Repository,
};

use super::timeseries::{normalise_obis, quality_to_str, str_to_quality, str_to_sparte};

/// PostgreSQL-backed ESA Typ-2 store.
#[derive(Clone, Debug)]
pub struct PgTyp2Repository {
    pool: PgPool,
}

impl PgTyp2Repository {
    #[must_use]
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl Typ2Repository for PgTyp2Repository {
    async fn store_typ2_reads(&self, reads: &[Typ2Read]) -> Result<(), EdmError> {
        if reads.is_empty() {
            return Ok(());
        }
        let malo_ids: Vec<&str> = reads.iter().map(|r| r.malo_id.as_str()).collect();
        let melo_ids: Vec<Option<&str>> = reads.iter().map(|r| r.melo_id.as_deref()).collect();
        let dtm_froms: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_from).collect();
        let dtm_tos: Vec<OffsetDateTime> = reads.iter().map(|r| r.dtm_to).collect();
        let quantities: Vec<rust_decimal::Decimal> = reads.iter().map(|r| r.quantity_kwh).collect();
        let qualities: Vec<&str> = reads.iter().map(|r| quality_to_str(r.quality)).collect();
        let pids: Vec<i32> = reads.iter().map(|r| r.pid as i32).collect();
        let spartes: Vec<&str> = reads.iter().map(|r| r.sparte.as_str()).collect();
        let units: Vec<&str> = reads
            .iter()
            .map(|r| r.sparte.billing_unit().as_str())
            .collect();
        let obis_codes: Vec<Option<&str>> = reads.iter().map(|r| r.obis_code.as_deref()).collect();
        let obis_norms: Vec<String> = reads
            .iter()
            .map(|r| normalise_obis(r.obis_code.as_deref()))
            .collect();
        let delivery_paths: Vec<&str> = reads.iter().map(|r| r.delivery_path.as_str()).collect();
        let sender_mp_ids: Vec<Option<&str>> =
            reads.iter().map(|r| r.sender_mp_id.as_deref()).collect();
        let tenants: Vec<&str> = reads.iter().map(|r| r.tenant.as_str()).collect();

        sqlx::query(
            r"INSERT INTO esa_typ2_reads
                  (malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                   pid, sparte, unit, obis_code, obis_code_norm, delivery_path,
                   sender_mp_id, tenant)
              SELECT * FROM unnest(
                  $1::text[], $2::text[], $3::timestamptz[], $4::timestamptz[],
                  $5::numeric[], $6::text[], $7::int4[], $8::text[], $9::text[],
                  $10::text[], $11::text[], $12::text[], $13::text[], $14::text[]
              )
              ON CONFLICT (tenant, malo_id, dtm_from, obis_code_norm) DO UPDATE
                  SET quantity_kwh  = EXCLUDED.quantity_kwh,
                      quality       = EXCLUDED.quality,
                      obis_code     = COALESCE(EXCLUDED.obis_code, esa_typ2_reads.obis_code),
                      delivery_path = EXCLUDED.delivery_path,
                      sender_mp_id  = COALESCE(EXCLUDED.sender_mp_id,
                                               esa_typ2_reads.sender_mp_id),
                      received_at   = now()",
        )
        .bind(&malo_ids as &[&str])
        .bind(&melo_ids as &[Option<&str>])
        .bind(&dtm_froms)
        .bind(&dtm_tos)
        .bind(&quantities)
        .bind(&qualities as &[&str])
        .bind(&pids)
        .bind(&spartes as &[&str])
        .bind(&units as &[&str])
        .bind(&obis_codes as &[Option<&str>])
        .bind(&obis_norms)
        .bind(&delivery_paths as &[&str])
        .bind(&sender_mp_ids as &[Option<&str>])
        .bind(&tenants as &[&str])
        .execute(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;
        Ok(())
    }

    async fn query_typ2(&self, q: &TimeSeriesQuery) -> Result<Vec<Typ2Read>, EdmError> {
        let rows = sqlx::query(
            r"SELECT malo_id, melo_id, dtm_from, dtm_to, quantity_kwh, quality,
                     pid, sparte, obis_code, delivery_path, sender_mp_id, received_at
              FROM esa_typ2_reads
              WHERE malo_id  = $1
                AND dtm_from >= $2
                AND dtm_to   <= $3
                AND tenant    = $4
              ORDER BY dtm_from",
        )
        .bind(&q.malo_id)
        .bind(q.from)
        .bind(q.to)
        .bind(&q.tenant)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| EdmError::Database(e.to_string()))?;

        rows.into_iter()
            .map(|row| {
                Ok(Typ2Read {
                    malo_id: row.try_get("malo_id").map_err(db)?,
                    melo_id: row.try_get("melo_id").map_err(db)?,
                    dtm_from: row.try_get("dtm_from").map_err(db)?,
                    dtm_to: row.try_get("dtm_to").map_err(db)?,
                    quantity_kwh: row.try_get("quantity_kwh").map_err(db)?,
                    quality: str_to_quality(row.try_get::<&str, _>("quality").map_err(db)?),
                    pid: row.try_get::<i32, _>("pid").map_err(db)? as u32,
                    sparte: str_to_sparte(row.try_get::<&str, _>("sparte").map_err(db)?),
                    obis_code: row.try_get("obis_code").map_err(db)?,
                    tenant: q.tenant.clone(),
                    delivery_path: Typ2DeliveryPath::from_db_str(
                        row.try_get::<&str, _>("delivery_path").map_err(db)?,
                    ),
                    sender_mp_id: row.try_get("sender_mp_id").map_err(db)?,
                    received_at: row.try_get("received_at").map_err(db)?,
                })
            })
            .collect()
    }
}

fn db(e: sqlx::Error) -> EdmError {
    EdmError::Database(e.to_string())
}
