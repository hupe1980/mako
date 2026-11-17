//! The reads `einsd` serves through two doors: a documented REST `GET` and an
//! MCP tool, both calling the one function here and answering its one type.
//!
//! A read's REST route and its tool authorize with the same Cedar action
//! (`read-anlage` or `read-settlement`), its answer type derives both
//! `schemars::JsonSchema` (the tool's `output_schema`) and `utoipa::ToSchema`
//! (the route's response in `/api/v1/openapi.json`), and the tests hold the two
//! schemas to each other. The routes mount through `utoipa-axum`'s
//! `OpenApiRouter` (`crate::routes::api_router`), so none of them exists
//! without its document entry.
//!
//! | Read | `GET` | Cedar | Answer |
//! |---|---|---|---|
//! | `get_plant` | `/api/v1/anlagen/{tr_id}` | `read-anlage` | [`AnlageRow`] |
//! | `list_expiring` | `/api/v1/anlagen/foerderung-auslaufend` | `read-anlage` | [`AuslaufendeAnlagen`] |
//! | `list_plants_without_mastr` | `/api/v1/anlagen/ohne-mastr` | `read-anlage` | [`AnlagenOhneMastr`] |
//! | `list_settlements` | `/api/v1/anlagen/{tr_id}/settlements` | `read-settlement` | [`Abrechnungen`] |
//! | `explain_settlement` | `/api/v1/anlagen/{tr_id}/settlements/{year}/{month}/erklaerung` | `read-settlement` | [`AbrechnungsErklaerung`] |
//! | `list_unsettled_plants` | `/api/v1/anlagen/unabgerechnet/{year}/{month}` | `read-settlement` | [`UnabgerechneteAnlagen`] |

use std::sync::Arc;

use axum::Extension;
use axum::extract::{Path, Query};
use mako_service::Json;
use mako_service::cedar::CedarEnforcer;
use mako_service::error::{ApiError, ApiResult};
use mako_service::oidc::Claims;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa_axum::{router::OpenApiRouter, routes};

use crate::config::EinsdConfig;
use crate::pg::AnlageRow;

/// The Cedar action of the plant reads.
pub const READ_ANLAGE: &str = "read-anlage";
/// The Cedar action of the settlement reads.
pub const READ_SETTLEMENT: &str = "read-settlement";

/// The Cedar action a shared read's MCP tool authorizes with — its REST twin's.
#[must_use]
pub fn tool_action(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "get_plant" | "list_expiring" | "list_plants_without_mastr" => READ_ANLAGE,
        "list_settlements" | "explain_settlement" | "list_unsettled_plants" => READ_SETTLEMENT,
        _ => return None,
    })
}

// ── Answers ───────────────────────────────────────────────────────────────────
//
// Money, energy and rates cross as decimal strings (`format: decimal`), dates as
// `YYYY-MM-DD` (`format: date`), instants as RFC 3339 (`format: date-time`).

/// `list_expiring`'s answer: active plants whose Förderung ends within
/// `horizon_days`, the earliest Förderende first.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct AuslaufendeAnlagen {
    pub horizon_days: i32,
    pub count: usize,
    pub plants: Vec<crate::pg::AnlageRow>,
}

/// `list_settlements`' answer: a plant's receipts, the newest month first.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct Abrechnungen {
    pub tr_id: String,
    pub count: usize,
    /// At least `limit` receipts matched: older ones were not listed.
    pub saturated: bool,
    pub settlements: Vec<crate::pg::SettlementReceipt>,
}

/// `list_unsettled_plants`' answer: active plants with no receipt for the month.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct UnabgerechneteAnlagen {
    pub billing_year: i16,
    pub billing_month: i16,
    pub unsettled_count: usize,
    pub plants: Vec<crate::pg::AnlageRow>,
}

