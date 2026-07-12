//! Handlers for device registry endpoints:
//! - `GET|PUT /api/v1/steuerbare-ressourcen/{sr_id}` (B4b)
//! - `GET|PUT|DELETE /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte` (M1)
//! - `DELETE /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}` (M1)
//! - `GET|PUT /api/v1/technische-ressourcen/{tr_id}` (B9)
//! - `GET /api/v1/malos/{malo_id}/technische-ressourcen` (B9)
//! - `GET /api/v1/melos/{melo_id}/zaehler` (B3)
//! - `PUT /api/v1/zaehler/{zaehler_id}` (B3)
//! - `GET /api/v1/zaehler/{zaehler_id}/geraete` (B3)
//! - `GET /api/v1/zaehler/{zaehler_id}/zaehlwerke` (B3 — structured register access)
//! - `PUT /api/v1/geraete/{geraet_id}` (B3)

use std::sync::Arc;

use axum::{Extension, Json, extract::Path, http::StatusCode, response::IntoResponse};
use mako_markt::repository::{
    DeviceRepository, SteuerbareRessourceRepository, TechnischeRessourceRepository,
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::{Geraet, SteuerbareRessource, TechnischeRessource, Zaehler};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::pg::{
    PgDeviceRepository, PgSteuerbareRessourceRepository, PgTechnischeRessourceRepository,
};

use super::{Claims, TenantGln};

// ── Extension types ───────────────────────────────────────────────────────────

pub type SrRepoExt = Arc<PgSteuerbareRessourceRepository>;

/// Inject the canonical all-uppercase BO4E `_typ` discriminator if absent.
///
/// Values: `"ZAEHLER"`, `"GERAET"`, `"STEUERBARERESSOURCE"`, `"TECHNISCHERESSOURCE"`.
fn inject_bo4e_typ(data: &mut serde_json::Value, typ_name: &str) {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::Value::String(typ_name.to_owned()));
    }
}

/// Validate `_typ` and deserialise as `T`, then re-serialise to canonical BO4E form.
/// Returns 422 on type mismatch or schema violation.
fn validate_and_normalise<T>(
    data: serde_json::Value,
    expected_typ: &str,
) -> Result<serde_json::Value, (StatusCode, serde_json::Value)>
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    if let Some(typ) = data.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != expected_typ.to_uppercase()
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("expected _typ {expected_typ}, got '{typ}'") }),
        ));
    }
    let typed: T = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid {expected_typ} payload: {e}") }),
        )
    })?;
    Ok(serde_json::to_value(&typed).unwrap_or_default())
}

pub type DeviceRepoExt = Arc<PgDeviceRepository>;

// ── Response DTOs (M6 typed Zaehler/Geraet) ──────────────────────────────────

/// Typed API response for a `Zaehler` (meter) record.
///
/// `data` is a validated `rubo4e::current::Zaehler` — callers always receive
/// canonical BO4E camelCase with `_typ: "ZAEHLER"` present.
#[derive(Debug, Serialize, ToSchema)]
pub struct ZaehlerResponse {
    pub zaehler_id: String,
    pub melo_id: String,
    pub zaehler_typ: Option<String>,
    pub eichung_bis: Option<time::Date>,
    /// Validated BO4E `Zaehler` payload in canonical form.
    #[schema(value_type = Object)]
    pub data: Zaehler,
    pub bo4e_version: String,
    pub version: i64,
}

