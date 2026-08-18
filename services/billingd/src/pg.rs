//! PostgreSQL persistence for `billingd`.
#![allow(clippy::too_many_arguments)]

use anyhow::Context as _;
use energy_billing::RoundMoney;
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
    /// § 14 Abs. 4 Nr. 4 UStG — the einmalige invoice number, unique per tenant.
    pub rechnungsnummer: String,
    pub period_from: Date,
    pub period_to: Date,
    pub rechnung_json: serde_json::Value,
    pub bo4e_version: String,
    /// EN 16931 semantic invoice model (serde), rendered to XRechnung/CII/UBL.
    /// `None` for records written before the model was attached.
    pub en16931_json: Option<serde_json::Value>,
    pub total_netto_eur: Option<Decimal>,
    pub total_brutto_eur: Option<Decimal>,
    pub outcome: String,
    pub dispatched_at: Option<OffsetDateTime>,
    /// TRUE = Stornorechnung / Korrekturrechnung.
    pub is_correction: bool,
    /// The original record this one corrects.
    pub original_record_id: Option<Uuid>,
    /// Human-readable correction reason.
    pub correction_reason: Option<String>,
    /// The consolidated document (SAMMEL) this record is grouped under.
    pub sammelrechnung_id: Option<Uuid>,
    /// Deterministic risk score 0–100 (release gate). NULL = not yet scored.
    pub risk_score: Option<i16>,
    /// AUTO_RELEASED | SAMPLE | REVIEW | HELD.
    pub risk_band: Option<String>,
    /// Coded findings explaining the score (XAI by construction).
    pub risk_findings: Option<serde_json::Value>,
    /// The outputd template hash that rendered this invoice's PDF, pinned
    /// on first render and never changed afterwards. `None` until the document
    /// has been rendered at all.
    pub template_hash: Option<String>,
    /// Analyst who released a HELD record.
    pub released_by: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub released_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// A document about to be written to `billing_records`.
///
/// One struct instead of eleven positional arguments: every invoice-producing
/// path (calculate, Tarifwechsel, GGV, Sammelrechnung, VPP, the §40b sweep)
/// fills the same fields, and a struct literal names them at the call site.
#[derive(Debug, Clone, Copy)]
pub struct NewBillingRecord<'a> {
    pub tenant: &'a str,
    pub malo_id: &'a str,
    pub lf_mp_id: &'a str,
    pub product_code: &'a str,
    pub category: &'a str,
    /// § 14 Abs. 4 Nr. 4 UStG — must be einmalig within the tenant.
    pub rechnungsnummer: &'a str,
    pub period_from: Date,
    pub period_to: Date,
    pub rechnung_json: &'a serde_json::Value,
    pub total_netto_eur: Decimal,
    pub total_brutto_eur: Decimal,
}

