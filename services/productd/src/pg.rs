//! PostgreSQL persistence for `productd`.

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
    mako_markt::bo4e::schema_version().to_owned()
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

    // Staging the next price version closes the one it succeeds. Scheduling a
    // price change is the ordinary way this table is used, and requiring the
    // operator to first go back and end-date the running version — the only
    // alternative once versions may not overlap — turns one act into two, with
    // an unpriced gap or a rejected write whenever they forget the first.
    //
    // Only a dated version can close anything: an open-ended one (`valid_from`
    // NULL) claims all of time, and if another version exists the exclusion
    // constraint says so rather than this silently truncating it.
    if let Some(from) = valid_from {
        sqlx::query(
            r"UPDATE products
              SET valid_to = $4 - 1, updated_at = now()
              WHERE tenant = $1 AND lf_mp_id = $2 AND product_code = $3
                AND COALESCE(valid_from, DATE '0001-01-01') < $4
                AND (valid_to IS NULL OR valid_to >= $4)",
        )
        .bind(tenant)
        .bind(lf_mp_id)
        .bind(product_code)
        .bind(from)
        .execute(pool)
        .await
        .context("close the superseded product version")?;
    }

    let row = sqlx::query(
        r"INSERT INTO products
              (lf_mp_id, product_code, category, name, sparte, register_count, kundentyp,
               dyn_source, valid_from, valid_to, data, bo4e_version, product_status,
               energiemix, oekolabel, tenant, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, now())
          ON CONFLICT (tenant, lf_mp_id, product_code, (COALESCE(valid_from, DATE '0001-01-01')))
          DO UPDATE
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
          WHERE products.tenant = EXCLUDED.tenant
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

/// The product version in force at `as_of`.
///
/// `as_of` defaults to today in Berlin.
///
/// Both bounds are applied here. Without `valid_from` the highest version won
/// outright, so a price staged for next quarter became the current one the
/// moment it was written. Without `valid_to` a withdrawn product — including
/// one soft-deleted through `DELETE /products/…` — went on pricing invoices
/// for ever, because the only thing that ever checked the end date was a
/// sentence in a doc comment telling callers to check it themselves.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn fetch_product(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
    as_of: Option<Date>,
) -> anyhow::Result<Option<ProductRow>> {
    sqlx::query_as::<_, ProductRow>(
        "SELECT * FROM products WHERE lf_mp_id = $1 AND product_code = $2 AND tenant = $3
           AND (valid_from IS NULL OR valid_from <= $4)
           AND (valid_to   IS NULL OR valid_to   >= $4)
         ORDER BY valid_from DESC NULLS LAST LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .bind(as_of.unwrap_or_else(berlin_today))
    .fetch_optional(pool)
    .await
    .context("fetch product")
}

/// Withdraw a product by setting `valid_to = today`.
///
/// The row stays for historical lookups and the audit log; [`fetch_product`]
/// stops returning it for any date after today, so nothing new can be priced
/// from it while an invoice for a past period still can.
///
/// Returns `true` if a row was found and updated.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn soft_delete_product(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<bool> {
    let today = berlin_today();
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

/// Fetch the `Energiemix` + `Oekolabel` of the product version in force today.
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
            AND (valid_from IS NULL OR valid_from <= $4)
          ORDER BY valid_from DESC NULLS LAST
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(tenant)
    .bind(berlin_today())
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

// ── EPEX day-ahead prices ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EpexImportRequest {
    /// Ordered ct/kWh values for the delivery day's market time units, in
    /// UTC-instant order (which equals local wall-clock order). The length must
    /// equal the number of MTUs in the local delivery day:
    /// **96** (15-min) / **92** (spring DST) / **100** (autumn DST), or
    /// **24 / 23 / 25** at 60-min.
    pub prices: Vec<Decimal>,
    /// MTU length in minutes: `15` (default; SDAC 15-min go-live 2025-10-01) or
    /// `60` (legacy hourly source). 60-min rows are expanded to quarter-hours on
    /// fetch.
    #[serde(default)]
    pub mtu_minutes: Option<u16>,
    pub source: Option<String>,
}

/// One quarter-hour spot price point (fetch result, expanded to 15-min).
#[derive(Debug, Clone)]
pub struct EpexPricePoint {
    /// UTC start instant of the 15-minute market time unit.
    pub mtu_start: OffsetDateTime,
    /// Spot price in ct/kWh.
    pub avg_ct_kwh: Decimal,
}

