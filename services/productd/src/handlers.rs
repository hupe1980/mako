//! HTTP handlers for `productd`.

use crate::rounding::RoundMoney;
use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_service::{ApiError, ApiResult, cedar::CedarEnforcer, oidc::Claims};
use rubo4e::current::{Energiemix, Tarifinfo, Tarifmerkmal, Tarifpreisblatt, Tariftyp};
use rust_decimal::Decimal;
use serde::Deserialize;
use sqlx::PgPool;

use crate::{
    config::ProductdConfig,
    pg::{
        CreateAngebotRequest, EnergimixUpsertRequest, EpexImportRequest, ProductListQuery,
        ProductUpsertRequest, accept_angebot, decline_angebot, delete_energiemix,
        expire_stale_angebote, fetch_angebot, fetch_energiemix, fetch_epex_day, fetch_product,
        fetch_product_history, insert_angebot, link_angebot_rahmenvertrag, list_angebote,
        list_products, mark_angebot_versandt, monthly_epex_average, next_angebotsnummer,
        soft_delete_product, upsert_energiemix, upsert_epex_day, upsert_product,
    },
};

// ── Authorization ────────────────────────────────────────────────────────────

/// The Cedar enforcer, as the routers inject it.
pub(crate) type Authz = Extension<std::sync::Arc<CedarEnforcer>>;

/// Authorize `action` for the caller against the service tenant.
///
/// Authentication establishes *who* is calling; this decides what they may do.
/// The two are separate questions, and this service used to ask only the first:
/// every route extracted `Claims` and, at most, compared the path's `lf_mp_id`
/// with the token's tenant — which answers "is this my tenant's catalogue?",
/// not "may this caller change its prices". Any token the verifier accepted for
/// the tenant could `PUT` a new Arbeitspreis onto a live tariff, and the next
/// `billingd` run would bill it.
///
/// The denial is a bare `403`. Which rule refused, and for which subject, goes
/// to the log: a caller holding a valid token learns that it may not, not which
/// policy shape would let it.
///
/// # Errors
///
/// [`ApiError::Forbidden`] when the policy does not permit the action.
pub(crate) fn authorize(
    enforcer: &CedarEnforcer,
    claims: &Claims,
    action: &'static str,
    tenant: &str,
) -> ApiResult<()> {
    enforcer
        .check(&claims.principal(), action, tenant)
        .map_err(|e| {
            tracing::warn!(action, sub = %claims.sub(), reason = %e, "productd: authorization denied");
            ApiError::Forbidden
        })
}

// ── BO4E Tarifpreisblatt validation ──────────────────────────────────────────

/// The BO4E schema release Tarifpreisblatt payloads are stamped with.
///
/// Server-derived from the generated types rather than written as a literal, so
/// a rubo4e upgrade cannot leave productd stamping a release it no longer
/// produces. See [`mako_markt::bo4e::schema_version`].
#[must_use]
pub fn bo4e_version() -> &'static str {
    mako_markt::bo4e::SCHEMA_VERSION
}

/// Every product category `productd` accepts, in the order the schema
/// constraint lists them.
///
/// The `products.category` CHECK constraint in `migrations/0001_schema.sql` is
/// the enforcing copy; this one is what the REST and MCP surfaces describe to
/// their callers, and `the_categories_match_the_schema_constraint` holds the two
/// against each other. Thirteen of them are `energy_billing::Product` variants;
/// `BUNDLE` is productd's own composite, which resolves to component products
/// before billing ever sees it.
pub const PRODUCT_CATEGORIES: &[&str] = &[
    "STROM",
    "GAS",
    "WAERME",
    "WASSER",
    "SOLAR",
    "EEG",
    "EINSPEISUNG",
    "WAERMEPUMPE",
    "WALLBOX",
    "HEMS",
    "EMOBILITY",
    "ENERGIEDIENSTLEISTUNG",
    "BUNDLE",
    "SHARING",
];

/// Product categories that store a BO4E `Tarifpreisblatt` payload.
///
/// For these categories `_typ: "TARIFPREISBLATT"` is required (injected if
/// absent) and the full BO4E envelope is validated via
/// `rubo4e::current::Tarifpreisblatt`.  All other categories
/// (`WASSER`, `HEMS`, `EMOBILITY`, `ENERGIEDIENSTLEISTUNG`, `BUNDLE`) use a
/// free-form structure — only `tarifpreise` is validated if present.
const TARIFPREISBLATT_CATEGORIES: &[&str] = &[
    "STROM",
    "GAS",
    "WAERME",
    "SOLAR",
    "EEG",
    "EINSPEISUNG",
    "WAERMEPUMPE",
    "WALLBOX",
    // SHARING (§42c EnWG) uses the same Tarifpreisblatt BO4E envelope as STROM;
    // billingd reads it as a SharingProduct with ElectricityProduct inside.
    "SHARING",
];

/// Whitelist of valid `preistyp` values for mako products.
///
/// Canonical ALLCAPS naming — values are normalised to ALLCAPS before the
/// check, so `"grundpreis"` is accepted and stored as `"GRUNDPREIS"`.
///
/// **Hard-cut:** any value not in this list is rejected with 422.
pub const VALID_PREISTYPEN: &[&str] = &[
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
    // The Mieterstromzuschlag (§ 21 Abs. 3 EEG 2023) is deliberately **not** a
    // retail Preistyp: it is the Anlagenbetreiber's claim against the
    // Netzbetreiber, settled by `einsd`/`eeg-billing`. Carrying it as a price
    // position here would invite billing it to the tenant.
    "GRUNDVERSORGUNG_ARBEITSPREIS",
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
/// | `STROM`, `GAS`, `WAERME`, `SOLAR`, `EEG`, `EINSPEISUNG`, `WAERMEPUMPE`, `WALLBOX`, `SHARING` | ✓ `"TARIFPREISBLATT"` | ✓ via `rubo4e::current::Tarifpreisblatt` |
/// | `WASSER`, `HEMS`, `EMOBILITY`, `ENERGIEDIENSTLEISTUNG`, `BUNDLE` | ✗ | ✗ |
///
/// ## Position validation (all categories)
///
/// Applied to every element of `tarifpreise` when the field is present:
///
/// - `preistyp` is normalised to ALLCAPS and validated against [`VALID_PREISTYPEN`].
/// - `preisstaffeln[*].preis` must be a **scalar** JSON string or number parseable
///   as `Decimal`.  The nested `{"wert": "..."}` form (non-BO4E) is rejected.
///
/// ## Canonicalisation
///
/// For BO4E categories the full envelope is re-serialised from the typed struct,
/// yielding canonical camelCase field names.  The normalised
/// `tarifpreise` (with ALLCAPS `preistyp`) are merged back so that
/// mako-extended preistyp values survive the round-trip without being mapped to
/// `"UNKNOWN"` by `Preistyp`'s catch-all serde variant.
pub fn normalize_tarifpreisblatt(
    category: &str,
    mut data: serde_json::Value,
) -> Result<serde_json::Value, (StatusCode, serde_json::Value)> {
    let is_bo4e_category = TARIFPREISBLATT_CATEGORIES.contains(&category);

    // ── 0. _version: reject payloads from another schema **series** ──────────
    //
    // Matched on the series (`202607`), not on the exact release: BO4E ships
    // patch releases inside a series and every one of them deserializes into
    // the same Rust types, so an equality check would reject a payload from a
    // producer one patch ahead that productd reads perfectly.
    //
    // Missing _version is accepted — the round-trip injects the current one.
    if is_bo4e_category
        && let Some(v) = data
            .get("_version")
            .and_then(|v| v.as_str())
            .filter(|&v| !mako_markt::bo4e::version_is_readable(v))
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({
                "error": format!(
                    "_version {v:?} is not accepted; this build reads BO4E series {:?}",
                    mako_markt::bo4e::SCHEMA_SERIES
                )
            }),
        ));
    }

    // ── 1. Normalise tarifpreise ─────────────────────────────────────
    //    - ALLCAPS preistyp normalisation + whitelist check
    //    - scalar Decimal validation for preisstaffeln[*].preis
    if let Some(positionen) = data.get_mut("tarifpreise").and_then(|v| v.as_array_mut()) {
        for (i, pos) in positionen.iter_mut().enumerate() {
            if let Some(pt) = pos.get("preistyp").and_then(|v| v.as_str()) {
                let upper = pt.to_uppercase();
                if !VALID_PREISTYPEN.contains(&upper.as_str()) {
                    return Err((
                        StatusCode::UNPROCESSABLE_ENTITY,
                        serde_json::json!({
                            "error": format!(
                                "tarifpreise[{i}].preistyp {pt:?} is not valid; \
                                 accepted values: {}",
                                VALID_PREISTYPEN.join(", ")
                            )
                        }),
                    ));
                }
                // A value BO4E defines stays in the BO4E field. A mako
                // extension moves to the `mako:preistyp` ZusatzAttribut and
                // `preistyp` is dropped: the standard's own enum field carries
                // standard values only (see `mako_markt::bo4e`).
                if let Some(obj) = pos.as_object_mut() {
                    if mako_markt::bo4e::is_bo4e_preistyp(&upper) {
                        obj.insert("preistyp".to_owned(), serde_json::json!(upper));
                    } else {
                        obj.remove("preistyp");
                        let attrs = obj
                            .entry("zusatzAttribute")
                            .or_insert_with(|| serde_json::json!([]));
                        if let Some(arr) = attrs.as_array_mut() {
                            arr.retain(|a| {
                                a.get("name").and_then(|v| v.as_str())
                                    != Some(mako_markt::bo4e::MAKO_PREISTYP_ATTRIBUT)
                            });
                            arr.push(serde_json::json!({
                                "name": mako_markt::bo4e::MAKO_PREISTYP_ATTRIBUT,
                                "wert": upper,
                            }));
                        }
                    }
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
                                        "tarifpreise[{i}].preisstaffeln[{j}].preis \
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

    // ── 2. The BO4E gate ─────────────────────────────────────────────────────
    //    `_typ`, typed deserialization, strict enums, and the BO4E-stated
    //    rules, in that order — see `mako_markt::bo4e::gate`.
    //
    //    The strict-enum stage is load-bearing *here* in a way it is not at a
    //    store-the-payload endpoint: this function returns the **canonical
    //    round-trip** as what gets stored, and a BO4E enum decoding to the
    //    `Unknown` catch-all serialises back as the literal `"UNKNOWN"` — so
    //    without it `"sparte": "STROMM"` would overwrite what the caller sent.
    //
    //    Step 1 has already moved every mako-only preistyp into the
    //    `mako:preistyp` ZusatzAttribut, so nothing in the tree is expected to
    //    reach the catch-all and the round-trip is lossless.
    if is_bo4e_category {
        let typed: Tarifpreisblatt = mako_markt::bo4e::decode(data.clone())
            .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))?;

        // ── Cross-validation: the BO4E `sparte` must match the product
        //    category. A GAS product carrying `sparte: Strom` (or vice versa)
        //    would misroute in billing, so it is rejected here rather than
        //    stored inconsistently. Categories in the Strom family
        //    (SOLAR/EEG/EINSPEISUNG/WAERMEPUMPE/WALLBOX/SHARING) expect Strom.
        if let Some(sparte) = typed.sparte {
            use rubo4e::current::Sparte;
            // WAERME is intentionally omitted — BO4E splits it into Fernwaerme
            // and Nahwaerme, so a single category → sparte mapping would be
            // wrong. Only the unambiguous Strom/Gas families are cross-checked.
            let expected = match category {
                "GAS" => Some(Sparte::Gas),
                "WASSER" => Some(Sparte::Wasser),
                "STROM" | "SOLAR" | "EEG" | "EINSPEISUNG" | "WAERMEPUMPE" | "WALLBOX"
                | "SHARING" => Some(Sparte::Strom),
                _ => None,
            };
            if let Some(exp) = expected
                && sparte != exp
            {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    serde_json::json!({
                        "error": format!(
                            "sparte {sparte:?} does not match category {category} \
                             (expected {exp:?})"
                        )
                    }),
                ));
            }
        }

        // With mako-only price types carried in `zusatzAttribute` rather than
        // in `preistyp`, the typed round-trip is lossless, so the canonical
        // form is what gets stored.
        let canonical = serde_json::to_value(&typed).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({
                    "error": format!("could not serialise Tarifpreisblatt: {e}")
                }),
            )
        })?;
        return Ok(canonical);
    }

    Ok(data)
}

