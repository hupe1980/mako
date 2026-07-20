//! PostgreSQL persistence for `billingd`.
#![allow(clippy::too_many_arguments)]

use anyhow::Context as _;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

/// Stored billing record returned by GET endpoints.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct BillingRecordRow {
    pub id: Uuid,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub product_code: String,
    pub category: String,
    pub period_from: Date,
    pub period_to: Date,
    pub rechnung_json: serde_json::Value,
    pub bo4e_version: String,
    pub total_netto_eur: Option<Decimal>,
    pub total_brutto_eur: Option<Decimal>,
    pub outcome: String,
    pub ce_id: Option<Uuid>,
    pub dispatched_at: Option<OffsetDateTime>,
    /// TRUE = Stornorechnung / Korrekturrechnung (migration 0002).
    pub is_correction: bool,
    /// FK to the original record being corrected (migration 0002).
    pub original_record_id: Option<Uuid>,
    /// Human-readable correction reason stored in the record (migration 0002).
    pub correction_reason: Option<String>,
    /// FK to the Sammelrechnung this record is grouped under (migration 0002).
    pub sammelrechnung_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

pub async fn insert_billing_record(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    product_code: &str,
    category: &str,
    period_from: Date,
    period_to: Date,
    rechnung_json: &serde_json::Value,
    total_netto_eur: Decimal,
    total_brutto_eur: Decimal,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO billing_records
              (malo_id, lf_mp_id, product_code, category, period_from, period_to,
               rechnung_json, total_netto_eur, total_brutto_eur)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
          ON CONFLICT (malo_id, lf_mp_id, period_from, period_to, product_code) DO UPDATE
          SET rechnung_json   = EXCLUDED.rechnung_json,
              total_netto_eur = EXCLUDED.total_netto_eur,
              total_brutto_eur= EXCLUDED.total_brutto_eur,
              updated_at      = now()
          RETURNING id",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(category)
    .bind(period_from)
    .bind(period_to)
    .bind(rechnung_json)
    .bind(total_netto_eur)
    .bind(total_brutto_eur)
    .fetch_one(pool)
    .await
    .context("insert_billing_record")?;

    Ok(row.try_get("id")?)
}

pub async fn fetch_billing_record(
    pool: &PgPool,
    id: Uuid,
) -> anyhow::Result<Option<BillingRecordRow>> {
    sqlx::query_as::<_, BillingRecordRow>("SELECT * FROM billing_records WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await
        .context("fetch_billing_record")
}

pub async fn list_billing_records(
    pool: &PgPool,
    malo_id: Option<&str>,
    lf_mp_id: Option<&str>,
    outcome: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<BillingRecordRow>> {
    sqlx::query_as::<_, BillingRecordRow>(
        r"SELECT * FROM billing_records
          WHERE ($1::text IS NULL OR malo_id = $1)
            AND ($2::text IS NULL OR lf_mp_id = $2)
            AND ($3::text IS NULL OR outcome = $3)
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(outcome)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_billing_records")
}

pub async fn mark_dispatched(pool: &PgPool, id: Uuid, ce_id: Uuid) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE billing_records
          SET outcome = 'dispatched', ce_id = $2, dispatched_at = now(), updated_at = now()
          WHERE id = $1",
    )
    .bind(id)
    .bind(ce_id)
    .execute(pool)
    .await
    .context("mark_dispatched")?;
    Ok(())
}

// ── Korrekturrechnung + Sammelrechnung (migration 0002) ───────────────────────

/// Insert a correction (Stornorechnung / Korrekturrechnung) record linked to an original.
///
/// Sets `is_correction = TRUE` and `original_record_id`.
/// The `rechnung_json` must already have `istOriginal: false` and
/// `originalRechnungsnummer` set by the caller; monetary amounts must already be negated.
pub async fn insert_correction_record(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    product_code: &str,
    category: &str,
    period_from: Date,
    period_to: Date,
    rechnung_json: &serde_json::Value,
    total_netto_eur: Decimal,
    total_brutto_eur: Decimal,
    original_record_id: Uuid,
    correction_reason: Option<&str>,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO billing_records
              (malo_id, lf_mp_id, product_code, category, period_from, period_to,
               rechnung_json, total_netto_eur, total_brutto_eur,
               is_correction, original_record_id, correction_reason)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, TRUE, $10, $11)
          RETURNING id",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(category)
    .bind(period_from)
    .bind(period_to)
    .bind(rechnung_json)
    .bind(total_netto_eur)
    .bind(total_brutto_eur)
    .bind(original_record_id)
    .bind(correction_reason)
    .fetch_one(pool)
    .await
    .context("insert_correction_record")?;

    Ok(row.try_get("id")?)
}

