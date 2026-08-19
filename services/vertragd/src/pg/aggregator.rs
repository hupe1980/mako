//! § 41e EnWG Aggregatorverträge (Art. 17 RL (EU) 2019/944).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row as _};
use time::Date;
use uuid::Uuid;

/// A § 41e EnWG Aggregatorvertrag: SR-ID → agreed capacity price and validity.
///
/// `billingd` reads this when settling a `de.vpp.dispatch.confirmed` event; it
/// keeps no copy of the contract.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct AggregatorvertragRow {
    pub id: Uuid,
    pub tenant: String,
    pub sr_id: String,
    pub vpp_id: String,
    pub malo_id: String,
    pub aggregator_mp_id: String,
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    pub vertragsbeginn: Date,
    pub vertragsende: Option<Date>,
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
    pub kunden_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Fields accepted when creating or replacing an Aggregatorvertrag.
#[derive(Debug, Clone, Deserialize)]
pub struct UpsertAggregatorvertragInput {
    pub vpp_id: String,
    pub malo_id: String,
    pub aggregator_mp_id: String,
    pub capacity_price_eur_per_kwh: rust_decimal::Decimal,
    pub vertragsbeginn: Date,
    #[serde(default)]
    pub vertragsende: Option<Date>,
    #[serde(default)]
    pub mwst_rate_override: Option<rust_decimal::Decimal>,
    #[serde(default)]
    pub kunden_id: Option<Uuid>,
}

const AGG_COLS: &str = "id, tenant, sr_id, vpp_id, malo_id, aggregator_mp_id,
     capacity_price_eur_per_kwh, vertragsbeginn, vertragsende,
     mwst_rate_override, kunden_id, updated_at";

/// Upsert an Aggregatorvertrag, keyed on `(tenant, sr_id, vertragsbeginn)`.
///
/// The `agg_no_overlap` exclusion constraint rejects a validity window that
/// overlaps an existing contract for the same SR — surfaced to the caller as a
/// conflict rather than silently creating a second active contract.
///
/// # Errors
///
/// Propagates storage errors, including the exclusion violation.
pub async fn upsert_aggregatorvertrag(
    pool: &PgPool,
    tenant: &str,
    sr_id: &str,
    input: &UpsertAggregatorvertragInput,
) -> Result<Uuid> {
    let r = sqlx::query(
        r"INSERT INTO aggregatorvertraege
              (tenant, sr_id, vpp_id, malo_id, aggregator_mp_id,
               capacity_price_eur_per_kwh, vertragsbeginn, vertragsende,
               mwst_rate_override, kunden_id)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          ON CONFLICT (tenant, sr_id, vertragsbeginn) DO UPDATE
          SET vpp_id                     = EXCLUDED.vpp_id,
              malo_id                    = EXCLUDED.malo_id,
              aggregator_mp_id           = EXCLUDED.aggregator_mp_id,
              capacity_price_eur_per_kwh = EXCLUDED.capacity_price_eur_per_kwh,
              vertragsende               = EXCLUDED.vertragsende,
              mwst_rate_override         = EXCLUDED.mwst_rate_override,
              kunden_id                  = EXCLUDED.kunden_id,
              updated_at                 = now()
          RETURNING id",
    )
    .bind(tenant)
    .bind(sr_id)
    .bind(&input.vpp_id)
    .bind(&input.malo_id)
    .bind(&input.aggregator_mp_id)
    .bind(input.capacity_price_eur_per_kwh)
    .bind(input.vertragsbeginn)
    .bind(input.vertragsende)
    .bind(input.mwst_rate_override)
    .bind(input.kunden_id)
    .fetch_one(pool)
    .await?;
    Ok(r.try_get("id")?)
}

/// The Aggregatorvertrag in force for `sr_id` on `on_date`, if any.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn find_active_aggregatorvertrag(
    pool: &PgPool,
    tenant: &str,
    sr_id: &str,
    on_date: Date,
) -> Result<Option<AggregatorvertragRow>> {
    Ok(sqlx::query_as::<_, AggregatorvertragRow>(&format!(
        r"SELECT {AGG_COLS}
          FROM aggregatorvertraege
          WHERE tenant = $1
            AND sr_id  = $2
            AND vertragsbeginn <= $3
            AND (vertragsende IS NULL OR vertragsende > $3)
          ORDER BY vertragsbeginn DESC
          LIMIT 1"
    ))
    .bind(tenant)
    .bind(sr_id)
    .bind(on_date)
    .fetch_optional(pool)
    .await?)
}

/// Every Aggregatorvertrag for a tenant, newest validity first.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn list_aggregatorvertraege(
    pool: &PgPool,
    tenant: &str,
) -> Result<Vec<AggregatorvertragRow>> {
    Ok(sqlx::query_as::<_, AggregatorvertragRow>(&format!(
        r"SELECT {AGG_COLS}
          FROM aggregatorvertraege
          WHERE tenant = $1
          ORDER BY sr_id, vertragsbeginn DESC"
    ))
    .bind(tenant)
    .fetch_all(pool)
    .await?)
}