/// Why a record could not be written.
///
/// The two refusals a caller can act on are named, so the HTTP layer answers
/// `409` with the record that already occupies the slot instead of a `500` with
/// a database string. Everything else is genuinely unexpected.
#[derive(Debug, thiserror::Error)]
pub enum InsertError {
    /// The period already carries an issued document. Correct it and re-bill —
    /// a re-run may replace a withheld record, never something the customer
    /// received.
    #[error(
        "MaLo {malo_id} product {product_code} already carries an issued document for \
         {period_from}..{period_to}; storno it (POST /api/v1/billing/{{id}}/correction) \
         and re-bill the released period"
    )]
    PeriodAlreadyIssued {
        malo_id: String,
        product_code: String,
        period_from: Date,
        period_to: Date,
    },
    /// § 14 Abs. 4 Nr. 4 UStG: this tenant already issued that number.
    #[error("Rechnungsnummer {0} is already used by another document of this tenant")]
    DuplicateRechnungsnummer(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

/// Allocate the next § 14 Abs. 4 Nr. 4 UStG Rechnungsnummer of a series.
///
/// `series` is the document class — `RE` ordinary invoice, `SR` consolidated
/// document, `ST` Storno/Korrektur, `VG` § 41e Gutschrift — and the counter runs
/// per tenant and calendar year, so the number reads `RE-2026-000123`.
///
/// A number derived from the billed facts (`BILL-{malo}-{product}-{period_from}`)
/// would be neither fortlaufend nor re-issuable: re-billing a cancelled period
/// reproduces the cancelled original's own string, `br_unique_rechnungsnummer`
/// refuses it, and the Storno-und-Neuberechnung flow the correction endpoint
/// recommends cannot be performed at all.
///
/// The upsert takes a row lock, so two concurrent runs of the same tenant
/// serialise here and no number is ever issued twice. A number is allocated
/// before the engine runs, so a refused calculation leaves a gap — legal, and
/// the cost of having the number available as BT-1 while the document is built.
///
/// # Errors
///
/// Only if the counter cannot be read or written.
pub async fn allocate_rechnungsnummer(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    series: &str,
    year: i32,
) -> anyhow::Result<String> {
    let value: i64 = sqlx::query_scalar(
        r"INSERT INTO invoice_number_series AS s (tenant, series, year, last_value)
          VALUES ($1, $2, $3, 1)
          ON CONFLICT (tenant, series, year)
              DO UPDATE SET last_value = s.last_value + 1, updated_at = now()
          RETURNING last_value",
    )
    .bind(tenant)
    .bind(series)
    .bind(i16::try_from(year).context("year out of range")?)
    .fetch_one(executor)
    .await
    .context("allocate_rechnungsnummer")?;
    Ok(format!("{series}-{year}-{value:06}"))
}

/// Insert an original document, replacing a draft for the same period.
///
/// `br_unique_original` is a *partial* index, so the `ON CONFLICT` must name
/// all six of its columns — `tenant` included — and repeat its predicate;
/// PostgreSQL cannot infer a partial index from a column list alone.
///
/// A re-run may replace a **withheld** record, never one that has been issued:
/// once `outcome` leaves `'generated'` the stored Rechnung is what the
/// counterparty received, and rewriting it breaks the § 147 AO audit trail.
/// Such a period is corrected through [`insert_correction_record`] instead.
///
/// Replacing a draft **clears every derived column** — the EN 16931 model, the
/// risk band and its findings, any release stamp — so a stale `HELD` band
/// cannot outlive the invoice it was scored against. The callers re-derive them
/// in this same transaction when they have a value.
///
/// # Errors
///
/// [`InsertError::PeriodAlreadyIssued`] when the guard refused the overwrite —
/// naming the document that holds the period — or
/// [`InsertError::DuplicateRechnungsnummer`] when the number collides with
/// another document of the same tenant (§ 14 Abs. 4 Nr. 4 UStG).
pub async fn insert_billing_record(
    executor: impl sqlx::PgExecutor<'_>,
    rec: &NewBillingRecord<'_>,
) -> Result<Uuid, InsertError> {
    let row = sqlx::query(
        r"INSERT INTO billing_records
              (tenant, malo_id, lf_mp_id, product_code, category, rechnungsnummer,
               period_from, period_to, rechnung_json, total_netto_eur, total_brutto_eur)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
          ON CONFLICT (malo_id, lf_mp_id, period_from, period_to, product_code, tenant)
              WHERE is_correction = false
                AND sammelrechnung_id IS NULL
                AND outcome <> 'cancelled'
                AND category <> 'VPP'
              DO UPDATE
          SET rechnungsnummer = EXCLUDED.rechnungsnummer,
              category        = EXCLUDED.category,
              rechnung_json   = EXCLUDED.rechnung_json,
              total_netto_eur = EXCLUDED.total_netto_eur,
              total_brutto_eur= EXCLUDED.total_brutto_eur,
              -- Every derived column describes the calculation being replaced,
              -- and none is re-derived unconditionally — `assess_risk` returns
              -- None whenever the gate is off or its history query fails. The
              -- callers re-attach these in this same transaction when they have
              -- a value; NULL is the honest state until they do.
              en16931_json    = NULL,
              risk_score      = NULL,
              risk_band       = NULL,
              risk_findings   = NULL,
              released_by     = NULL,
              released_at     = NULL,
              updated_at      = now()
          WHERE billing_records.outcome = 'generated'
          RETURNING id",
    )
    .bind(rec.tenant)
    .bind(rec.malo_id)
    .bind(rec.lf_mp_id)
    .bind(rec.product_code)
    .bind(rec.category)
    .bind(rec.rechnungsnummer)
    .bind(rec.period_from)
    .bind(rec.period_to)
    .bind(rec.rechnung_json)
    .bind(rec.total_netto_eur)
    .bind(rec.total_brutto_eur)
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        match e
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::constraint)
        {
            Some("br_unique_rechnungsnummer") => {
                InsertError::DuplicateRechnungsnummer(rec.rechnungsnummer.to_owned())
            }
            _ => InsertError::Other(anyhow::Error::new(e).context("insert_billing_record")),
        }
    })?;

    match row {
        Some(row) => Ok(row
            .try_get("id")
            .map_err(|e| InsertError::Other(anyhow::Error::new(e)))?),
        // The guard refused the overwrite: the period already left the house.
        None => Err(InsertError::PeriodAlreadyIssued {
            malo_id: rec.malo_id.to_owned(),
            product_code: rec.product_code.to_owned(),
            period_from: rec.period_from,
            period_to: rec.period_to,
        }),
    }
}

