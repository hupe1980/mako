//! PostgreSQL persistence for `tarifbd`.

use anyhow::Context as _;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use time::{Date, OffsetDateTime};
use uuid::Uuid;

// ── Product ───────────────────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/products/{lf_mp_id}/{product_code}`.
#[derive(Debug, Deserialize)]
pub struct ProductUpsertRequest {
    pub category: String,
    pub name: String,
    pub sparte: Option<String>,
    pub register_count: Option<String>,
    pub kundentyp: Option<String>,
    pub dyn_source: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    /// Full BO4E `Tarifpreisblatt` / `Preisblatt` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e")]
    pub bo4e_version: String,
    /// `DRAFT` = staged/preview; `PUBLISHED` (default) = active for billing.
    #[serde(default = "default_published")]
    pub product_status: String,
    /// Optional \u00a742 EnWG `Energiemix` payload (camelCase BO4E COM JSON).
    /// If supplied here it is stored in the dedicated `energiemix` column
    /// and also exposed via `GET /energiemix`.
    #[serde(default)]
    pub energiemix: Option<serde_json::Value>,
    /// Optional list of `Oekolabel` enum codes (e.g. \`[\"OK_POWER\", \"NATURWATT_STROM\"]\`).
    #[serde(default)]
    pub oekolabel: Option<Vec<String>>,
}

fn default_bo4e() -> String {
    "v202607.0.0".to_owned()
}

fn default_published() -> String {
    "PUBLISHED".to_owned()
}