/// One active plant not registered in the MaStR.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct AnlageOhneMastr {
    pub tr_id: String,
    pub malo_id: String,
    pub erzeugungsart: String,
    #[schemars(with = "String", extend("format" = "decimal"))]
    #[schema(value_type = String, format = "decimal")]
    pub leistung_kwp: rust_decimal::Decimal,
    pub eeg_gesetz: i16,
    /// § 52 Abs. 2 EEG 2023: 10 €/kW per month; `None` for a plant under the
    /// pre-2023 regime, which owes no Pflichtzahlung.
    #[schemars(with = "Option<String>", extend("format" = "decimal"))]
    #[schema(value_type = Option<String>, format = "decimal")]
    pub monthly_penalty_eur: Option<rust_decimal::Decimal>,
    /// The rule the plant falls under, as text.
    pub regime: String,
}

/// `list_plants_without_mastr`' answer, the largest plant first.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct AnlagenOhneMastr {
    pub count: usize,
    /// The sum of `monthly_penalty_eur` over the plants that owe one.
    #[schemars(with = "String", extend("format" = "decimal"))]
    #[schema(value_type = String, format = "decimal")]
    pub total_monthly_penalty_eur: rust_decimal::Decimal,
    pub plants: Vec<AnlageOhneMastr>,
    pub regulatory_note: String,
}

/// One position of a stored settlement receipt.
#[derive(Debug, Serialize, Deserialize, JsonSchema, utoipa::ToSchema)]
pub struct Abrechnungsposition {
    pub description: String,
    /// The provision the position rests on.
    pub legal_basis: String,
    #[schemars(with = "String", extend("format" = "decimal"))]
    #[schema(value_type = String, format = "decimal")]
    pub kwh: rust_decimal::Decimal,
    #[schemars(with = "String", extend("format" = "decimal"))]
    #[schema(value_type = String, format = "decimal")]
    pub rate_ct_kwh: rust_decimal::Decimal,
    #[schemars(with = "String", extend("format" = "decimal"))]
    #[schema(value_type = String, format = "decimal")]
    pub eur: rust_decimal::Decimal,
}

/// `explain_settlement`'s answer: the newest receipt for the month and its
/// positions as stored.
#[derive(Debug, Serialize, JsonSchema, utoipa::ToSchema)]
pub struct AbrechnungsErklaerung {
    pub tr_id: String,
    pub billing_year: i16,
    pub billing_month: i16,
    pub settlement_model: String,
    /// The receipt's status code (see `list_settlements`).
    pub status: String,
    #[schemars(with = "Option<String>", extend("format" = "decimal"))]
    #[schema(value_type = Option<String>, format = "decimal")]
    pub einspeisemenge_kwh: Option<rust_decimal::Decimal>,
    /// The sum of the positions' `eur`.
    #[schemars(with = "Option<String>", extend("format" = "decimal"))]
    #[schema(value_type = Option<String>, format = "decimal")]
    pub settlement_eur: Option<rust_decimal::Decimal>,
    /// A separate § 52 EEG 2023 Pflichtzahlung owed to the Netzbetreiber.
    #[schemars(with = "Option<String>", extend("format" = "decimal"))]
    #[schema(value_type = Option<String>, format = "decimal")]
    pub pflichtzahlung_eur: Option<rust_decimal::Decimal>,
    pub verlaengerungsanspruch_qh: i64,
    #[schemars(with = "Option<String>", extend("format" = "decimal"))]
    #[schema(value_type = Option<String>, format = "decimal")]
    pub billing_days_fraction: Option<rust_decimal::Decimal>,
    #[schemars(with = "String", extend("format" = "date-time"))]
    #[schema(value_type = String, format = DateTime)]
    pub settled_at: String,
    pub positions: Vec<Abrechnungsposition>,
    pub interpretation: String,
}

// ── The reads ─────────────────────────────────────────────────────────────────

/// A refusal for a month outside 1–12.
fn month_in_range(month: i16) -> ApiResult<()> {
    if (1..=12).contains(&month) {
        Ok(())
    } else {
        Err(ApiError::bad_request("month must be 1–12").with_code("invalid_month"))
    }
}

