//! Handlers for device registry endpoints:
//! - `GET|PUT /api/v1/steuerbare-ressourcen/{sr_id}` (B4b)
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
use rubo4e::current::{Geraet, Zaehler};
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

    let mut data = req.data;
    inject_bo4e_typ(&mut data, "TECHNISCHERESSOURCE");

    match repo
        .upsert_tr(
            &tr_id,
            &tenant_gln,
            req.malo_id.as_deref(),
            req.melo_id.as_deref(),
            req.tr_typ.as_deref(),
            req.ist_fernschaltbar,
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