/// The live original occupying a period, if there is one.
///
/// Used only on the [`InsertError::PeriodAlreadyIssued`] path, to answer the
/// `409` with the document that holds the slot — a retrying caller reconciles
/// against a record id instead of guessing what blocked it.
pub async fn find_live_original(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    product_code: &str,
    period_from: Date,
    period_to: Date,
) -> Option<(Uuid, String, String)> {
    sqlx::query_as::<_, (Uuid, String, String)>(
        r"SELECT id, rechnungsnummer, outcome FROM billing_records
          WHERE tenant = $1 AND malo_id = $2 AND product_code = $3
            AND period_from = $4 AND period_to = $5
            AND is_correction = false
            AND sammelrechnung_id IS NULL
            AND outcome <> 'cancelled'
          LIMIT 1",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(product_code)
    .bind(period_from)
    .bind(period_to)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn fetch_billing_record(
    pool: &PgPool,
    tenant: &str,
    id: Uuid,
) -> anyhow::Result<Option<BillingRecordRow>> {
    // Tenant is part of a record's identity — a UUID known to one tenant must
    // never resolve another tenant's invoice (cross-tenant disclosure).
    sqlx::query_as::<_, BillingRecordRow>(
        "SELECT * FROM billing_records WHERE id = $1 AND tenant = $2",
    )
    .bind(id)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch_billing_record")
}

/// What a record listing selects. Every set field narrows; `None` does not filter.
///
/// The category and correction filters live here rather than in the callers,
/// because a filter that runs **after** the limit is not a filter: fetch
/// `limit` rows and keep the VPP ones, and a MaLo with fifty ordinary invoices
/// and three Stornos answers "no corrections" at `limit = 50` — an audit tool
/// (§ 147 AO) reporting that a correction chain does not exist when it does.
#[derive(Debug, Default, Clone, Copy)]
pub struct RecordFilter<'a> {
    pub malo_id: Option<&'a str>,
    pub lf_mp_id: Option<&'a str>,
    pub outcome: Option<&'a str>,
    /// One `billing_records.category` — `VPP`, `SAMMEL`, `STROM`, …
    pub category: Option<&'a str>,
    /// `Some(true)` = only Storno/Korrektur rows, `Some(false)` = only originals.
    pub is_correction: Option<bool>,
    pub limit: i64,
}

pub async fn list_billing_records(
    pool: &PgPool,
    tenant: &str,
    f: &RecordFilter<'_>,
) -> anyhow::Result<Vec<BillingRecordRow>> {
    sqlx::query_as::<_, BillingRecordRow>(
        r"SELECT * FROM billing_records
          WHERE tenant = $1
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR lf_mp_id = $3)
            AND ($4::text IS NULL OR outcome = $4)
            AND ($5::text IS NULL OR category = $5)
            AND ($6::bool IS NULL OR is_correction = $6)
          ORDER BY created_at DESC
          LIMIT $7",
    )
    .bind(tenant)
    .bind(f.malo_id)
    .bind(f.lf_mp_id)
    .bind(f.outcome)
    .bind(f.category)
    .bind(f.is_correction)
    .bind(f.limit)
    .fetch_all(pool)
    .await
    .context("list_billing_records")
}

/// Advance a record to `dispatched` inside the caller's transaction, right after
/// its CloudEvent is enqueued.
///
/// Persist-before-dispatch guarantees the enqueued CE will reach the ERP (retry +
/// DLQ), so once it is on its way the invoice is final: advancing `outcome` past
/// `'generated'` activates the `insert_billing_record` overwrite guard (a re-run
/// of the same period is refused → correction path). Idempotent — re-stamping a
/// dispatched row is a no-op UPDATE.
pub async fn mark_dispatched_tx(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE billing_records
          SET outcome = 'dispatched', dispatched_at = now(), updated_at = now()
          WHERE id = $1 AND outcome = 'generated'",
    )
    .bind(id)
    .execute(executor)
    .await
    .context("mark_dispatched_tx")?;
    Ok(())
}

/// Attach the EN 16931 semantic model to a record in the caller's transaction.
///
/// Written alongside the BO4E `rechnung_json` at bill time; it is the source the
/// XRechnung/CII/UBL renderers read, so the e-invoicing syntaxes never round-trip
/// through BO4E.
pub async fn attach_en16931(
    executor: impl sqlx::PgExecutor<'_>,
    id: Uuid,
    model: &serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE billing_records SET en16931_json = $2, updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(model)
        .execute(executor)
        .await
        .context("attach_en16931")?;
    Ok(())
}