/// One plant by its TechnischeRessource ID.
///
/// # Errors
///
/// `404 anlage_not_found` for a TR-ID this tenant keeps no plant under.
pub async fn get_plant(pool: &PgPool, tenant: &str, tr_id: &str) -> ApiResult<AnlageRow> {
    crate::pg::fetch_anlage(pool, tenant, tr_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::not_found(format!("EEG-Anlage {tr_id} not found"))
                .with_code("anlage_not_found")
        })
}

/// Active plants whose Förderung ends within `days` (default 180).
///
/// # Errors
///
/// `400` for a negative horizon.
pub async fn list_expiring(
    pool: &PgPool,
    tenant: &str,
    days: Option<i32>,
) -> ApiResult<AuslaufendeAnlagen> {
    let days = days.unwrap_or(180);
    if days < 0 {
        return Err(ApiError::bad_request("days must not be negative").with_code("invalid_days"));
    }
    let plants = crate::pg::list_expiring(pool, tenant, days)
        .await
        .map_err(ApiError::internal)?;
    Ok(AuslaufendeAnlagen {
        horizon_days: days,
        count: plants.len(),
        plants,
    })
}

/// A plant's settlement receipts, optionally one Abrechnungsjahr and/or
/// -monat, at most `limit` (default 24, at most 200).
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn list_settlements(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    q: &SettlementsQuery,
) -> ApiResult<Abrechnungen> {
    if let Some(month) = q.month {
        month_in_range(month)?;
    }
    let limit = q.limit.unwrap_or(24).clamp(1, 200);
    let settlements =
        crate::pg::list_settlement_receipts(pool, tenant, tr_id, q.year, q.month, limit)
            .await
            .map_err(ApiError::internal)?;
    Ok(Abrechnungen {
        count: settlements.len(),
        saturated: i64::try_from(settlements.len()).unwrap_or(i64::MAX) >= limit,
        tr_id: tr_id.to_owned(),
        settlements,
    })
}

/// Active plants with no receipt for the month.
///
/// # Errors
///
/// `400` for a month outside 1–12.
pub async fn list_unsettled_plants(
    pool: &PgPool,
    tenant: &str,
    billing_year: i16,
    billing_month: i16,
) -> ApiResult<UnabgerechneteAnlagen> {
    month_in_range(billing_month)?;
    let plants = crate::pg::list_unsettled(pool, tenant, billing_year, billing_month)
        .await
        .map_err(ApiError::internal)?;
    Ok(UnabgerechneteAnlagen {
        billing_year,
        billing_month,
        unsettled_count: plants.len(),
        plants,
    })
}