/// Insert a consolidated Sammelrechnung record (category = "SAMMEL").
///
/// Individual per-MaLo records are linked back via `sammelrechnung_id`.
pub async fn insert_sammelrechnung_record(
    pool: &PgPool,
    rahmenvertrag_id: &str,
    lf_mp_id: &str,
    period_from: Date,
    period_to: Date,
    rechnung_json: &serde_json::Value,
    total_netto_eur: Decimal,
    total_brutto_eur: Decimal,
) -> anyhow::Result<Uuid> {
    let row = sqlx::query(
        r"INSERT INTO billing_records
              (malo_id, lf_mp_id, product_code, category, period_from, period_to,
               rechnung_json, total_netto_eur, total_brutto_eur)
          VALUES ($1, $2, $3, 'SAMMEL', $4, $5, $6, $7, $8)
          RETURNING id",
    )
    .bind(rahmenvertrag_id)
    .bind(lf_mp_id)
    .bind(format!("SAMMEL-{rahmenvertrag_id}"))
    .bind(period_from)
    .bind(period_to)
    .bind(rechnung_json)
    .bind(total_netto_eur)
    .bind(total_brutto_eur)
    .fetch_one(pool)
    .await
    .context("insert_sammelrechnung_record")?;

    Ok(row.try_get("id")?)
}

/// Tag each per-MaLo record as belonging to a Sammelrechnung.
pub async fn link_to_sammelrechnung(
    pool: &PgPool,
    record_ids: &[Uuid],
    sammelrechnung_id: Uuid,
) -> anyhow::Result<()> {
    for &id in record_ids {
        sqlx::query(
            "UPDATE billing_records SET sammelrechnung_id = $2, updated_at = now() WHERE id = $1",
        )
        .bind(id)
        .bind(sammelrechnung_id)
        .execute(pool)
        .await
        .context("link_to_sammelrechnung")?;
    }
    Ok(())
}

// ── Billing Anomaly Detection (B6 / L1) ──────────────────────────────────────

/// Rolling 3-month baseline and deviation for a MaLo's billing amounts.
///
/// Used by the `billing-anomaly-agent` in `agentd` to detect invoices that
/// deviate >20 % from the rolling baseline — powercloud's headline AI feature.
#[allow(dead_code)]
#[derive(Debug, serde::Serialize)]
pub struct BillingAnomalyReport {
    pub malo_id: String,
    pub lf_mp_id: String,
    /// Latest non-correction billing record id.
    pub latest_record_id: Option<Uuid>,
    /// Latest total_brutto_eur.
    pub latest_brutto_eur: Option<Decimal>,
    /// Rolling average of the prior N records (up to 3).
    pub rolling_avg_brutto_eur: Option<Decimal>,
    /// Deviation as percentage: `(latest - avg) / avg * 100`.
    /// Positive = over-billing; negative = under-billing.
    pub deviation_pct: Option<Decimal>,
    /// `true` when `|deviation_pct| > threshold_pct`.
    pub is_anomaly: bool,
    /// Number of historical records used for the average (0 = insufficient baseline).
    pub sample_count: i64,
    /// Anomaly threshold used (default 20.0 %).
    pub threshold_pct: Decimal,
}