/// Stored product row returned by GET endpoints.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProductRow {
    pub id: Uuid,
    pub lf_mp_id: String,
    pub product_code: String,
    pub category: String,
    pub name: String,
    pub sparte: Option<String>,
    pub register_count: Option<String>,
    pub kundentyp: Option<String>,
    pub dyn_source: Option<String>,
    pub valid_from: Option<Date>,
    pub valid_to: Option<Date>,
    pub data: serde_json::Value,
    pub bo4e_version: String,
    /// `DRAFT` or `PUBLISHED`.
    pub product_status: String,
    /// \u00a742 EnWG `Energiemix` COM payload. `None` = no green certification.
    pub energiemix: Option<serde_json::Value>,
    /// Active `Oekolabel` certification codes.
    pub oekolabel: Option<Vec<String>>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub async fn upsert_product(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
    req: ProductUpsertRequest,
) -> anyhow::Result<Uuid> {
    let valid_from = parse_date_opt(&req.valid_from).context("parse valid_from")?;
    let valid_to = parse_date_opt(&req.valid_to).context("parse valid_to")?;

    // Archive previous version before upsert (includes energiemix for §42 audit trail).
    let _ = sqlx::query(
        r"INSERT INTO product_history (lf_mp_id, product_code, data, energiemix, bo4e_version)
          SELECT lf_mp_id, product_code, data, energiemix, bo4e_version
          FROM products
          WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $4
            AND (valid_from = $3 OR $3 IS NULL)
          ORDER BY updated_at DESC
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(valid_from)
    .bind(tenant)
    .execute(pool)
    .await
    .context("archive product_history before upsert")?;

    let row = sqlx::query(
        r"INSERT INTO products
              (lf_mp_id, product_code, category, name, sparte, register_count, kundentyp,
               dyn_source, valid_from, valid_to, data, bo4e_version, product_status,
               energiemix, oekolabel, tenant, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, now())
          ON CONFLICT (lf_mp_id, product_code, valid_from) DO UPDATE
          SET category      = EXCLUDED.category,
              name          = EXCLUDED.name,
              sparte        = EXCLUDED.sparte,
              register_count= EXCLUDED.register_count,
              kundentyp     = EXCLUDED.kundentyp,
              dyn_source    = EXCLUDED.dyn_source,
              valid_to      = EXCLUDED.valid_to,
              data          = EXCLUDED.data,
              bo4e_version  = EXCLUDED.bo4e_version,
              product_status= EXCLUDED.product_status,
              energiemix    = COALESCE(EXCLUDED.energiemix, products.energiemix),
              oekolabel     = COALESCE(EXCLUDED.oekolabel, products.oekolabel),
              updated_at    = now()
          RETURNING id",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(&req.category)
    .bind(&req.name)
    .bind(&req.sparte)
    .bind(&req.register_count)
    .bind(&req.kundentyp)
    .bind(&req.dyn_source)
    .bind(valid_from)
    .bind(valid_to)
    .bind(&req.data)
    .bind(&req.bo4e_version)
    .bind(&req.product_status)
    .bind(&req.energiemix)
    .bind(&req.oekolabel)
    .bind(tenant)
    .fetch_one(pool)
    .await
    .context("upsert product")?;

    Ok(row.try_get("id")?)
}

pub async fn fetch_product(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<Option<ProductRow>> {
    sqlx::query_as::<_, ProductRow>(
        "SELECT * FROM products WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $3
         ORDER BY valid_from DESC NULLS LAST LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch product")
}

/// Soft-delete a product by setting `valid_to = today`.
///
/// Only touches `valid_to` and `updated_at` — the product remains in the
/// database for historical billing lookups and the audit log.  Billing engines
/// should call `fetch_product` and check `valid_to` when deciding whether a
/// product is still applicable for a given billing period.
///
/// Returns `true` if a row was found and updated, `false` if the product did
/// not exist.
pub async fn soft_delete_product(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<bool> {
    let today = time::OffsetDateTime::now_utc().date();
    let res = sqlx::query(
        r"UPDATE products
          SET valid_to = $3, updated_at = now()
          WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $4
            AND (valid_to IS NULL OR valid_to > $3)",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(today)
    .bind(tenant)
    .execute(pool)
    .await
    .context("soft_delete_product")?;
    Ok(res.rows_affected() > 0)
}

pub async fn fetch_product_history(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    // product_history carries no tenant column; scope it through the owning
    // product row so one operator cannot read another's price history.
    let rows = sqlx::query(
        "SELECT h.id, h.lf_mp_id, h.product_code, h.data, h.energiemix, h.bo4e_version, h.changed_at
         FROM product_history h
         WHERE h.lf_mp_id = $1 AND h.product_code = $2
           AND EXISTS (SELECT 1 FROM products p
                       WHERE p.lf_mp_id = h.lf_mp_id AND p.product_code = h.product_code
                         AND p.tenant = $3)
         ORDER BY h.changed_at DESC LIMIT 100",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("fetch_product_history")?;

    Ok(rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "id": r.try_get::<Uuid,_>("id").ok().map(|u| u.to_string()),
                "lf_mp_id": r.try_get::<String,_>("lf_mp_id").ok(),
                "product_code": r.try_get::<String,_>("product_code").ok(),
                "data": r.try_get::<serde_json::Value,_>("data").ok(),
                "energiemix": r.try_get::<Option<serde_json::Value>,_>("energiemix").ok().flatten(),
                "bo4e_version": r.try_get::<String,_>("bo4e_version").ok(),
                "changed_at": r.try_get::<OffsetDateTime,_>("changed_at").ok().map(|t| t.to_string()),
            })
        })
        .collect())
}

// ── Energiemix (§42 EnWG) ────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/products/{lf_mp_id}/{product_code}/energiemix`.
///
/// Stores validated `rubo4e::current::Energiemix` + optional `Oekolabel` list.
/// This is the **dedicated sub-resource** for green energy certification —
/// separate from the main product PUT so the annual Herkunftsnachweis update
/// does not archive the entire product and pricing definition.
#[derive(Debug, Deserialize)]
pub struct EnergimixUpsertRequest {
    /// Full `rubo4e::current::Energiemix` COM payload (camelCase JSON).
    /// Validation: deserialisable as `Energiemix`; invalid enum fields return 422.
    pub energiemix: serde_json::Value,
    /// Oekolabel certification codes.
    /// Valid values: ENERGREEN, OK_POWER, NATURWATT_STROM, GRUENER_STROM, etc.
    #[serde(default)]
    pub oekolabel: Option<Vec<String>>,
}

/// Response from `GET /api/v1/products/{lf_mp_id}/{product_code}/energiemix`.
#[derive(Debug, Serialize)]
pub struct EnergiemixResponse {
    pub lf_mp_id: String,
    pub product_code: String,
    /// Validated `rubo4e::current::Energiemix` COM payload.
    pub energiemix: serde_json::Value,
    /// Active certification codes.
    pub oekolabel: Option<Vec<String>>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

/// Upsert the `Energiemix` + `Oekolabel` for a product.
///
/// Only touches the `energiemix` and `oekolabel` columns — does NOT re-archive
/// the product and does NOT change pricing.  This allows the annual
/// Herkunftsnachweis update without triggering a billing-period change.
pub async fn upsert_energiemix(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
    req: EnergimixUpsertRequest,
) -> anyhow::Result<()> {
    let updated = sqlx::query(
        r"UPDATE products
          SET energiemix = $3,
              oekolabel  = $4,
              updated_at = now()
          WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $5",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(&req.energiemix)
    .bind(&req.oekolabel)
    .bind(tenant)
    .execute(pool)
    .await
    .context("upsert energiemix")?;

    if updated.rows_affected() == 0 {
        anyhow::bail!("product {lf_mp_id}/{product_code} not found");
    }
    Ok(())
}

/// Fetch the `Energiemix` + `Oekolabel` for a product.
pub async fn fetch_energiemix(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<Option<EnergiemixResponse>> {
    let row = sqlx::query(
        r"SELECT lf_mp_id, product_code, energiemix, oekolabel, updated_at
          FROM products
          WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $3
          ORDER BY valid_from DESC NULLS LAST
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("fetch energiemix")?;

    Ok(row.map(|r| EnergiemixResponse {
        lf_mp_id: r.try_get("lf_mp_id").unwrap_or_default(),
        product_code: r.try_get("product_code").unwrap_or_default(),
        energiemix: r
            .try_get::<Option<serde_json::Value>, _>("energiemix")
            .unwrap_or_default()
            .unwrap_or(serde_json::Value::Null),
        oekolabel: r.try_get("oekolabel").unwrap_or_default(),
        updated_at: r
            .try_get("updated_at")
            .unwrap_or_else(|_| OffsetDateTime::now_utc()),
    }))
}

/// Delete the `Energiemix` + `Oekolabel` for a product (hard cut, no archive).
pub async fn delete_energiemix(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE products SET energiemix = NULL, oekolabel = NULL, updated_at = now()
         WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $3",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .execute(pool)
    .await
    .context("delete energiemix")?;
    Ok(res.rows_affected() > 0)
}

#[derive(Debug, Deserialize)]
pub struct ProductListQuery {
    pub category: Option<String>,
    pub sparte: Option<String>,
    pub kundentyp: Option<String>,
    /// Include DRAFT products.  Default: `false` (only PUBLISHED).
    pub include_drafts: Option<bool>,
    /// Include products whose `valid_to < today`.  Default: `false`.
    pub include_expired: Option<bool>,
    pub limit: Option<i64>,
}

pub async fn list_products(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    q: &ProductListQuery,
) -> anyhow::Result<Vec<ProductRow>> {
    let include_drafts = q.include_drafts.unwrap_or(false);
    let include_expired = q.include_expired.unwrap_or(false);
    sqlx::query_as::<_, ProductRow>(
        r"SELECT DISTINCT ON (product_code) *
          FROM products
          WHERE lf_mp_id = $1 AND tenant = $8
            AND ($2::text IS NULL OR category = $2)
            AND ($3::text IS NULL OR sparte = $3)
            AND ($4::text IS NULL OR kundentyp = $4)
            AND ($5::bool IS TRUE OR product_status = 'PUBLISHED')
            AND ($6::bool IS TRUE OR valid_to IS NULL OR valid_to >= CURRENT_DATE)
          ORDER BY product_code, valid_from DESC NULLS LAST
          LIMIT $7",
    )
    .bind(lf_mp_id)
    .bind(&q.category)
    .bind(&q.sparte)
    .bind(&q.kundentyp)
    .bind(include_drafts)
    .bind(include_expired)
    .bind(q.limit.unwrap_or(100).min(1000))
    .bind(tenant)
    .fetch_all(pool)
    .await
    .context("list_products")
}

// ── Customer → product assignment ─────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct CustomerProductRow {
    pub malo_id: String,
    pub lf_mp_id: String,
    pub product_code: String,
    pub assigned_from: Date,
    pub assigned_to: Option<Date>,
    pub product: Option<ProductRow>,
}

pub async fn get_customer_product(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
) -> anyhow::Result<Option<CustomerProductRow>> {
    let row = sqlx::query(
        r"SELECT cp.malo_id, cp.lf_mp_id, cp.product_code, cp.assigned_from, cp.assigned_to
          FROM customer_products cp
          WHERE cp.malo_id = $1 AND cp.lf_mp_id = $2 AND cp.assigned_to IS NULL
          ORDER BY cp.assigned_from DESC
          LIMIT 1",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .fetch_optional(pool)
    .await
    .context("get_customer_product")?;

    if let Some(r) = row {
        let product_code: String = r.try_get("product_code")?;
        let product = fetch_product(pool, lf_mp_id, tenant, &product_code).await?;
        Ok(Some(CustomerProductRow {
            malo_id: r.try_get("malo_id")?,
            lf_mp_id: r.try_get("lf_mp_id")?,
            product_code,
            assigned_from: r.try_get("assigned_from")?,
            assigned_to: r.try_get("assigned_to")?,
            product,
        }))
    } else {
        Ok(None)
    }
}

#[derive(Debug, Deserialize)]
pub struct AssignProductRequest {
    pub product_code: String,
    pub assigned_from: String,
}

pub async fn assign_product(
    pool: &PgPool,
    malo_id: &str,
    lf_mp_id: &str,
    tenant: &str,
    req: AssignProductRequest,
) -> anyhow::Result<()> {
    use time::format_description::well_known::Iso8601;
    let assigned_from =
        Date::parse(&req.assigned_from, &Iso8601::DEFAULT).context("parse assigned_from")?;

    // Guard: product must exist and be PUBLISHED.
    let product = sqlx::query(
        r"SELECT valid_from, valid_to, sparte, product_status
          FROM products
          WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $3
          ORDER BY valid_from DESC NULLS LAST
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(&req.product_code)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .context("check product exists")?;

    let product = product
        .ok_or_else(|| anyhow::anyhow!("product {}/{} not found", lf_mp_id, req.product_code))?;

    // Reject DRAFT products — operators must publish before assigning.
    let status: String = product.try_get("product_status").unwrap_or_default();
    if status == "DRAFT" {
        anyhow::bail!(
            "product {} is DRAFT; publish it before assigning to a MaLo",
            req.product_code
        );
    }

    // Guard: assigned_from must not predate the product's valid_from.
    let prod_valid_from: Option<Date> = product.try_get("valid_from").ok().flatten();
    if let Some(vf) = prod_valid_from.filter(|&vf| assigned_from < vf) {
        anyhow::bail!(
            "assigned_from ({assigned_from}) is before product valid_from ({vf}); \
             retroactive assignment is not allowed"
        );
    }

    // Guard: product must not be expired at the assignment date.
    let prod_valid_to: Option<Date> = product.try_get("valid_to").ok().flatten();
    if let Some(vt) = prod_valid_to.filter(|&vt| assigned_from > vt) {
        anyhow::bail!(
            "product {} expired on {vt}; cannot assign after expiry",
            req.product_code
        );
    }

    // Close the previous assignment and open the new one in ONE transaction.
    // Two separate statements let a failure between them leave the MaLo with
    // no active product (a Tarifwechsel that lost the customer's tariff), and
    // the DEFERRABLE INITIALLY DEFERRED FK only has effect inside a shared tx.
    let mut tx = pool.begin().await.context("begin assign tx")?;
    sqlx::query(
        r"UPDATE customer_products SET assigned_to = $3, updated_at = now()
          WHERE malo_id = $1 AND lf_mp_id = $2 AND assigned_to IS NULL",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(assigned_from)
    .execute(&mut *tx)
    .await
    .context("close previous assignment")?;

    sqlx::query(
        r"INSERT INTO customer_products (malo_id, lf_mp_id, product_code, assigned_from)
          VALUES ($1, $2, $3, $4)
          ON CONFLICT (malo_id, lf_mp_id, assigned_from) DO UPDATE
          SET product_code = EXCLUDED.product_code, updated_at = now()",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(&req.product_code)
    .bind(assigned_from)
    .execute(&mut *tx)
    .await
    .context("assign product")?;
    tx.commit().await.context("commit assign tx")?;

    Ok(())
}

// ── EPEX day-ahead prices ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EpexImportRequest {
    /// 24-entry array of ct/kWh values for hours 0..23.
    pub prices: Vec<Decimal>,
    pub source: Option<String>,
}

pub async fn upsert_epex_day(
    pool: &PgPool,
    date: Date,
    req: EpexImportRequest,
) -> anyhow::Result<()> {
    if req.prices.len() != 24 {
        anyhow::bail!("prices must have exactly 24 entries (one per hour)");
    }
    let source = req.source.as_deref().unwrap_or("manual");

    for (hour, price) in req.prices.iter().enumerate() {
        sqlx::query(
            r"INSERT INTO epex_prices (price_date, hour, avg_ct_kwh, source)
              VALUES ($1, $2, $3, $4)
              ON CONFLICT (price_date, hour) DO UPDATE
              SET avg_ct_kwh = EXCLUDED.avg_ct_kwh,
                  source = EXCLUDED.source,
                  imported_at = now()",
        )
        .bind(date)
        .bind(hour as i16)
        .bind(price)
        .bind(source)
        .execute(pool)
        .await
        .context("upsert epex hour")?;
    }
    Ok(())
}

pub async fn fetch_epex_day(pool: &PgPool, date: Date) -> anyhow::Result<Option<Vec<Decimal>>> {
    let rows =
        sqlx::query("SELECT avg_ct_kwh FROM epex_prices WHERE price_date = $1 ORDER BY hour ASC")
            .bind(date)
            .fetch_all(pool)
            .await
            .context("fetch_epex_day")?;

    if rows.is_empty() {
        return Ok(None);
    }
    let prices = rows
        .iter()
        .filter_map(|r| r.try_get::<Decimal, _>("avg_ct_kwh").ok())
        .collect::<Vec<_>>();
    Ok(Some(prices))
}

/// Average EPEX price for a month (ct/kWh).
/// Used by `billingd` for §41a dynamic tariff billing and by `einsd` for Direktvermarktung.
pub async fn monthly_epex_average(
    pool: &PgPool,
    year: i32,
    month: u8,
) -> anyhow::Result<Option<Decimal>> {
    let row = sqlx::query(
        r"SELECT AVG(avg_ct_kwh) as avg
          FROM epex_prices
          WHERE EXTRACT(YEAR  FROM price_date) = $1
            AND EXTRACT(MONTH FROM price_date) = $2",
    )
    .bind(year)
    .bind(month as i32)
    .fetch_optional(pool)
    .await
    .context("monthly_epex_average")?;

    Ok(row.and_then(|r| r.try_get::<Option<Decimal>, _>("avg").ok().flatten()))
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn parse_date_opt(s: &Option<String>) -> anyhow::Result<Option<Date>> {
    match s.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => {
            use time::format_description::well_known::Iso8601;
            Ok(Some(Date::parse(s, &Iso8601::DEFAULT)?))
        }
    }
}

/// Returns the most recent date for which EPEX Day-Ahead prices have been imported.
///
/// `None` = no prices at all in the database.  Used by `check_41a_epex_status` MCP
/// tool to alert operators when tomorrow's D-1 prices are missing after 13:00 CET.
pub async fn fetch_epex_latest_date(pool: &PgPool) -> anyhow::Result<Option<Date>> {
    let row: Option<(time::Date,)> = sqlx::query_as("SELECT MAX(price_date) FROM epex_prices")
        .fetch_optional(pool)
        .await
        .context("fetch_epex_latest_date")?;
    Ok(row.map(|(d,)| d))
}

/// Returns customer product assignments (Lieferverträge) ending within `days_ahead` days.
/// Used by `list_expiring_contracts` MCP tool for churn prevention / renewal campaigns.
#[allow(dead_code)]
pub async fn list_expiring_assignments(
    pool: &sqlx::PgPool,
    lf_mp_id: &str,
    days_ahead: i64,
) -> anyhow::Result<Vec<serde_json::Value>> {
    use sqlx::Row;
    let rows = sqlx::query(
        r"SELECT malo_id, lf_mp_id, product_code, assigned_from, assigned_to
          FROM customer_products
          WHERE lf_mp_id = $1
            AND assigned_to IS NOT NULL
            AND assigned_to <= CURRENT_DATE + ($2 * INTERVAL '1 day')
            AND assigned_to >= CURRENT_DATE
          ORDER BY assigned_to ASC",
    )
    .bind(lf_mp_id)
    .bind(days_ahead)
    .fetch_all(pool)
    .await
    .context("list_expiring_assignments")?;

    let out: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "malo_id":       r.try_get::<String, _>("malo_id").unwrap_or_default(),
                "lf_mp_id":      r.try_get::<String, _>("lf_mp_id").unwrap_or_default(),
                "product_code":  r.try_get::<String, _>("product_code").unwrap_or_default(),
                "assigned_from": r.try_get::<time::Date, _>("assigned_from").map(|d| d.to_string()).unwrap_or_default(),
                "assigned_to":   r.try_get::<Option<time::Date>, _>("assigned_to").ok().flatten().map(|d| d.to_string()),
            })
        })
        .collect();
    Ok(out)
}

// ── Angebot (B2B Quotation, L4) ───────────────────────────────────────────────

/// Stored Angebot row.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct AngebotRow {
    pub id: Uuid,
    pub tenant: String,
    pub lf_mp_id: String,
    pub kunden_id: Option<Uuid>,
    pub interessent_name: Option<String>,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    pub angebotsnummer: String,
    pub status: String,
    pub gueltig_bis: Date,
    pub lieferbeginn: Option<Date>,
    pub laufzeit_monate: i16,
    pub positionen: serde_json::Value,
    pub varianten: serde_json::Value,
    /// BO4E `Angebot` business object for the priced quotation.
    ///
    /// `{}` until the quotation has been priced; written by
    /// `GET /api/v1/angebote/{id}/comparison`.
    pub bo4e: serde_json::Value,
    pub jahreskosten_netto_eur: Option<Decimal>,
    pub jahreskosten_brutto_eur: Option<Decimal>,
    pub gewaehlte_variante: Option<i16>,
    pub rahmenvertrag_id: Option<Uuid>,
    pub accepted_at: Option<time::OffsetDateTime>,
    pub declined_at: Option<time::OffsetDateTime>,
    pub erp_angebot_id: Option<String>,
    pub notizen: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
}

/// Request body for `POST /api/v1/angebote`.
#[derive(Debug, Deserialize)]
pub struct CreateAngebotRequest {
    pub lf_mp_id: Option<String>,
    /// Existing Kunde UUID in `vertragd`.
    pub kunden_id: Option<Uuid>,
    /// Free-text name for new prospects (when `kunden_id` is absent).
    pub interessent_name: Option<String>,
    pub contact_email: Option<String>,
    pub contact_phone: Option<String>,
    /// Proposal validity (YYYY-MM-DD).  Defaults to today + 10 Werktage.
    pub gueltig_bis: Option<String>,
    pub lieferbeginn: Option<String>,
    /// Contract duration in months: 1, 3, 6, 12, 24, 36, 48, or 60.
    pub laufzeit_monate: Option<i16>,
    /// Commodity positions to price.
    pub positionen: Vec<AngebotPositionInput>,
    /// Alternative scenarios (Varianten).  Optional — empty means single scenario.
    pub varianten: Option<Vec<AngebotVariante>>,
    pub erp_angebot_id: Option<String>,
    pub notizen: Option<String>,
}

/// One commodity/site position within an Angebot.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AngebotPositionInput {
    pub product_code: String,
    pub sparte: String,
    pub malo_id: Option<String>,
    pub standort_bezeichnung: Option<String>,
    /// Estimated annual consumption (kWh).  Required for price calculation.
    pub jahresverbrauch_kwh: Decimal,
    /// Peak power for RLM/C&I customers (kW) — required for capacity price.
    pub leistung_kw: Option<Decimal>,
    /// Tag for scenario display (e.g. "Eintarif", "Zweitarif HT/NT").
    pub szenario_tag: Option<String>,

    // ── NNE pass-through (DSO-specific, look up from marktd or NB Preisblatt) ────
    // These are mandatory for a customer-facing quotation.
    // NNE is typically 40–50 % of a commercial energy bill (BNetzA).
    // Source: PreisblattNetznutzung published by the NB; also available via
    // `marktd GET /api/v1/preisblaetter/{nb_mp_id}`.
    /// NNE Arbeitspreis in ct/kWh (Strom) or ct/kWh_Hs (Gas).
    pub nne_arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// NNE Grundpreis in EUR/year.
    pub nne_grundpreis_eur_per_year: Option<Decimal>,
    /// NNE Leistungspreis in EUR/kW/year — RLM/C&I only (≥ 2500 Jahresbenutzungsstunden).
    pub nne_leistungspreis_eur_per_kw_year: Option<Decimal>,
    /// Konzessionsabgabe in ct/kWh (KAV §2).
    /// Typical value: 0.11–1.99 ct/kWh depending on municipality size.
    pub ka_ct_per_kwh: Option<Decimal>,

    // ── Statutory levies ──────────────────────────────────────────────────────────
    // Defaults: Stromsteuer 2.05 ct/kWh (§3 StromStG), Gas Energiesteuer 0.55 ct/kWh.
    // For industry / §9a/§9b StromStG relief: set override to 0 or reduced rate.
    /// Stromsteuer override in ct/kWh (Strom). Default 2.05 (§3 StromStG).
    pub stromsteuer_ct_per_kwh: Option<Decimal>,
    /// Energiesteuer Gas override in ct/kWh_Hs (Gas). Default 0.55 (§2 EnergieStG).
    pub energiesteuer_gas_ct_per_kwh: Option<Decimal>,
    /// BEHG CO₂ levy override in ct/kWh_Hs (Gas only). Default 1.109 (55 EUR/t, 2025).
    pub behg_gas_ct_per_kwh: Option<Decimal>,
}

/// One alternative pricing scenario.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct AngebotVariante {
    /// Human-readable label, e.g. "12 Monate Festpreis" / "24 Monate Festpreis".
    pub label: String,
    pub laufzeit_monate: i16,
    /// Percentage discount applied to the base Arbeitspreis (e.g. 5.0 = 5 %).
    pub rabatt_pct: Option<Decimal>,
    /// Override product codes per position (index-aligned with top-level positionen).
    pub product_codes_override: Option<Vec<Option<String>>>,
}

/// Persist the BO4E `Angebot` document for a priced quotation.
///
/// # Errors
///
/// Returns an error when the update fails.
pub async fn store_angebot_bo4e(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    bo4e: &serde_json::Value,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE angebote SET bo4e = $1 WHERE id = $2 AND tenant = $3")
        .bind(bo4e)
        .bind(id)
        .bind(tenant)
        .execute(pool)
        .await?;
    Ok(())
}

/// Insert a new Angebot.
#[allow(clippy::too_many_arguments)]
/// Look up an existing Angebot by its ERP-supplied idempotency key.
///
/// `erp_angebot_id` is a tenant-scoped idempotency handle: an ERP that retries
/// `POST /angebote` with the same key must get the existing quotation back, not
/// a duplicate with a fresh Angebotsnummer.
pub async fn fetch_angebot_id_by_erp_id(
    pool: &PgPool,
    tenant: &str,
    erp_angebot_id: &str,
) -> anyhow::Result<Option<(Uuid, String)>> {
    let row = sqlx::query(
        "SELECT id, angebotsnummer FROM angebote
         WHERE tenant = $1 AND erp_angebot_id = $2 LIMIT 1",
    )
    .bind(tenant)
    .bind(erp_angebot_id)
    .fetch_optional(pool)
    .await
    .context("fetch_angebot_id_by_erp_id")?;
    Ok(row.map(|r| {
        (
            r.try_get("id").unwrap_or_default(),
            r.try_get("angebotsnummer").unwrap_or_default(),
        )
    }))
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_angebot(
    pool: &PgPool,
    tenant: &str,
    lf_mp_id: &str,
    angebotsnummer: &str,
    req: &CreateAngebotRequest,
    positionen_json: &serde_json::Value,
    varianten_json: &serde_json::Value,
    bo4e_json: &serde_json::Value,
    netto: Option<Decimal>,
    brutto: Option<Decimal>,
    gueltig_bis: Date,
    lieferbeginn: Option<Date>,
) -> anyhow::Result<Uuid> {
    let laufzeit = req.laufzeit_monate.unwrap_or(12);
    let row = sqlx::query(
        r"INSERT INTO angebote
              (tenant, lf_mp_id, kunden_id, interessent_name, contact_email, contact_phone,
               angebotsnummer, gueltig_bis, lieferbeginn, laufzeit_monate,
               positionen, varianten, bo4e,
               jahreskosten_netto_eur, jahreskosten_brutto_eur,
               erp_angebot_id, notizen)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17)
          RETURNING id",
    )
    .bind(tenant)
    .bind(lf_mp_id)
    .bind(req.kunden_id)
    .bind(&req.interessent_name)
    .bind(&req.contact_email)
    .bind(&req.contact_phone)
    .bind(angebotsnummer)
    .bind(gueltig_bis)
    .bind(lieferbeginn)
    .bind(laufzeit)
    .bind(positionen_json)
    .bind(varianten_json)
    .bind(bo4e_json)
    .bind(netto)
    .bind(brutto)
    .bind(&req.erp_angebot_id)
    .bind(&req.notizen)
    .fetch_one(pool)
    .await
    .context("insert_angebot")?;

    Ok(row.try_get("id")?)
}

pub async fn fetch_angebot(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
) -> anyhow::Result<Option<AngebotRow>> {
    sqlx::query_as::<_, AngebotRow>("SELECT * FROM angebote WHERE id = $1 AND tenant = $2")
        .bind(id)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        .context("fetch_angebot")
}

pub async fn list_angebote(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    status_filter: Option<&str>,
    limit: i64,
) -> anyhow::Result<Vec<AngebotRow>> {
    sqlx::query_as::<_, AngebotRow>(
        r"SELECT * FROM angebote
          WHERE tenant = $1 AND lf_mp_id = $2
            AND ($3::text IS NULL OR status = $3)
          ORDER BY created_at DESC
          LIMIT $4",
    )
    .bind(tenant)
    .bind(lf_mp_id)
    .bind(status_filter)
    .bind(limit)
    .fetch_all(pool)
    .await
    .context("list_angebote")
}

/// Transition Angebot to ANGENOMMEN.
///
/// Validates that `gueltig_bis >= today` before accepting.
/// Returns Err if the Angebot is already in a terminal state or has expired.
pub async fn accept_angebot(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    gewaehlte_variante: Option<i16>,
) -> anyhow::Result<AngebotRow> {
    let angebot = fetch_angebot(pool, id, tenant)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Angebot {id} not found"))?;

    let today = time::OffsetDateTime::now_utc().date();
    if angebot.gueltig_bis < today {
        // Auto-expire
        sqlx::query("UPDATE angebote SET status='ABGELAUFEN', updated_at=now() WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await?;
        anyhow::bail!(
            "Angebot {id} has expired (gueltig_bis={})",
            angebot.gueltig_bis
        );
    }
    if !matches!(angebot.status.as_str(), "ANGELEGT" | "VERSANDT") {
        anyhow::bail!(
            "Angebot {id} is in status '{}' — only ANGELEGT or VERSANDT can be accepted",
            angebot.status
        );
    }

    let row = sqlx::query_as::<_, AngebotRow>(
        r"UPDATE angebote
          SET status = 'ANGENOMMEN',
              gewaehlte_variante = $3,
              accepted_at = now(),
              updated_at  = now()
          WHERE id = $1 AND tenant = $2
          RETURNING *",
    )
    .bind(id)
    .bind(tenant)
    .bind(gewaehlte_variante)
    .fetch_one(pool)
    .await
    .context("accept_angebot")?;

    Ok(row)
}

/// Transition Angebot to ABGELEHNT.
pub async fn decline_angebot(pool: &PgPool, id: Uuid, tenant: &str) -> anyhow::Result<()> {
    let updated = sqlx::query(
        r"UPDATE angebote
          SET status = 'ABGELEHNT', declined_at = now(), updated_at = now()
          WHERE id = $1 AND tenant = $2
            AND status IN ('ANGELEGT', 'VERSANDT')",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("decline_angebot")?
    .rows_affected();

    anyhow::ensure!(
        updated > 0,
        "Angebot {id} not found or not in a declinable state"
    );
    Ok(())
}

/// Transition Angebot to VERSANDT (mark as sent to customer).
pub async fn mark_angebot_versandt(pool: &PgPool, id: Uuid, tenant: &str) -> anyhow::Result<()> {
    let updated = sqlx::query(
        r"UPDATE angebote
          SET status = 'VERSANDT', updated_at = now()
          WHERE id = $1 AND tenant = $2 AND status = 'ANGELEGT'",
    )
    .bind(id)
    .bind(tenant)
    .execute(pool)
    .await
    .context("mark_angebot_versandt")?
    .rows_affected();

    anyhow::ensure!(
        updated > 0,
        "Angebot {id} not found or not in ANGELEGT state"
    );
    Ok(())
}

/// Update rahmenvertrag_id after successful acceptance + contract creation.
pub async fn link_angebot_rahmenvertrag(
    pool: &PgPool,
    id: Uuid,
    rahmenvertrag_id: Uuid,
) -> anyhow::Result<()> {
    sqlx::query("UPDATE angebote SET rahmenvertrag_id=$2, updated_at=now() WHERE id=$1")
        .bind(id)
        .bind(rahmenvertrag_id)
        .execute(pool)
        .await
        .context("link_angebot_rahmenvertrag")?;
    Ok(())
}

/// Auto-expire all Angebote past their gueltig_bis date.
/// Called periodically by the background task.
pub async fn expire_stale_angebote(pool: &PgPool) -> anyhow::Result<u64> {
    let r = sqlx::query(
        r"UPDATE angebote
          SET status = 'ABGELAUFEN', updated_at = now()
          WHERE status IN ('ANGELEGT', 'VERSANDT')
            AND gueltig_bis < CURRENT_DATE",
    )
    .execute(pool)
    .await
    .context("expire_stale_angebote")?;
    Ok(r.rows_affected())
}

/// Generate the next Angebotsnummer in sequence.
/// Format: `ANG-{YYYY}-{6-digit-seq}` — e.g. `ANG-2026-000001`.
pub async fn next_angebotsnummer(pool: &PgPool, tenant: &str) -> anyhow::Result<String> {
    let year = time::OffsetDateTime::now_utc().year();
    let row = sqlx::query(
        r"SELECT COUNT(*)+1 AS seq FROM angebote
          WHERE tenant = $1
            AND extract(year FROM created_at) = $2",
    )
    .bind(tenant)
    .bind(year)
    .fetch_one(pool)
    .await
    .context("next_angebotsnummer")?;
    let seq: i64 = row.try_get("seq").unwrap_or(1);
    Ok(format!("ANG-{year}-{seq:06}"))
}

// ── Comparison portal feed ────────────────────────────────────────────────────

/// Categories that appear in comparison portals (energy tariffs only).
///
/// Excludes HEMS, EMOBILITY, ENERGIEDIENSTLEISTUNG, BUNDLE, EEG, EINSPEISUNG —
/// those are not consumer-facing energy tariffs suitable for portal listing.
pub const FEED_CATEGORIES: &[&str] = &["STROM", "GAS", "WAERME", "SOLAR", "WAERMEPUMPE", "WALLBOX"];

/// Query parameters for `GET /api/v1/comparison-feed`.
#[derive(Debug, serde::Deserialize)]
pub struct ComparisonFeedQuery {
    /// Filter by LF operator (defaults to `cfg.tenant`).
    pub lf_mp_id: Option<String>,
    /// Filter by Sparte: `STROM` | `GAS` | `WAERME`.
    pub sparte: Option<String>,
    /// Filter by customer segment: `Haushalt` | `Gewerbe` | `Waermepumpe` | `Ladesaeule`.
    pub kundentyp: Option<String>,
    /// Annual consumption in kWh used to estimate `jahreskosten_supply_*`.
    /// Defaults to `3500` (BNetzA reference household).
    pub verbrauch_kwh: Option<rust_decimal::Decimal>,
    /// Filter to products carrying a specific Oekolabel (e.g. `OK_POWER`).
    /// Use `oekolabel=OK_POWER` to list only certified green tariffs.
    pub oekolabel: Option<String>,
    /// Include §41a EPEX-linked dynamic tariffs.  Default: `true`.
    pub include_dynamic: Option<bool>,
    /// Return **only** §41a dynamic tariffs.  Default: `false`.
    pub only_dynamic: Option<bool>,
    /// Max results per page (1–500, default 100).
    pub limit: Option<i64>,
    /// Pagination cursor — the `updated_at` value of the last item on the
    /// previous page (ISO 8601 UTC).  Absent on first request.
    pub cursor: Option<String>,
}

/// Extracted tariff price points for a single product.
///
/// All prices are in **ct/kWh** or **ct/day**.  `None` means the product
/// does not define that price dimension (e.g. no ARBEITSPREIS_NT on an
/// Eintarif product).
#[derive(Debug, serde::Serialize)]
pub struct TarifPreise {
    /// Daily standing charge in ct/day (= Grundpreis).
    pub grundpreis_ct_per_day: Option<rust_decimal::Decimal>,
    /// Working price for single-rate tariffs (= ARBEITSPREIS_EINTARIF).
    /// `None` on dual-rate (HT/NT) tariffs; use `arbeitspreis_ht` instead.
    pub arbeitspreis_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// High-tariff rate (= ARBEITSPREIS_HT).  `None` on single-rate tariffs.
    pub arbeitspreis_ht_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Low-tariff rate (= ARBEITSPREIS_NT).  `None` on single-rate tariffs.
    pub arbeitspreis_nt_ct_per_kwh: Option<rust_decimal::Decimal>,
    /// Demand charge in ct/kW/month for RLM products (= LEISTUNGSPREIS).
    pub leistungspreis_ct_per_kw_month: Option<rust_decimal::Decimal>,
}

/// One entry in the comparison portal feed response.
///
/// Includes the full validated `tarifpreisblatt` BO4E payload alongside
/// computed `jahreskosten` and extracted portal-relevant fields.
#[derive(Debug, serde::Serialize)]
pub struct ComparisonFeedEntry {
    pub product_code: String,
    pub name: String,
    pub category: String,
    pub sparte: Option<String>,
    /// Customer segment this tariff is designed for (portal category filter).
    pub kundentyp: Option<String>,
    /// Meter register count: `Eintarif` | `Zweitarif` | `Mehrtarif`.
    pub register_count: Option<String>,
    /// `true` if the product has at least one Oekolabel certification.
    pub ist_oekostrom: bool,
    /// `true` if the product is a §41a EPEX-linked dynamic tariff.
    pub ist_dynamisch: bool,
    /// Product validity start (inclusive).  `null` = no start constraint.
    pub valid_from: Option<time::Date>,
    /// Product validity end (inclusive).  `null` = indefinitely valid.
    pub valid_to: Option<time::Date>,
    /// Extracted price points from `tarifpreisblatt.tarifpreispositionen`.
    pub preise: TarifPreise,
    /// Estimated annual supply cost in EUR **netto** (excl. MwSt) for
    /// `verbrauch_kwh`.  Includes Grundpreis + Arbeitspreis.
    ///
    /// Does **not** include NNE, KA, or statutory levies — those vary by
    /// DSO/PLZ and must be added by the comparison portal integrator.
    /// `null` if no standard Grundpreis or Arbeitspreis is defined.
    pub jahreskosten_supply_netto_eur: Option<rust_decimal::Decimal>,
    /// Estimated annual supply cost **brutto** (incl. 19 % MwSt).
    /// Derived from `jahreskosten_supply_netto_eur × 1.19`.
    /// `null` if `jahreskosten_supply_netto_eur` is `null`.
    pub jahreskosten_supply_brutto_eur: Option<rust_decimal::Decimal>,
    /// MwSt rate applied to compute the brutto estimate.
    pub mwst_pct: &'static str,
    /// Contract term extracted from `vertragskonditionen.laufzeit` in months.
    pub laufzeit_monate: Option<i32>,
    /// Notice period from `vertragskonditionen.kuendigungsfrist` in weeks.
    pub kuendigungsfrist_wochen: Option<i32>,
    /// Minimum contract term in months (= `vertragskonditionen.mindestlaufzeit`).
    pub mindestlaufzeit_monate: Option<i32>,
    /// Price guarantee end date (ISO 8601) from `preisgarantie.preisgarantieBis`.
    /// `null` if no price guarantee is defined.
    pub preisgarantie_bis: Option<String>,
    /// Total customer bonus/discount from `aufAbschlaege` RABATT entries in EUR.
    /// `null` if no bonuses are defined.
    pub bonus_rabatt_eur: Option<rust_decimal::Decimal>,
    /// §42 EnWG `Energiemix` COM payload.  `null` if not set.
    pub energiemix: Option<serde_json::Value>,
    /// Oekolabel certification codes (e.g. `["OK_POWER", "GRUENER_STROM"]`).
    pub oekolabel: Option<Vec<String>>,
    /// Full validated BO4E `Tarifpreisblatt` payload.
    /// Portal integrators may use this for deep tariff analysis.
    pub tarifpreisblatt: serde_json::Value,
    /// §42d EnWG: Full BO4E `Tarifinfo` Business Object envelope.
    ///
    /// Ready for direct schema-validated import by Verivox, Check24, and the
    /// BNetzA Markttransparenzstelle.  Eliminates the manual ETL step for portal
    /// integration: the portal receives a standard BO4E object, not a custom JSON.
    ///
    /// Fields mapped:
    /// - `bezeichnung` ← product `name`
    /// - `sparte` ← product `sparte` → `rubo4e::Sparte`
    /// - `kundentypen` ← product `kundentyp` → `[rubo4e::Kundentyp]`
    /// - `registeranzahl` ← product `register_count` → `rubo4e::Registeranzahl`
    /// - `tariftyp` ← `data.tariftyp` → `rubo4e::Tariftyp`
    /// - `tarifmerkmale` ← derived from preisgarantie, category, dyn_source
    /// - `energiemix` ← product `energiemix` → `rubo4e::Energiemix`
    /// - `zeitlicheGueltigkeit` ← product `valid_from/valid_to`
    /// - `vertragskonditionen` ← `data.vertragskonditionen`
    /// - `anbietername` ← `lf_mp_id`
    /// - `_id` ← `product_code`
    pub tarifinfo: serde_json::Value,
    /// RFC 3339 timestamp of the last product update.
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

/// Response metadata for `GET /api/v1/comparison-feed`.
#[derive(Debug, serde::Serialize)]
pub struct ComparisonFeedMeta {
    /// UTC timestamp when this response was generated.
    #[serde(with = "time::serde::rfc3339")]
    pub generated_at: time::OffsetDateTime,
    /// LF operator identifier (BDEW-Codenummer).
    pub lf_mp_id: String,
    /// Annual consumption used for `jahreskosten` estimates.
    pub verbrauch_kwh: rust_decimal::Decimal,
    /// Active Sparte filter, or `null` for all Sparten.
    pub sparte_filter: Option<String>,
    /// Active Kundentyp filter, or `null` for all customer types.
    pub kundentyp_filter: Option<String>,
    /// Number of tariff entries returned in this page.
    pub total_returned: usize,
    /// Pagination cursor for the next page.  `null` if this is the last page.
    /// Pass as `?cursor=<value>` in the next request.
    pub next_cursor: Option<String>,
}

/// `GET /api/v1/comparison-feed` response envelope.
#[derive(Debug, serde::Serialize)]
pub struct ComparisonFeedResponse {
    pub meta: ComparisonFeedMeta,
    pub tarife: Vec<ComparisonFeedEntry>,
}

/// Fetch products suitable for a comparison portal feed.
///
/// Returns the **currently valid** version of each energy tariff product for
/// the given LF, ordered by `(updated_at DESC, product_code ASC)` for stable
/// cursor-based pagination.
///
/// ## Filters applied
///
/// | Filter | SQL condition |
/// |---|---|
/// | Category allowlist | `category IN ('STROM','GAS','WAERME','SOLAR','WAERMEPUMPE','WALLBOX')` |
/// | Validity window | `valid_to IS NULL OR valid_to >= CURRENT_DATE` |
/// | Validity start | `valid_from IS NULL OR valid_from <= CURRENT_DATE` |
/// | Sparte | optional equality |
/// | Kundentyp | optional equality |
/// | Oekolabel | optional `@>` array containment |
/// | Dynamic only | `dyn_source IS NOT NULL` |
/// | Exclude dynamic | `dyn_source IS NULL` |
/// | Cursor | `(updated_at, product_code) < (cursor_ts, cursor_code)` |
pub async fn fetch_comparison_feed(
    pool: &PgPool,
    lf_mp_id: &str,
    q: &ComparisonFeedQuery,
) -> anyhow::Result<Vec<ProductRow>> {
    use time::format_description::well_known::Rfc3339;

    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    // Fetch one extra row to detect whether a next page exists.
    let fetch_limit = limit + 1;

    // Parse cursor: "<rfc3339_timestamp>,<product_code>"
    let (cursor_ts, cursor_code): (Option<time::OffsetDateTime>, Option<String>) =
        if let Some(c) = q.cursor.as_deref() {
            if let Some((ts_part, code_part)) = c.split_once(',') {
                let ts = time::OffsetDateTime::parse(ts_part, &Rfc3339).ok();
                (ts, Some(code_part.to_owned()))
            } else {
                // Legacy: cursor is just a timestamp (no product_code tie-breaker)
                let ts = time::OffsetDateTime::parse(c, &Rfc3339).ok();
                (ts, None)
            }
        } else {
            (None, None)
        };

    let only_dynamic = q.only_dynamic.unwrap_or(false);
    let exclude_dynamic = q.include_dynamic.map(|b| !b).unwrap_or(false);

    // Wrap an oekolabel filter: NULL = no filter; Some("X") = must contain "X".
    let oekolabel_filter: Option<Vec<String>> = q.oekolabel.as_ref().map(|l| vec![l.clone()]);

    sqlx::query_as::<_, ProductRow>(
        r"SELECT DISTINCT ON (product_code) *
          FROM products
          WHERE lf_mp_id = $1
            AND category = ANY($2)
            AND (valid_to IS NULL OR valid_to >= CURRENT_DATE)
            AND (valid_from IS NULL OR valid_from <= CURRENT_DATE)
            AND ($3::text IS NULL OR sparte = $3)
            AND ($4::text IS NULL OR kundentyp = $4)
            AND ($5::bool IS FALSE OR dyn_source IS NOT NULL)
            AND ($6::bool IS FALSE OR dyn_source IS NULL)
            AND ($7::text[] IS NULL OR oekolabel @> $7)
            AND product_status = 'PUBLISHED'
            AND (
                $8::timestamptz IS NULL
                OR updated_at < $8
                OR (updated_at = $8 AND ($9::text IS NULL OR product_code > $9))
            )
          ORDER BY product_code, valid_from DESC NULLS LAST",
    )
    .bind(lf_mp_id)
    .bind(FEED_CATEGORIES)
    .bind(&q.sparte)
    .bind(&q.kundentyp)
    .bind(only_dynamic)
    .bind(exclude_dynamic)
    .bind(&oekolabel_filter)
    .bind(cursor_ts)
    .bind(&cursor_code)
    .fetch_all(pool)
    .await
    .context("fetch_comparison_feed: DISTINCT ON query")
    .map(|mut rows| {
        // Re-sort by (updated_at DESC, product_code ASC) for stable pagination
        // after DISTINCT ON picks the latest valid_from per product_code.
        rows.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then(a.product_code.cmp(&b.product_code))
        });
        // Apply pagination limit (with one extra for next-page detection)
        rows.truncate(fetch_limit as usize);
        rows
    })
}