/// Active plants not registered in the MaStR, the largest first.
///
/// # Errors
///
/// Only a database failure.
pub async fn list_plants_without_mastr(pool: &PgPool, tenant: &str) -> ApiResult<AnlagenOhneMastr> {
    use sqlx::Row;
    let rows = sqlx::query(
        r"SELECT tr_id, malo_id, erzeugungsart, leistung_kwp, eeg_gesetz, foerderendedatum
          FROM eeg_anlagen
          WHERE tenant = $1
            AND mastr_registriert = false
            AND status = 'aktiv'
            AND (foerderendedatum IS NULL OR foerderendedatum >= heute())
          ORDER BY leistung_kwp DESC",
    )
    .bind(tenant)
    .fetch_all(pool)
    .await
    .map_err(ApiError::internal)?;

    let plants = rows
        .iter()
        .map(|r| {
            let leistung: rust_decimal::Decimal = r.try_get("leistung_kwp")?;
            let eeg_gesetz: i16 = r.try_get("eeg_gesetz")?;
            // §52 Abs. 2 charges 10 €/kW/month only under the EEG 2023
            // regime. For an older plant §100 keeps the pre-2023 rule, where
            // the breach reduces the Vergütung to zero and no Pflichtzahlung
            // exists — reporting 10 €/kW for those, and summing it into a
            // portfolio total, invents money nobody owes.
            let eeg_2023 = eeg_gesetz >= 2023;
            Ok(AnlageOhneMastr {
                tr_id: r.try_get("tr_id")?,
                malo_id: r.try_get("malo_id")?,
                erzeugungsart: r.try_get("erzeugungsart")?,
                leistung_kwp: leistung,
                eeg_gesetz,
                monthly_penalty_eur: eeg_2023.then(|| leistung * rust_decimal::Decimal::from(10)),
                regime: if eeg_2023 {
                    "§52 Abs. 1 Nr. 11 i.V.m. Abs. 2 EEG 2023: 10 €/kW/Monat"
                } else {
                    "EEG ≤2021 via §100: Vergütung auf null — keine Pflichtzahlung"
                }
                .to_owned(),
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()
        .map_err(ApiError::internal)?;

    // Only the plants that actually owe a Pflichtzahlung are in the total.
    let total_monthly_penalty_eur = plants.iter().filter_map(|p| p.monthly_penalty_eur).sum();

    Ok(AnlagenOhneMastr {
        count: plants.len(),
        total_monthly_penalty_eur,
        plants,
        regulatory_note: "Register all plants at https://www.marktstammdatenregister.de. POST /api/v1/anlagen/{tr_id}/mastr-registrierung after successful registration.".to_owned(),
    })
}

/// The newest receipt for a plant and month, with its positions as stored.
///
/// # Errors
///
/// `404 settlement_not_found` for a month with no receipt, `400` for a month
/// outside 1–12.
pub async fn explain_settlement(
    pool: &PgPool,
    tenant: &str,
    tr_id: &str,
    billing_year: i16,
    billing_month: i16,
) -> ApiResult<AbrechnungsErklaerung> {
    use sqlx::Row as _;
    month_in_range(billing_month)?;
    let row = sqlx::query(
        "SELECT id, billing_year, billing_month, settlement_model, \
          einspeisemenge_kwh, settlement_eur, status, positions_json, \
          pflichtzahlung_eur, verlaengerungsanspruch_qh, \
          billing_days_fraction, settled_at \
         FROM settlement_receipts \
         WHERE tr_id = $1 AND tenant = $2 \
           AND billing_year = $3 AND billing_month = $4 \
         ORDER BY settled_at DESC LIMIT 1",
    )
    .bind(tr_id)
    .bind(tenant)
    .bind(billing_year)
    .bind(billing_month)
    .fetch_optional(pool)
    .await
    .map_err(ApiError::internal)?;

    let Some(row) = row else {
        return Err(ApiError::not_found(format!(
            "No settlement for plant {tr_id} in {billing_year}/{billing_month:02}."
        ))
        .with_code("settlement_not_found"));
    };

    // `positions_json` is written by `pg::run_settlement` in exactly this
    // shape; a row that does not parse is a defect, not an empty receipt.
    let positions: Vec<Abrechnungsposition> = match row
        .try_get::<Option<serde_json::Value>, _>("positions_json")
        .map_err(ApiError::internal)?
    {
        Some(v) => serde_json::from_value(v).map_err(|e| {
            ApiError::internal(anyhow::anyhow!("positions_json does not parse: {e}"))
        })?,
        None => Vec::new(),
    };
    let settled_at = row
        .try_get::<time::OffsetDateTime, _>("settled_at")
        .map_err(ApiError::internal)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(ApiError::internal)?;

    Ok(AbrechnungsErklaerung {
        interpretation: format!(
            "Settlement for {billing_year}/{billing_month:02}: {} positions listed above. \
             Each position shows the legal paragraph, kWh quantity, rate, and EUR amount. \
             The 'settlement_eur' is the sum of all position EUR values. \
             A pflichtzahlung_eur > 0 means a separate §52 EEG penalty is owed to the NB.",
            positions.len()
        ),
        tr_id: tr_id.to_owned(),
        billing_year,
        billing_month,
        settlement_model: row
            .try_get("settlement_model")
            .map_err(ApiError::internal)?,
        status: row.try_get("status").map_err(ApiError::internal)?,
        einspeisemenge_kwh: row
            .try_get("einspeisemenge_kwh")
            .map_err(ApiError::internal)?,
        settlement_eur: row.try_get("settlement_eur").map_err(ApiError::internal)?,
        pflichtzahlung_eur: row
            .try_get("pflichtzahlung_eur")
            .map_err(ApiError::internal)?,
        verlaengerungsanspruch_qh: row
            .try_get("verlaengerungsanspruch_qh")
            .map_err(ApiError::internal)?,
        billing_days_fraction: row
            .try_get("billing_days_fraction")
            .map_err(ApiError::internal)?,
        settled_at,
        positions,
    })
}

// ── REST ──────────────────────────────────────────────────────────────────────

/// `GET /api/v1/anlagen/foerderung-auslaufend`'s query.
#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct HorizonQuery {
    /// Look-ahead window in days (default 180).
    pub days: Option<i32>,
}

/// `GET /api/v1/anlagen/{tr_id}/settlements`' query.
///
/// `deny_unknown_fields` on purpose: a misspelt filter (`jahr`) must not
/// silently return the whole history with a 200.
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
pub struct SettlementsQuery {
    /// Abrechnungsjahr, e.g. `2026`.
    pub year: Option<i16>,
    /// Abrechnungsmonat, 1–12. A month without a year narrows every year to
    /// that month, which is what a year-on-year comparison asks for.
    pub month: Option<i16>,
    /// At most this many receipts (default 24, at most 200).
    pub limit: Option<i64>,
}

/// Refuse a caller the Cedar action does not admit.
fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &str,
    tenant: &str,
) -> ApiResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))
}