// ── BO4E Energiemix validation ────────────────────────────────────────────────

/// Validate an `Energiemix` COM payload.
///
/// Runs the BO4E gate (`_typ`, schema, strict enums, BO4E rules) and then the
/// §42 EnWG completeness rule the standard does not state. The strict-enum
/// stage matters here for the same reason it does for the price sheet: the
/// canonical round-trip is what gets stored, so an unrecognised
/// `erzeugungsart` would be written back as the literal `"UNKNOWN"` and the
/// disclosure would name a source that does not exist.
fn normalize_energiemix(
    data: serde_json::Value,
) -> Result<(Energiemix, serde_json::Value), (StatusCode, serde_json::Value)> {
    let mix: Energiemix = mako_markt::bo4e::decode(data)
        .map_err(|e| (StatusCode::UNPROCESSABLE_ENTITY, e.to_json()))?;

    // §42 Abs. 2 Nr. 2 EnWG completeness: the energy-source breakdown must be
    // present and account for the whole supply. An empty `{}` Energiemix used
    // to be accepted and would satisfy neither the invoice nor the portal
    // disclosure obligation. The `anteil[]` shares (Prozent) must sum to
    // ~100 % (±0.5 for rounding).
    let berr = |msg: String| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": msg }),
        )
    };
    let anteile = mix
        .anteil
        .as_ref()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            berr(
                "§42 EnWG: Energiemix requires a non-empty `anteil[]` energy-source \
                  breakdown (Erneuerbare/Kernenergie/fossil shares)."
                    .to_owned(),
            )
        })?;
    let sum: rust_decimal::Decimal = anteile
        .iter()
        .filter_map(|h| h.anteil_prozent.as_ref())
        .copied()
        .sum();
    if (sum - rust_decimal::Decimal::from(100)).abs() > rust_decimal::Decimal::new(5, 1) {
        return Err(berr(format!(
            "§42 EnWG: Energiemix `anteil[]` shares sum to {sum} %, must be ~100 %."
        )));
    }

    let canonical = serde_json::to_value(&mix).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({ "error": format!("could not serialise Energiemix: {e}") }),
        )
    })?;
    Ok((mix, canonical))
}

// ── Product CRUD ──────────────────────────────────────────────────────────────

/// `PUT /api/v1/products/{lf_mp_id}/{product_code}`
pub async fn put_product(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
    Json(mut req): Json<ProductUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-product", &cfg.tenant) {
        return e.into_response();
    }
    // Tenant isolation: each LF operator can only manage their own products.
    if lf_mp_id != claims.tenant() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "cannot manage products for {lf_mp_id:?}: token tenant is {:?}",
                    claims.tenant()
                )
            })),
        )
            .into_response();
    }
    // dyn_source is validated by the DB CHECK constraint, but we surface a
    // clear 422 here before hitting the DB.
    if let Some(ref ds) = req
        .dyn_source
        .as_deref()
        .filter(|&ds| ds != "epex-spot-day-ahead")
    {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "dyn_source {ds:?} is not valid; the only accepted value is \
                     'epex-spot-day-ahead'"
                )
            })),
        )
            .into_response();
    }
    // product_status validation.
    if !matches!(req.product_status.as_str(), "DRAFT" | "PUBLISHED") {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": format!(
                    "product_status {:?} is not valid; accepted values: DRAFT, PUBLISHED",
                    req.product_status
                )
            })),
        )
            .into_response();
    }
    // Validate + canonicalise product data against BO4E Tarifpreisblatt schema.
    req.data = match normalize_tarifpreisblatt(&req.category, req.data) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };
    let category = req.category.clone();
    let status = req.product_status.clone();
    match upsert_product(&pool, &lf_mp_id, claims.tenant(), &product_code, req).await {
        Ok(id) => {
            // Notify the ERP / productd-agent so §42 Energiemix completeness
            // and §41a EPEX checks run against the new version. This is the only
            // emitter of the agent's `de.tarif.product.updated` trigger.
            emit_productd_event(
                &cfg,
                mako_events::tarif::PRODUCT_UPDATED,
                &product_code,
                serde_json::json!({
                    "lf_mp_id": lf_mp_id,
                    "product_code": product_code,
                    "category": category,
                    "product_status": status,
                }),
            )
            .await;
            (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response()
        }
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// Fire-and-forget CloudEvent to the ERP webhook, HMAC-signed when a secret is
/// configured. The signature is computed over the exact bytes sent (the body
/// is transmitted verbatim, not re-serialised), so a verifying subscriber sees
/// a matching digest.
async fn emit_productd_event(
    cfg: &ProductdConfig,
    event_type: &str,
    subject: &str,
    data: serde_json::Value,
) {
    let Some(webhook_url) = cfg.erp_webhook_url.as_deref() else {
        return;
    };
    let ce = mako_service::CloudEvent::new(
        mako_service::source("productd", &cfg.tenant),
        event_type,
        subject,
        data,
    )
    .extension("tenantid", cfg.tenant.clone());
    let client = mako_service::http::default_client();
    if let Err(e) = mako_service::post_ce_with_retry(
        &client,
        webhook_url,
        &ce,
        cfg.erp_hmac_secret.as_deref().map(str::as_bytes),
    )
    .await
    {
        tracing::warn!(error = %e, event_type, "productd: ERP webhook error");
    }
}

/// `GET /api/v1/products/{lf_mp_id}/{product_code}`
pub async fn get_product(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-product", &cfg.tenant) {
        return e.into_response();
    }
    match fetch_product(&pool, &lf_mp_id, claims.tenant(), &product_code, None).await {
        Ok(Some(row)) => Json(row).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}`
pub async fn list_products_handler(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(lf_mp_id): Path<String>,
    Query(q): Query<ProductListQuery>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-product", &cfg.tenant) {
        return e.into_response();
    }
    match list_products(&pool, &lf_mp_id, claims.tenant(), &q).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/products/{lf_mp_id}/{product_code}/history`
pub async fn get_product_history(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-product", &cfg.tenant) {
        return e.into_response();
    }
    match fetch_product_history(&pool, &lf_mp_id, claims.tenant(), &product_code).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `DELETE /api/v1/products/{lf_mp_id}/{product_code}`
///
/// Soft-delete a product by setting `valid_to = today`.
/// The product remains in the database for historical billing lookups but is
/// excluded from the comparison feed and `list_products` (unless
/// `?include_expired=true` is passed).
pub async fn delete_product(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-product", &cfg.tenant) {
        return e.into_response();
    }
    if lf_mp_id != claims.tenant() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "cannot delete products for {lf_mp_id:?}: token tenant is {:?}",
                    claims.tenant()
                )
            })),
        )
            .into_response();
    }
    match soft_delete_product(&pool, &lf_mp_id, claims.tenant(), &product_code).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

// ── Product resolution ────────────────────────────────────────────────────────

/// One product to resolve: a code and the day it has to be valid on.
#[derive(Debug, Deserialize)]
pub struct ProductQuery {
    pub product_code: String,
    /// The day the version must be in force on. Defaults to today (Berlin).
    #[serde(default)]
    pub as_of: Option<time::Date>,
}

#[derive(Debug, Deserialize)]
pub struct ResolveProductsRequest {
    pub anfragen: Vec<ProductQuery>,
}

/// `POST /api/v1/products/{lf_mp_id}/resolve` — product versions by code+date.
///
/// A billing period split by a Tarifwechsel needs one product version per leg,
/// each valid on that leg's own dates. Asking one request per leg is an N+1 on
/// every invoice; asking here is one round trip.
///
/// A code with no version valid on its date comes back as `null` in place, so
/// the caller can tell *which* of its legs is unpriceable rather than getting a
/// shorter list than it asked for.
pub async fn post_resolve_products(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(lf_mp_id): Path<String>,
    Json(req): Json<ResolveProductsRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "read-product", &cfg.tenant)?;
    if req.anfragen.len() > 100 {
        return Err(ApiError::bad_request(
            "at most 100 products may be resolved per request",
        ));
    }
    let mut produkte = Vec::with_capacity(req.anfragen.len());
    for q in &req.anfragen {
        let row = fetch_product(&pool, &lf_mp_id, &cfg.tenant, &q.product_code, q.as_of)
            .await
            .map_err(ApiError::Internal)?;
        produkte.push(serde_json::json!({
            "product_code": q.product_code,
            "as_of": q.as_of.map(|d| d.to_string()),
            "product": row,
        }));
    }
    Ok(Json(serde_json::json!({
        "lf_mp_id": lf_mp_id,
        "produkte": produkte,
    })))
}

// ── EPEX Spot day-ahead prices ────────────────────────────────────────────────

/// `PUT /api/v1/epex-prices/{date}`
///
/// Import a delivery day's EPEX day-ahead prices. Body:
/// `{ "prices": [ct_kwh_mtu0, …], "mtu_minutes": 15, "source": "..." }`.
/// `prices` length must match the local day's MTU count (96/92/100 at 15-min,
/// 24/23/25 at 60-min). `mtu_minutes` defaults to 15.
pub async fn put_epex_prices(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(date_str): Path<String>,
    Json(req): Json<EpexImportRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-marktpreise", &cfg.tenant) {
        return e.into_response();
    }
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

