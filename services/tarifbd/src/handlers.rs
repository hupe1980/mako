//! HTTP handlers for `tarifbd`.

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use rubo4e::current::{Energiemix, Tarifpreisblatt};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;

use crate::{
    config::TarifbdConfig,
    pg::{
        AssignProductRequest, CreateAngebotRequest, EnergimixUpsertRequest, EpexImportRequest,
        ProductListQuery, ProductUpsertRequest, accept_angebot, assign_product, decline_angebot,
        delete_energiemix, expire_stale_angebote, fetch_angebot, fetch_energiemix, fetch_epex_day,
        fetch_product, fetch_product_history, get_customer_product, insert_angebot,
        link_angebot_rahmenvertrag, list_angebote, list_products, mark_angebot_versandt,
        monthly_epex_average, next_angebotsnummer, upsert_energiemix, upsert_epex_day,
        upsert_product,
    },
};

// ── BO4E Tarifpreisblatt validation ──────────────────────────────────────────

/// Product categories that store a BO4E `Tarifpreisblatt` payload.
///
/// For these categories `_typ: "TARIFPREISBLATT"` is required (injected if
/// absent) and the full BO4E envelope is validated via
/// `rubo4e::current::Tarifpreisblatt`.  All other categories
/// (`HEMS`, `EMOBILITY`, `ENERGIEDIENSTLEISTUNG`, `BUNDLE`) use a free-form
/// structure — only `tarifpreispositionen` is validated if present.
const TARIFPREISBLATT_CATEGORIES: &[&str] = &[
    "STROM",
    "GAS",
    "WAERME",
    "SOLAR",
    "EEG",
    "EINSPEISUNG",
    "WAERMEPUMPE",
    "WALLBOX",
];

/// Whitelist of valid `preistyp` values for mako products.
///
/// Canonical ALLCAPS naming — values are normalised to ALLCAPS before the
/// check, so `"grundpreis"` is accepted and stored as `"GRUNDPREIS"`.
///
/// **Hard-cut:** any value not in this list is rejected with 422.
const VALID_PREISTYPEN: &[&str] = &[
    // ── Standard BO4E Preistyp (rubo4e v202607) ─────────────────────────────
    "GRUNDPREIS",
    "ARBEITSPREIS_EINTARIF",
    "ARBEITSPREIS_HT",
    "ARBEITSPREIS_NT",
    "LEISTUNGSPREIS",
    "MESSPREIS",
    "ENTGELT_ABLESUNG",
    "ENTGELT_ABRECHNUNG",
    "ENTGELT_MSB",
    "PROVISION",
    // ── mako extensions: EEG / KWKG / Direktvermarktung / §14a ──────────────
    "SOLAR_ARBEITSPREIS",
    "EEG_VERGUETUNG",
    "EEG_MARKTPRAEMIE",
    "EEG_MANAGEMENTPRAEMIE",
    "KWKG_ZUSCHLAG",
    "MARKTWERT",
    "VERMARKTUNGSGEBUEHR",
    "MIETERSTROM_AUFSCHLAG",
    "GEMEINSCHAFT_RABATT",
    "STEUERUNGSRABATT_MODUL1",
    "STEUERUNGSRABATT_MODUL3",
    // ── mako extensions: HEMS ────────────────────────────────────────────────
    "HEMS_PLATTFORMGEBUEHR",
    "HEMS_OPTIMIERUNGSEVENT",
    "HEMS_AUSLESUNG",
    // ── mako extensions: E-mobility ──────────────────────────────────────────
    "EMOBILITY_SERVICEGEBUEHR",
    "EMOBILITY_ARBEITSPREIS",
    "EMOBILITY_SESSION",
    "EMOBILITY_ROAMING",
    // ── mako extensions: generic services ────────────────────────────────────
    "SERVICE_GEBUEHR",
    "SERVICE_EVENT",
];

/// Validate and canonicalise a product `data` JSONB payload.
///
/// ## Category dispatch
///
/// | Category | `_typ` injection | BO4E envelope validation |
/// |---|---|---|
/// | `STROM`, `GAS`, `WAERME`, `SOLAR`, `EEG`, `EINSPEISUNG`, `WAERMEPUMPE`, `WALLBOX` | ✓ `"TARIFPREISBLATT"` | ✓ via `rubo4e::current::Tarifpreisblatt` |
/// | `HEMS`, `EMOBILITY`, `ENERGIEDIENSTLEISTUNG`, `BUNDLE` | ✗ | ✗ |
///
/// ## Position validation (all categories)
///
/// Applied to every element of `tarifpreispositionen` when the field is present:
///
/// - `preistyp` is normalised to ALLCAPS and validated against [`VALID_PREISTYPEN`].
/// - `preisstaffeln[*].preis` must be a **scalar** JSON string or number parseable
///   as `Decimal`.  The nested `{"wert": "..."}` form (non-BO4E) is rejected.
///
/// ## Canonicalisation
///
/// For BO4E categories the full envelope is re-serialised from the typed struct,
/// yielding canonical camelCase field names.  The normalised
/// `tarifpreispositionen` (with ALLCAPS `preistyp`) are merged back so that
/// mako-extended preistyp values survive the round-trip without being mapped to
/// `"UNKNOWN"` by `Preistyp`'s catch-all serde variant.
fn normalize_tarifpreisblatt(
    category: &str,
    mut data: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, serde_json::Value)> {
    let is_bo4e_category = TARIFPREISBLATT_CATEGORIES.contains(&category);

    // ── 1. _typ: inject for BO4E categories, reject mismatches ───────────────
    if is_bo4e_category {
        match data.get("_typ").and_then(|v| v.as_str()) {
            None => {
                data["_typ"] = serde_json::json!("TARIFPREISBLATT");
            }
            Some("TARIFPREISBLATT") => {}
            Some(other) => {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    serde_json::json!({
                        "error": format!(
                            "expected _typ=TARIFPREISBLATT for category {category}, got {other:?}"
                        )
                    }),
                ));
            }
        }
    }

    // ── 2. Normalise tarifpreispositionen ─────────────────────────────────────
    //    - ALLCAPS preistyp normalisation + whitelist check
    //    - scalar Decimal validation for preisstaffeln[*].preis
    if let Some(positionen) = data
        .get_mut("tarifpreispositionen")
        .and_then(|v| v.as_array_mut())
    {
        for (i, pos) in positionen.iter_mut().enumerate() {
            if let Some(pt) = pos.get("preistyp").and_then(|v| v.as_str()) {
                let upper = pt.to_uppercase();
                if !VALID_PREISTYPEN.contains(&upper.as_str()) {
                    return Err((
                        StatusCode::UNPROCESSABLE_ENTITY,
                        serde_json::json!({
                            "error": format!(
                                "tarifpreispositionen[{i}].preistyp {pt:?} is not valid; \
                                 accepted values: {}",
                                VALID_PREISTYPEN.join(", ")
                            )
                        }),
                    ));
                }
                if let Some(obj) = pos.as_object_mut() {
                    obj.insert("preistyp".to_owned(), serde_json::json!(upper));
                }
            }

            if let Some(staffeln) = pos.get("preisstaffeln").and_then(|v| v.as_array()) {
                for (j, staffel) in staffeln.iter().enumerate() {
                    if let Some(preis) = staffel.get("preis") {
                        let is_scalar_decimal = match preis {
                            serde_json::Value::String(s) => s.parse::<Decimal>().is_ok(),
                            serde_json::Value::Number(_) => true,
                            _ => false,
                        };
                        if !is_scalar_decimal {
                            return Err((
                                StatusCode::UNPROCESSABLE_ENTITY,
                                serde_json::json!({
                                    "error": format!(
                                        "tarifpreispositionen[{i}].preisstaffeln[{j}].preis \
                                         must be a scalar decimal (string or number), \
                                         not a nested object"
                                    )
                                }),
                            ));
                        }
                    }
                }
            }
        }
    }

    // ── 3. BO4E envelope roundtrip (BO4E categories only) ────────────────────
    //    Validates sparte, tariftyp, kundentypen, registeranzahl,
    //    berechnungsparameter, preisgarantie, vertragskonditionen, tarifmerkmale.
    //    Extended preistyp values (e.g. "EEG_VERGUETUNG") map to
    //    Preistyp::Unknown — already validated in step 2, so safe.
    //    Restore the normalised positionen after re-serialisation so that
    //    the stored JSONB has ALLCAPS preistyp, not "UNKNOWN".
    if is_bo4e_category {
        let typed: Tarifpreisblatt = serde_json::from_value(data.clone()).map_err(|e| {
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                serde_json::json!({
                    "error": format!("invalid Tarifpreisblatt: {e}")
                }),
            )
        })?;
        let mut canonical = serde_json::to_value(&typed).unwrap_or_default();
        if let Some(positionen) = data.get("tarifpreispositionen") {
            canonical["tarifpreispositionen"] = positionen.clone();
        }
        return Ok(canonical);
    }

    Ok(data)
}