/// Pin the template that rendered an *issued* document — once.
///
/// `COALESCE(template_hash, $2)` is half the point: the first render fixes how
/// the invoice looks, and every later render must reproduce that document
/// rather than restyle it with whatever layout is current. Rolling out a new
/// template changes what new invoices look like and nothing about one already
/// sent, which is what § 147 AO reproducibility means in practice.
///
/// `outcome <> 'generated'` is the other half. A record still in `generated` is
/// a **draft**: nobody has received it, so there is nothing to reproduce, and
/// pinning one would trap an operator's own preview — publish, look at it, fix a
/// typo, roll out the correction, and the invoice they were looking at is stuck
/// on the version with the typo, permanently, because the store never deletes.
/// A draft therefore renders with the current layout every time and pins
/// nothing; the first render *after* dispatch is the one that fixes it.
///
/// Returns the pinned hash — the one just written, or the one already there.
/// `None` means nothing is pinned, which for a draft is the normal answer.
pub async fn pin_template(pool: &PgPool, id: Uuid, hash: &str) -> anyhow::Result<Option<String>> {
    sqlx::query_scalar::<_, Option<String>>(
        r"UPDATE billing_records
             SET template_hash = COALESCE(template_hash, $2), updated_at = now()
           WHERE id = $1 AND outcome <> 'generated'
       RETURNING template_hash",
    )
    .bind(id)
    .bind(hash)
    .fetch_optional(pool)
    .await
    .context("pin_template")
    .map(Option::flatten)
}

// ── Korrekturrechnung + Sammelrechnung ────────────────────────────────────────

/// Insert a Stornorechnung / Korrekturrechnung and cancel the original.
///
/// The original row is **never** rewritten (§ 147 AO Unveränderbarkeit) — only
/// its `outcome` advances to `cancelled`, which drops the period out of
/// `br_unique_original` so the corrected amounts can be re-billed as a fresh
/// original — the Storno-und-Neuberechnung flow German accounting expects, and
/// the flow the correction endpoint's own response body recommends.
///
/// `rec.rechnung_json` must already carry `istOriginal: false`,
/// `originalRechnungsnummer` and negated amounts.
pub async fn insert_correction_record(
    executor: &mut sqlx::PgConnection,
    rec: &NewBillingRecord<'_>,
    original_record_id: Uuid,
    correction_reason: Option<&str>,
) -> Result<Uuid, InsertError> {
    let row = sqlx::query(
        r"INSERT INTO billing_records
              (tenant, malo_id, lf_mp_id, product_code, category, rechnungsnummer,
               period_from, period_to, rechnung_json, total_netto_eur, total_brutto_eur,
               is_correction, original_record_id, correction_reason)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, TRUE, $12, $13)
          RETURNING id",
    )
    .bind(rec.tenant)
    .bind(rec.malo_id)
    .bind(rec.lf_mp_id)
    .bind(rec.product_code)
    .bind(rec.category)
    .bind(rec.rechnungsnummer)
    .bind(rec.period_from)
    .bind(rec.period_to)
    .bind(rec.rechnung_json)
    .bind(rec.total_netto_eur)
    .bind(rec.total_brutto_eur)
    .bind(original_record_id)
    .bind(correction_reason)
    .fetch_one(&mut *executor)
    .await
    .map_err(|e| {
        match e
            .as_database_error()
            .and_then(sqlx::error::DatabaseError::constraint)
        {
            Some("br_unique_rechnungsnummer") => {
                InsertError::DuplicateRechnungsnummer(rec.rechnungsnummer.to_owned())
            }
            _ => InsertError::Other(anyhow::Error::new(e).context("insert_correction_record")),
        }
    })?;

    sqlx::query(
        "UPDATE billing_records SET outcome = 'cancelled', updated_at = now() \
         WHERE id = $1 AND tenant = $2",
    )
    .bind(original_record_id)
    .bind(rec.tenant)
    .execute(&mut *executor)
    .await
    .map_err(|e| InsertError::Other(anyhow::Error::new(e).context("cancel corrected original")))?;

    row.try_get("id")
        .map_err(|e| InsertError::Other(anyhow::Error::new(e)))
}