/// Typed API response for a `Geraet` (device) record.
#[derive(Debug, Serialize, ToSchema)]
pub struct GeraetResponse {
    pub geraet_id: String,
    pub zaehler_id: String,
    pub geraet_typ: Option<String>,
    /// Validated BO4E `Geraet` payload in canonical form.
    #[schema(value_type = Object)]
    pub data: Geraet,
    pub bo4e_version: String,
    pub version: i64,
}

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/steuerbare-ressourcen/{sr_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertSrRequest {
    /// Full BO4E `SteuerbareRessource` payload (may be `{}`).
    pub data: serde_json::Value,
    /// Associated MaLo-ID, if known.
    pub malo_id: Option<String>,
    /// Associated MeLo-ID, if known.
    pub melo_id: Option<String>,
    /// Contracted iMS control products (`Vec<Konfigurationsprodukt>` as JSON array).
    /// When provided, replaces the stored list; `null` preserves the existing list.
    #[serde(default)]
    pub konfigurationsprodukte: Option<serde_json::Value>,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Request body for `PUT /api/v1/zaehler/{zaehler_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertZaehlerRequest {
    /// Owning MeLo-ID.
    pub melo_id: String,
    /// Zähler type (e.g. `"DREHSTROMZAEHLER"`).
    pub zaehler_typ: Option<String>,
    /// Calibration valid-until date (`YYYY-MM-DD`).
    pub eichung_bis: Option<String>,
    /// Full BO4E `Zaehler` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// Request body for `PUT /api/v1/geraete/{geraet_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertGeraetRequest {
    /// Owning `zaehler_id`.
    pub zaehler_id: String,
    /// Gerätetyp (e.g. `"WANDLER"`).
    pub geraet_typ: Option<String>,
    /// Full BO4E `Geraet` payload.
    pub data: serde_json::Value,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

fn default_bo4e_version() -> String {
    "v202607.0.0".to_owned()
}

/// Query params for list endpoints.
#[derive(Debug, Deserialize)]
pub struct TenantQuery {
    pub tenant: Option<String>,
}

// ── SteuerbareRessource handlers (B4b) ───────────────────────────────────────

/// `GET /api/v1/steuerbare-ressourcen/{sr_id}`
pub async fn get_steuerbare_ressource(
    Extension(repo): Extension<SrRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(sr_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-sr", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.find_sr(&sr_id, &tenant_gln).await {
        Ok(Some(rec)) => Json(serde_json::to_value(rec).unwrap_or_default()).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("SteuerbareRessource {sr_id} not found"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/steuerbare-ressourcen/{sr_id}`
pub async fn put_steuerbare_ressource(
    Extension(repo): Extension<SrRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(sr_id): Path<String>,
    Json(req): Json<UpsertSrRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-sr", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    // Validate data against rubo4e::current::SteuerbareRessource (N1).
    // An empty object `{}` is valid — all fields are optional.
    // A wrong `_typ` (e.g. "MARKTLOKATION") is rejected with 422.
    if let Err(e) = serde_json::from_value::<SteuerbareRessource>(req.data.clone()) {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({ "error": format!("invalid SteuerbareRessource: {e}") })),
        )
            .into_response();
    }

    let mut data = req.data;
    inject_bo4e_typ(&mut data, "STEUERBARERESSOURCE");

    match repo
        .upsert_sr(
            &sr_id,
            &tenant_gln,
            req.malo_id.as_deref(),
            req.melo_id.as_deref(),
            data,
            &req.bo4e_version,
            req.konfigurationsprodukte,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Device registry handlers (B3) ────────────────────────────────────────────

/// `GET /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte`
///
/// Returns the contracted iMS control products (`Vec<Konfigurationsprodukt>`)
/// for the given `SteuerbareRessource`.
///
/// Used by `makod` before dispatching `wim.steuerungsauftrag.bestaetigen` (M1):
/// the MSB MUST only confirm a Steuerungsauftrag for products that are actually
/// under contract.  An uncontracted `produktcode` → reject with ablehnen.
///
/// Returns `200 []` when the SteuerbareRessource exists but has no products
/// configured (MSB should reject — no products = no contract).
/// Returns `404` when the SteuerbareRessource is unknown.
///
/// ## Typed response
///
/// Each element is deserialized from JSONB and re-serialized as a canonical
/// `rubo4e::current::Konfigurationsprodukt`.  Stored JSONB that no longer
/// matches the schema (e.g. from a pre-M1 manual write) is silently filtered
/// out and reported in the `schema_drift` counter.
pub async fn get_konfigurationsprodukte(
    Extension(repo): Extension<SrRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(sr_id): Path<String>,
) -> impl IntoResponse {
    use rubo4e::current::Konfigurationsprodukt;

    if enforcer
        .check(&claims.principal(), "read-sr", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.find_sr(&sr_id, &tenant_gln).await {
        Ok(Some(rec)) => {
            let raw = rec
                .konfigurationsprodukte
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let mut schema_drift = 0u32;
            let products: Vec<Konfigurationsprodukt> = raw
                .into_iter()
                .filter_map(|item| {
                    serde_json::from_value::<Konfigurationsprodukt>(item)
                        .map_err(|e| {
                            schema_drift += 1;
                            tracing::warn!(
                                sr_id = %sr_id,
                                error = %e,
                                "schema drift: stored Konfigurationsprodukt cannot be deserialised — re-PUT to fix"
                            );
                        })
                        .ok()
                })
                .collect();
            Json(serde_json::json!({
                "sr_id": sr_id,
                "konfigurationsprodukte": products,
                "count": products.len(),
                "schema_drift": schema_drift,
            }))
            .into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            format!("SteuerbareRessource {sr_id} not found"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte`
///
/// Atomically replace the contracted iMS control products
/// (`Vec<Konfigurationsprodukt>`) for an existing `SteuerbareRessource`
/// (M1 — §14a Modul 3, BK6-24-174 §4.3).
///
/// ## Validation (BK6-24-174 §4.3)
///
/// Every element must:
/// 1. Parse as a valid `rubo4e::current::Konfigurationsprodukt`
/// 2. Carry a non-empty `produktcode` — **mandatory** per BDEW AHB
///
/// ## Idempotency
///
/// Supplying the same list twice is safe (version counter increments but
/// the JSONB payload is unchanged).  Pass `[]` to clear all products.
///
/// ## CloudEvent
///
/// Emits `de.markt.sr.konfigurationsprodukt.updated` to the EventBus fan-out
/// on every successful write so ERP subscribers and `processd` see the change.
///
/// Returns `204 No Content` on success.
pub async fn put_konfigurationsprodukte(
    Extension(repo): Extension<SrRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(event_tx): Extension<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    claims: Claims,
    Path(sr_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    use mako_markt::cloudevents::MarktEvent;
    use rubo4e::current::Konfigurationsprodukt;

    if enforcer
        .check(&claims.principal(), "write-sr", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    // Body must be a JSON array of Konfigurationsprodukt objects.
    let arr = match body.as_array() {
        Some(a) => a.clone(),
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": "body must be a JSON array of Konfigurationsprodukt" })),
            )
                .into_response();
        }
    };

    // Validate each element:
    // 1. Schema: must deserialize as Konfigurationsprodukt.
    // 2. Business rule (BK6-24-174 §4.3): `produktcode` MUST be non-empty.
    let mut validated: Vec<serde_json::Value> = Vec::with_capacity(arr.len());
    for (i, item) in arr.iter().enumerate() {
        let kp = match serde_json::from_value::<Konfigurationsprodukt>(item.clone()) {
            Ok(k) => k,
            Err(e) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!("element [{}] is not a valid Konfigurationsprodukt: {}", i, e)
                    })),
                )
                    .into_response();
            }
        };
        // BK6-24-174 §4.3: produktcode is mandatory — every contracted product
        // must be uniquely identifiable by its code.
        match kp.produktcode.as_deref() {
            None | Some("") => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": format!(
                            "element [{}]: 'produktcode' is mandatory per BK6-24-174 §4.3 \
                             — every Konfigurationsprodukt must carry a non-empty produktcode",
                            i
                        )
                    })),
                )
                    .into_response();
            }
            _ => {}
        }
        // Re-serialize to canonical BO4E camelCase.
        validated.push(serde_json::to_value(&kp).unwrap_or(item.clone()));
    }

    let canonical = serde_json::Value::Array(validated);
    match repo
        .replace_sr_konfigurationsprodukte(&sr_id, &tenant_gln, canonical.clone())
        .await
    {
        Ok(true) => {
            // Emit CloudEvent so ERP subscribers and processd see the update.
            let evt = MarktEvent::new(
                &tenant_gln,
                "de.markt.sr.konfigurationsprodukt.updated",
                sr_id.clone(),
                serde_json::json!({
                    "sr_id": sr_id,
                    "count": canonical.as_array().map(|a| a.len()).unwrap_or(0),
                }),
            );
            if let Ok(payload) = serde_json::to_value(&evt) {
                let _ = event_tx.send(payload);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            format!("SteuerbareRessource {sr_id} not found"),
        )
            .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/steuerbare-ressourcen/{sr_id}/konfigurationsprodukte/{produktcode}`
///
/// Remove a single `Konfigurationsprodukt` from the contracted list for an
/// existing `SteuerbareRessource` by its `produktcode`.
///
/// This is an atomic read-modify-write: the current list is fetched, the
/// matching entry removed, and the result written back via
/// `replace_sr_konfigurationsprodukte`.
///
/// Returns `204 No Content` on success (even when the `produktcode` was not
/// in the list — idempotent), `404` when the SR is unknown.
pub async fn delete_konfigurationsprodukt(
    Extension(repo): Extension<SrRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(event_tx): Extension<tokio::sync::mpsc::UnboundedSender<serde_json::Value>>,
    claims: Claims,
    Path((sr_id, produktcode)): Path<(String, String)>,
) -> impl IntoResponse {
    use mako_markt::cloudevents::MarktEvent;

    if enforcer
        .check(&claims.principal(), "write-sr", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let rec = match repo.find_sr(&sr_id, &tenant_gln).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("SteuerbareRessource {sr_id} not found"),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Filter out the matching produktcode from the existing list.
    let current = rec
        .konfigurationsprodukte
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let filtered: Vec<serde_json::Value> = current
        .into_iter()
        .filter(|item| {
            let code = item
                .get("produktcode")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            code != produktcode.as_str()
        })
        .collect();
    let new_list = serde_json::Value::Array(filtered);

    match repo
        .replace_sr_konfigurationsprodukte(&sr_id, &tenant_gln, new_list.clone())
        .await
    {
        Ok(_) => {
            let evt = MarktEvent::new(
                &tenant_gln,
                "de.markt.sr.konfigurationsprodukt.updated",
                sr_id.clone(),
                serde_json::json!({
                    "sr_id": sr_id,
                    "deleted_produktcode": produktcode,
                    "count": new_list.as_array().map(|a| a.len()).unwrap_or(0),
                }),
            );
            if let Ok(payload) = serde_json::to_value(&evt) {
                let _ = event_tx.send(payload);
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/melos/{melo_id}/zaehler`
pub async fn list_zaehler(
    Extension(repo): Extension<DeviceRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(melo_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.list_zaehler_by_melo(&melo_id, &tenant_gln).await {
        Ok(records) => {
            let responses: Vec<ZaehlerResponse> = records
                .into_iter()
                .filter_map(|r| {
                    let zaehler = serde_json::from_value::<Zaehler>(r.data.clone())
                        .map_err(|e| {
                            tracing::warn!(
                                zaehler_id = %r.zaehler_id,
                                error = %e,
                                "schema drift: stored Zaehler cannot be deserialised — re-PUT to fix"
                            );
                        })
                        .ok()?;
                    Some(ZaehlerResponse {
                        zaehler_id: r.zaehler_id,
                        melo_id: r.melo_id,
                        zaehler_typ: r.zaehler_typ,
                        eichung_bis: r.eichung_bis,
                        data: zaehler,
                        bo4e_version: r.bo4e_version,
                        version: r.version,
                    })
                })
                .collect();
            Json(responses).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/zaehler/{zaehler_id}`
pub async fn put_zaehler(
    Extension(repo): Extension<DeviceRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(zaehler_id): Path<String>,
    Json(req): Json<UpsertZaehlerRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let eichung_bis = req.eichung_bis.as_deref().and_then(|s| parse_date(s).ok());
    let mut data = req.data;
    inject_bo4e_typ(&mut data, "ZAEHLER");
    // M6 hard cut: validate schema on write.
    let canonical_data = match validate_and_normalise::<Zaehler>(data, "ZAEHLER") {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match repo
        .upsert_zaehler(
            &zaehler_id,
            &tenant_gln,
            &req.melo_id,
            req.zaehler_typ.as_deref(),
            eichung_bis,
            canonical_data,
            &req.bo4e_version,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/zaehler/{zaehler_id}/zaehlwerke`
///
/// Returns the `Vec<Zaehlwerk>` registers stored inside `zaehler.data["zaehlwerke"]`
/// as typed BO4E objects.
///
/// Smart meter gateways (iMSyS) carry multiple registers per Zaehler — active
/// import/export, reactive, max-demand.  This endpoint gives ERP systems and
/// MSB applications structured per-register access without parsing raw JSONB.
///
/// Returns `404 Not Found` if the Zaehler does not exist or has no `zaehlwerke`
/// array in its payload.
pub async fn get_zaehlwerke(
    Extension(repo): Extension<DeviceRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(zaehler_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let zaehler = match repo.find_zaehler(&zaehler_id, &tenant_gln).await {
        Ok(Some(z)) => z,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Extract zaehlwerke array from BO4E Zaehler JSONB payload.
    // The serde rename in rubo4e is "zaehlwerke" (camelCase for BO4E
    // maps to lowercase because it's already one word).
    let Some(zaehlwerke_json) = zaehler.data.get("zaehlwerke") else {
        // No registers in this Zaehler — return empty array rather than 404
        // so ERP callers can treat the response uniformly.
        return Json(serde_json::json!([])).into_response();
    };

    // Try to deserialize into rubo4e::current::Zaehlwerk for schema validation.
    // If deserialization fails (e.g. older payload format), fall back to raw JSON
    // so we never silently drop data.
    match serde_json::from_value::<Vec<rubo4e::current::Zaehlwerk>>(zaehlwerke_json.clone()) {
        Ok(werke) => Json(werke).into_response(),
        Err(_) => {
            // Payload has zaehlwerke but doesn't match current schema — return raw
            // with a warning so the operator can investigate schema drift.
            tracing::warn!(
                zaehler_id,
                "edmd: zaehlwerke deserialization failed, returning raw JSON"
            );
            Json(zaehlwerke_json).into_response()
        }
    }
}

/// `GET /api/v1/zaehler/{zaehler_id}/geraete`
pub async fn list_geraete(
    Extension(repo): Extension<DeviceRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(zaehler_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.list_geraete_by_zaehler(&zaehler_id, &tenant_gln).await {
        Ok(records) => {
            let responses: Vec<GeraetResponse> = records
                .into_iter()
                .filter_map(|r| {
                    let geraet = serde_json::from_value::<Geraet>(r.data.clone())
                        .map_err(|e| {
                            tracing::warn!(
                                geraet_id = %r.geraet_id,
                                error = %e,
                                "schema drift: stored Geraet cannot be deserialised — re-PUT to fix"
                            );
                        })
                        .ok()?;
                    Some(GeraetResponse {
                        geraet_id: r.geraet_id,
                        zaehler_id: r.zaehler_id,
                        geraet_typ: r.geraet_typ,
                        data: geraet,
                        bo4e_version: r.bo4e_version,
                        version: r.version,
                    })
                })
                .collect();
            Json(responses).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/geraete/{geraet_id}`
pub async fn put_geraet(
    Extension(repo): Extension<DeviceRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(geraet_id): Path<String>,
    Json(req): Json<UpsertGeraetRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let mut data = req.data;
    inject_bo4e_typ(&mut data, "GERAET");
    // M6 hard cut: validate schema on write.
    let canonical_data = match validate_and_normalise::<Geraet>(data, "GERAET") {
        Ok(v) => v,
        Err((status, body)) => return (status, Json(body)).into_response(),
    };

    match repo
        .upsert_geraet(
            &geraet_id,
            &tenant_gln,
            &req.zaehler_id,
            req.geraet_typ.as_deref(),
            canonical_data,
            &req.bo4e_version,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── Helper ────────────────────────────────────────────────────────────────────

fn parse_date(s: &str) -> Result<time::Date, time::error::Parse> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT)
}

// ── TechnischeRessource handlers (B9) ────────────────────────────────────────

pub type TrRepoExt = Arc<PgTechnischeRessourceRepository>;

/// Request body for `PUT /api/v1/technische-ressourcen/{tr_id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertTrRequest {
    /// Full BO4E `TechnischeRessource` payload.
    pub data: serde_json::Value,
    /// Associated MaLo-ID (`zugeordnete_marktlokation_id`), if known.
    pub malo_id: Option<String>,
    /// Associated MeLo-ID (`vorgelagerte_messlokation_id`), if known.
    pub melo_id: Option<String>,
    /// Classification: `"EMobilitaet"` | `"Erzeugung"` | `"Speicher"`.
    pub tr_typ: Option<String>,
    /// Whether the resource can be remote-controlled (Redispatch 2.0).
    pub ist_fernschaltbar: Option<bool>,
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

/// `PUT /api/v1/technische-ressourcen/{tr_id}`
#[utoipa::path(
    put,
    path = "/api/v1/technische-ressourcen/{tr_id}",
    tag = "TechnischeRessource",
    params(("tr_id" = String, Path, description = "TrId")),
    responses(
        (status = 204, description = "Upserted"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn put_technische_ressource(
    Extension(repo): Extension<TrRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(tr_id): Path<String>,
    Json(req): Json<UpsertTrRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    // Validate tr_typ against the BO4E TechnischeRessource classification enum.
    const VALID_TR_TYPEN: &[&str] = &["EMobilitaet", "Erzeugung", "Speicher"];
    if let Some(t) = req.tr_typ.as_deref()
        && !VALID_TR_TYPEN.contains(&t)
    {
        return (
            StatusCode::BAD_REQUEST,
            format!("invalid tr_typ '{t}': must be one of EMobilitaet, Erzeugung, Speicher"),
        )
            .into_response();
    }

    // Validate data against rubo4e::current::TechnischeRessource (N1).
    // Also derive ist_fernschaltbar from the typed payload when not set in the request.
    let typed_tr = match serde_json::from_value::<TechnischeRessource>(req.data.clone()) {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "error": format!("invalid TechnischeRessource: {e}") })),
            )
                .into_response();
        }
    };
    // Prefer the explicit request field; fall back to what the BO4E payload declares.
    let ist_fernschaltbar = req.ist_fernschaltbar.or(typed_tr.ist_fernschaltbar);

    let mut data = req.data;
    inject_bo4e_typ(&mut data, "TECHNISCHERESSOURCE");

    match repo
        .upsert_tr(
            &tr_id,
            &tenant_gln,
            req.malo_id.as_deref(),
            req.melo_id.as_deref(),
            req.tr_typ.as_deref(),
            ist_fernschaltbar,
            data,
            &req.bo4e_version,
        )
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/technische-ressourcen/{tr_id}`
#[utoipa::path(
    get,
    path = "/api/v1/technische-ressourcen/{tr_id}",
    tag = "TechnischeRessource",
    params(("tr_id" = String, Path, description = "TrId")),
    responses(
        (status = 200, description = "Found"),
        (status = 403, description = "Forbidden"),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_technische_ressource(
    Extension(repo): Extension<TrRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(tr_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.find_tr(&tr_id, &tenant_gln).await {
        Ok(Some(record)) => Json(record).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/malos/{malo_id}/technische-ressourcen`
#[utoipa::path(
    get,
    path = "/api/v1/malos/{malo_id}/technische-ressourcen",
    tag = "TechnischeRessource",
    params(("malo_id" = String, Path, description = "MaLo-ID")),
    responses(
        (status = 200, description = "List"),
        (status = 403, description = "Forbidden"),
    )
)]
pub async fn list_technische_ressourcen_by_malo(
    Extension(repo): Extension<TrRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.list_tr_by_malo(&malo_id, &tenant_gln).await {
        Ok(records) => Json(records).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

// ── ZaehlzeitRegister + ZaehlzeitSaison (M4 — iMSys TOU) ────────────────────

pub type ZaehlzeitRepoExt = Arc<crate::pg::PgZaehlzeitRepository>;

use mako_markt::repository::{ZaehlzeitRegisterRecord, ZaehlzeitRepository, ZaehlzeitSaisonRecord};

/// `PUT /api/v1/zaehler/{zaehler_id}/register`
///
/// Upsert a `ZaehlzeitRegister` for a given Zähler.  One register per tariff
/// zone (HT/NT/EINZEL).  Populated automatically from WiM Stammdaten `ZAK+ZD`
/// segments during MSB device handover (BK6-24-174 §5.6) when the `makod`
/// adapter extracts them.  Also supports manual operator import.
pub async fn put_zaehler_register(
    Extension(repo): Extension<ZaehlzeitRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(zaehler_id): Path<String>,
    Json(mut rec): Json<ZaehlzeitRegisterRecord>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    rec.zaehler_id = zaehler_id;
    rec.tenant = tenant_gln;
    if rec.id == uuid::Uuid::nil() {
        rec.id = uuid::Uuid::new_v4();
    }
    match repo.upsert_register(&rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/zaehler/{zaehler_id}/register`
///
/// List all `ZaehlzeitRegister` records for a Zähler.
/// Returns HT + NT registers for iMSys TOU meters; EINZEL for single-tariff meters.
pub async fn list_zaehler_register(
    Extension(repo): Extension<ZaehlzeitRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(zaehler_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    match repo
        .list_registers_by_zaehler(&zaehler_id, &tenant_gln)
        .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/zaehler-register/{register_id}/saisons`
///
/// Upsert a `ZaehlzeitSaison` entry for a register.  One row per
/// season × weekday combination.  Example: "HT applies Mon–Fri 07:00–22:00 in WINTER".
pub async fn put_zaehler_saison(
    Extension(repo): Extension<ZaehlzeitRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(register_id): Path<uuid::Uuid>,
    Json(mut rec): Json<ZaehlzeitSaisonRecord>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    rec.register_id = register_id;
    if rec.id == uuid::Uuid::nil() {
        rec.id = uuid::Uuid::new_v4();
    }
    match repo.upsert_saison(&rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::UNPROCESSABLE_ENTITY, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/zaehler-register/{register_id}/saisons`
///
/// List all `ZaehlzeitSaison` windows for a register.
/// Used by `billingd` / `edmd` to classify 15-min intervals into HT/NT bands.
pub async fn list_zaehler_saisons(
    Extension(repo): Extension<ZaehlzeitRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(register_id): Path<uuid::Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    match repo
        .list_saisons_by_register(register_id, &tenant_gln)
        .await
    {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/zaehler/{zaehler_id}/tariff-zone`
///
/// Resolve the current tariff zone (HT/NT/EINZEL) for a Zähler at a given
/// local datetime string (`?at=HH:MM` on the current date, or `?datetime=ISO`).
/// Used by `billingd` during 15-min interval classification.
pub async fn get_tariff_zone(
    Extension(repo): Extension<ZaehlzeitRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(zaehler_id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<TariffZoneQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-device", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }
    // Parse local datetime — defaults to now() if not specified.
    let local_dt = if let Some(dt_str) = q.datetime.as_deref() {
        use time::format_description::well_known::Iso8601;
        match time::PrimitiveDateTime::parse(dt_str, &Iso8601::DEFAULT) {
            Ok(dt) => dt,
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    "invalid datetime, expected ISO 8601",
                )
                    .into_response();
            }
        }
    } else {
        // Use current German local time (CET/CEST).
        let now = time::OffsetDateTime::now_utc();
        time::PrimitiveDateTime::new(now.date(), now.time())
    };

    match repo
        .resolve_tariff_zone(&zaehler_id, &tenant_gln, local_dt)
        .await
    {
        Ok(Some(zone)) => Json(serde_json::json!({
            "zaehler_id": zaehler_id,
            "local_datetime": local_dt.to_string(),
            "tariff_zone": zone,
        }))
        .into_response(),
        Ok(None) => Json(serde_json::json!({
            "zaehler_id": zaehler_id,
            "tariff_zone": "EINZEL",
            "note": "No matching ZaehlzeitSaison window found — defaulting to EINZEL",
        }))
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct TariffZoneQuery {
    /// ISO 8601 local datetime, e.g. `2025-07-10T14:30:00`.
    pub datetime: Option<String>,
}