/// `GET /api/v1/epex-prices/{date}/quarter-hourly`
///
/// Returns the delivery day's spot prices as 15-minute market time units, each
/// with its UTC `mtu_start` instant and `price_ct_kwh` (legacy hourly rows are
/// expanded to quarter-hours). Used by `billingd` for §41a dynamic billing.
pub async fn get_epex_prices_quarter_hourly(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(date_str): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-marktpreise", &cfg.tenant) {
        return e.into_response();
    }
    use time::format_description::well_known::{Iso8601, Rfc3339};
    let date = match time::Date::parse(&date_str, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid date, expected YYYY-MM-DD").into_response();
        }
    };
    match fetch_epex_day(&pool, date).await {
        Ok(Some(points)) => {
            let prices: Vec<serde_json::Value> = points
                .iter()
                .map(|p| {
                    serde_json::json!({
                        "mtu_start": p.mtu_start.format(&Rfc3339).unwrap_or_default(),
                        "price_ct_kwh": p.avg_ct_kwh.to_string(),
                    })
                })
                .collect();
            Json(serde_json::json!({
                "price_date": date_str,
                "mtu_minutes": 15,
                "unit": "ct_per_kwh",
                "prices": prices,
            }))
            .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/epex-prices/{year}/{month}/average`
///
/// Returns the monthly average ct/kWh for EPEX Spot.
/// Used by `einsd` for Direktvermarktung Marktprämie calculation.
pub async fn get_epex_monthly_average(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((year, month)): Path<(i32, u8)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-marktpreise", &cfg.tenant) {
        return e.into_response();
    }
    match monthly_epex_average(&pool, year, month).await {
        Ok(Some(avg)) => Json(serde_json::json!({
            "year": year,
            "month": month,
            "avg_ct_kwh": avg,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

// ── nEHS certificate prices (BEHG CO₂) ────────────────────────────────────────

/// `PUT /api/v1/nehs-prices/{date}`
///
/// Import one dated nEHS certificate price (EUR/t CO₂) — an EEX auction
/// clearing price (weekly from 01.07.2026), the Verkaufsphase price (68 EUR/t)
/// or a manual entry. Body: `{ "eur_per_t": "63.50", "source": "auktion" }`.
pub async fn put_nehs_price(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(date_str): Path<String>,
    Json(req): Json<crate::pg::NehsImportRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-marktpreise", &cfg.tenant) {
        return e.into_response();
    }
    use time::format_description::well_known::Iso8601;
    let date = match time::Date::parse(&date_str, &Iso8601::DEFAULT) {
        Ok(d) => d,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, "invalid date, expected YYYY-MM-DD").into_response();
        }
    };
    match crate::pg::upsert_nehs_price(&pool, date, req).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/nehs-prices/latest?date=YYYY-MM-DD`
///
/// Most recent nEHS price at or before `date` (defaults to today). Used by
/// `billingd` to derive the Gas CO₂ component (CO2KostAufG §3 pass-through).
pub async fn get_nehs_price_latest(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-marktpreise", &cfg.tenant) {
        return e.into_response();
    }
    use time::format_description::well_known::Iso8601;
    let date = match q.get("date") {
        Some(s) => match time::Date::parse(s, &Iso8601::DEFAULT) {
            Ok(d) => d,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, "invalid date, expected YYYY-MM-DD")
                    .into_response();
            }
        },
        None => mako_fristen::heute(),
    };
    match crate::pg::latest_nehs_price(&pool, date).await {
        Ok(Some((price_date, eur_per_t))) => Json(serde_json::json!({
            "price_date": price_date.to_string(),
            "eur_per_t": eur_per_t,
        }))
        .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
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
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
    Json(mut req): Json<EnergimixUpsertRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-product", &cfg.tenant) {
        return e.into_response();
    }
    if lf_mp_id != claims.tenant() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "cannot update energiemix for {lf_mp_id:?}: token tenant is {:?}",
                    claims.tenant()
                )
            })),
        )
            .into_response();
    }
    // Validate and canonicalise the Energiemix COM payload.
    let (_typed_mix, canonical) = match normalize_energiemix(req.energiemix) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };
    req.energiemix = canonical;

    match upsert_energiemix(&pool, &lf_mp_id, claims.tenant(), &product_code, req).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) if e.to_string().contains("not found") => {
            (StatusCode::NOT_FOUND,
             Json(serde_json::json!({ "error": format!("product {lf_mp_id}/{product_code} not found") })))
                .into_response()
        }
        Err(e) => ApiError::Internal(e).into_response(),
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
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-product", &cfg.tenant) {
        return e.into_response();
    }
    match fetch_energiemix(&pool, &lf_mp_id, claims.tenant(), &product_code).await {
        Ok(Some(row)) if !row.energiemix.is_null() => Json(row).into_response(),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "no Energiemix set for this product" })),
        )
            .into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `DELETE /api/v1/products/{lf_mp_id}/{product_code}/energiemix`
///
/// Remove the `Energiemix` and `Oekolabel` from a product (hard cut).
/// Use when a product transitions from green-certified back to standard.
pub async fn delete_energiemix_handler(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path((lf_mp_id, product_code)): Path<(String, String)>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-product", &cfg.tenant) {
        return e.into_response();
    }
    if lf_mp_id != claims.tenant() {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": format!(
                    "cannot delete energiemix for {lf_mp_id:?}: token tenant is {:?}",
                    claims.tenant()
                )
            })),
        )
            .into_response();
    }
    match delete_energiemix(&pool, &lf_mp_id, claims.tenant(), &product_code).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

// ── Angebot (B2B Quotation, L4) ───────────────────────────────────────────────

/// Detailed cost breakdown for one position within one Angebot scenario.
///
/// All amounts in EUR with 2 decimal places. The estimated all-in annual cost
/// of one supply point: product + NNE + statutory levies.
///
/// ## Cost components (§ 3 StromStG, § 2 EnergieStG, KAV § 2, BNetzA Netzentgelt)
///
/// | Component | Typical share (SLP Gewerbe) | Source |
/// |---|---|---|
/// | Supply (Grundpreis + Arbeitspreis) | 30–40 % | product `Tarifpreisblatt` |
/// | NNE (Netzentgelt Arbeitspreis) | 30–40 % | `marktd.PreisblattNetznutzung` |
/// | NNE (Netzentgelt Grundpreis) | 5–10 % | `marktd.PreisblattNetznutzung` |
/// | Konzessionsabgabe | 2–3 % | `marktd.PreisblattKonzessionsabgabe` |
/// | Stromsteuer / Energiesteuer | 5–10 % | statutory (§ 3 StromStG, § 2 Abs. 3 EnergieStG) |
/// | BEHG (Gas only) | ~2 % | the dated nEHS certificate series |
/// | Umsatzsteuer | 19 % | § 12 Abs. 1 UStG, unless the product states its own rate |
///
/// NNE + statutory components are taken from the position-level overrides when
/// provided.  Statutory defaults apply when the override is `None`:
/// - Stromsteuer: 2.05 ct/kWh (20,50 EUR/MWh, § 3 StromStG)
/// - Energiesteuer Gas: 0.55 ct/kWh (5,50 EUR/MWh, § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG)
/// - BEHG Gas: derived from the nEHS certificate price for the quotation's
///   Stichtag. Certificates have been auctioned since 2026 (§ 10 Abs. 1 BEHG),
///   so there is no fixed rate to fall back on: a Gas position is refused
///   unless the series holds a price or the caller states one.
///
/// NNE is NOT auto-fetched from `marktd` — the caller must supply it via
/// `nne_arbeitspreis_ct_per_kwh` + `nne_grundpreis_eur_per_year` in the
/// `AngebotPositionInput`.  This keeps `productd` stateless with respect to
/// `marktd`.  For automated quoting workflows, pre-fetch the NNE from
/// `marktd GET /api/v1/preisblaetter/{nb_mp_id}` and pass it in.
#[derive(Debug, serde::Serialize)]
pub struct PositionCostBreakdown {
    pub product_code: String,
    pub sparte: String,
    /// Marktlokation this cost belongs to.
    ///
    /// Without it a breakdown line identifies its supply point only by the
    /// free-text `standort_bezeichnung`, which is not a key — and the BO4E
    /// projection has nothing to put in `lieferstellenangebotsteil`.
    pub malo_id: Option<String>,
    /// Messlokation — carried through so the accepted quotation can be
    /// registered. A gas supply point without it produces a contract nothing
    /// can file a Lieferbeginn for.
    pub melo_id: Option<String>,
    /// Netzbetreiber behind the supply point — the UTILMD's recipient.
    pub nb_mp_id: Option<String>,
    pub standort_bezeichnung: Option<String>,
    pub jahresverbrauch_kwh: Decimal,
    /// Supply cost only (Grundpreis + Arbeitspreis + Leistungspreis, after discount).
    pub supply_netto_eur: Decimal,
    /// DSO grid fees (NNE Grundpreis + Arbeitspreis + Leistungspreis).
    pub nne_netto_eur: Decimal,
    /// Konzessionsabgabe (KAV §2).
    pub ka_eur: Decimal,
    /// Statutory levies: Stromsteuer (Strom) or Energiesteuer + BEHG (Gas).
    pub levies_eur: Decimal,
    /// Supply + NNE + KA + levies (no MwSt).
    pub total_netto_eur: Decimal,
    /// `total_netto_eur × (1 + mwst_satz)`.
    pub total_brutto_eur: Decimal,
    /// The Umsatzsteuersatz this position is quoted at, as a fraction of the
    /// net: 0.19 under § 12 Abs. 1 UStG, the product's own `mwst_rate_override`
    /// where it carries one, and 0 where the recipient owes the tax under
    /// § 13b Abs. 2 Nr. 5 Buchst. b i.V.m. Abs. 5 UStG.
    pub mwst_satz: Decimal,
    /// Effective supply Arbeitspreis in ct/kWh (for at-a-glance comparison).
    /// For a Zweitarif product this is the consumption-weighted average of the
    /// HT and NT rates, which is the figure a buyer compares on.
    pub arbeitspreis_ct_per_kwh: Option<Decimal>,
    /// Zweitarif band rates in ct/kWh; `None` for a single-rate product.
    pub arbeitspreis_ht_ct_per_kwh: Option<Decimal>,
    pub arbeitspreis_nt_ct_per_kwh: Option<Decimal>,
    /// Grundpreis in EUR/year.
    pub grundpreis_eur_per_year: Option<Decimal>,
}

/// Full cost breakdown for one scenario (base or variant).
#[derive(Debug, serde::Serialize)]
pub struct ScenarioCostBreakdown {
    /// Scenario name (e.g. "Basis (12 Monate)" or "24 Monate Festpreis −5%").
    pub label: String,
    pub laufzeit_monate: i16,
    /// `true` for the base scenario (index −1 in variants array).
    pub ist_basis: bool,
    /// `None` for the base scenario; `Some(idx)` for variants.
    pub variante_index: Option<usize>,
    pub rabatt_pct: Option<Decimal>,
    /// Sum of `total_netto_eur` across all positions.
    pub jahreskosten_netto_eur: Decimal,
    /// Sum of `total_brutto_eur` across all positions — summed rather than
    /// derived from the net, because each position carries its own
    /// Umsatzsteuersatz and a scenario may mix Sparten.
    pub jahreskosten_brutto_eur: Decimal,
    /// Saving vs. base scenario in EUR/year (negative = more expensive).
    pub ersparnis_vs_basis_eur: Option<Decimal>,
    pub positionen_detail: Vec<PositionCostBreakdown>,
}

/// Why one quotation position cannot be priced.
///
/// Carried instead of a dropped position: an Angebot is a binding offer, so a
/// position that cannot be priced refuses the quotation and names what is
/// missing. Quoting the sum of the positions that *did* resolve prices a
/// five-site Rahmenvertrag for four sites.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PositionPricingError {
    /// Index into the quotation's `positionen`, so the caller can point at the row.
    pub position_index: usize,
    pub product_code: String,
    pub malo_id: Option<String>,
    /// Machine-readable reason — one of [`grund`].
    pub grund: &'static str,
    pub detail: String,
}

/// The reasons a position can be refused.
pub mod grund {
    /// The product catalogue could not be read at all.
    pub const KATALOG_NICHT_ERREICHBAR: &str = "KATALOG_NICHT_ERREICHBAR";
    /// No product with this code is in force for the quotation's Stichtag.
    pub const PRODUKT_UNBEKANNT: &str = "PRODUKT_UNBEKANNT";
    /// The product carries no `tarifpreise`, or they do not parse.
    pub const KEIN_TARIFPREISBLATT: &str = "KEIN_TARIFPREISBLATT";
    /// No Preisstaffel of the product covers the quantity asked about.
    pub const KEINE_PREISSTAFFEL: &str = "KEINE_PREISSTAFFEL";
    /// A Zweitarif product was quoted without the HT/NT split of the volume.
    pub const HT_NT_AUFTEILUNG_FEHLT: &str = "HT_NT_AUFTEILUNG_FEHLT";
    /// A product with a Leistungspreis was quoted without a `leistung_kw`.
    pub const LEISTUNG_FEHLT: &str = "LEISTUNG_FEHLT";
    /// No nEHS certificate price for the Stichtag and no explicit override.
    pub const BEHG_PREIS_FEHLT: &str = "BEHG_PREIS_FEHLT";
    /// The product's `mwst_rate_override` is not a rate.
    pub const MWST_SATZ_UNGUELTIG: &str = "MWST_SATZ_UNGUELTIG";
}