/// Insert a consolidated document (`category = SAMMEL`) for a Rahmenvertrag or
/// a § 42b GGV community.
///
/// Goes through the same upsert as every other original, so re-running a bundle
/// for a period replaces its draft instead of raising a unique violation.
/// `subject_id` (the Rahmenvertrag or GGV id) stands in the `malo_id` column —
/// a bundle bills a contract holder, not a Marktlokation.
pub async fn insert_sammelrechnung_record(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    subject_id: &str,
    lf_mp_id: &str,
    rechnungsnummer: &str,
    period_from: Date,
    period_to: Date,
    rechnung_json: &serde_json::Value,
    total_netto_eur: Decimal,
    total_brutto_eur: Decimal,
) -> Result<Uuid, InsertError> {
    let product_code = format!("SAMMEL-{subject_id}");
    insert_billing_record(
        executor,
        &NewBillingRecord {
            tenant,
            malo_id: subject_id,
            lf_mp_id,
            product_code: &product_code,
            category: "SAMMEL",
            rechnungsnummer,
            period_from,
            period_to,
            rechnung_json,
            total_netto_eur,
            total_brutto_eur,
        },
    )
    .await
}

/// Tag the per-MaLo children of a consolidated document, in one statement.
pub async fn link_to_sammelrechnung(
    executor: impl sqlx::PgExecutor<'_>,
    record_ids: &[Uuid],
    sammelrechnung_id: Uuid,
) -> anyhow::Result<()> {
    if record_ids.is_empty() {
        return Ok(());
    }
    sqlx::query(
        "UPDATE billing_records SET sammelrechnung_id = $2, updated_at = now() \
         WHERE id = ANY($1)",
    )
    .bind(record_ids)
    .bind(sammelrechnung_id)
    .execute(executor)
    .await
    .context("link_to_sammelrechnung")?;
    Ok(())
}

// ── Billing Anomaly Detection (B6 / L1) ──────────────────────────────────────

/// Rolling 3-month baseline and deviation for a MaLo's billing amounts.
///
/// Used by the `billing-anomaly-agent` in `agentd` to detect invoices that
/// deviate >20 % from the rolling baseline. Surfaced by the `check_billing_anomaly`
/// MCP tool.
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
pub async fn check_billing_anomaly(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    lf_mp_id: &str,
    threshold_pct: Option<Decimal>,
) -> anyhow::Result<BillingAnomalyReport> {
    use rust_decimal::dec;
    let threshold = threshold_pct.unwrap_or(dec!(20));

    // The same population the risk baseline and the summary use: live
    // originals and consolidated documents, never the per-MaLo children of a
    // bundle alongside the bundle itself, and never a reversed document. This
    // query counted both halves of a Sammelrechnung and kept cancelled
    // invoices in the baseline, so the "rolling average" it compared against
    // was not the one the risk gate had scored.
    //
    // Ordered by `period_to`, not `created_at`: a back-dated catch-up run
    // inserts an *older* period last, and "latest invoice" means the latest
    // period billed, not the row most recently written.
    let rows = sqlx::query(
        r"SELECT id, total_brutto_eur
          FROM billing_records
          WHERE tenant = $3
            AND malo_id = $1
            AND lf_mp_id = $2
            AND is_correction = FALSE
            AND sammelrechnung_id IS NULL
            AND outcome <> 'cancelled'
            AND total_brutto_eur IS NOT NULL
            AND total_brutto_eur > 0
          ORDER BY period_to DESC
          LIMIT 4",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(tenant)
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
        rolling_avg_brutto_eur: Some(avg.round_kfm(2)),
        deviation_pct: Some(deviation_pct.round_kfm(2)),
        is_anomaly,
        sample_count: prior.len() as i64,
        threshold_pct: threshold,
    })
}