/// Today's civil date in Europe/Berlin.
///
/// Every deadline in this service is a German civil date. Reading the UTC date
/// makes the day roll over an hour early in summer, which expires an Angebot on
/// its `gueltig_bis` evening and switches a price version a day early.
#[must_use]
pub fn berlin_today() -> Date {
    use time_tz::OffsetDateTimeExt as _;
    OffsetDateTime::now_utc()
        .to_timezone(time_tz::timezones::db::europe::BERLIN)
        .date()
}

/// UTC instant of Europe/Berlin local midnight for `date`.
///
/// Midnight never falls in a DST gap/overlap (transitions are at 02:00/03:00),
/// so `take_first` is unambiguous.
fn berlin_midnight_utc(date: Date) -> OffsetDateTime {
    use time_tz::PrimitiveDateTimeExt;
    let berlin = time_tz::timezones::db::europe::BERLIN;
    date.midnight()
        .assume_timezone(berlin)
        .take_first()
        .expect("Berlin local midnight is unambiguous")
}

pub async fn upsert_epex_day(
    pool: &PgPool,
    date: Date,
    req: EpexImportRequest,
) -> anyhow::Result<()> {
    let mtu_minutes: i64 = req.mtu_minutes.unwrap_or(15).into();
    if mtu_minutes != 15 && mtu_minutes != 60 {
        anyhow::bail!("mtu_minutes must be 15 or 60");
    }

    // Delivery day boundaries as UTC instants. The local day spans 23/24/25 h
    // across DST, so the MTU count is derived — never hard-coded to 24/96.
    let day_start = berlin_midnight_utc(date);
    let next_date = date.next_day().context("date overflow")?;
    let day_end = berlin_midnight_utc(next_date);
    let span_minutes = (day_end - day_start).whole_minutes();
    let expected = usize::try_from(span_minutes / mtu_minutes).unwrap_or(0);
    if req.prices.len() != expected {
        anyhow::bail!(
            "prices must have exactly {expected} entries for {date} at {mtu_minutes}-min MTU \
             (local day is {}h; got {})",
            span_minutes / 60,
            req.prices.len()
        );
    }

    let source = req.source.as_deref().unwrap_or("manual");
    let step = time::Duration::minutes(mtu_minutes);

    // Replace the whole day so a resolution change (e.g. legacy hourly → 15-min)
    // never leaves stale rows behind (hard cut, no backfill reconciliation).
    sqlx::query("DELETE FROM epex_prices WHERE price_date = $1")
        .bind(date)
        .execute(pool)
        .await
        .context("clear epex day")?;

    for (i, price) in req.prices.iter().enumerate() {
        let mtu_start = day_start + step * i32::try_from(i).unwrap_or(i32::MAX);
        sqlx::query(
            r"INSERT INTO epex_prices (mtu_start, price_date, mtu_minutes, avg_ct_kwh, source)
              VALUES ($1, $2, $3, $4, $5)
              ON CONFLICT (mtu_start) DO UPDATE
              SET price_date = EXCLUDED.price_date,
                  mtu_minutes = EXCLUDED.mtu_minutes,
                  avg_ct_kwh = EXCLUDED.avg_ct_kwh,
                  source = EXCLUDED.source,
                  imported_at = now()",
        )
        .bind(mtu_start)
        .bind(date)
        .bind(i16::try_from(mtu_minutes).unwrap_or(15))
        .bind(price)
        .bind(source)
        .execute(pool)
        .await
        .context("upsert epex mtu")?;
    }
    Ok(())
}

/// Fetch a delivery day's spot prices, normalised to **15-minute** points.
///
/// Legacy 60-minute rows are expanded to four identical quarter-hours so the
/// billing layer always sees a uniform 15-min series.
pub async fn fetch_epex_day(
    pool: &PgPool,
    date: Date,
) -> anyhow::Result<Option<Vec<EpexPricePoint>>> {
    let rows = sqlx::query(
        "SELECT mtu_start, mtu_minutes, avg_ct_kwh FROM epex_prices \
         WHERE price_date = $1 ORDER BY mtu_start ASC",
    )
    .bind(date)
    .fetch_all(pool)
    .await
    .context("fetch_epex_day")?;

    if rows.is_empty() {
        return Ok(None);
    }

    let mut out = Vec::with_capacity(rows.len() * 4);
    for r in &rows {
        let mtu_start: OffsetDateTime = r.try_get("mtu_start")?;
        let mtu_minutes: i16 = r.try_get("mtu_minutes")?;
        let price: Decimal = r.try_get("avg_ct_kwh")?;
        let quarters = i64::from(mtu_minutes).max(15) / 15; // 1 (15-min) or 4 (60-min)
        for q in 0..quarters {
            out.push(EpexPricePoint {
                mtu_start: mtu_start + time::Duration::minutes(15 * q),
                avg_ct_kwh: price,
            });
        }
    }
    Ok(Some(out))
}

