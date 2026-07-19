//! PostgreSQL implementations of [`MmmaPreisGasRepository`] and [`MmmPreisStromRepository`].
//!
//! Both tables are populated via REST (`PUT /api/v1/mmma-preise/gas/{year}/{month}` and
//! `PUT /api/v1/mmm-preise/strom/{year}/{month}`) and queried by `netzbilanzd` (billing
//! run) and `invoicd` (MMM position plausibility check 6).

use mako_markt::{
    error::MdmError,
    repository::{
        MmmPreisStromRecord, MmmPreisStromRepository, MmmaPreisGasRecord, MmmaPreisGasRepository,
    },
};
use rust_decimal::Decimal;
use sqlx::Row as _;
use time::Date;

// ── Gas ───────────────────────────────────────────────────────────────────────

/// PostgreSQL-backed [`MmmaPreisGasRepository`].
#[derive(Clone, Debug)]
pub struct PgMmmaPreisGasRepository {
    pool: sqlx::PgPool,
}

impl PgMmmaPreisGasRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl MmmaPreisGasRepository for PgMmmaPreisGasRepository {
    async fn upsert_gas(
        &self,
        price_month: Date,
        marktgebiet: &str,
        mehr_ct_kwh: Decimal,
        minder_ct_kwh: Decimal,
        source: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO mmma_preise_gas (price_month, marktgebiet, mehr_ct_kwh, minder_ct_kwh, source, updated_at)
               VALUES ($1, $2, $3, $4, $5, now())
               ON CONFLICT (price_month, marktgebiet) DO UPDATE
               SET mehr_ct_kwh   = EXCLUDED.mehr_ct_kwh,
                   minder_ct_kwh = EXCLUDED.minder_ct_kwh,
                   source        = EXCLUDED.source,
                   updated_at    = now()"#,
        )
        .bind(price_month)
        .bind(marktgebiet)
        .bind(mehr_ct_kwh)
        .bind(minder_ct_kwh)
        .bind(source)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_gas(
        &self,
        price_month: Date,
        marktgebiet: &str,
    ) -> Result<Option<MmmaPreisGasRecord>, MdmError> {
        let row = sqlx::query(
            r#"SELECT price_month, marktgebiet, mehr_ct_kwh, minder_ct_kwh, source, updated_at
               FROM mmma_preise_gas
               WHERE price_month = $1 AND marktgebiet = $2"#,
        )
        .bind(price_month)
        .bind(marktgebiet)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| MmmaPreisGasRecord {
            price_month: r.get("price_month"),
            marktgebiet: r.get("marktgebiet"),
            mehr_ct_kwh: r.get("mehr_ct_kwh"),
            minder_ct_kwh: r.get("minder_ct_kwh"),
            source: r.get("source"),
            updated_at: r.get("updated_at"),
        }))
    }

    async fn list_gas(&self, limit: i64) -> Result<Vec<MmmaPreisGasRecord>, MdmError> {
        let rows = sqlx::query(
            r#"SELECT price_month, marktgebiet, mehr_ct_kwh, minder_ct_kwh, source, updated_at
               FROM mmma_preise_gas
               ORDER BY price_month DESC, marktgebiet
               LIMIT $1"#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| MmmaPreisGasRecord {
                price_month: r.get("price_month"),
                marktgebiet: r.get("marktgebiet"),
                mehr_ct_kwh: r.get("mehr_ct_kwh"),
                minder_ct_kwh: r.get("minder_ct_kwh"),
                source: r.get("source"),
                updated_at: r.get("updated_at"),
            })
            .collect())
    }
}

// ── Strom ─────────────────────────────────────────────────────────────────────

/// PostgreSQL-backed [`MmmPreisStromRepository`].
#[derive(Clone, Debug)]
pub struct PgMmmPreisStromRepository {
    pool: sqlx::PgPool,
}

impl PgMmmPreisStromRepository {
    #[must_use]
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

impl MmmPreisStromRepository for PgMmmPreisStromRepository {
    async fn upsert_strom(
        &self,
        price_month: Date,
        vnb_mp_id: &str,
        mehr_ct_kwh: Decimal,
        minder_ct_kwh: Decimal,
        source: &str,
    ) -> Result<(), MdmError> {
        sqlx::query(
            r#"INSERT INTO mmm_preise_strom (price_month, vnb_mp_id, mehr_ct_kwh, minder_ct_kwh, source, updated_at)
               VALUES ($1, $2, $3, $4, $5, now())
               ON CONFLICT (price_month, vnb_mp_id) DO UPDATE
               SET mehr_ct_kwh   = EXCLUDED.mehr_ct_kwh,
                   minder_ct_kwh = EXCLUDED.minder_ct_kwh,
                   source        = EXCLUDED.source,
                   updated_at    = now()"#,
        )
        .bind(price_month)
        .bind(vnb_mp_id)
        .bind(mehr_ct_kwh)
        .bind(minder_ct_kwh)
        .bind(source)
        .execute(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;
        Ok(())
    }

    async fn find_strom(
        &self,
        price_month: Date,
        vnb_mp_id: &str,
    ) -> Result<Option<MmmPreisStromRecord>, MdmError> {
        let row = sqlx::query(
            r#"SELECT price_month, vnb_mp_id, mehr_ct_kwh, minder_ct_kwh, source, updated_at
               FROM mmm_preise_strom
               WHERE price_month = $1 AND vnb_mp_id = $2"#,
        )
        .bind(price_month)
        .bind(vnb_mp_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| MdmError::Internal(e.to_string()))?;

        Ok(row.map(|r| MmmPreisStromRecord {
            price_month: r.get("price_month"),
            vnb_mp_id: r.get("vnb_mp_id"),
            mehr_ct_kwh: r.get("mehr_ct_kwh"),
            minder_ct_kwh: r.get("minder_ct_kwh"),
            source: r.get("source"),
            updated_at: r.get("updated_at"),
        }))
    }
}