/// A refusal before it knows which position it belongs to.
#[derive(Debug, Clone)]
pub struct Unpreisbar {
    /// One of [`grund`].
    pub grund: &'static str,
    pub detail: String,
}

impl Unpreisbar {
    fn new(grund: &'static str, detail: impl Into<String>) -> Self {
        Self {
            grund,
            detail: detail.into(),
        }
    }

    fn at(self, index: usize, pos: &crate::pg::AngebotPositionInput) -> PositionPricingError {
        PositionPricingError {
            position_index: index,
            product_code: pos.product_code.clone(),
            malo_id: pos.malo_id.clone(),
            grund: self.grund,
            detail: self.detail,
        }
    }
}

/// The statutory inputs a quotation is priced under that the product itself
/// does not carry.
#[derive(Debug, Clone, Copy)]
pub struct PricingContext {
    /// BEHG CO₂ cost in ct/kWh_Hs, derived from the dated nEHS certificate
    /// series for the quotation's Stichtag.
    ///
    /// `None` when the series holds no price at or before that date. A Gas
    /// position then has to carry its own `behg_gas_ct_per_kwh`: § 10 Abs. 1
    /// BEHG prices certificates by auction from 2026, so there is no statutory
    /// figure left to fall back on and a constant would quote a past year's
    /// CO₂ cost.
    pub behg_gas_ct_per_kwh: Option<Decimal>,
    /// The recipient is a Wiederverkäufer i.S.d. § 3g UStG, so under
    /// § 13b Abs. 2 Nr. 5 Buchst. b i.V.m. Abs. 5 UStG they owe the tax and the
    /// quotation states none.
    pub reverse_charge_13b: bool,
}

/// The context the public comparison feed prices under: supply only, so no Gas
/// CO₂ component, and a household tariff, so never a reverse charge.
const FEED_PRICING_CONTEXT: PricingContext = PricingContext {
    behg_gas_ct_per_kwh: None,
    reverse_charge_13b: false,
};

/// The Erdgas CO₂ cost of one kWh_Hs at a given certificate price.
///
/// EBeV Standardwert 55,8 t CO₂/TJ for Erdgas at the ordinance's Umrechnungs-
/// faktor of 3,2508 GJ/MWh_Hs — 0,18139464 kg CO₂ per kWh_Hs — turned from
/// EUR/t into ct/kWh.
fn behg_erdgas_ct_per_kwh(eur_per_t: Decimal) -> Decimal {
    use rust_decimal::dec;
    eur_per_t * dec!(0.18139464) / dec!(10)
}

/// The Umsatzsteuersatz a position is quoted at, as a fraction of the net.
///
/// § 12 Abs. 1 UStG's 19 % is the default, and the product's own
/// `mwst_rate_override` is the only thing that departs from it — the same field
/// and the same scale (`0.19`, never `19`) that `billingd` bills from, so the
/// quotation and the invoice that follows it cannot disagree. Reverse charge
/// under § 13b Abs. 2 Nr. 5 Buchst. b i.V.m. Abs. 5 UStG moves the tax to the
/// recipient, and the offer then states none.
fn mwst_satz(
    product_data: &serde_json::Value,
    ctx: &PricingContext,
) -> Result<Decimal, Unpreisbar> {
    use rust_decimal::dec;
    if ctx.reverse_charge_13b {
        return Ok(Decimal::ZERO);
    }
    match product_data.get("mwst_rate_override") {
        None | Some(serde_json::Value::Null) => Ok(dec!(0.19)),
        Some(v) => {
            let rate = parse_decimal_value(v).ok_or_else(|| {
                Unpreisbar::new(
                    grund::MWST_SATZ_UNGUELTIG,
                    format!("mwst_rate_override {v} ist keine Dezimalzahl"),
                )
            })?;
            if rate < Decimal::ZERO || rate > Decimal::ONE {
                return Err(Unpreisbar::new(
                    grund::MWST_SATZ_UNGUELTIG,
                    format!(
                        "mwst_rate_override {rate} liegt außerhalb von 0…1 — der Satz ist ein \
                         Anteil am Netto (0.19), kein Prozentwert"
                    ),
                ));
            }
            Ok(rate)
        }
    }
}

/// Compute a detailed cost breakdown for one position + one scenario's `rabatt_pct`.
///
/// # What is refused rather than guessed
///
/// * The applicable **Preisstaffel is selected by the quantity**, through
///   `rubo4e`'s `select_for`, which also implements BO4E's rule that a value
///   between two tiers *„rutscht in die obere Zone"*. A quantity above every
///   stated tier has no price at all and refuses the position.
/// * **HT and NT are priced separately** against their own volumes. A Zweitarif
///   product quoted without that split refuses, because pricing the whole
///   volume at HT invents the cheaper band away.
/// * A product with a **Leistungspreis** quoted without `leistung_kw` refuses:
///   the demand charge dominates an RLM offer and cannot be dropped.
/// * A **Gas** position needs a CO₂ certificate price, from the position or
///   from the dated nEHS series.
pub fn compute_cost_breakdown(
    product_data: &serde_json::Value,
    pos: &crate::pg::AngebotPositionInput,
    rabatt_pct: Option<Decimal>,
    ctx: &PricingContext,
) -> Result<PositionCostBreakdown, Unpreisbar> {
    use rubo4e::convenience::PreisstaffelSliceExt as _;
    use rubo4e::current::Preisstaffel;
    use rust_decimal::dec;

    let positionen = product_data
        .get("tarifpreise")
        .and_then(|v| v.as_array())
        .filter(|a| !a.is_empty())
        .ok_or_else(|| {
            Unpreisbar::new(
                grund::KEIN_TARIFPREISBLATT,
                format!("Produkt {} führt kein Tarifpreisblatt", pos.product_code),
            )
        })?;

    // The tiers of every Preistyp, still unselected: which quantity picks a
    // tier depends on the tariff shape, and that is known only once every
    // Preisposition has been read.
    let mut staffeln: std::collections::HashMap<&str, Vec<Preisstaffel>> =
        std::collections::HashMap::new();
    for pp in positionen {
        let pt = mako_markt::bo4e::position_preistyp(pp);
        if pt.is_empty() {
            continue;
        }
        let Some(raw) = pp.get("preisstaffeln") else {
            continue;
        };
        let parsed: Vec<Preisstaffel> = serde_json::from_value(raw.clone()).map_err(|e| {
            Unpreisbar::new(
                grund::KEIN_TARIFPREISBLATT,
                format!("{pt}: preisstaffeln sind keine BO4E-Preisstaffeln ({e})"),
            )
        })?;
        staffeln.entry(pt).or_default().extend(parsed);
    }

    let pick = |pt: &str, menge: Decimal| -> Result<Option<Decimal>, Unpreisbar> {
        let Some(tiers) = staffeln.get(pt) else {
            return Ok(None);
        };
        let staffel = tiers.select_for(menge).ok_or_else(|| {
            Unpreisbar::new(
                grund::KEINE_PREISSTAFFEL,
                format!(
                    "{pt}: keine Preisstaffel gilt für {menge} (Produkt {})",
                    pos.product_code
                ),
            )
        })?;
        match staffel.preis {
            Some(p) => Ok(Some(p)),
            None => Err(Unpreisbar::new(
                grund::KEINE_PREISSTAFFEL,
                format!("{pt}: die für {menge} geltende Preisstaffel führt keinen Preis"),
            )),
        }
    };

    let verbrauch = pos.jahresverbrauch_kwh;

    // A Zweitarif product prices two bands, so it needs two volumes. Without
    // them the whole year would be billed at HT.
    let zweitarif = staffeln.contains_key("ARBEITSPREIS_NT");
    let (ht_kwh, nt_kwh) = if zweitarif {
        match (pos.jahresverbrauch_ht_kwh, pos.jahresverbrauch_nt_kwh) {
            (Some(ht), Some(nt)) if ht + nt == verbrauch => (ht, nt),
            (Some(ht), Some(nt)) => {
                return Err(Unpreisbar::new(
                    grund::HT_NT_AUFTEILUNG_FEHLT,
                    format!(
                        "HT {ht} kWh + NT {nt} kWh ergeben nicht den Jahresverbrauch \
                         {verbrauch} kWh"
                    ),
                ));
            }
            _ => {
                return Err(Unpreisbar::new(
                    grund::HT_NT_AUFTEILUNG_FEHLT,
                    format!(
                        "Produkt {} führt einen NT-Arbeitspreis; jahresverbrauch_ht_kwh und \
                         jahresverbrauch_nt_kwh sind anzugeben",
                        pos.product_code
                    ),
                ));
            }
        }
    } else {
        (verbrauch, Decimal::ZERO)
    };

    let grundpreis_ct = pick("GRUNDPREIS", verbrauch)?;
    let mut ap_eintarif_ct = pick("ARBEITSPREIS_EINTARIF", verbrauch)?;
    if ap_eintarif_ct.is_none() {
        ap_eintarif_ct = pick("SOLAR_ARBEITSPREIS", verbrauch)?;
    }
    let ap_ht_ct = pick("ARBEITSPREIS_HT", ht_kwh)?;
    let ap_nt_ct = if zweitarif {
        pick("ARBEITSPREIS_NT", nt_kwh)?
    } else {
        None
    };

    // A Leistungspreis is tiered by kW, not by kWh, and it is the dominant cost
    // of an RLM offer — so it is selected by the position's own demand, and a
    // position that states none cannot be quoted on this product.
    let leistungspreis_ct = match (staffeln.contains_key("LEISTUNGSPREIS"), pos.leistung_kw) {
        (true, Some(kw)) => pick("LEISTUNGSPREIS", kw)?,
        (true, None) => {
            return Err(Unpreisbar::new(
                grund::LEISTUNG_FEHLT,
                format!(
                    "Produkt {} führt einen Leistungspreis; leistung_kw ist anzugeben",
                    pos.product_code
                ),
            ));
        }
        (false, _) => None,
    };

    let rabatt = rabatt_pct
        .map(|r| Decimal::ONE - r / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ONE);

    // Jahreskosten are quoted on a 365-day year: the offer prices a year of
    // supply, not a named calendar year, so the Grundpreis is not anchored to a
    // Lieferbeginn that may still move.
    let supply_gp_eur = grundpreis_ct
        .map(|gp| gp * dec!(365) / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);
    let ap_basis_ct = if let Some(ap) = ap_eintarif_ct {
        ap * verbrauch
    } else if zweitarif {
        ap_ht_ct.unwrap_or(Decimal::ZERO) * ht_kwh + ap_nt_ct.unwrap_or(Decimal::ZERO) * nt_kwh
    } else {
        ap_ht_ct.unwrap_or(Decimal::ZERO) * verbrauch
    };
    let supply_ap_eur = ap_basis_ct * rabatt / Decimal::ONE_HUNDRED;
    let supply_lp_eur = leistungspreis_ct
        .zip(pos.leistung_kw)
        .map(|(lp, kw)| lp * kw * dec!(12) / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);
    let supply_netto_eur = supply_gp_eur + supply_ap_eur + supply_lp_eur;

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
    let nne_netto_eur = nne_gp_eur + nne_ap_eur + nne_lp_eur;

    let ka_eur = pos
        .ka_ct_per_kwh
        .map(|ct| ct * pos.jahresverbrauch_kwh / Decimal::ONE_HUNDRED)
        .unwrap_or(Decimal::ZERO);

    let sparte_upper = pos.sparte.to_uppercase();
    let levies_ct_per_kwh = if sparte_upper.contains("GAS") {
        // § 2 Abs. 3 Satz 1 Nr. 4 EnergieStG: 5,50 EUR/MWh = 0,55 ct/kWh.
        let energiesteuer = pos.energiesteuer_gas_ct_per_kwh.unwrap_or(dec!(0.55));
        let behg = pos
            .behg_gas_ct_per_kwh
            .or(ctx.behg_gas_ct_per_kwh)
            .ok_or_else(|| {
                Unpreisbar::new(
                    grund::BEHG_PREIS_FEHLT,
                    "für den Stichtag liegt kein nEHS-Zertifikatspreis vor; \
                     behg_gas_ct_per_kwh ist anzugeben oder der Preis zu importieren \
                     (PUT /api/v1/nehs-prices/{datum})",
                )
            })?;
        energiesteuer + behg
    } else if sparte_upper.contains("STROM") {
        // § 3 StromStG: 20,50 EUR/MWh = 2,05 ct/kWh.
        pos.stromsteuer_ct_per_kwh.unwrap_or(dec!(2.05))
    } else {
        // Fernwärme and Wasser bear neither Strom- nor Energiesteuer on the
        // delivery itself — what is owed is taxed in the fuel upstream — so
        // nothing is added unless the caller states a rate.
        pos.stromsteuer_ct_per_kwh.unwrap_or(Decimal::ZERO)
            + pos.energiesteuer_gas_ct_per_kwh.unwrap_or(Decimal::ZERO)
            + pos.behg_gas_ct_per_kwh.unwrap_or(Decimal::ZERO)
    };
    let levies_eur = levies_ct_per_kwh * verbrauch / Decimal::ONE_HUNDRED;

    let total_netto_eur = supply_netto_eur + nne_netto_eur + ka_eur + levies_eur;
    let satz = mwst_satz(product_data, ctx)?;
    let total_brutto_eur = total_netto_eur * (Decimal::ONE + satz);

    // The rate a buyer compares on: for a Zweitarif product the volume-weighted
    // average of the bands actually quoted, not the HT rate standing in for both.
    let arbeitspreis_ct_per_kwh = if ap_eintarif_ct.is_none() && ap_ht_ct.is_none() {
        None
    } else if verbrauch.is_zero() {
        ap_eintarif_ct.or(ap_ht_ct).map(|ct| ct * rabatt)
    } else {
        Some(ap_basis_ct * rabatt / verbrauch)
    };

    Ok(PositionCostBreakdown {
        product_code: pos.product_code.clone(),
        sparte: pos.sparte.clone(),
        malo_id: pos.malo_id.clone(),
        melo_id: pos.melo_id.clone(),
        nb_mp_id: pos.nb_mp_id.clone(),
        standort_bezeichnung: pos.standort_bezeichnung.clone(),
        jahresverbrauch_kwh: verbrauch,
        supply_netto_eur: supply_netto_eur.round_kfm(2),
        nne_netto_eur: nne_netto_eur.round_kfm(2),
        ka_eur: ka_eur.round_kfm(2),
        levies_eur: levies_eur.round_kfm(2),
        total_netto_eur: total_netto_eur.round_kfm(2),
        total_brutto_eur: total_brutto_eur.round_kfm(2),
        mwst_satz: satz,
        arbeitspreis_ct_per_kwh: arbeitspreis_ct_per_kwh.map(|ct| ct.round_kfm(4)),
        arbeitspreis_ht_ct_per_kwh: ap_ht_ct.map(|ct| (ct * rabatt).round_kfm(4)),
        arbeitspreis_nt_ct_per_kwh: ap_nt_ct.map(|ct| (ct * rabatt).round_kfm(4)),
        grundpreis_eur_per_year: grundpreis_ct
            .map(|gp| (gp * dec!(365) / Decimal::ONE_HUNDRED).round_kfm(2)),
    })
}