/// Average EPEX price for a month (ct/kWh).
/// Used by `billingd` for §41a dynamic tariff billing and by `einsd` for Direktvermarktung.
pub async fn monthly_epex_average(
    pool: &PgPool,
    year: i32,
    month: u8,
) -> anyhow::Result<Option<Decimal>> {
    // Weighted by MTU length: a month that mixes hourly and quarter-hourly rows
    // (the SDAC 15-min go-live falls inside one) skews a plain AVG toward
    // whichever resolution has more rows, not more energy.
    let row = sqlx::query(
        r"SELECT SUM(avg_ct_kwh * mtu_minutes) / NULLIF(SUM(mtu_minutes), 0) AS avg
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

// ── nEHS certificate prices (BEHG CO₂) ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct NehsImportRequest {
    /// EUR per tonne CO₂ — an auction clearing price, an Einführungsphase
    /// Festpreis, or the Mehrmengenpreis of a Nachkauf.
    pub eur_per_t: Decimal,
    /// `auktion` | `verkaufsphase` | `nachkauf` | `manual`.
    pub source: Option<String>,
}

/// Check an nEHS price point against § 10 BEHG for its date.
///
/// The CO₂ component of every Gas and Wärme invoice is derived from this
/// series, so a decimal slip here mis-bills every gas customer at once and is
/// invisible on any single invoice. What the statute pins is checked exactly,
/// what it bounds is bounded, and what it leaves open is accepted — see
/// [`crate::behg`].
///
/// # Errors
///
/// The value cannot be right for that date and source.
pub fn validate_nehs_import(req: &NehsImportRequest, price_date: Date) -> anyhow::Result<&str> {
    let source = req.source.as_deref().unwrap_or("manual");
    let quelle = crate::behg::Quelle::parse(source).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown source {source:?} — expected one of: auktion, verkaufsphase, nachkauf, manual"
        )
    })?;
    if let Err(b) = crate::behg::pruefe(price_date, req.eur_per_t, quelle) {
        anyhow::bail!("{} ({})", b.grund, b.rechtsgrundlage);
    }
    Ok(source)
}

pub async fn upsert_nehs_price(
    pool: &PgPool,
    date: Date,
    req: NehsImportRequest,
) -> anyhow::Result<()> {
    let source = validate_nehs_import(&req, date)?;
    sqlx::query(
        r"INSERT INTO nehs_prices (price_date, eur_per_t, source)
          VALUES ($1, $2, $3)
          ON CONFLICT (price_date) DO UPDATE
            SET eur_per_t = EXCLUDED.eur_per_t,
                source = EXCLUDED.source,
                imported_at = now()",
    )
    .bind(date)
    .bind(req.eur_per_t)
    .bind(source)
    .execute(pool)
    .await
    .context("upsert_nehs_price")?;
    Ok(())
}