#[utoipa::path(
    get,
    operation_id = "get_plant",
    path = "/api/v1/anlagen/{tr_id}",
    tag = "anlagen",
    params(("tr_id" = String, Path, description = "TechnischeRessource ID of the plant")),
    responses(
        (status = 200, description = "The plant", body = AnlageRow),
        (status = 403, description = "Cedar `read-anlage` refused", body = ApiError),
        (status = 404, description = "`anlage_not_found`", body = ApiError),
    )
)]
async fn rest_get_plant(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
) -> ApiResult<Json<AnlageRow>> {
    authorize(&enforcer, &claims, READ_ANLAGE, &cfg.tenant)?;
    get_plant(&pool, &cfg.tenant, &tr_id).await.map(Json)
}

#[utoipa::path(
    get,
    operation_id = "list_expiring",
    path = "/api/v1/anlagen/foerderung-auslaufend",
    tag = "anlagen",
    params(HorizonQuery),
    responses(
        (status = 200, description = "Active plants whose Förderung ends within the horizon", body = AuslaufendeAnlagen),
        (status = 400, description = "A negative horizon", body = ApiError),
        (status = 403, description = "Cedar `read-anlage` refused", body = ApiError),
    )
)]
async fn rest_list_expiring(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Query(q): Query<HorizonQuery>,
) -> ApiResult<Json<AuslaufendeAnlagen>> {
    authorize(&enforcer, &claims, READ_ANLAGE, &cfg.tenant)?;
    list_expiring(&pool, &cfg.tenant, q.days).await.map(Json)
}

#[utoipa::path(
    get,
    operation_id = "list_plants_without_mastr",
    path = "/api/v1/anlagen/ohne-mastr",
    tag = "anlagen",
    responses(
        (status = 200, description = "Active plants not registered in the MaStR", body = AnlagenOhneMastr),
        (status = 403, description = "Cedar `read-anlage` refused", body = ApiError),
    )
)]
async fn rest_list_plants_without_mastr(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
) -> ApiResult<Json<AnlagenOhneMastr>> {
    authorize(&enforcer, &claims, READ_ANLAGE, &cfg.tenant)?;
    list_plants_without_mastr(&pool, &cfg.tenant)
        .await
        .map(Json)
}