// ── BO4E Energiemix validation ────────────────────────────────────────────────

/// Validate an `Energiemix` COM payload.
///
/// `Energiemix` is a COM (not a BO) — it has no `_typ` discriminator.
/// Validation: deserialise as `rubo4e::current::Energiemix` to enforce
/// enum fields (`erzeugungsart`, `sparte`, …).  Re-serialise to canonical
/// BO4E camelCase form.
fn normalize_energiemix(
    data: serde_json::Value,
) -> Result<(Energiemix, serde_json::Value), (StatusCode, serde_json::Value)> {
    let mix: Energiemix = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Energiemix payload: {e}") }),
        )
    })?;
    let canonical = serde_json::to_value(&mix).unwrap_or_default();
    Ok((mix, canonical))
}

// ── Product CRUD ──────────────────────────────────────────────────────────────

/// `PUT /api/v1/products/{lf_mp_id}/{product_code}`
pub async fn put_product(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
    Json(mut req): Json<ProductUpsertRequest>,
) -> impl IntoResponse {
    // Validate + canonicalise product data against BO4E Tarifpreisblatt schema.
    req.data = match normalize_tarifpreisblatt(&req.category, req.data) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };
    match upsert_product(&pool, &lf_mp_id, &product_code, req).await {
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}/{product_code}`
pub async fn get_product(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    match fetch_product(&pool, &lf_mp_id, &product_code).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}`
pub async fn list_products_handler(
    Extension(pool): Extension<PgPool>,
    Path(lf_mp_id): Path<String>,
    Query(q): Query<ProductListQuery>,
) -> impl IntoResponse {
    match list_products(&pool, &lf_mp_id, &q).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}/{product_code}/history`
pub async fn get_product_history(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    match fetch_product_history(&pool, &lf_mp_id, &product_code).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Customer assignment ───────────────────────────────────────────────────────

/// `GET /api/v1/customer/{malo_id}/product`
///
/// Returns the currently active product for a MaLo, including the full
/// `data` (Tarifpreisblatt JSONB).  Used by `billingd` to look up pricing.
pub async fn get_customer_product_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(malo_id): Path<String>,
    Query(q): Query<CustomerProductQuery>,
) -> impl IntoResponse {
    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);
    match get_customer_product(&pool, &malo_id, lf_mp_id).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/customer/{malo_id}/product`  — Tarifwechsel