/// One position of a quotation, priced.
struct PricedPosition {
    product_name: String,
    breakdown: PositionCostBreakdown,
}

/// Product rows already read for this request, by product code.
///
/// `None` means the catalogue holds no such product; a *failed* read is never
/// cached, so it surfaces as the transport error it is instead of as an unknown
/// product code.
type ProductCache = std::collections::HashMap<String, Option<crate::pg::ProductRow>>;

async fn ensure_cached(
    cache: &mut ProductCache,
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    product_code: &str,
) -> anyhow::Result<()> {
    if !cache.contains_key(product_code) {
        let row = fetch_product(pool, lf_mp_id, tenant, product_code, None).await?;
        cache.insert(product_code.to_owned(), row);
    }
    Ok(())
}

/// Price every position of a quotation, or refuse with a reason for each one
/// that cannot be priced.
///
/// All-or-nothing on purpose. An Angebot is a binding offer: a position whose
/// product code does not resolve, or whose price sheet does not cover the
/// quantity asked about, refuses the quotation instead of dropping out of the
/// total.
async fn price_positionen(
    pool: &PgPool,
    lf_mp_id: &str,
    tenant: &str,
    positionen: &[crate::pg::AngebotPositionInput],
    rabatt_pct: Option<Decimal>,
    ctx: &PricingContext,
    cache: &mut ProductCache,
) -> Result<Vec<PricedPosition>, Vec<PositionPricingError>> {
    let mut priced = Vec::with_capacity(positionen.len());
    let mut errors: Vec<PositionPricingError> = Vec::new();

    for (i, pos) in positionen.iter().enumerate() {
        if let Err(e) = ensure_cached(cache, pool, lf_mp_id, tenant, &pos.product_code).await {
            errors.push(Unpreisbar::new(grund::KATALOG_NICHT_ERREICHBAR, e.to_string()).at(i, pos));
            continue;
        }
        let Some(product) = cache.get(&pos.product_code).and_then(|p| p.as_ref()) else {
            errors.push(
                Unpreisbar::new(
                    grund::PRODUKT_UNBEKANNT,
                    format!(
                        "kein gültiges Produkt {} im Katalog von {lf_mp_id}",
                        pos.product_code
                    ),
                )
                .at(i, pos),
            );
            continue;
        };
        match compute_cost_breakdown(&product.data, pos, rabatt_pct, ctx) {
            Ok(breakdown) => priced.push(PricedPosition {
                product_name: product.name.clone(),
                breakdown,
            }),
            Err(u) => errors.push(u.at(i, pos)),
        }
    }

    if errors.is_empty() {
        Ok(priced)
    } else {
        Err(errors)
    }
}

/// One scenario of a comparison, before it is priced.
struct ScenarioPlan<'a> {
    label: String,
    laufzeit_monate: i16,
    ist_basis: bool,
    variante_index: Option<usize>,
    rabatt_pct: Option<Decimal>,
    /// Product codes replacing the base positions', index-aligned.
    overrides: Option<&'a Vec<Option<String>>>,
}

/// The positions of one variant: the base positions with that variant's product
/// overrides applied.
fn variant_positionen(
    positionen: &[crate::pg::AngebotPositionInput],
    overrides: Option<&Vec<Option<String>>>,
) -> Vec<crate::pg::AngebotPositionInput> {
    positionen
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut p = p.clone();
            if let Some(Some(code)) = overrides.and_then(|ov| ov.get(i)) {
                p.product_code = code.clone();
            }
            p
        })
        .collect()
}

/// Refuse a quotation that cannot be priced in full.
fn unpreisbar_response(errors: &[PositionPricingError]) -> axum::response::Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "error": "das Angebot kann nicht vollständig bepreist werden",
            "hinweis": "Ein Angebot ist ein bindendes Vertragsangebot. Eine Position, deren \
                        Preis nicht ermittelt werden kann, wird nicht übergangen — sonst \
                        stünde die Jahressumme für weniger Lieferstellen als das Angebot.",
            "unpreisbare_positionen": errors,
        })),
    )
        .into_response()
}

/// Resolve the statutory inputs a quotation is priced under for its Stichtag.
async fn pricing_context(
    pool: &PgPool,
    stichtag: time::Date,
    reverse_charge_13b: bool,
) -> PricingContext {
    let behg_gas_ct_per_kwh = match crate::pg::latest_nehs_price(pool, stichtag).await {
        Ok(Some((_, eur_per_t))) => Some(behg_erdgas_ct_per_kwh(eur_per_t)),
        Ok(None) => None,
        Err(e) => {
            tracing::error!(
                error = %e,
                "productd: nEHS-Preisreihe nicht lesbar — Gaspositionen ohne eigenen \
                 CO₂-Preis werden abgelehnt"
            );
            None
        }
    };
    PricingContext {
        behg_gas_ct_per_kwh,
        reverse_charge_13b,
    }
}

/// Default Angebot validity: today + 10 Werktage (≈ 14 calendar days).
fn default_gueltig_bis() -> time::Date {
    mako_fristen::heute() + time::Duration::days(14)
}