/// Aggregate billing statistics for a MaLo or an LF, computed in the database.
///
/// Folding a capped page of rows in Rust got three things wrong at once: it
/// averaged per *record* while calling the result a monthly average, it stopped
/// at 100 rows without saying so, and it counted the per-MaLo children of a
/// Sammelrechnung alongside the bundle that already contains them.
///
/// `sammelrechnung_id IS NULL` is the non-double-counted set — standalone
/// invoices plus consolidated documents, never both halves of the same money —
/// and it is the same predicate the risk baseline uses.
pub async fn billing_summary(
    pool: &PgPool,
    tenant: &str,
    malo_id: Option<&str>,
    lf_mp_id: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let row = sqlx::query(
        r"SELECT count(*)                                   AS records,
                 coalesce(sum(total_brutto_eur), 0)         AS total_brutto,
                 coalesce(sum(total_netto_eur), 0)          AS total_netto,
                 min(period_from)                           AS first_period,
                 max(period_to)                             AS last_period,
                 coalesce(sum(period_to - period_from + 1), 0) AS billed_days
          FROM billing_records
          WHERE tenant = $1
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR lf_mp_id = $3)
            AND is_correction = false
            AND sammelrechnung_id IS NULL
            AND outcome <> 'cancelled'",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_one(pool)
    .await
    .context("billing_summary totals")?;

    let records: i64 = row.try_get("records")?;
    // The stored columns are NUMERIC(16,5) — the scale the engine needs for
    // unit prices. A summary reports money, so it reports cents.
    let total_brutto: Decimal = row.try_get::<Decimal, _>("total_brutto")?.round_kfm(2);
    let total_netto: Decimal = row.try_get::<Decimal, _>("total_netto")?.round_kfm(2);
    let billed_days: i64 = row.try_get("billed_days")?;

    // Per *month of supply*, not per record: a portfolio of annual invoices and
    // one of monthly invoices are not comparable per document, and the label
    // said "monthly" either way.
    const DAYS_PER_MONTH: i64 = 30;
    let avg_monthly = (billed_days > 0).then(|| {
        (total_brutto / Decimal::from(billed_days) * Decimal::from(DAYS_PER_MONTH)).round_kfm(2)
    });

    let cats = sqlx::query(
        r"SELECT category,
                 count(*)                           AS records,
                 coalesce(sum(total_brutto_eur), 0) AS total_brutto
          FROM billing_records
          WHERE tenant = $1
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR lf_mp_id = $3)
            AND is_correction = false
            AND sammelrechnung_id IS NULL
            AND outcome <> 'cancelled'
          GROUP BY category
          ORDER BY sum(total_brutto_eur) DESC NULLS LAST",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_all(pool)
    .await
    .context("billing_summary by category")?;

    let by_category: Vec<serde_json::Value> = cats
        .iter()
        .map(|r| {
            Ok::<_, sqlx::Error>(serde_json::json!({
                "category": r.try_get::<String, _>("category")?,
                "records": r.try_get::<i64, _>("records")?,
                "total_brutto_eur": r.try_get::<Decimal, _>("total_brutto")?.round_kfm(2).to_string(),
            }))
        })
        .collect::<Result<_, _>>()?;

    // Corrections are excluded from the totals above — a Storno and its original
    // net to zero and would flatter or distort every figure — but their number
    // is itself the interesting signal.
    let corrections: i64 = sqlx::query_scalar(
        r"SELECT count(*) FROM billing_records
          WHERE tenant = $1
            AND ($2::text IS NULL OR malo_id = $2)
            AND ($3::text IS NULL OR lf_mp_id = $3)
            AND is_correction = true",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_one(pool)
    .await
    .context("billing_summary corrections")?;

    Ok(serde_json::json!({
        "malo_id": malo_id,
        "lf_mp_id": lf_mp_id,
        "records": records,
        "corrections": corrections,
        "total_netto_eur": total_netto.to_string(),
        "total_brutto_eur": total_brutto.to_string(),
        "billed_days": billed_days,
        "avg_brutto_eur_per_30d": avg_monthly.map(|d| d.to_string()),
        "first_period_from": row.try_get::<Option<Date>, _>("first_period")?.map(|d| d.to_string()),
        "last_period_to": row.try_get::<Option<Date>, _>("last_period")?.map(|d| d.to_string()),
        "by_category": by_category,
        "basis": "originals and consolidated documents only — Storno rows and the per-MaLo \
                  children of a Sammelrechnung are excluded so nothing is counted twice",
    }))
}

// ── VPP dispatch idempotency ─────────────────────────────────────────────────
//
// The §41e Aggregatorvertrag itself lives in `vertragd`; billingd keeps only
// the guard that stops a redelivered dispatch from being billed twice.

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

/// Which of these dispatch transactions this tenant has already settled.
///
/// The manual settlement endpoint takes a list of dispatch events, and the
/// webhook settles the same events one at a time. Without this the two paths
/// could not see each other: a period back-filled by hand after the webhook had
/// already auto-billed some of its dispatches paid the provider twice for the
/// same flexibility, and nothing in the store said so — `vpp_dispatch_ledger`
/// existed but only one of the two writers consulted it.
pub async fn settled_vpp_dispatches(
    pool: &PgPool,
    tenant: &str,
    tx_ids: &[String],
) -> anyhow::Result<std::collections::HashSet<String>> {
    if tx_ids.is_empty() {
        return Ok(std::collections::HashSet::new());
    }
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT tx_id FROM vpp_dispatch_ledger WHERE tenant = $1 AND tx_id = ANY($2)",
    )
    .bind(tenant)
    .bind(tx_ids)
    .fetch_all(pool)
    .await
    .context("settled_vpp_dispatches")?;
    Ok(rows.into_iter().collect())
}

/// Record a processed VPP dispatch for idempotency.
pub async fn record_vpp_dispatch(
    executor: impl sqlx::PgExecutor<'_>,
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
    .execute(executor)
    .await
    .context("record_vpp_dispatch")?;
    Ok(())
}

// ── §40b EnWG billing-run + Abrechnungsinformation logs ───────────────────────