pub async fn put_customer_product_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(malo_id): Path<String>,
    Json(req): Json<AssignProductRequest>,
) -> impl IntoResponse {
    match assign_product(&pool, &malo_id, &cfg.tenant, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct CustomerProductQuery {
    pub lf_mp_id: Option<String>,
}

// ── EPEX Spot day-ahead prices ────────────────────────────────────────────────

/// `PUT /api/v1/epex-prices/{date}`
///
/// Import all 24 hourly EPEX day-ahead prices for a date.
/// Body: `{ "prices": [ct_kwh_h0, ct_kwh_h1, ..., ct_kwh_h23], "source": "..." }`
pub async fn put_epex_prices(
    Extension(pool): Extension<PgPool>,
    Path(date_str): Path<String>,
    Json(req): Json<EpexImportRequest>,
) -> impl IntoResponse {
    use time::format_description::well_known::Iso8601;
    let date = match time::Date::parse(&date_str, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid date, expected YYYY-MM-DD").into_response();
        }
    };
    match upsert_epex_day(&pool, date, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/epex-prices/{date}/hourly`
///
/// Returns the 24-hour array of ct/kWh values for `date`.
/// Used by `billingd` for §41a dynamic tariff billing.
pub async fn get_epex_prices_hourly(
    Extension(pool): Extension<PgPool>,
    Path(date_str): Path<String>,
) -> impl IntoResponse {
    use time::format_description::well_known::Iso8601;
    let date = match time::Date::parse(&date_str, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid date, expected YYYY-MM-DD").into_response();
        }
    };
    match fetch_epex_day(&pool, date).await {
        Ok(Some(prices)) => Json(serde_json::json!({
            "price_date": date_str,
            "prices": prices,
            "unit": "ct_per_kwh",
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/epex-prices/{year}/{month}/average`
///
/// Returns the monthly average ct/kWh for EPEX Spot.
/// Used by `einsd` for Direktvermarktung Marktprämie calculation.
pub async fn get_epex_monthly_average(
    Extension(pool): Extension<PgPool>,
    Path((year, month)): Path<(i32, u8)>,
) -> impl IntoResponse {
    match monthly_epex_average(&pool, year, month).await {
        Ok(Some(avg)) => Json(serde_json::json!({
            "year": year,
            "month": month,
            "avg_ct_kwh": avg,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Energiemix sub-resource ───────────────────────────────────────────────────

/// `PUT /api/v1/products/{lf_mp_id}/{product_code}/energiemix`
///
/// Store or replace the §42 EnWG `Energiemix` and `Oekolabel` for a product.
///
/// Body: `{ "energiemix": <Energiemix COM JSON>, "oekolabel": ["OK_POWER", …] }`
///
/// Validation:
/// - `energiemix` is deserialized as `rubo4e::current::Energiemix`; invalid
///   enum values (e.g. unknown `erzeugungsart`) are rejected with 422.
/// - Re-serialized to canonical BO4E camelCase before storage.
/// - Does NOT re-archive or change product pricing — only touches the
///   `energiemix` / `oekolabel` columns.
pub async fn put_energiemix(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
    Json(mut req): Json<EnergimixUpsertRequest>,
) -> impl IntoResponse {
    // Validate and canonicalise the Energiemix COM payload.
    let (_typed_mix, canonical) = match normalize_energiemix(req.energiemix) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };
    req.energiemix = canonical;

    match upsert_energiemix(&pool, &lf_mp_id, &product_code, req).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND,
             Json(serde_json::json!({ "error": format!("product {lf_mp_id}/{product_code} not found") })))
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}/{product_code}/energiemix`
///
/// Retrieve the §42 EnWG `Energiemix` and `Oekolabel` for a product.
///
/// Returns:
/// ```json
/// {
///   "lf_mp_id": "...",
///   "product_code": "...",
///   "energiemix": { "anteil": [...], "co2Emission": 42.0, ... },
///   "oekolabel": ["OK_POWER"],
///   "updated_at": "2026-07-12T00:00:00Z"
/// }
/// ```
///
/// Returns 404 if the product has no Energiemix set.
pub async fn get_energiemix(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    match fetch_energiemix(&pool, &lf_mp_id, &product_code).await {
        Ok(Some(row)) if !row.energiemix.is_null() => Json(row).into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no Energiemix set for this product" })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/products/{lf_mp_id}/{product_code}/energiemix`
///
/// Remove the `Energiemix` and `Oekolabel` from a product (hard cut).
/// Use when a product transitions from green-certified back to standard.
pub async fn delete_energiemix_handler(
    Extension(pool): Extension<PgPool>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    match delete_energiemix(&pool, &lf_mp_id, &product_code).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Angebot (B2B Quotation, L4) ───────────────────────────────────────────────

/// Compute estimated all-in annual cost from product + NNE + statutory levies.
///
/// ## Cost components (per §§1-3 StromStG, §17 StromNZV, BNetzA Netzentgelt)
///
/// | Component | Typical share (SLP Gewerbe) | Source |
/// |---|---|---|
/// | Supply (Grundpreis + Arbeitspreis) | 30–40 % | product `Tarifpreisblatt` |
/// | NNE (Netzentgelt Arbeitspreis) | 30–40 % | `marktd.PreisblattNetznutzung` |
/// | NNE (Netzentgelt Grundpreis) | 5–10 % | `marktd.PreisblattNetznutzung` |
/// | Konzessionsabgabe | 2–3 % | `marktd.PreisblattKonzessionsabgabe` |
/// | Stromsteuer / Energiesteuer | 5–10 % | statutory (§3 StromStG, §2 EnergieStG) |
/// | BEHG (Gas only) | ~2 % | statutory (2025: 1.109 ct/kWh_Hs) |
/// | MwSt 19 % | 19 % | statutory |
///
/// NNE + statutory components are taken from the position-level overrides when
/// provided.  Statutory defaults apply when the override is `None`:
/// - Stromsteuer: 2.05 ct/kWh (§3 StromStG)
/// - Energiesteuer Gas: 0.55 ct/kWh (§2 EnergieStG)
/// - BEHG Gas: 1.109 ct/kWh (55 EUR/t CO₂ × 0.20160 kg/kWh, 2025)
///
/// NNE is NOT auto-fetched from `marktd` — the caller must supply it via
/// `nne_arbeitspreis_ct_per_kwh` + `nne_grundpreis_eur_per_year` in the
/// `AngebotPositionInput`.  This keeps `tarifbd` stateless with respect to
/// `marktd`.  For automated quoting workflows, pre-fetch the NNE from
/// `marktd GET /api/v1/preisblaetter/{nb_mp_id}` and pass it in.
fn estimate_jahreskosten(
    product_data: &serde_json::Value,
    pos: &crate::pg::AngebotPositionInput,
    rabatt_pct: Option<Decimal>,
) -> Option<Decimal> {
    use rust_decimal::prelude::ToPrimitive as _;
    use rust_decimal_macros::dec;

    let positionen = product_data
        .get("tarifpreispositionen")
        .and_then(|v| v.as_array())?;

    // ── 1. Supply cost from Tarifpreisblatt ──────────────────────────────────
    let mut grundpreis_ct: Option<Decimal> = None;
    let mut arbeitspreis_ct: Option<Decimal> = None;
    let mut leistungspreis_ct: Option<Decimal> = None;

    for pp in positionen {
        let pt = pp.get("preistyp").and_then(|v| v.as_str()).unwrap_or("");
        let preis = pp
            .get("preisstaffeln")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.get("preis"))
            .and_then(|v| match v {
                serde_json::Value::String(s) => s.parse::<Decimal>().ok(),
                serde_json::Value::Number(n) => n.to_string().parse::<Decimal>().ok(),
                _ => None,
            });
        match pt {
            "GRUNDPREIS" => grundpreis_ct = preis,
            "ARBEITSPREIS_EINTARIF" | "ARBEITSPREIS_HT" | "SOLAR_ARBEITSPREIS" => {
                arbeitspreis_ct = preis;
            }
            "LEISTUNGSPREIS" => leistungspreis_ct = preis,
            _ => {}
        }
    }

    let rabatt = rabatt_pct
        .map(|r| Decimal::ONE - r / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ONE);

    let supply_gp_eur = grundpreis_ct
        .map(|gp| gp * Decimal::from(365) / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);

    let supply_ap_eur = arbeitspreis_ct
        .map(|ap| ap * rabatt * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);

    let supply_lp_eur = leistungspreis_ct
        .zip(pos.leistung_kw)
        .map(|(lp, kw)| lp * kw * Decimal::from(12) / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);

    let supply_netto = supply_gp_eur + supply_ap_eur + supply_lp_eur;

    // ── 2. NNE pass-through (DSO-specific, caller must supply) ───────────────
    let nne_gp_eur = pos.nne_grundpreis_eur_per_year.unwrap_or(Decimal::ZERO);
    let nne_ap_eur = pos
        .nne_arbeitspreis_ct_per_kwh
        .map(|ct| ct * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);
    let nne_lp_eur = pos
        .nne_leistungspreis_eur_per_kw_year
        .zip(pos.leistung_kw)
        .map(|(lp, kw)| lp * kw)
        .unwrap_or(Decimal::ZERO);
    let nne_netto = nne_gp_eur + nne_ap_eur + nne_lp_eur;

    // ── 3. Konzessionsabgabe (§17 StromNZV / §7 GasNZV) ─────────────────────
    let ka_eur = pos
        .ka_ct_per_kwh
        .map(|ct| ct * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);

    // ── 4. Statutory levies ───────────────────────────────────────────────────
    let sparte_upper = pos.sparte.to_uppercase();
    let is_gas = sparte_upper.contains("GAS");

    let levy_eur = if is_gas {
        // Energiesteuer Gas + BEHG CO₂ levy (§2 EnergieStG, BEHG)
        let energiesteuer = pos.energiesteuer_gas_ct_per_kwh.unwrap_or(dec!(0.55));
        let behg = pos.behg_gas_ct_per_kwh.unwrap_or(dec!(1.109));
        (energiesteuer + behg) * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED
    } else {
        // Stromsteuer (§3 StromStG; 0 for §9a/§9b relief customers)
        let stromsteuer = pos.stromsteuer_ct_per_kwh.unwrap_or(dec!(2.05));
        stromsteuer * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED
    };

    let total_netto = supply_netto + nne_netto + ka_eur + levy_eur;

    if total_netto.to_f64().is_some_and(|f| f > 0.0) {
        Some(total_netto)
    } else {
        None
    }
}

/// Default Angebot validity: today + 10 Werktage (≈ 14 calendar days).
fn default_gueltig_bis() -> time::Date {
    time::OffsetDateTime::now_utc().date() + time::Duration::days(14)
}

/// `POST /api/v1/angebote`
///
/// Create a formal B2B Angebot (quotation) for a C&I or RLM customer.
///
/// ## Price calculation
///
/// For each position, `tarifbd` fetches the product's `Tarifpreisblatt` and
/// estimates `jahreskosten_netto_eur` from:
/// - `GRUNDPREIS` position: `ct/day × 365 / 100`
/// - `ARBEITSPREIS_EINTARIF` position: `ct/kWh × jahresverbrauch_kwh / 100`
/// - Optional `rabatt_pct` from the Angebot variant
///
/// MwSt (19 %) is applied to derive `jahreskosten_brutto_eur`.
///
/// ## Varianten (scenarios)
///
/// Multiple `varianten` (e.g., 12M vs 24M, with/without rebate) can be included
/// in a single Angebot.  On acceptance, the customer picks one via
/// `gewaehlte_variante` (index into the `varianten` array).
///
/// ## Acceptance lifecycle
///
/// `POST /api/v1/angebote/{id}/annehmen` transitions to ANGENOMMEN and emits
/// `de.angebot.angenommen` → ERP webhook.  The ERP or `vertragd` creates the
/// `Rahmenvertrag` + `Versorgungsverträge` from the accepted Angebot data.
pub async fn post_angebot(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Json(req): Json<CreateAngebotRequest>,
) -> impl IntoResponse {
    let lf_mp_id = req.lf_mp_id.as_deref().unwrap_or(&cfg.tenant);

    // Validate that at least one of kunden_id or interessent_name is set.
    if req.kunden_id.is_none()
        && req
            .interessent_name
            .as_ref()
            .map(|s| s.is_empty())
            .unwrap_or(true)
    {
        return (
            StatusCode::BAD_REQUEST,
            "either kunden_id or interessent_name must be supplied",
        )
            .into_response();
    }
    if req.positionen.is_empty() {
        return (StatusCode::BAD_REQUEST, "positionen must not be empty").into_response();
    }

    // Parse optional dates.
    let gueltig_bis = if let Some(ref s) = req.gueltig_bis {
        match time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE) {
            Ok(d) => d,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "gueltig_bis must be YYYY-MM-DD").into_response();
            }
        }
    } else {
        default_gueltig_bis()
    };
    let lieferbeginn: Option<time::Date> = if let Some(ref s) = req.lieferbeginn {
        match time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE) {
            Ok(d) => Some(d),
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "lieferbeginn must be YYYY-MM-DD")
                    .into_response();
            }
        }
    } else {
        None
    };

    // Calculate prices for each base position.
    let mwst = Decimal::new(119, 2); // 1.19
    let mut enriched_positionen: Vec<serde_json::Value> = Vec::new();
    let mut total_netto = Decimal::ZERO;

    for pos in &req.positionen {
        let product = fetch_product(&pool, lf_mp_id, &pos.product_code)
            .await
            .ok()
            .flatten();
        let jahreskosten_netto = product
            .as_ref()
            .and_then(|p| estimate_jahreskosten(&p.data, pos, None));
        if let Some(ref n) = jahreskosten_netto {
            total_netto += n;
        }
        let mut pos_json = serde_json::to_value(pos).unwrap_or_default();
        if let Some(obj) = pos_json.as_object_mut() {
            obj.insert(
                "product_name".into(),
                serde_json::json!(product.as_ref().map(|p| p.name.clone()).unwrap_or_default()),
            );
            obj.insert(
                "jahreskosten_netto_eur".into(),
                serde_json::json!(jahreskosten_netto.map(|d| d.to_string())),
            );
        }
        enriched_positionen.push(pos_json);
    }

    let total_brutto = total_netto * mwst;
    let total_netto_opt = if total_netto > Decimal::ZERO {
        Some(total_netto)
    } else {
        None
    };
    let total_brutto_opt = if total_brutto > Decimal::ZERO {
        Some(total_brutto)
    } else {
        None
    };

    // Enrich varianten with their own price estimates.
    let varianten_json: serde_json::Value = if let Some(ref vars) = req.varianten {
        let mut enriched_vars = Vec::new();
        for var in vars {
            let var_netto: Decimal = req
                .positionen
                .iter()
                .filter_map(|pos| {
                    fetch_product_sync_estimate(
                        &enriched_positionen,
                        &pos.product_code,
                        var.rabatt_pct,
                    )
                })
                .sum();
            let mut v = serde_json::to_value(var).unwrap_or_default();
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "jahreskosten_netto_eur".into(),
                    serde_json::json!(var_netto.to_string()),
                );
                obj.insert(
                    "jahreskosten_brutto_eur".into(),
                    serde_json::json!((var_netto * mwst).to_string()),
                );
            }
            enriched_vars.push(v);
        }
        serde_json::Value::Array(enriched_vars)
    } else {
        serde_json::Value::Array(vec![])
    };

    let positionen_json = serde_json::Value::Array(enriched_positionen);

    let angebotsnummer = match next_angebotsnummer(&pool, &cfg.tenant).await {
        Ok(n) => n,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    match insert_angebot(
        &pool,
        &cfg.tenant,
        lf_mp_id,
        &angebotsnummer,
        &req,
        &positionen_json,
        &varianten_json,
        total_netto_opt,
        total_brutto_opt,
        gueltig_bis,
        lieferbeginn,
    )
    .await
    {
        Ok(id) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "id": id,
                "angebotsnummer": angebotsnummer,
                "gueltig_bis": gueltig_bis.to_string(),
                "jahreskosten_netto_eur": total_netto_opt,
                "jahreskosten_brutto_eur": total_brutto_opt,
                "positionen_count": req.positionen.len(),
                "varianten_count": req.varianten.as_ref().map(|v| v.len()).unwrap_or(0),
            })),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Helper: re-estimate position cost using pre-enriched positionen (avoids DB re-fetch).