/// Compute the rolling billing anomaly score for a MaLo.
///
/// Compares the most recent original (non-correction) record against the rolling
/// average of the 3 preceding records.  Returns a report with `is_anomaly = false`
/// and `sample_count = 0` when there are fewer than 2 historical records.
#[allow(dead_code)]
pub async fn check_billing_anomaly(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    threshold_pct: Option<Decimal>,
) -> anyhow::Result<BillingAnomalyReport> {
    use rust_decimal::dec;
    let threshold = threshold_pct.unwrap_or(dec!(20));

    let rows = sqlx::query(
        r"SELECT id, total_brutto_eur
          FROM billing_records
          WHERE malo_id = $1
            AND lf_mp_id = $2
            AND is_correction = FALSE
            AND total_brutto_eur IS NOT NULL
            AND total_brutto_eur > 0
          ORDER BY created_at DESC
          LIMIT 4",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_all(pool)
    .await
    .context("check_billing_anomaly")?;

    if rows.is_empty() {
        return Ok(BillingAnomalyReport {
            malo_id: malo_id.to_owned(),
            lf_mp_id: lf_mp_id.to_owned(),
            latest_record_id: None,
            latest_brutto_eur: None,
            rolling_avg_brutto_eur: None,
            deviation_pct: None,
            is_anomaly: false,
            sample_count: 0,
            threshold_pct: threshold,
        });
    }

    let latest_id: Uuid = rows[0].try_get("id")?;
    let latest: Decimal = rows[0].try_get("total_brutto_eur")?;

    let prior: Vec<Decimal> = rows[1..]
        .iter()
        .filter_map(|r| r.try_get::<Decimal, _>("total_brutto_eur").ok())
        .collect();

    if prior.is_empty() {
        return Ok(BillingAnomalyReport {
            malo_id: malo_id.to_owned(),
            lf_mp_id: lf_mp_id.to_owned(),
            latest_record_id: Some(latest_id),
            latest_brutto_eur: Some(latest),
            rolling_avg_brutto_eur: None,
            deviation_pct: None,
            is_anomaly: false,
            sample_count: 0,
            threshold_pct: threshold,
        });
    }

    let sum: Decimal = prior.iter().copied().sum();
    let count = Decimal::from(prior.len() as u64);
    let avg = sum / count;
    let deviation_pct = if avg > Decimal::ZERO {
        ((latest - avg) / avg) * dec!(100)
    } else {
        Decimal::ZERO
    };
    let is_anomaly = deviation_pct.abs() > threshold;

    Ok(BillingAnomalyReport {
        malo_id: malo_id.to_owned(),
        lf_mp_id: lf_mp_id.to_owned(),
        latest_record_id: Some(latest_id),
        latest_brutto_eur: Some(latest),
        rolling_avg_brutto_eur: Some(avg.round_dp(2)),
        deviation_pct: Some(deviation_pct.round_dp(2)),
        is_anomaly,
        sample_count: prior.len() as i64,
        threshold_pct: threshold,
    })
}

// ── VPP Contract Registry (migration 0002) ───────────────────────────────────

/// Stored VPP contract record.
///
/// Maps a `SteuerbareRessource` (SR-ID) to the billing parameters for
/// automatic VPP settlement triggered by `de.vpp.dispatch.confirmed` events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, sqlx::FromRow)]
pub struct VppContractRow {
    pub id: Uuid,
    pub sr_id: String,
    pub vpp_id: String,
    pub malo_id: String,
    pub lf_mp_id: String,
    pub capacity_price_eur_per_kwh: Decimal,
    pub valid_from: Date,
    pub valid_to: Option<Date>,
    pub mwst_rate_override: Option<Decimal>,
    pub tenant: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Upsert a VPP contract (idempotent on `sr_id + tenant + valid_from`).
pub async fn upsert_vpp_contract(pool: &PgPool, row: &VppContractRow) -> anyhow::Result<Uuid> {
    let r = sqlx::query(
        r"INSERT INTO vpp_contracts
              (id, sr_id, vpp_id, malo_id, lf_mp_id, capacity_price_eur_per_kwh,
               valid_from, valid_to, mwst_rate_override, tenant)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
          ON CONFLICT (sr_id, tenant, valid_from) DO UPDATE
          SET vpp_id                      = EXCLUDED.vpp_id,
              malo_id                     = EXCLUDED.malo_id,
              lf_mp_id                    = EXCLUDED.lf_mp_id,
              capacity_price_eur_per_kwh  = EXCLUDED.capacity_price_eur_per_kwh,
              valid_to                    = EXCLUDED.valid_to,
              mwst_rate_override          = EXCLUDED.mwst_rate_override,
              updated_at                  = now()
          RETURNING id",
    )
    .bind(row.id)
    .bind(&row.sr_id)
    .bind(&row.vpp_id)
    .bind(&row.malo_id)
    .bind(&row.lf_mp_id)
    .bind(row.capacity_price_eur_per_kwh)
    .bind(row.valid_from)
    .bind(row.valid_to)
    .bind(row.mwst_rate_override)
    .bind(&row.tenant)
    .fetch_one(pool)
    .await
    .context("upsert_vpp_contract")?;
    Ok(r.try_get("id")?)
}

/// Find the active VPP contract for an SR-ID on a given date.
///
/// Returns the most-recently-started contract that is still valid
/// (`valid_from ≤ on_date` AND `valid_to IS NULL OR valid_to > on_date`).
pub async fn find_active_vpp_contract(
    pool: &PgPool,
    sr_id: &str,
    tenant: &str,
    on_date: Date,
) -> anyhow::Result<Option<VppContractRow>> {
    sqlx::query_as::<_, VppContractRow>(
        r"SELECT id, sr_id, vpp_id, malo_id, lf_mp_id,
                 capacity_price_eur_per_kwh, valid_from, valid_to,
                 mwst_rate_override, tenant, updated_at
          FROM vpp_contracts
          WHERE sr_id = $1
            AND tenant = $2
            AND valid_from <= $3
            AND (valid_to IS NULL OR valid_to > $3)
          ORDER BY valid_from DESC
          LIMIT 1",
    )
    .bind(sr_id)
    .bind(tenant)
    .bind(on_date)
    .fetch_optional(pool)
    .await
    .context("find_active_vpp_contract")
}

/// List all VPP contracts for a tenant.
pub async fn list_vpp_contracts(
    pool: &PgPool,
    tenant: &str,
) -> anyhow::Result<Vec<VppContractRow>> {
    sqlx::query_as::<_, VppContractRow>(
        r"SELECT id, sr_id, vpp_id, malo_id, lf_mp_id,
                 capacity_price_eur_per_kwh, valid_from, valid_to,
                 mwst_rate_override, tenant, updated_at
          FROM vpp_contracts
          WHERE tenant = $1
          ORDER BY sr_id, valid_from DESC",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_vpp_contracts")
}

/// Check if a `tx_id` has already been processed (idempotency guard).
pub async fn is_vpp_dispatch_processed(
    pool: &PgPool,
    tx_id: &str,
    tenant: &str,
) -> anyhow::Result<bool> {
    let row = sqlx::query("SELECT 1 FROM vpp_dispatch_ledger WHERE tx_id = $1 AND tenant = $2")
        .bind(tx_id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("is_vpp_dispatch_processed")?;
    Ok(row.is_some())
}

/// Record a processed VPP dispatch for idempotency.
pub async fn record_vpp_dispatch(
    pool: &PgPool,
    tx_id: &str,
    tenant: &str,
    record_id: Option<Uuid>,
) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO vpp_dispatch_ledger (tx_id, tenant, record_id)
          VALUES ($1, $2, $3)
          ON CONFLICT (tx_id, tenant) DO NOTHING",
    )
    .bind(tx_id)
    .bind(tenant)
    .bind(record_id)
    .execute(pool)
    .await
    .context("record_vpp_dispatch")?;
    Ok(())
}
