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
    product_code: &str,
    req: ProductUpsertRequest,
) -> anyhow::Result<Uuid> {
    let valid_from = parse_date_opt(&req.valid_from).context("parse valid_from")?;
    let valid_to = parse_date_opt(&req.valid_to).context("parse valid_to")?;

    // Archive previous version before upsert.
    let _ = sqlx::query(
        r"INSERT INTO product_history (lf_mp_id, product_code, data, bo4e_version)
          SELECT lf_mp_id, product_code, data, bo4e_version
          FROM products
          WHERE lf_mp_id = $1 AND product_code = $2 AND (valid_from = $3 OR $3 IS NULL)
          ORDER BY updated_at DESC
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(valid_from)
    .execute(pool)
    .await;

    let row = sqlx::query(
        r"INSERT INTO products
              (lf_mp_id, product_code, category, name, sparte, register_count, kundentyp,
               dyn_source, valid_from, valid_to, data, bo4e_version, energiemix, oekolabel, updated_at)
          VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, now())
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
    .bind(&req.energiemix)
    .bind(&req.oekolabel)
    .fetch_one(pool)
    .await
    .context("upsert product")?;

    Ok(row.try_get("id")?)
}

pub async fn fetch_product(
    pool: &PgPool,
    lf_mp_id: &str,
    product_code: &str,
) -> anyhow::Result<Option<ProductRow>> {
    sqlx::query_as::<_, ProductRow>(
        "SELECT * FROM products WHERE lf_mp_id = $1 AND product_code = $2
         ORDER BY valid_from DESC NULLS LAST LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .fetch_optional(pool)
    .await
    .context("fetch product")
}

pub async fn fetch_product_history(
    pool: &PgPool,
    lf_mp_id: &str,
    product_code: &str,
) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows = sqlx::query(
        "SELECT id, lf_mp_id, product_code, data, bo4e_version, changed_at
         FROM product_history WHERE lf_mp_id = $1 AND product_code = $2
         ORDER BY changed_at DESC LIMIT 100",
    )
    .bind(lf_mp_id)
    .bind(product_code)
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
    product_code: &str,
    req: EnergimixUpsertRequest,
) -> anyhow::Result<()> {
    let updated = sqlx::query(
        r"UPDATE products
          SET energiemix = $3,
              oekolabel  = $4,
              updated_at = now()
          WHERE lf_mp_id = $1 AND product_code = $2",
    )
    .bind(lf_mp_id)
    .bind(product_code)
    .bind(&req.energiemix)
    .bind(&req.oekolabel)
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
    product_code: &str,
) -> anyhow::Result<Option<EnergiemixResponse>> {
    let row = sqlx::query(
        r"SELECT lf_mp_id, product_code, energiemix, oekolabel, updated_at
          FROM products
          WHERE lf_mp_id = $1 AND product_code = $2
          ORDER BY valid_from DESC NULLS LAST
          LIMIT 1",
    )
    .bind(lf_mp_id)
    .bind(product_code)
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
    product_code: &str,
) -> anyhow::Result<bool> {
    let res = sqlx::query(
        "UPDATE products SET energiemix = NULL, oekolabel = NULL, updated_at = now()
         WHERE lf_mp_id = $1 AND product_code = $2",
    )
    .bind(lf_mp_id)
    .bind(product_code)
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
    pub limit: Option<i64>,
}

pub async fn list_products(
    pool: &PgPool,
    lf_mp_id: &str,
    q: &ProductListQuery,
) -> anyhow::Result<Vec<ProductRow>> {
    sqlx::query_as::<_, ProductRow>(
        r"SELECT DISTINCT ON (product_code) *
          FROM products
          WHERE lf_mp_id = $1
            AND ($2::text IS NULL OR category = $2)
            AND ($3::text IS NULL OR sparte = $3)
            AND ($4::text IS NULL OR kundentyp = $4)
          ORDER BY product_code, valid_from DESC NULLS LAST
          LIMIT $5",
    )
    .bind(lf_mp_id)
    .bind(&q.category)
    .bind(&q.sparte)
    .bind(&q.kundentyp)
    .bind(q.limit.unwrap_or(100).min(1000))
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
        let product = fetch_product(pool, lf_mp_id, &product_code).await?;
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
    req: AssignProductRequest,
) -> anyhow::Result<()> {
    use time::format_description::well_known::Iso8601;
    let assigned_from =
        Date::parse(&req.assigned_from, &Iso8601::DEFAULT).context("parse assigned_from")?;

    // Close previous assignment if active.
    sqlx::query(
        r"UPDATE customer_products SET assigned_to = $3, updated_at = now()
          WHERE malo_id = $1 AND lf_mp_id = $2 AND assigned_to IS NULL",
    )
    .bind(malo_id)
    .bind(lf_mp_id)
    .bind(assigned_from)
    .execute(pool)
    .await
    .context("close previous assignment")?;

    // Insert new assignment.
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
    .execute(pool)
    .await
    .context("assign product")?;

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
    /// Konzessionsabgabe in ct/kWh (§17 StromNZV / §7 GasNZV).
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

/// Insert a new Angebot.
#[allow(clippy::too_many_arguments)]
pub async fn insert_angebot(
    pool: &PgPool,
    tenant: &str,
    lf_mp_id: &str,
    angebotsnummer: &str,
    req: &CreateAngebotRequest,
    positionen_json: &serde_json::Value,
    varianten_json: &serde_json::Value,
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
               positionen, varianten,
               jahreskosten_netto_eur, jahreskosten_brutto_eur,
               erp_angebot_id, notizen)
          VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)
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