///
/// For variants with rabatt_pct, we scale the supply component from the base estimate
/// but leave NNE + levies unchanged (discounts only apply to the supply cost).
fn fetch_product_sync_estimate(
    enriched: &[serde_json::Value],
    product_code: &str,
    rabatt_pct: Option<Decimal>,
) -> Option<Decimal> {
    let pos = enriched
        .iter()
        .find(|p| p.get("product_code").and_then(|v| v.as_str()) == Some(product_code))?;
    let base: Decimal = pos
        .get("jahreskosten_netto_eur")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse().ok())
        .or_else(|| {
            pos.get("jahreskosten_netto_eur")
                .and_then(|v| v.as_f64())
                .and_then(|f| rust_decimal::Decimal::try_from(f).ok())
        })?;
    if let Some(r) = rabatt_pct {
        // Apply discount only to the supply portion (~35% of total as conservative estimate).
        // Exact split would require re-fetching the product; for a variant estimate this is
        // acceptable. Operators should verify with billingd.preview() before finalising.
        let supply_fraction = Decimal::new(35, 2); // 35%
        let supply_eur = base * supply_fraction;
        let non_supply_eur = base - supply_eur;
        let discounted_supply = supply_eur * (Decimal::ONE - r / Decimal::ONE_HUNDRED);
        Some(non_supply_eur + discounted_supply)
    } else {
        Some(base)
    }
}