#[utoipa::path(
    get,
    operation_id = "list_settlements",
    path = "/api/v1/anlagen/{tr_id}/settlements",
    tag = "settlements",
    params(("tr_id" = String, Path, description = "TechnischeRessource ID of the plant"), SettlementsQuery),
    responses(
        (status = 200, description = "The plant's receipts, the newest month first", body = Abrechnungen),
        (status = 400, description = "A month outside 1–12, or an unknown parameter", body = ApiError),
        (status = 403, description = "Cedar `read-settlement` refused", body = ApiError),
    )
)]
async fn rest_list_settlements(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path(tr_id): Path<String>,
    Query(q): Query<SettlementsQuery>,
) -> ApiResult<Json<Abrechnungen>> {
    authorize(&enforcer, &claims, READ_SETTLEMENT, &cfg.tenant)?;
    list_settlements(&pool, &cfg.tenant, &tr_id, &q)
        .await
        .map(Json)
}

#[utoipa::path(
    get,
    operation_id = "explain_settlement",
    path = "/api/v1/anlagen/{tr_id}/settlements/{year}/{month}/erklaerung",
    tag = "settlements",
    params(
        ("tr_id" = String, Path, description = "TechnischeRessource ID of the plant"),
        ("year" = i16, Path, description = "Abrechnungsjahr"),
        ("month" = i16, Path, description = "Abrechnungsmonat, 1–12"),
    ),
    responses(
        (status = 200, description = "The newest receipt for the month with its positions", body = AbrechnungsErklaerung),
        (status = 400, description = "A month outside 1–12", body = ApiError),
        (status = 403, description = "Cedar `read-settlement` refused", body = ApiError),
        (status = 404, description = "`settlement_not_found`", body = ApiError),
    )
)]
async fn rest_explain_settlement(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path((tr_id, year, month)): Path<(String, i16, i16)>,
) -> ApiResult<Json<AbrechnungsErklaerung>> {
    authorize(&enforcer, &claims, READ_SETTLEMENT, &cfg.tenant)?;
    explain_settlement(&pool, &cfg.tenant, &tr_id, year, month)
        .await
        .map(Json)
}

#[utoipa::path(
    get,
    operation_id = "list_unsettled_plants",
    path = "/api/v1/anlagen/unabgerechnet/{year}/{month}",
    tag = "settlements",
    params(
        ("year" = i16, Path, description = "Abrechnungsjahr"),
        ("month" = i16, Path, description = "Abrechnungsmonat, 1–12"),
    ),
    responses(
        (status = 200, description = "Active plants with no receipt for the month", body = UnabgerechneteAnlagen),
        (status = 400, description = "A month outside 1–12", body = ApiError),
        (status = 403, description = "Cedar `read-settlement` refused", body = ApiError),
    )
)]
async fn rest_list_unsettled_plants(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<Arc<EinsdConfig>>,
    Path((year, month)): Path<(i16, i16)>,
) -> ApiResult<Json<UnabgerechneteAnlagen>> {
    authorize(&enforcer, &claims, READ_SETTLEMENT, &cfg.tenant)?;
    list_unsettled_plants(&pool, &cfg.tenant, year, month)
        .await
        .map(Json)
}

/// The documented shared reads, each mounted with its document entry; the
/// document's frame and the remaining routes are `crate::routes::api_router`'s.
#[must_use]
pub fn api_router() -> OpenApiRouter {
    OpenApiRouter::new()
        .routes(routes!(rest_list_expiring))
        .routes(routes!(rest_list_plants_without_mastr))
        .routes(routes!(rest_get_plant))
        .routes(routes!(rest_list_settlements))
        .routes(routes!(rest_explain_settlement))
        .routes(routes!(rest_list_unsettled_plants))
}