/// Accumulate a daily billing-run sweep into the month's `billing_run_log`
/// row (§40b EnWG audit): first sweep of the month inserts, later sweeps add
/// their counts. `failed` sticks once set — a month with any failed sweep
/// needs operator attention even if a later sweep succeeded.
///
/// `skipped` counts periods the sweep deliberately did not bill (an annual
/// settlement it cannot supply the Abschläge for). Counting those as errors
/// marked every month `failed` for any operator with annual contracts and the
/// default `jahresrechnung = false` — the audit signal drowned in its own noise.
pub async fn record_billing_run(
    pool: &PgPool,
    tenant: &str,
    lf_mp_id: &str,
    year: i16,
    month: i16,
    records: i32,
    skipped: i32,
    errors: i32,
) -> anyhow::Result<()> {
    sqlx::query(
        r"INSERT INTO billing_run_log
              (tenant, lf_mp_id, billing_year, billing_month,
               records_count, skipped_count, errors_count, status)
          VALUES ($1, $2, $3, $4, $5, $6, $7,
                  CASE WHEN $7 > 0 THEN 'failed' ELSE 'completed' END)
          ON CONFLICT (tenant, lf_mp_id, billing_year, billing_month) DO UPDATE
          SET records_count = billing_run_log.records_count + EXCLUDED.records_count,
              skipped_count = billing_run_log.skipped_count + EXCLUDED.skipped_count,
              errors_count  = billing_run_log.errors_count  + EXCLUDED.errors_count,
              status        = CASE
                                  WHEN billing_run_log.status = 'failed'
                                       OR EXCLUDED.errors_count > 0 THEN 'failed'
                                  ELSE 'completed'
                              END,
              run_at        = now()",
    )
    .bind(tenant)
    .bind(lf_mp_id)
    .bind(year)
    .bind(month)
    .bind(records)
    .bind(skipped)
    .bind(errors)
    .execute(pool)
    .await
    .context("record_billing_run")?;
    Ok(())
}

/// A live invoice for this MaLo and period already exists.
///
/// The billing-run worker's per-invoice idempotency. A **cancelled** original
/// does not count: a Storno releases the period precisely so it can be billed
/// again, and treating the reversed row as coverage would leave the customer
/// with no invoice at all for that window.
pub async fn billing_record_exists_for_period(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    period_from: Date,
    period_to: Date,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        "SELECT 1 FROM billing_records
         WHERE tenant = $1 AND malo_id = $2
           AND period_from = $3 AND period_to = $4
           AND is_correction = false
           AND outcome <> 'cancelled'
         LIMIT 1",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_from)
    .bind(period_to)
    .fetch_optional(pool)
    .await
    .context("billing_record_exists_for_period")?;
    Ok(row.is_some())
}