#[derive(Debug, Deserialize)]
pub struct AngebotListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/angebote`
pub async fn list_angebote_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Query(q): Query<AngebotListQuery>,
) -> impl IntoResponse {
    match list_angebote(
        &pool,
        &cfg.tenant,
        &cfg.tenant,
        q.status.as_deref(),
        q.limit.unwrap_or(50).min(200),
    )
    .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/angebote/{id}`
pub async fn get_angebot_handler(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match fetch_angebot(&pool, id, &cfg.tenant).await {
        Ok(Some(a)) => Json(a).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/angebote/{id}/versenden`
///
/// Mark an Angebot as VERSANDT (sent to customer).
pub async fn post_angebot_versenden(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match mark_angebot_versandt(&pool, id, &cfg.tenant).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

/// Request body for `POST /api/v1/angebote/{id}/annehmen`.
#[derive(Debug, Deserialize)]
pub struct AnnehmenRequest {
    /// Index into `varianten` array (0-based).  `None` = accept the base offer.
    pub gewaehlte_variante: Option<i16>,
}

/// `POST /api/v1/angebote/{id}/annehmen`
///
/// Digitally accept an Angebot.
///
/// Validates that the Angebot is still within its `gueltig_bis` window, then
/// transitions to `ANGENOMMEN` and emits `de.angebot.angenommen` to the
/// configured ERP webhook.  The ERP or `vertragd` creates the `Rahmenvertrag`
/// from the CloudEvent payload.
pub async fn post_angebot_annehmen(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<AnnehmenRequest>,
) -> impl IntoResponse {
    let angebot = match accept_angebot(&pool, id, &cfg.tenant, req.gewaehlte_variante).await {
        Ok(a) => a,
        Err(e) => return (StatusCode::CONFLICT, e.to_string()).into_response(),
    };

    // Emit de.angebot.angenommen CloudEvent.
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        let ce = serde_json::json!({
            "specversion": "1.0",
            "type": "de.angebot.angenommen",
            "source": format!("urn:tarifbd:lf:{}", cfg.tenant),
            "id": uuid::Uuid::new_v4().to_string(),
            "time": time::OffsetDateTime::now_utc().to_string(),
            "subject": angebot.id.to_string(),
            "datacontenttype": "application/json",
            "data": {
                "angebot_id": angebot.id,
                "angebotsnummer": angebot.angebotsnummer,
                "kunden_id": angebot.kunden_id,
                "interessent_name": angebot.interessent_name,
                "lf_mp_id": angebot.lf_mp_id,
                "lieferbeginn": angebot.lieferbeginn.map(|d| d.to_string()),
                "laufzeit_monate": angebot.laufzeit_monate,
                "gewaehlte_variante": angebot.gewaehlte_variante,
                "positionen": angebot.positionen,
                "varianten": angebot.varianten,
                "jahreskosten_netto_eur": angebot.jahreskosten_netto_eur,
                "jahreskosten_brutto_eur": angebot.jahreskosten_brutto_eur,
            }
        });
        let client = reqwest::Client::new();
        if let Ok(resp) = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .json(&ce)
            .send()
            .await
            && resp.status().is_success()
            && let Ok(body) = resp.json::<serde_json::Value>().await
            && let Some(rid) = body
                .get("rahmenvertrag_id")
                .and_then(|v: &serde_json::Value| v.as_str())
                .and_then(|s: &str| s.parse::<uuid::Uuid>().ok())
        {
            let _ = link_angebot_rahmenvertrag(&pool, id, rid).await;
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": angebot.id,
            "angebotsnummer": angebot.angebotsnummer,
            "status": "ANGENOMMEN",
            "gewaehlte_variante": angebot.gewaehlte_variante,
            "message": "Angebot angenommen — de.angebot.angenommen CloudEvent dispatched",
        })),
    )
        .into_response()
}

/// `POST /api/v1/angebote/{id}/ablehnen`
///
/// Mark an Angebot as ABGELEHNT (declined by customer).
pub async fn post_angebot_ablehnen(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    match decline_angebot(&pool, id, &cfg.tenant).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/angebote/expire`  (internal maintenance endpoint)
///
/// Mark all Angebote past `gueltig_bis` as ABGELAUFEN.
/// Called by the background task; also available for manual triggers.
pub async fn post_expire_angebote(Extension(pool): Extension<PgPool>) -> impl IntoResponse {
    match expire_stale_angebote(&pool).await {
        Ok(n) => Json(serde_json::json!({ "expired": n })).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// Request body for `PUT /api/v1/angebote/{id}` — edit before sending.
#[derive(Debug, Deserialize)]
pub struct UpdateAngebotRequest {
    /// New validity end date (YYYY-MM-DD).
    pub gueltig_bis: Option<String>,
    /// New proposed Lieferbeginn (YYYY-MM-DD).
    pub lieferbeginn: Option<String>,
    /// New contract duration in months.
    pub laufzeit_monate: Option<i16>,
    /// Replace all positions with this new list.
    pub positionen: Option<Vec<crate::pg::AngebotPositionInput>>,
    /// Replace all Varianten with this new list.
    pub varianten: Option<Vec<crate::pg::AngebotVariante>>,
    /// Internal notes.
    pub notizen: Option<String>,
}

/// `PUT /api/v1/angebote/{id}` — update an Angebot before it is sent.
///
/// Only Angebote in `ANGELEGT` status can be updated.  Once sent (`VERSANDT`),
/// the quotation is immutable — create a new Angebot to supersede it.
///
/// Re-calculates `jahreskosten_netto_eur` / `jahreskosten_brutto_eur` when
/// `positionen` are updated so the totals stay in sync.
pub async fn put_angebot(
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<UpdateAngebotRequest>,
) -> impl IntoResponse {
    // Fetch existing Angebot and guard status.
    let existing = match fetch_angebot(&pool, id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    if existing.status != "ANGELEGT" {
        return (
            StatusCode::CONFLICT,
            format!(
                "Angebot {} is in status '{}' — only ANGELEGT can be updated",
                id, existing.status
            ),
        )
            .into_response();
    }

    let lf_mp_id = existing.lf_mp_id.as_str();
    let mwst = rust_decimal::Decimal::new(119, 2); // 1.19

    // Re-calculate prices if positions are being replaced.
    let (positionen_json, total_netto_opt, total_brutto_opt) = if let Some(ref new_pos) =
        req.positionen
    {
        let mut enriched: Vec<serde_json::Value> = Vec::new();
        let mut total_netto = rust_decimal::Decimal::ZERO;
        for pos in new_pos {
            let product = fetch_product(&pool, lf_mp_id, &pos.product_code)
                .await
                .ok()
                .flatten();
            let jk = product
                .as_ref()
                .and_then(|p| estimate_jahreskosten(&p.data, pos, None));
            if let Some(n) = jk {
                total_netto += n;
            }
            let mut pj = serde_json::to_value(pos).unwrap_or_default();
            if let Some(obj) = pj.as_object_mut() {
                obj.insert(
                    "product_name".into(),
                    serde_json::json!(product.as_ref().map(|p| p.name.clone()).unwrap_or_default()),
                );
                obj.insert(
                    "jahreskosten_netto_eur".into(),
                    serde_json::json!(jk.map(|d| d.to_string())),
                );
            }
            enriched.push(pj);
        }
        let total_brutto = total_netto * mwst;
        let netto_opt = (total_netto > rust_decimal::Decimal::ZERO).then_some(total_netto);
        let brutto_opt = (total_brutto > rust_decimal::Decimal::ZERO).then_some(total_brutto);
        (
            Some(serde_json::Value::Array(enriched)),
            netto_opt,
            brutto_opt,
        )
    } else {
        (None, None, None)
    };

    // Build the varianten JSON if being replaced.
    let varianten_json: Option<serde_json::Value> = req.varianten.as_ref().map(|vars| {
        serde_json::Value::Array(
            vars.iter()
                .map(|v| serde_json::to_value(v).unwrap_or_default())
                .collect(),
        )
    });

    // Parse optional date overrides.
    let new_gueltig_bis = if let Some(ref s) = req.gueltig_bis {
        match time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE) {
            Ok(d) => Some(d),
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "gueltig_bis must be YYYY-MM-DD").into_response();
            }
        }
    } else {
        None
    };
    let new_lieferbeginn: Option<Option<time::Date>> = if req.lieferbeginn.is_some() {
        let s = req.lieferbeginn.as_deref().unwrap_or("");
        if s.is_empty() {
            Some(None)
        } else {
            match time::Date::parse(s, &time::format_description::well_known::Iso8601::DATE) {
                Ok(d) => Some(Some(d)),
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "lieferbeginn must be YYYY-MM-DD")
                        .into_response();
                }
            }
        }
    } else {
        None
    };

    // Persist updates.
    let result = sqlx::query(
        r"UPDATE angebote
          SET gueltig_bis              = COALESCE($3, gueltig_bis),
              lieferbeginn             = CASE WHEN $4::bool THEN $5 ELSE lieferbeginn END,
              laufzeit_monate          = COALESCE($6, laufzeit_monate),
              positionen               = COALESCE($7, positionen),
              varianten                = COALESCE($8, varianten),
              jahreskosten_netto_eur   = COALESCE($9, jahreskosten_netto_eur),
              jahreskosten_brutto_eur  = COALESCE($10, jahreskosten_brutto_eur),
              notizen                  = COALESCE($11, notizen),
              updated_at               = now()
          WHERE id = $1 AND tenant = $2 AND status = 'ANGELEGT'",
    )
    .bind(id)
    .bind(&cfg.tenant)
    .bind(new_gueltig_bis)
    .bind(new_lieferbeginn.is_some()) // $4: flag whether lieferbeginn is being updated
    .bind(new_lieferbeginn.and_then(|v| v)) // $5: new lieferbeginn value (may be NULL)
    .bind(req.laufzeit_monate)
    .bind(positionen_json)
    .bind(varianten_json)
    .bind(total_netto_opt)
    .bind(total_brutto_opt)
    .bind(req.notizen)
    .execute(&pool)
    .await;

    match result {
        Ok(r) if r.rows_affected() > 0 => StatusCode::NO_CONTENT.into_response(),
        Ok(_) => (
            StatusCode::CONFLICT,
            "Angebot not found or no longer in ANGELEGT state",
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Comparison portal feed ────────────────────────────────────────────────────

/// Extract `TarifPreise` from a product's `tarifpreispositionen` JSONB array.
///
/// Prices are stored as scalar `Decimal` strings after `normalize_tarifpreisblatt`
/// validation (never nested `{"wert": ...}` objects).  Unknown preistypen are
/// silently ignored so extended types (e.g. `EEG_VERGUETUNG`) do not pollute
/// portal price display.
///
/// For dual-rate (HT/NT) tariffs:
/// - `arbeitspreis_ct_per_kwh` is set to the HT rate (dominant rate for portals)
/// - `arbeitspreis_ht_ct_per_kwh` and `arbeitspreis_nt_ct_per_kwh` are set separately
///
/// For single-rate tariffs:
/// - `arbeitspreis_ct_per_kwh` is set to ARBEITSPREIS_EINTARIF
/// - `arbeitspreis_ht_ct_per_kwh` and `arbeitspreis_nt_ct_per_kwh` are `None`
pub fn extract_tarif_preise(data: &serde_json::Value) -> crate::pg::TarifPreise {
    let positionen = data
        .get("tarifpreispositionen")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    let mut gp: Option<Decimal> = None;
    let mut ap_eintarif: Option<Decimal> = None;
    let mut ap_ht: Option<Decimal> = None;
    let mut ap_nt: Option<Decimal> = None;
    let mut lp: Option<Decimal> = None;

    for pos in positionen {
        let pt = pos.get("preistyp").and_then(|v| v.as_str()).unwrap_or("");
        let first_staffel_preis = pos
            .get("preisstaffeln")
            .and_then(|s| s.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.get("preis"))
            .and_then(parse_decimal_value);

        match pt {
            "GRUNDPREIS" => gp = gp.or(first_staffel_preis),
            "ARBEITSPREIS_EINTARIF" => ap_eintarif = ap_eintarif.or(first_staffel_preis),
            "ARBEITSPREIS_HT" => ap_ht = ap_ht.or(first_staffel_preis),
            "ARBEITSPREIS_NT" => ap_nt = ap_nt.or(first_staffel_preis),
            "LEISTUNGSPREIS" => lp = lp.or(first_staffel_preis),
            _ => {}
        }
    }

    crate::pg::TarifPreise {
        grundpreis_ct_per_day: gp,
        // Single-rate tariff: use ARBEITSPREIS_EINTARIF.
        // Dual-rate tariff: use HT as the "primary" rate for portal display.
        arbeitspreis_ct_per_kwh: ap_eintarif.or(ap_ht),
        arbeitspreis_ht_ct_per_kwh: ap_ht,
        arbeitspreis_nt_ct_per_kwh: ap_nt,
        leistungspreis_ct_per_kw_month: lp,
    }
}

/// Parse a JSON value as a scalar Decimal.
///
/// Accepts strings (`"31.20"`) and JSON numbers (`31.20`).
/// Rejects nested objects (already rejected by `normalize_tarifpreisblatt`).
fn parse_decimal_value(v: &serde_json::Value) -> Option<Decimal> {
    match v {
        serde_json::Value::String(s) => s.parse().ok(),
        serde_json::Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

/// Compute estimated annual supply cost (netto, excl. MwSt) for a given
/// annual consumption.
///
/// ## Formula
///
/// ```text
/// supply_netto = (grundpreis_ct/day × 365 / 100)  +  (arbeitspreis_ct/kWh × verbrauch_kWh / 100)
/// ```
///
/// Returns `None` if neither Grundpreis nor Arbeitspreis is defined (e.g. pure
/// Leistungspreis RLM products where the demand charge dominates).
///
/// **NNE, KA, Stromsteuer, and MwSt are excluded** — comparison portals add
/// DSO-specific components by PLZ after fetching this feed.
pub fn compute_jahreskosten_supply_netto(
    preise: &crate::pg::TarifPreise,
    verbrauch_kwh: Decimal,
) -> Option<Decimal> {
    use rust_decimal_macros::dec;

    let gp_eur = preise
        .grundpreis_ct_per_day
        .map(|gp| (gp * dec!(365)) / dec!(100))
        .unwrap_or(Decimal::ZERO);

    let ap_eur = preise
        .arbeitspreis_ct_per_kwh
        .map(|ap| (ap * verbrauch_kwh) / dec!(100))
        .unwrap_or(Decimal::ZERO);

    if gp_eur == Decimal::ZERO && ap_eur == Decimal::ZERO {
        return None;
    }
    Some(gp_eur + ap_eur)
}

/// Extract the price guarantee end date from the stored BO4E JSONB.
///
/// Looks for `data.preisgarantie.preisgarantieBis` (camelCase after BO4E roundtrip).
/// Returns the raw string value (ISO 8601 date) as-is — no parsing needed by portals.
pub fn extract_preisgarantie_bis(data: &serde_json::Value) -> Option<String> {
    data.pointer("/preisgarantie/preisgarantieBis")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract the contract term in months from `vertragskonditionen.laufzeit`.
///
/// Handles `einheit` values `MONAT` (direct), `JAHR` (× 12), and `WOCHE` (÷ 4 approx).
/// Returns `None` if `vertragskonditionen` or `laufzeit` is absent.
pub fn extract_laufzeit_monate(data: &serde_json::Value) -> Option<i32> {
    let einheit = data
        .pointer("/vertragskonditionen/laufzeit/einheit")
        .and_then(|v| v.as_str());
    let dauer = data
        .pointer("/vertragskonditionen/laufzeit/dauer")
        .and_then(|v| v.as_i64())
        .map(|d| d as i32);
    match (einheit, dauer) {
        (Some("MONAT"), Some(d)) => Some(d),
        (Some("JAHR"), Some(d)) => Some(d * 12),
        (Some("WOCHE"), Some(d)) => Some(d / 4),
        (None, Some(d)) => Some(d), // unit missing → assume months
        _ => None,
    }
}

/// Extract the minimum contract term in months from `vertragskonditionen.mindestlaufzeit`.
pub fn extract_mindestlaufzeit_monate(data: &serde_json::Value) -> Option<i32> {
    let einheit = data
        .pointer("/vertragskonditionen/mindestlaufzeit/einheit")
        .and_then(|v| v.as_str());
    let dauer = data
        .pointer("/vertragskonditionen/mindestlaufzeit/dauer")
        .and_then(|v| v.as_i64())
        .map(|d| d as i32);
    match (einheit, dauer) {
        (Some("MONAT"), Some(d)) => Some(d),
        (Some("JAHR"), Some(d)) => Some(d * 12),
        (Some("WOCHE"), Some(d)) => Some(d / 4),
        (None, Some(d)) => Some(d),
        _ => None,
    }
}

/// Extract the notice period in **weeks** from `vertragskonditionen.kuendigungsfrist`.
///
/// Handles `einheit` values `WOCHE` (direct), `MONAT` (× 4 approx), `TAG` (÷ 7 approx).
pub fn extract_kuendigungsfrist_wochen(data: &serde_json::Value) -> Option<i32> {
    let einheit = data
        .pointer("/vertragskonditionen/kuendigungsfrist/einheit")
        .and_then(|v| v.as_str());
    let dauer = data
        .pointer("/vertragskonditionen/kuendigungsfrist/dauer")
        .and_then(|v| v.as_i64())
        .map(|d| d as i32);
    match (einheit, dauer) {
        (Some("WOCHE"), Some(d)) => Some(d),
        (Some("MONAT"), Some(d)) => Some(d * 4),
        (Some("TAG"), Some(d)) => Some(d / 7),
        (None, Some(d)) => Some(d), // unit missing → assume weeks
        _ => None,
    }
}

/// Extract the total customer bonus/discount (RABATT sum) from `aufAbschlaege`.
///
/// Sums the first `staffeln[0].wert` of every `aufAbschlaege` entry where
/// `typ == "RABATT"`.  Returns `None` if no bonus is configured.
///
/// Note: Returns the gross bonus value as stored; MwSt distinction is encoded
/// in `aufAbschlaege[i].bezug` (`BRUTTO` / `NETTO`), visible in `tarifpreisblatt`.
pub fn extract_bonus_rabatt_eur(data: &serde_json::Value) -> Option<Decimal> {
    use rust_decimal_macros::dec;
    let auf = data.get("aufAbschlaege")?.as_array()?;
    let total: Decimal = auf
        .iter()
        .filter(|a| {
            a.get("typ")
                .and_then(|v| v.as_str())
                .map(|t| t.eq_ignore_ascii_case("RABATT"))
                .unwrap_or(false)
        })
        .filter_map(|a| {
            a.get("staffeln")?
                .as_array()?
                .first()?
                .get("wert")
                .and_then(parse_decimal_value)
        })
        .sum();
    if total == dec!(0) { None } else { Some(total) }
}

/// Compute a deterministic ETag string for the comparison feed response.
///
/// The ETag is `"<max_updated_at_nanos>-<verbrauch_kwh>-<sparte_tag>"` —
/// it changes whenever any product in the feed is updated, and is unique
/// per (`verbrauch_kwh`, `sparte`) combination (different consumption levels
/// produce different `jahreskosten` estimates).
///
/// Format: strong ETag per RFC 9110 §8.8.3 (quoted string).
pub fn compute_feed_etag(
    rows: &[crate::pg::ProductRow],
    verbrauch_kwh: Decimal,
    sparte: Option<&str>,
) -> String {
    let max_ns = rows
        .iter()
        .map(|r| r.updated_at.unix_timestamp_nanos())
        .max()
        .unwrap_or(0);
    // Deterministic, process-restart-stable representation.
    // No sha2 needed — nanosecond precision + query params make collisions
    // practically impossible for a tariff feed of typical size.
    format!(
        "\"{}-{}-{}\"",
        max_ns,
        verbrauch_kwh,
        sparte.unwrap_or("all")
    )
}

/// `GET /api/v1/comparison-feed`
///
/// Returns a machine-readable tariff listing suitable for comparison portals
/// (Verivox, Check24, Eon portal) and the BNetzA Markttransparenzstelle.
///
/// ## Query parameters
///
/// | Parameter | Type | Default | Description |
/// |---|---|---|---|
/// | `lf_mp_id` | string | `cfg.tenant` | LF operator ID |
/// | `sparte` | string | — | Filter: `STROM` \| `GAS` \| `WAERME` |
/// | `kundentyp` | string | — | Filter: `Haushalt` \| `Gewerbe` \| `Waermepumpe` \| `Ladesaeule` |
/// | `verbrauch_kwh` | decimal | `3500` | Annual consumption for `jahreskosten` estimation |
/// | `oekolabel` | string | — | Filter to products with this label (e.g. `OK_POWER`) |
/// | `include_dynamic` | bool | `true` | Include §41a EPEX-linked dynamic tariffs |
/// | `only_dynamic` | bool | `false` | Return only dynamic tariffs |
/// | `limit` | integer | `100` | Page size (1–500) |
/// | `cursor` | string | — | Pagination cursor from previous response `meta.next_cursor` |
///
/// ## Caching
///
/// Responses include an ETag and `Cache-Control: public, max-age=300`.
/// Clients **should** send `If-None-Match` on subsequent polls — the server
/// returns 304 Not Modified when no products have changed.
///
/// ## Supply-cost estimate
///
/// `jahreskosten_supply_netto_eur` = Grundpreis (EUR/a) + Arbeitspreis (EUR/a).
/// **NNE, KA, Stromsteuer, and MwSt are excluded** — these vary by DSO/PLZ and
/// must be added by the integrator after fetching from the respective APIs.
/// `jahreskosten_supply_brutto_eur` applies 19 % MwSt to the netto estimate.
///
/// ## Pagination
///
/// The feed is ordered `(updated_at DESC, product_code ASC)`.  When
/// `meta.next_cursor` is non-null, pass it as `?cursor=<value>` to retrieve the
/// next page.  The cursor is stable: new or updated products appear on page 1
/// without affecting subsequent pages.
pub async fn get_comparison_feed(
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(cfg): Extension<std::sync::Arc<TarifbdConfig>>,
    Query(q): Query<crate::pg::ComparisonFeedQuery>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    use rust_decimal_macros::dec;
    use time::format_description::well_known::Rfc3339;

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let verbrauch_kwh = q.verbrauch_kwh.unwrap_or(dec!(3500));
    let limit = q.limit.unwrap_or(100).clamp(1, 500) as usize;

    let mut rows = match crate::pg::fetch_comparison_feed(&pool, &lf_mp_id, &q).await {
        Ok(r) => r,
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
        }
    };

    // ── ETag / 304 Not Modified ───────────────────────────────────────────────
    let etag = compute_feed_etag(&rows, verbrauch_kwh, q.sparte.as_deref());
    if let Some(inm) = req_headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        && inm == etag
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    // ── Pagination: detect next page ─────────────────────────────────────────
    let has_next = rows.len() > limit;
    if has_next {
        rows.truncate(limit);
    }
    let next_cursor: Option<String> = if has_next {
        rows.last().map(|r| {
            // Compound cursor: "<updated_at>,<product_code>"
            // The product_code tie-breaker prevents skipping rows when multiple
            // products share the same updated_at timestamp.
            format!(
                "{},{}",
                r.updated_at.format(&Rfc3339).unwrap_or_default(),
                r.product_code
            )
        })
    } else {
        None
    };

    // ── Build response entries ────────────────────────────────────────────────
    let tarife: Vec<crate::pg::ComparisonFeedEntry> = rows
        .iter()
        .map(|row| {
            let preise = extract_tarif_preise(&row.data);
            let netto = compute_jahreskosten_supply_netto(&preise, verbrauch_kwh);
            let brutto = netto.map(|n| {
                use rust_decimal_macros::dec;
                (n * dec!(1.19)).round_dp(2)
            });
            let netto = netto.map(|n| n.round_dp(2));

            crate::pg::ComparisonFeedEntry {
                product_code: row.product_code.clone(),
                name: row.name.clone(),
                category: row.category.clone(),
                sparte: row.sparte.clone(),
                kundentyp: row.kundentyp.clone(),
                register_count: row.register_count.clone(),
                ist_oekostrom: row
                    .oekolabel
                    .as_ref()
                    .map(|o| !o.is_empty())
                    .unwrap_or(false),
                ist_dynamisch: row.dyn_source.is_some(),
                valid_from: row.valid_from,
                valid_to: row.valid_to,
                preise,
                jahreskosten_supply_netto_eur: netto,
                jahreskosten_supply_brutto_eur: brutto,
                mwst_pct: "19",
                laufzeit_monate: extract_laufzeit_monate(&row.data),
                kuendigungsfrist_wochen: extract_kuendigungsfrist_wochen(&row.data),
                mindestlaufzeit_monate: extract_mindestlaufzeit_monate(&row.data),
                preisgarantie_bis: extract_preisgarantie_bis(&row.data),
                bonus_rabatt_eur: extract_bonus_rabatt_eur(&row.data),
                energiemix: row.energiemix.clone(),
                oekolabel: row.oekolabel.clone(),
                tarifpreisblatt: row.data.clone(),
                updated_at: row.updated_at,
            }
        })
        .collect();

    let meta = crate::pg::ComparisonFeedMeta {
        generated_at: time::OffsetDateTime::now_utc(),
        lf_mp_id,
        verbrauch_kwh,
        sparte_filter: q.sparte.clone(),
        kundentyp_filter: q.kundentyp.clone(),
        total_returned: tarife.len(),
        next_cursor,
    };

    let response = crate::pg::ComparisonFeedResponse { meta, tarife };

    (
        StatusCode::OK,
        [
            ("ETag", etag.as_str()),
            ("Cache-Control", "public, max-age=300"),
            ("Vary", "Accept-Encoding"),
            ("X-Content-Type-Options", "nosniff"),
        ],
        Json(response),
    )
        .into_response()
}