/// `POST /api/v1/angebote`
///
/// Create a formal B2B Angebot (quotation) for a C&I or RLM customer.
///
/// ## Price calculation
///
/// For each position, `productd` fetches the product's `Tarifpreisblatt` and
/// estimates `jahreskosten_netto_eur` from:
/// - `GRUNDPREIS` position: `ct/day × 365 / 100`
/// - `ARBEITSPREIS_EINTARIF` position: `ct/kWh × jahresverbrauch_kwh / 100`
/// - `ARBEITSPREIS_HT` / `ARBEITSPREIS_NT`: each band against its own volume
/// - Optional `rabatt_pct` from the Angebot variant
///
/// Every rate comes from the Preisstaffel that covers the position's own
/// quantity. `jahreskosten_brutto_eur` is the sum of the positions' gross
/// figures, each at its own Umsatzsteuersatz.
///
/// A position that cannot be priced — unresolvable product code, a quantity
/// outside every Preisstaffel, a Zweitarif product with no HT/NT split, a Gas
/// position with no CO₂ certificate price — refuses the whole quotation with
/// 422 and the reason per position. An Angebot is a binding offer, so a
/// position is never quietly dropped from the total.
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
/// `de.tarif.angebot.angenommen` → ERP webhook.  The ERP or `vertragd` creates the
/// `Rahmenvertrag` + `Versorgungsverträge` from the accepted Angebot data.
pub async fn post_angebot(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Json(req): Json<CreateAngebotRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-angebot", &cfg.tenant) {
        return e.into_response();
    }
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

    // Idempotency: an ERP that retries the same `erp_angebot_id` gets the
    // existing quotation back (200), never a duplicate. Without this a retry
    // storm would mint a fresh Angebotsnummer per attempt.
    if let Some(ref erp_id) = req.erp_angebot_id {
        match crate::pg::fetch_angebot_id_by_erp_id(&pool, &cfg.tenant, erp_id).await {
            Ok(Some((id, angebotsnummer))) => {
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "id": id,
                        "angebotsnummer": angebotsnummer,
                        "idempotent_replay": true,
                    })),
                )
                    .into_response();
            }
            Ok(None) => {}
            Err(e) => return ApiError::Internal(e).into_response(),
        }
    }

    // Calculate prices for each base position. The Stichtag is the proposed
    // Lieferbeginn where the quotation names one: the CO₂ certificate price it
    // is quoted at is the one in force when supply starts.
    let stichtag = lieferbeginn.unwrap_or_else(mako_fristen::heute);
    let ctx = pricing_context(&pool, stichtag, req.wiederverkaeufer_13b).await;
    let mut cache = ProductCache::new();

    let priced = match price_positionen(
        &pool,
        lf_mp_id,
        &cfg.tenant,
        &req.positionen,
        None,
        &ctx,
        &mut cache,
    )
    .await
    {
        Ok(p) => p,
        Err(errors) => return unpreisbar_response(&errors),
    };

    let total_netto: Decimal = priced.iter().map(|p| p.breakdown.total_netto_eur).sum();
    let total_brutto: Decimal = priced.iter().map(|p| p.breakdown.total_brutto_eur).sum();

    let mut enriched_positionen: Vec<serde_json::Value> = Vec::new();
    for (pos, p) in req.positionen.iter().zip(&priced) {
        let mut pos_json = serde_json::to_value(pos).unwrap_or_default();
        if let Some(obj) = pos_json.as_object_mut() {
            obj.insert("product_name".into(), serde_json::json!(p.product_name));
            obj.insert(
                "jahreskosten_netto_eur".into(),
                serde_json::json!(p.breakdown.total_netto_eur.to_string()),
            );
            obj.insert(
                "jahreskosten_brutto_eur".into(),
                serde_json::json!(p.breakdown.total_brutto_eur.to_string()),
            );
        }
        enriched_positionen.push(pos_json);
    }

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

    // Every variant is priced through the same engine as the base scenario —
    // its own discount, and its own product codes where it overrides them.
    let varianten_json: serde_json::Value = if let Some(ref vars) = req.varianten {
        let mut enriched_vars = Vec::new();
        for var in vars {
            let positionen =
                variant_positionen(&req.positionen, var.product_codes_override.as_ref());
            let var_priced = match price_positionen(
                &pool,
                lf_mp_id,
                &cfg.tenant,
                &positionen,
                var.rabatt_pct,
                &ctx,
                &mut cache,
            )
            .await
            {
                Ok(p) => p,
                Err(errors) => return unpreisbar_response(&errors),
            };
            let var_netto: Decimal = var_priced.iter().map(|p| p.breakdown.total_netto_eur).sum();
            let var_brutto: Decimal = var_priced
                .iter()
                .map(|p| p.breakdown.total_brutto_eur)
                .sum();
            let mut v = serde_json::to_value(var).unwrap_or_default();
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "jahreskosten_netto_eur".into(),
                    serde_json::json!(var_netto.to_string()),
                );
                obj.insert(
                    "jahreskosten_brutto_eur".into(),
                    serde_json::json!(var_brutto.to_string()),
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
        Err(e) => return ApiError::Internal(e).into_response(),
    };

    match insert_angebot(
        &pool,
        &cfg.tenant,
        lf_mp_id,
        &angebotsnummer,
        &req,
        &positionen_json,
        &varianten_json,
        &serde_json::json!({}), // bo4e populated on GET .../comparison, where pricing happens
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
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct AngebotListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

/// `GET /api/v1/angebote`
pub async fn list_angebote_handler(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Query(q): Query<AngebotListQuery>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-angebot", &cfg.tenant) {
        return e.into_response();
    }
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
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/angebote/{id}`
pub async fn get_angebot_handler(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-angebot", &cfg.tenant) {
        return e.into_response();
    }
    match fetch_angebot(&pool, id, &cfg.tenant).await {
        Ok(Some(a)) => Json(a).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => ApiError::Internal(e).into_response(),
    }
}

/// `GET /api/v1/angebote/{id}/comparison`
///
/// Side-by-side cost comparison for all scenarios (base + variants) in an Angebot.
///
/// Returns a `ComparisonResponse` with one `ScenarioCostBreakdown` per scenario:
/// - **Basis** — the base quotation (laufzeit from `angebote.laufzeit_monate`, no discount)
/// - **Variante 0..N** — each `AngebotVariante` with its `rabatt_pct` and/or
///   alternative products applied
///
/// Each scenario includes per-position detail:
///
/// | Field | Formula |
/// |---|---|
/// | `supply_netto_eur` | Grundpreis × 365/100 + Arbeitspreis × kWh/100 × (1−rabatt/100) + Leistungspreis × kW × 12/100 |
/// | `nne_netto_eur` | NNE Grundpreis + NNE Arbeitspreis × kWh/100 + NNE Leistungspreis × kW |
/// | `ka_eur` | KA ct/kWh × kWh/100 |
/// | `levies_eur` | Stromsteuer (§ 3 StromStG) or Energiesteuer (§ 2 Abs. 3 EnergieStG) + BEHG × kWh/100 |
/// | `total_netto_eur` | Supply + NNE + KA + Levies |
/// | `total_brutto_eur` | `total_netto_eur × (1 + mwst_satz)` |
///
/// Every Arbeitspreis is taken from the Preisstaffel that covers the position's
/// own quantity, and HT and NT are priced against their own volumes.
///
/// A scenario that cannot be priced in full — an unresolvable product code, a
/// quantity outside every Preisstaffel — is refused with 422 and the reason per
/// position, never rendered as the sum of the positions that did resolve.
///
/// The `ersparnis_vs_basis_eur` field shows the annual saving vs. the base scenario —
/// negative values mean the variant is more expensive (useful for index-linked
/// price formulas).
///
/// ## When is this useful?
///
/// C&I/RLM customers request formal Angebote with multiple scenario comparisons
/// (12M fixed vs. 24M fixed, with/without demand-side flexibility rebate). This
/// endpoint renders the comparison table that a sales engineer sends to the customer,
/// or that the B2B portal renders for self-service CPQ.
///
/// ## BO4E output
///
/// Scenarios are priced live from the current product data and the stored
/// positions. The response also carries the priced quotation as a BO4E
/// [`Angebot`](rubo4e::current::Angebot) under `bo4e`, and persists it to
/// `angebote.bo4e` — this endpoint is where pricing happens, so it is where the
/// interchange document is produced.
pub async fn get_angebot_comparison(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "read-angebot", &cfg.tenant) {
        return e.into_response();
    }
    let angebot = match fetch_angebot(&pool, id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return ApiError::Internal(e).into_response(),
    };

    let lf_mp_id = angebot.lf_mp_id.clone();

    // Deserialise positions from JSONB.
    let positionen: Vec<crate::pg::AngebotPositionInput> =
        match serde_json::from_value(angebot.positionen.clone()) {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("cannot deserialise positionen: {e}"),
                )
                    .into_response();
            }
        };

    // Unlike `positionen` above, an Angebot legitimately has no variants, so a
    // deserialization failure and an absent list are not the same thing — the
    // first is logged rather than silently rendered as the second.
    let varianten: Vec<crate::pg::AngebotVariante> = if angebot.varianten.is_null() {
        Vec::new()
    } else {
        serde_json::from_value(angebot.varianten.clone()).unwrap_or_else(|e| {
            tracing::error!(
                angebot_id = %angebot.id,
                error = %e,
                "schema drift: stored Angebot varianten do not deserialise — \
                 rendering the offer without them"
            );
            Vec::new()
        })
    };

    // The Stichtag is the proposed Lieferbeginn where the quotation names one:
    // the CO₂ certificate price it is priced at is the one in force when supply
    // starts.
    let stichtag = angebot.lieferbeginn.unwrap_or_else(mako_fristen::heute);
    let ctx = pricing_context(&pool, stichtag, angebot.wiederverkaeufer_13b).await;
    let mut cache = ProductCache::new();

    // One row per scenario: the base quotation first, then each variant with
    // its own Laufzeit, discount and product overrides.
    let mut plan: Vec<ScenarioPlan<'_>> = vec![ScenarioPlan {
        label: format!("Basis ({} Monate)", angebot.laufzeit_monate),
        laufzeit_monate: angebot.laufzeit_monate,
        ist_basis: true,
        variante_index: None,
        rabatt_pct: None,
        overrides: None,
    }];
    for (i, v) in varianten.iter().enumerate() {
        plan.push(ScenarioPlan {
            label: if v.label.is_empty() {
                format!("Variante {}", i + 1)
            } else {
                v.label.clone()
            },
            laufzeit_monate: v.laufzeit_monate,
            ist_basis: false,
            variante_index: Some(i),
            rabatt_pct: v.rabatt_pct,
            overrides: v.product_codes_override.as_ref(),
        });
    }

    let mut szenarien: Vec<ScenarioCostBreakdown> = Vec::with_capacity(plan.len());
    for row in plan {
        let effektive = variant_positionen(&positionen, row.overrides);
        let priced = match price_positionen(
            &pool,
            &lf_mp_id,
            &cfg.tenant,
            &effektive,
            row.rabatt_pct,
            &ctx,
            &mut cache,
        )
        .await
        {
            Ok(p) => p,
            Err(errors) => return unpreisbar_response(&errors),
        };
        let netto: Decimal = priced.iter().map(|p| p.breakdown.total_netto_eur).sum();
        let brutto: Decimal = priced.iter().map(|p| p.breakdown.total_brutto_eur).sum();
        szenarien.push(ScenarioCostBreakdown {
            label: row.label,
            laufzeit_monate: row.laufzeit_monate,
            ist_basis: row.ist_basis,
            variante_index: row.variante_index,
            rabatt_pct: row.rabatt_pct,
            jahreskosten_netto_eur: netto.round_kfm(2),
            jahreskosten_brutto_eur: brutto.round_kfm(2),
            ersparnis_vs_basis_eur: None,
            positionen_detail: priced.into_iter().map(|p| p.breakdown).collect(),
        });
    }

    // The saving a variant shows is measured against the base scenario, which
    // is the first row by construction.
    let base_total = szenarien[0].jahreskosten_netto_eur;
    for s in szenarien.iter_mut().skip(1) {
        s.ersparnis_vs_basis_eur = Some((base_total - s.jahreskosten_netto_eur).round_kfm(2));
    }

    // Project the priced scenarios into the BO4E `Angebot` and persist it: this
    // is the CPQ/ERP interchange payload, and pricing happens here.
    let sparte = positionen.first().map(|p| p.sparte.as_str());
    let bo4e = crate::bo4e_angebot::build_angebot(
        &angebot.angebotsnummer,
        &angebot.status,
        angebot.gueltig_bis,
        angebot.lieferbeginn,
        sparte,
        &szenarien,
    );
    match serde_json::to_value(&bo4e) {
        Ok(v) => {
            if let Err(e) = crate::pg::store_angebot_bo4e(&pool, angebot.id, &cfg.tenant, &v).await
            {
                tracing::warn!(angebot_id = %angebot.id, error = %e, "productd: BO4E persist failed");
            }
        }
        Err(e) => {
            tracing::warn!(angebot_id = %angebot.id, error = %e, "productd: BO4E encode failed")
        }
    }

    Json(serde_json::json!({
        "angebot_id":          angebot.id,
        "angebotsnummer":      angebot.angebotsnummer,
        "kunden_id":           angebot.kunden_id,
        "interessent_name":    angebot.interessent_name,
        "status":              angebot.status,
        "gueltig_bis":         angebot.gueltig_bis.to_string(),
        "lieferbeginn":        angebot.lieferbeginn.map(|d| d.to_string()),
        "positionen_count":    positionen.len(),
        "varianten_count":     varianten.len(),
        "gewaehlte_variante":  angebot.gewaehlte_variante,
        "szenarien":           szenarien,
        "bo4e":                bo4e,
        "steuerhinweis": if angebot.wiederverkaeufer_13b {
            "Steuerschuldnerschaft des Leistungsempfängers (§ 13b Abs. 2 Nr. 5 Buchst. b \
             i.V.m. Abs. 5 UStG) — die Beträge sind ohne Umsatzsteuer ausgewiesen."
        } else {
            "Die Umsatzsteuer je Position folgt dem Steuersatz des Produkts \
             (Regelsatz § 12 Abs. 1 UStG, sofern das Produkt keinen abweichenden Satz führt)."
        },
        "hinweis": "Preise sind Schätzungen auf Basis aktueller Tarifpreisblätter und eines \
                    Jahres von 365 Tagen. NNE und KA werden nur ausgewiesen wenn sie in den \
                    Positionen angegeben sind. Verbindlich ist der unterzeichnete Rahmenvertrag.",
    }))
    .into_response()
}

/// `POST /api/v1/angebote/{id}/versenden`
///
/// Mark an Angebot as VERSANDT (sent to customer).
pub async fn post_angebot_versenden(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "versenden-angebot", &cfg.tenant) {
        return e.into_response();
    }
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
/// transitions to `ANGENOMMEN` and emits `de.tarif.angebot.angenommen` to the
/// configured ERP webhook.  The ERP or `vertragd` creates the `Rahmenvertrag`
/// from the CloudEvent payload.
pub async fn post_angebot_annehmen(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<AnnehmenRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "entscheiden-angebot", &cfg.tenant) {
        return e.into_response();
    }
    let angebot = match accept_angebot(&pool, id, &cfg.tenant, req.gewaehlte_variante).await {
        Ok(a) => a,
        Err(e) => return (StatusCode::CONFLICT, e.to_string()).into_response(),
    };

    // Emit de.tarif.angebot.angenommen CloudEvent.
    if let Some(ref webhook_url) = cfg.erp_webhook_url {
        let ce = mako_service::CloudEvent::new(
            mako_service::source("productd", &cfg.tenant),
            mako_events::tarif::ANGEBOT_ANGENOMMEN,
            angebot.id.to_string(),
            serde_json::json!({
                "angebot_id": angebot.id,
                "angebotsnummer": angebot.angebotsnummer,
                "kunden_id": angebot.kunden_id,
                "interessent_name": angebot.interessent_name,
                "lf_mp_id": angebot.lf_mp_id,
                "lieferbeginn": angebot.lieferbeginn.map(|d| d.to_string()),
                "laufzeit_monate": angebot.laufzeit_monate,
                "gewaehlte_variante": angebot.gewaehlte_variante,
                // The BO4E `Angebot` for the priced quotation. Consumers build
                // the contract from the same object the customer was quoted,
                // rather than re-deriving it from the scalars below.
                "bo4e": angebot.bo4e,
                "positionen": angebot.positionen,
                "varianten": angebot.varianten,
                "jahreskosten_netto_eur": angebot.jahreskosten_netto_eur,
                "jahreskosten_brutto_eur": angebot.jahreskosten_brutto_eur,
            }),
        );
        let client = mako_service::http::default_client();
        // This emit consumes the webhook response to link the created
        // Rahmenvertrag back to the Angebot, so it POSTs directly rather than
        // via the fire-and-forget `post_ce_with_retry`. Envelope construction
        // and the Standard Webhooks headers still come from `mako_service`, so
        // this path signs exactly like every other outbound.
        let body_bytes = ce.to_bytes().unwrap_or_default();
        let mut builder = client
            .post(webhook_url)
            .header("Content-Type", "application/cloudevents+json")
            .header(mako_service::webhook::ID_HEADER, ce.id.clone());
        if let Some(ref secret) = cfg.erp_hmac_secret {
            for (name, value) in mako_service::webhook::headers(
                secret.as_bytes(),
                &ce.id,
                time::OffsetDateTime::now_utc().unix_timestamp(),
                &body_bytes,
            ) {
                builder = builder.header(name, value);
            }
        }
        if let Ok(resp) = builder.body(body_bytes).send().await
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
            "message": "Angebot angenommen — de.tarif.angebot.angenommen CloudEvent dispatched",
        })),
    )
        .into_response()
}

/// `POST /api/v1/angebote/{id}/ablehnen`
///
/// Mark an Angebot as ABGELEHNT (declined by customer).
pub async fn post_angebot_ablehnen(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "entscheiden-angebot", &cfg.tenant) {
        return e.into_response();
    }
    match decline_angebot(&pool, id, &cfg.tenant).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::CONFLICT, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/angebote/expire`  (internal maintenance endpoint)
///
/// Mark all Angebote past `gueltig_bis` as ABGELAUFEN.
/// Called by the background task; also available for manual triggers.
pub async fn post_expire_angebote(
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
) -> ApiResult<Json<serde_json::Value>> {
    authorize(&cedar, &claims, "expire-angebote", &cfg.tenant)?;
    let expired = expire_stale_angebote(&pool, &cfg.tenant)
        .await
        .map_err(ApiError::Internal)?;
    Ok(Json(serde_json::json!({ "expired": expired })))
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
    claims: Claims,
    Extension(cedar): Authz,
    Extension(pool): Extension<PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Path(id): Path<uuid::Uuid>,
    Json(req): Json<UpdateAngebotRequest>,
) -> impl IntoResponse {
    if let Err(e) = authorize(&cedar, &claims, "write-angebot", &cfg.tenant) {
        return e.into_response();
    }
    // Fetch existing Angebot and guard status.
    let existing = match fetch_angebot(&pool, id, &cfg.tenant).await {
        Ok(Some(a)) => a,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return ApiError::Internal(e).into_response(),
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

    let lf_mp_id = existing.lf_mp_id.clone();

    // Re-calculate prices if positions are being replaced.
    let (positionen_json, total_netto_opt, total_brutto_opt) =
        if let Some(ref new_pos) = req.positionen {
            let stichtag = existing.lieferbeginn.unwrap_or_else(mako_fristen::heute);
            let ctx = pricing_context(&pool, stichtag, existing.wiederverkaeufer_13b).await;
            let mut cache = ProductCache::new();
            let priced = match price_positionen(
                &pool,
                &lf_mp_id,
                &cfg.tenant,
                new_pos,
                None,
                &ctx,
                &mut cache,
            )
            .await
            {
                Ok(p) => p,
                Err(errors) => return unpreisbar_response(&errors),
            };
            let total_netto: Decimal = priced.iter().map(|p| p.breakdown.total_netto_eur).sum();
            let total_brutto: Decimal = priced.iter().map(|p| p.breakdown.total_brutto_eur).sum();

            let mut enriched: Vec<serde_json::Value> = Vec::new();
            for (pos, p) in new_pos.iter().zip(&priced) {
                let mut pj = serde_json::to_value(pos).unwrap_or_default();
                if let Some(obj) = pj.as_object_mut() {
                    obj.insert("product_name".into(), serde_json::json!(p.product_name));
                    obj.insert(
                        "jahreskosten_netto_eur".into(),
                        serde_json::json!(p.breakdown.total_netto_eur.to_string()),
                    );
                    obj.insert(
                        "jahreskosten_brutto_eur".into(),
                        serde_json::json!(p.breakdown.total_brutto_eur.to_string()),
                    );
                }
                enriched.push(pj);
            }
            let netto_opt = (total_netto > Decimal::ZERO).then_some(total_netto);
            let brutto_opt = (total_brutto > Decimal::ZERO).then_some(total_brutto);
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
        Err(e) => ApiError::from(e).into_response(),
    }
}

// ── Comparison portal feed ────────────────────────────────────────────────────

/// Extract `TarifPreise` from a product's `tarifpreise` JSONB array.
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
        .get("tarifpreise")
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
    use rust_decimal::dec;

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
    use rust_decimal::dec;
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

/// Build a `rubo4e::current::Tarifinfo` BO4E envelope from a product row.
///
/// This is the § 41c EnWG canonical form: comparison portals (Verivox, Check24)
/// and the BNetzA Markttransparenzstelle can import this object directly without
/// custom ETL, since it conforms to the published BO4E `Tarifinfo` JSON schema.
///
/// Mapping:
///
/// | BO4E field | Source |
/// |---|---|
/// | `bezeichnung` | `product.name` |
/// | `anbietername` | `lf_mp_id` |
/// | `_id` | `product.product_code` |
/// | `sparte` | `product.sparte` → `rubo4e::Sparte` |
/// | `kundentypen` | `product.kundentyp` → `[rubo4e::Kundentyp]` |
/// | `registeranzahl` | `product.register_count` → `rubo4e::Registeranzahl` |
/// | `tariftyp` | `data.tariftyp` → `rubo4e::Tariftyp` |
/// | `tarifmerkmale` | derived from preisgarantie, category, dyn_source |
/// | `energiemix` | `product.energiemix` → `rubo4e::Energiemix` |
/// | `zeitlicheGueltigkeit` | `product.valid_from/valid_to` |
/// | `vertragskonditionen` | `data.vertragskonditionen` (passed through) |
pub fn build_tarifinfo(row: &crate::pg::ProductRow, lf_mp_id: &str) -> Tarifinfo {
    use rubo4e::current::{Kundentyp, Registeranzahl, Sparte, Vertragskonditionen, Zeitraum};

    // ── Sparte ────────────────────────────────────────────────────────────────
    let sparte: Option<Sparte> = row.sparte.as_deref().and_then(|s| match s {
        "STROM" => Some(Sparte::Strom),
        "GAS" => Some(Sparte::Gas),
        "WAERME" | "FERNWAERME" => Some(Sparte::Fernwaerme),
        _ => None,
    });

    // ── Kundentypen ───────────────────────────────────────────────────────────
    let kundentypen: Option<Vec<Kundentyp>> = row.kundentyp.as_deref().map(|kt| {
        let variant = match kt {
            "Haushalt" => Kundentyp::Privat,
            "Gewerbe" | "Gewerbe_RLM" => Kundentyp::Gewerbe,
            _ => Kundentyp::Privat,
        };
        vec![variant]
    });

    // ── Registeranzahl ────────────────────────────────────────────────────────
    let registeranzahl: Option<Registeranzahl> =
        row.register_count.as_deref().and_then(|r| match r {
            "Eintarif" => Some(Registeranzahl::Eintarif),
            "Zweitarif" => Some(Registeranzahl::Zweitarif),
            "Mehrtarif" => Some(Registeranzahl::Mehrtarif),
            _ => None,
        });

    // ── Tariftyp (from data.tariftyp JSONB field) ─────────────────────────────
    let tariftyp: Option<Tariftyp> = row
        .data
        .pointer("/tariftyp")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    // ── Tarifmerkmale (derived) ────────────────────────────────────────────────
    // Derive Tarifmerkmale from observable product properties:
    //   FESTPREIS   — product defines a preisgarantie end date (price-locked)
    //   PAKET       — category is BUNDLE (multi-commodity package)
    //   ONLINE      — dyn_source set (§41a dynamic tariffs are typically online-only)
    //   STANDARD    — fallback when no other merkmal applies
    let mut merkmale: Vec<Tarifmerkmal> = Vec::new();
    let has_preisgarantie = extract_preisgarantie_bis(&row.data).is_some();
    if has_preisgarantie {
        merkmale.push(Tarifmerkmal::Festpreis);
    }
    if row.category == "BUNDLE" {
        merkmale.push(Tarifmerkmal::Paket);
    }
    if row.dyn_source.is_some() {
        merkmale.push(Tarifmerkmal::Online);
    }
    if merkmale.is_empty() {
        merkmale.push(Tarifmerkmal::Standard);
    }
    let tarifmerkmale = if merkmale.is_empty() {
        None
    } else {
        Some(merkmale)
    };

    // ── Energiemix (deserialise from JSONB) ───────────────────────────────────
    let energiemix: Option<Energiemix> = row
        .energiemix
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    // ── Zeitliche Gültigkeit ──────────────────────────────────────────────────
    // Map valid_from / valid_to DATE → Zeitraum (uses time::Date, not OffsetDateTime).
    let zeitliche_gueltigkeit: Option<Zeitraum> =
        if row.valid_from.is_some() || row.valid_to.is_some() {
            let mut z = Zeitraum::default();
            if let Some(d) = row.valid_from {
                z.startdatum = Some(d);
            }
            if let Some(d) = row.valid_to {
                z.enddatum = Some(d);
            }
            Some(z)
        } else {
            None
        };

    // ── Vertragskonditionen (pass through from data JSONB) ────────────────────
    let vertragskonditionen: Option<Vertragskonditionen> = row
        .data
        .pointer("/vertragskonditionen")
        .and_then(|v| serde_json::from_value(v.clone()).ok());

    Tarifinfo {
        id: Some(row.product_code.clone()),
        bezeichnung: Some(row.name.clone()),
        anbietername: Some(lf_mp_id.to_owned()),
        anbieter: None, // optional: could populate with Marktteilnehmer if marktd URL configured
        sparte,
        kundentypen,
        registeranzahl,
        tariftyp,
        tarifmerkmale,
        energiemix,
        zeitliche_gueltigkeit,
        vertragskonditionen,
        // `..Default::default()` stamps `_typ` and `_version` from this type's
        // own schema — the only source that cannot disagree with itself.
        ..Default::default()
    }
}

/// `GET /api/v1/comparison-feed/bo4e`
///
/// Returns the same feed as `GET /api/v1/comparison-feed` but wraps every tariff
/// in a full BO4E `Tarifinfo` Business Object — the format expected by comparison
/// portals (Verivox, Check24) and the BNetzA Markttransparenzstelle per § 41c EnWG.
///
/// Unlike the standard feed which returns a mako-specific JSON structure alongside
/// the BO4E `Tarifpreisblatt`, this endpoint returns a schema-validated BO4E array
/// that can be imported directly without custom ETL.
///
/// ## Query parameters
///
/// Identical to `GET /api/v1/comparison-feed`.
///
/// ## Response
///
/// ```json
/// {
///   "meta": { "generated_at": "...", "total_returned": 12, ... },
///   "tarife": [
///     {
///       "_typ": "TARIFINFO",
///       "_version": "202607.1.0",
///       "_id": "STROM_OEKO_2026",
///       "bezeichnung": "Ökostrom Plus",
///       "sparte": "STROM",
///       "kundentypen": ["Privat"],
///       "registeranzahl": "Eintarif",
///       "tariftyp": "SONDERTARIF",
///       "tarifmerkmale": ["FESTPREIS"],
///       "energiemix": { ... },
///       "zeitlicheGueltigkeit": { "startdatum": "2026-01-01T00:00:00Z" },
///       "vertragskonditionen": { ... }
///     }
///   ]
/// }
/// ```
pub async fn get_comparison_feed_bo4e(
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Query(q): Query<crate::pg::ComparisonFeedQuery>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    use time::format_description::well_known::Rfc3339;

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let limit = q.limit.unwrap_or(100).clamp(1, 500) as usize;

    let mut rows = match crate::pg::fetch_comparison_feed(&pool, &lf_mp_id, &q).await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(e).into_response();
        }
    };

    // ── ETag / 304 Not Modified ───────────────────────────────────────────────
    let verbrauch_kwh = q.verbrauch_kwh.unwrap_or(rust_decimal::dec!(3500));
    let etag = compute_feed_etag(&rows, verbrauch_kwh, q.sparte.as_deref());
    if let Some(inm) = req_headers
        .get("if-none-match")
        .and_then(|v| v.to_str().ok())
        && inm == etag
    {
        return StatusCode::NOT_MODIFIED.into_response();
    }

    // ── Pagination ────────────────────────────────────────────────────────────
    let has_next = rows.len() > limit;
    if has_next {
        rows.truncate(limit);
    }
    let next_cursor: Option<String> = if has_next {
        rows.last().map(|r| {
            format!(
                "{},{}",
                r.updated_at.format(&Rfc3339).unwrap_or_default(),
                r.product_code
            )
        })
    } else {
        None
    };

    // ── Build BO4E TarifInfo objects ──────────────────────────────────────────
    //
    // Not `unwrap_or(Value::Null)`: a `null` in this array is served to
    // consumers as a tariff and counted in `total_returned` like one. The feed
    // fails rather than publishing a hole in it.
    let tarife: Vec<serde_json::Value> = match rows
        .iter()
        .map(|row| mako_markt::bo4e::to_canonical_json(&build_tarifinfo(row, &lf_mp_id)))
        .collect::<Result<Vec<_>, _>>()
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "comparison feed: a TarifInfo is not serialisable");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let meta = crate::pg::ComparisonFeedMeta {
        generated_at: time::OffsetDateTime::now_utc(),
        lf_mp_id,
        verbrauch_kwh,
        sparte_filter: q.sparte.clone(),
        kundentyp_filter: q.kundentyp.clone(),
        total_returned: tarife.len(),
        next_cursor,
    };

    (
        StatusCode::OK,
        [
            ("ETag", etag.as_str()),
            ("Cache-Control", "public, max-age=300"),
            ("Content-Type", "application/json"),
            ("Vary", "Accept-Encoding"),
            ("X-Content-Type-Options", "nosniff"),
        ],
        Json(serde_json::json!({
            "meta": meta,
            "tarife": tarife,
        })),
    )
        .into_response()
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
/// `jahreskosten_supply_brutto_eur` applies the product's own Umsatzsteuersatz
/// (`mwst_satz`) to the netto estimate.
///
/// ## Pagination
///
/// The feed is ordered `(updated_at DESC, product_code ASC)`.  When
/// `meta.next_cursor` is non-null, pass it as `?cursor=<value>` to retrieve the
/// next page.  The cursor is stable: new or updated products appear on page 1
/// without affecting subsequent pages.
pub async fn get_comparison_feed(
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(cfg): Extension<std::sync::Arc<ProductdConfig>>,
    Query(q): Query<crate::pg::ComparisonFeedQuery>,
    req_headers: HeaderMap,
) -> impl IntoResponse {
    use rust_decimal::dec;
    use time::format_description::well_known::Rfc3339;

    let lf_mp_id = q.lf_mp_id.as_deref().unwrap_or(&cfg.tenant).to_owned();
    let verbrauch_kwh = q.verbrauch_kwh.unwrap_or(dec!(3500));
    let limit = q.limit.unwrap_or(100).clamp(1, 500) as usize;

    let mut rows = match crate::pg::fetch_comparison_feed(&pool, &lf_mp_id, &q).await {
        Ok(r) => r,
        Err(e) => {
            return ApiError::Internal(e).into_response();
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
    //
    // Collected as a `Result`: a `TarifInfo` that will not serialise fails the
    // feed rather than riding in it as a JSON `null` counted in
    // `total_returned` — see `mako_markt::bo4e::to_canonical_json`.
    let tarife: Vec<crate::pg::ComparisonFeedEntry> = match rows
        .iter()
        .map(|row| {
            let preise = extract_tarif_preise(&row.data);
            let netto = compute_jahreskosten_supply_netto(&preise, verbrauch_kwh);
            // The product's own Umsatzsteuersatz, not a fixed 19 %: the feed
            // and the invoice have to agree about what a household pays.
            let satz = match mwst_satz(&row.data, &FEED_PRICING_CONTEXT) {
                Ok(s) => Some(s),
                Err(u) => {
                    tracing::error!(
                        product_code = %row.product_code, detail = %u.detail,
                        "productd: Vergleichs-Feed weist für dieses Produkt keinen Bruttopreis aus"
                    );
                    None
                }
            };
            let brutto = netto
                .zip(satz)
                .map(|(n, s)| (n * (Decimal::ONE + s)).round_kfm(2));
            let netto = netto.map(|n| n.round_kfm(2));

            Ok(crate::pg::ComparisonFeedEntry {
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
                mwst_satz: satz,
                laufzeit_monate: extract_laufzeit_monate(&row.data),
                kuendigungsfrist_wochen: extract_kuendigungsfrist_wochen(&row.data),
                mindestlaufzeit_monate: extract_mindestlaufzeit_monate(&row.data),
                preisgarantie_bis: extract_preisgarantie_bis(&row.data),
                bonus_rabatt_eur: extract_bonus_rabatt_eur(&row.data),
                energiemix: row.energiemix.clone(),
                oekolabel: row.oekolabel.clone(),
                tarifpreisblatt: row.data.clone(),
                tarifinfo: mako_markt::bo4e::to_canonical_json(&build_tarifinfo(row, &lf_mp_id))?,
                updated_at: row.updated_at,
            })
        })
        .collect::<Result<Vec<_>, mako_markt::bo4e::Bo4eSerialiseError>>()
    {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "comparison feed: a TarifInfo is not serialisable");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

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

#[cfg(test)]
mod category_tests {
    use super::{PRODUCT_CATEGORIES, TARIFPREISBLATT_CATEGORIES};

    /// Three copies of the category list exist — the schema constraint that
    /// enforces it, the Rust constant the REST layer describes it with, and the
    /// MCP tool descriptions an agent reads before it calls anything. They had
    /// drifted: `WASSER` and `SHARING` were missing from the tool descriptions,
    /// so no agent could list or create either, and no test noticed because the
    /// database accepted them all along.
    #[test]
    fn the_categories_match_the_schema_constraint() {
        let sql = include_str!("../migrations/0001_schema.sql");
        let constraint = sql
            .split_once("category        TEXT    NOT NULL CHECK (category IN (")
            .expect("products.category CHECK constraint")
            .1
            .split_once("))")
            .expect("closing paren")
            .0;
        let mut from_sql: Vec<&str> = constraint
            .split(',')
            .map(|t| t.trim().trim_matches('\'').trim())
            .filter(|t| !t.is_empty())
            .collect();
        from_sql.sort_unstable();
        let mut from_rust = PRODUCT_CATEGORIES.to_vec();
        from_rust.sort_unstable();
        assert_eq!(from_sql, from_rust);
    }

    /// Every category the MCP tools name to an agent must be one the database
    /// accepts, and every category the database accepts must be named — an
    /// omission is a capability the agent plane cannot reach.
    #[test]
    fn the_mcp_descriptions_name_every_category() {
        let mcp = include_str!("mcp_server.rs");
        let piped = PRODUCT_CATEGORIES.join("|");
        let slashed = PRODUCT_CATEGORIES.join("/");
        let listings = mcp.matches("STROM|").count() + mcp.matches("STROM/").count();
        assert!(listings > 0, "no category listing found in mcp_server.rs");
        assert_eq!(
            mcp.matches(piped.as_str()).count() + mcp.matches(slashed.as_str()).count(),
            listings,
            "an MCP category listing does not name all {} categories",
            PRODUCT_CATEGORIES.len()
        );
    }

    /// A BO4E-envelope category must be a real category.
    #[test]
    fn the_tarifpreisblatt_categories_are_a_subset() {
        for category in TARIFPREISBLATT_CATEGORIES {
            assert!(
                PRODUCT_CATEGORIES.contains(category),
                "{category} carries a Tarifpreisblatt but is not an accepted category"
            );
        }
    }
}