/// Claim the monthly §40b Abs. 2 Abrechnungsinformation for a MaLo.
///
/// Returns `true` when this call inserted the claim (caller must now send the
/// info), `false` when the month was already delivered — the UNIQUE guard
/// makes the daily worker idempotent.
pub async fn claim_abrechnungsinfo(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<bool> {
    let row = sqlx::query(
        r"INSERT INTO abrechnungsinfo_log (tenant, malo_id, info_year, info_month)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (tenant, malo_id, info_year, info_month) DO NOTHING
          RETURNING id",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(year)
    .bind(month)
    .fetch_optional(pool)
    .await
    .context("claim_abrechnungsinfo")?;
    Ok(row.is_some())
}

/// Release a §40b Abs. 2 claim whose delivery did not happen.
///
/// The claim is taken *before* the work so two concurrent sweeps cannot both
/// deliver. When the work then fails, holding the claim would suppress that
/// month's Abrechnungsinformation permanently — the customer's statutory
/// entitlement lost to a transient edmd outage. Releasing lets tomorrow's
/// sweep retry inside the same month.
pub async fn release_abrechnungsinfo_claim(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    year: i16,
    month: i16,
) -> anyhow::Result<()> {
    sqlx::query(
        r"DELETE FROM abrechnungsinfo_log
          WHERE tenant = $1 AND malo_id = $2 AND info_year = $3 AND info_month = $4",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(year)
    .bind(month)
    .execute(pool)
    .await
    .context("release_abrechnungsinfo_claim")?;
    Ok(())
}

// ── Risk scoring persistence ──────────────────────────────────────────────────

/// History context for the deterministic risk scorer.
pub async fn risk_context(
    pool: &PgPool,
    tenant: &str,
    malo_id: &str,
    period_from: Date,
) -> anyhow::Result<crate::risk::RiskContext> {
    // Baseline: mean gross of the up-to-3 latest previous invoices (≥2 needed).
    let rolling: Option<Decimal> = sqlx::query_scalar(
        r"SELECT CASE WHEN count(*) >= 2 THEN avg(total_brutto_eur) END
          FROM (SELECT total_brutto_eur
                FROM billing_records
                WHERE tenant = $1 AND malo_id = $2
                  AND is_correction = FALSE AND sammelrechnung_id IS NULL
                  AND outcome <> 'cancelled'
                  AND total_brutto_eur IS NOT NULL AND total_brutto_eur > 0
                  AND period_from < $3
                ORDER BY period_to DESC
                LIMIT 3) t",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_from)
    .fetch_one(pool)
    .await
    .context("risk rolling baseline")?;

    // Continuity: the latest previous invoice's period end.
    let prev_period_to: Option<Date> = sqlx::query_scalar(
        r"SELECT max(period_to) FROM billing_records
          WHERE tenant = $1 AND malo_id = $2
            AND is_correction = FALSE AND sammelrechnung_id IS NULL
            AND outcome <> 'cancelled'
            AND period_from < $3",
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_from)
    .fetch_one(pool)
    .await
    .context("risk prev period")?;

    // § 60 Abs. 2 MsbG: how many of the latest 3 invoices were estimate-based.
    let recent_estimated_count: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM (
              SELECT risk_findings FROM billing_records
              WHERE tenant = $1 AND malo_id = $2
                AND is_correction = FALSE AND sammelrechnung_id IS NULL
                AND outcome <> 'cancelled'
                AND period_from < $3
              ORDER BY period_to DESC LIMIT 3) t
          WHERE t.risk_findings @> '[{"code": "ESTIMATED_READING"}]'"#,
    )
    .bind(tenant)
    .bind(malo_id)
    .bind(period_from)
    .fetch_one(pool)
    .await
    .context("risk estimated count")?;

    Ok(crate::risk::RiskContext {
        rolling_avg_brutto_eur: rolling.map(|d| d.round_kfm(2)),
        prev_period_to,
        recent_estimated_count,
    })
}

/// Persist a risk assessment on its record, in the caller's transaction.
///
/// This must commit **with** the record, not after it. A HELD invoice is not
/// dispatched, and `release_held_record` only finds rows whose `risk_band` is
/// `'HELD'` — so a record that was held but whose band failed to persist is
/// invisible to the review queue and unreleasable by the endpoint. It would sit
/// as a permanent draft that no operator could see or act on. Writing it in the
/// same transaction makes "held" and "known to be held" the same fact.
pub async fn set_risk(
    executor: impl sqlx::PgExecutor<'_>,
    record_id: Uuid,
    assessment: &crate::risk::RiskAssessment,
) -> anyhow::Result<()> {
    sqlx::query(
        r"UPDATE billing_records
          SET risk_score = $2, risk_band = $3, risk_findings = $4
          WHERE id = $1",
    )
    .bind(record_id)
    .bind(i16::from(assessment.score))
    .bind(assessment.band.as_str())
    .bind(serde_json::to_value(&assessment.findings)?)
    .execute(executor)
    .await
    .context("set_risk")?;
    Ok(())
}

/// Analyst release of a HELD record: stamps the release and returns the row
/// for dispatch. `None` when the record is not currently HELD.
pub async fn release_held_record(
    executor: impl sqlx::PgExecutor<'_>,
    tenant: &str,
    record_id: Uuid,
    released_by: &str,
) -> anyhow::Result<Option<BillingRecordRow>> {
    let row: Option<BillingRecordRow> = sqlx::query_as(
        r"UPDATE billing_records
          SET released_by = $3, released_at = now()
          WHERE id = $1 AND tenant = $2 AND risk_band = 'HELD' AND released_at IS NULL
          RETURNING *",
    )
    .bind(record_id)
    .bind(tenant)
    .bind(released_by)
    .fetch_optional(executor)
    .await
    .context("release_held_record")?;
    Ok(row)
}

/// The analyst review queue: REVIEW + HELD records, highest risk first.
pub async fn list_review_queue(
    pool: &PgPool,
    tenant: &str,
    band: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<BillingRecordRow>> {
    // The band filter narrows the queue; it never widens it. Asking for
    // AUTO_RELEASED returns nothing, because such a record is by definition not
    // on an analyst's work list.
    let rows: Vec<BillingRecordRow> = sqlx::query_as(
        r"SELECT * FROM billing_records
          WHERE tenant = $1
            AND risk_band IN ('REVIEW','HELD')
            AND ($2::text IS NULL OR risk_band = $2)
          ORDER BY risk_score DESC NULLS LAST, created_at DESC
          LIMIT $3",
    )
    .bind(tenant)
    .bind(band)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_review_queue")?;
    Ok(rows)
}