/// Most recent nEHS price on or before `date` (EUR/t), if any.
///
/// The auction series is weekly (Wednesdays, from 01.07.2026), so billing
/// looks up the latest price at or before the delivery/billing date.
pub async fn latest_nehs_price(
    pool: &PgPool,
    date: Date,
) -> anyhow::Result<Option<(Date, Decimal)>> {
    let row = sqlx::query(
        r"SELECT price_date, eur_per_t
          FROM nehs_prices
          WHERE price_date <= $1
          ORDER BY price_date DESC
          LIMIT 1",
    )
    .bind(date)
    .fetch_optional(pool)
    .await
    .context("latest_nehs_price")?;
    Ok(row.map(|r| {
        (
            r.get::<Date, _>("price_date"),
            r.get::<Decimal, _>("eur_per_t"),
        )
    }))
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
    /// Messlokation of the supply point. **Mandatory for GAS**: the
    /// registration `vertragd` files on acceptance carries it as the
    /// Zählpunktbezeichnung (RFF+Z13), and a MaLo-ID is not one — a quotation
    /// accepted without it produced a contract that could never be registered.
    pub melo_id: Option<String>,
    /// The Netzbetreiber behind the supply point — the UTILMD's recipient.
    /// Without it the contract created on acceptance has nothing to register
    /// against and sits in ANGELEGT for ever.
    pub nb_mp_id: Option<String>,
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
    /// BEHG CO₂ levy override in ct/kWh_Hs (Gas only).
    ///
    /// Absent, the rate is derived from the dated `nehs_prices` series — the
    /// certificate price is market-formed since the 2026 auctions (§ 10 Abs. 1
    /// BEHG), so a constant baked in here quotes last year's CO₂ cost.
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

/// Returns `true` when a quotation's BO4E `Angebot` has been priced.
///
/// `angebote.bo4e` is stored as `{}` (empty object) until
/// `GET /api/v1/angebote/{id}/comparison` prices it; an empty object, JSON
/// `null`, or any non-object value all count as unpriced.
#[must_use]
pub fn angebot_is_priced(bo4e: &serde_json::Value) -> bool {
    bo4e.as_object().is_some_and(|m| !m.is_empty())
}

/// Transition Angebot to ANGENOMMEN.
///
/// Validates that `gueltig_bis >= today` **and** that the quotation has been
/// priced (`bo4e` populated) before accepting. Returns Err if the Angebot is in
/// a terminal state, has expired, or has not been priced.
pub async fn accept_angebot(
    pool: &PgPool,
    id: Uuid,
    tenant: &str,
    gewaehlte_variante: Option<i16>,
) -> anyhow::Result<AngebotRow> {
    let angebot = fetch_angebot(pool, id, tenant)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Angebot {id} not found"))?;

    let today = berlin_today();
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

    // A quotation must be priced before it can be accepted — otherwise
    // `de.tarif.angebot.angenommen` would carry an empty BO4E `Angebot` and the
    // downstream Rahmenvertrag build has nothing to derive the contract from.
    if !angebot_is_priced(&angebot.bo4e) {
        anyhow::bail!(
            "Angebot {id} is not priced yet — call GET /api/v1/angebote/{id}/comparison \
             before accepting"
        );
    }

    // The status was read on a separate statement, so a decline or an expiry
    // landing in between was overwritten and a terminal ABGELEHNT/ABGELAUFEN
    // became ANGENOMMEN. The state the read established is re-asserted here.
    let row = sqlx::query_as::<_, AngebotRow>(
        r"UPDATE angebote
          SET status = 'ANGENOMMEN',
              gewaehlte_variante = $3,
              accepted_at = now(),
              updated_at  = now()
          WHERE id = $1 AND tenant = $2
            AND status IN ('ANGELEGT', 'VERSANDT')
          RETURNING *",
    )
    .bind(id)
    .bind(tenant)
    .bind(gewaehlte_variante)
    .fetch_optional(pool)
    .await
    .context("accept_angebot")?;

    // The handler maps any error here to 409, which is what a lost race is.
    row.ok_or_else(|| {
        anyhow::anyhow!(
            "Angebot {id} left its acceptable state concurrently — it was declined or expired"
        )
    })
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
/// Mark this tenant's open Angebote whose Bindefrist has passed as ABGELAUFEN.
///
/// Tenant-scoped: a sweep that ran across every tenant let one deployment's
/// worker close another's quotations, and neither operator could see why.
///
/// # Errors
///
/// Propagates storage errors.
pub async fn expire_stale_angebote(pool: &PgPool, tenant: &str) -> anyhow::Result<u64> {
    let res = sqlx::query(
        r"UPDATE angebote
          SET status = 'ABGELAUFEN', updated_at = now()
          WHERE tenant = $1
            AND status IN ('ANGELEGT', 'VERSANDT')
            AND gueltig_bis < $2",
    )
    .bind(tenant)
    .bind(berlin_today())
    .execute(pool)
    .await
    .context("expire_stale_angebote")?;
    Ok(res.rows_affected())
}

/// Generate the next Angebotsnummer in sequence.
/// Format: `ANG-{YYYY}-{6-digit-seq}` — e.g. `ANG-2026-000001`.
///
/// Counted in a per-(tenant, year) row rather than as `COUNT(*) + 1`: two
/// concurrent quotations both read the same count and then collided on the
/// `angebotsnummer` unique constraint, so one of them simply failed.
pub async fn next_angebotsnummer(pool: &PgPool, tenant: &str) -> anyhow::Result<String> {
    let year = berlin_today().year();
    let seq: i64 = sqlx::query_scalar(
        r"INSERT INTO angebot_sequenzen (tenant, jahr, letzte_nummer)
          VALUES ($1, $2, 1)
          ON CONFLICT (tenant, jahr) DO UPDATE
          SET letzte_nummer = angebot_sequenzen.letzte_nummer + 1
          RETURNING letzte_nummer",
    )
    .bind(tenant)
    .bind(year)
    .fetch_one(pool)
    .await
    .context("next_angebotsnummer")?;
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
    /// Extracted price points from `tarifpreisblatt.tarifpreise`.
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
    /// § 41c EnWG: Full BO4E `Tarifinfo` Business Object envelope.
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

#[cfg(test)]
mod angebot_pricing_tests {
    use super::angebot_is_priced;
    use serde_json::json;

    #[test]
    fn empty_object_is_unpriced() {
        assert!(!angebot_is_priced(&json!({})));
    }

    #[test]
    fn null_is_unpriced() {
        assert!(!angebot_is_priced(&serde_json::Value::Null));
    }

    #[test]
    fn non_object_is_unpriced() {
        assert!(!angebot_is_priced(&json!("not-an-object")));
        assert!(!angebot_is_priced(&json!([1, 2, 3])));
    }

    #[test]
    fn populated_bo4e_object_is_priced() {
        assert!(angebot_is_priced(
            &json!({ "_typ": "ANGEBOT", "angebotsnummer": "A-1" })
        ));
    }
}

#[cfg(test)]
mod nehs_validation_tests {
    use super::{NehsImportRequest, validate_nehs_import};
    use rust_decimal::dec;
    use time::macros::date;

    fn req(eur: rust_decimal::Decimal, source: Option<&str>) -> NehsImportRequest {
        NehsImportRequest {
            eur_per_t: eur,
            source: source.map(str::to_owned),
        }
    }

    #[test]
    fn an_auction_price_inside_the_corridor_is_accepted() {
        assert_eq!(
            validate_nehs_import(&req(dec!(63.50), Some("auktion")), date!(2026 - 07 - 08))
                .unwrap(),
            "auktion"
        );
    }

    #[test]
    fn a_decimal_slip_is_refused_with_the_corridor_named() {
        let err = validate_nehs_import(&req(dec!(6.35), Some("auktion")), date!(2026 - 07 - 08))
            .expect_err("6.35 EUR/t is a typo, not a clearing price");
        let msg = err.to_string();
        assert!(msg.contains("Preiskorridor"), "got: {msg}");
        assert!(msg.contains("§ 10 Abs. 2 BEHG"), "got: {msg}");
    }

    #[test]
    fn the_nachkauf_price_is_not_the_corridor() {
        assert!(
            validate_nehs_import(&req(dec!(68), Some("nachkauf")), date!(2026 - 11 - 10)).is_ok()
        );
        assert!(
            validate_nehs_import(&req(dec!(68), Some("auktion")), date!(2026 - 11 - 10)).is_err(),
            "68 EUR/t is above the auction corridor — it is the Mehrmengenpreis"
        );
    }

    #[test]
    fn omitted_source_defaults_to_manual() {
        assert_eq!(
            validate_nehs_import(&req(dec!(68), None), date!(2026 - 11 - 10)).unwrap(),
            "manual"
        );
    }

    #[test]
    fn unknown_source_is_rejected_with_the_valid_ones_listed() {
        let err = validate_nehs_import(&req(dec!(63.50), Some("boerse")), date!(2026 - 07 - 08))
            .expect_err("unknown source must fail");
        let msg = err.to_string();
        assert!(msg.contains("unknown source"), "got: {msg}");
        assert!(
            msg.contains("auktion"),
            "message lists valid sources: {msg}"
        );
    }

    #[test]
    fn a_non_positive_price_is_rejected() {
        assert!(
            validate_nehs_import(&req(dec!(0), Some("manual")), date!(2026 - 07 - 08)).is_err()
        );
    }
}
