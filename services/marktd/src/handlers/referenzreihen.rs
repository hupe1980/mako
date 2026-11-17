//! Routes of the Referenzreihen — see [`crate::referenzreihen`].

use std::sync::Arc;

use axum::{
    Extension,
    body::Bytes,
    extract::{Path, Query},
};
use mako_markt::referenzreihen::{Referenzwert, Zeitraum};
use mako_service::cedar::CedarEnforcer;
use mako_service::{ApiError, ApiResult, Json};
use sqlx::PgPool;

use super::{Claims, Tenant};
use crate::referenzreihen::{
    ImportAntwort, ImportQuery, MAX_WERTE, ReiheMitStand, WertQuery, WerteQuery, importiere,
    katalog, reihe_of, wert_fuer,
};

/// `GET /api/v1/referenzreihen`
#[utoipa::path(
    get,
    path = "/api/v1/referenzreihen",
    tag = "referenzreihen",
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
    )
)]
pub async fn list_referenzreihen(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
) -> ApiResult<Json<Vec<ReiheMitStand>>> {
    enforcer
        .check(&claims.principal(), "read-referenzreihen", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    katalog(&pool).await.map(Json)
}

/// `GET /api/v1/referenzreihen/{reihe}/werte`
#[utoipa::path(
    get,
    path = "/api/v1/referenzreihen/{reihe}/werte",
    tag = "referenzreihen",
    params(("reihe" = String, Path, description = "Referenzreihe name"), WerteQuery),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, description = "Not found", body = ApiError),
    )
)]
pub async fn get_werte(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(reihe): Path<String>,
    Query(q): Query<WerteQuery>,
) -> ApiResult<Json<Vec<Referenzwert>>> {
    enforcer
        .check(&claims.principal(), "read-referenzreihen", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let reihe = reihe_of(&reihe)?;
    let von = q.von.map(mako_fristen::berlin_midnight);
    let bis = q
        .bis
        .map(|d| mako_fristen::berlin_midnight(d.next_day().unwrap_or(d)));
    let limit = q.limit.unwrap_or(MAX_WERTE).clamp(1, MAX_WERTE);
    crate::pg::referenzreihen::werte(&pool, reihe, q.dim.as_deref(), von, bis, q.stand, limit)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}

/// `GET /api/v1/referenzreihen/{reihe}/wert`
#[utoipa::path(
    get,
    path = "/api/v1/referenzreihen/{reihe}/wert",
    tag = "referenzreihen",
    params(("reihe" = String, Path, description = "Referenzreihe name"), WerteQuery),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 404, description = "Not found", body = ApiError),
    )
)]
pub async fn get_wert(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(reihe): Path<String>,
    Query(q): Query<WertQuery>,
) -> ApiResult<Json<Referenzwert>> {
    enforcer
        .check(&claims.principal(), "read-referenzreihen", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let reihe = reihe_of(&reihe)?;
    let zeitraum = match q.monat {
        None => Zeitraum::Jahr { jahr: q.jahr },
        Some(monat) => Zeitraum::Monat {
            jahr: q.jahr,
            monat,
        },
    };
    wert_fuer(&pool, reihe, zeitraum, &q.dim, q.stand)
        .await
        .map(Json)
}

/// `POST /api/v1/referenzreihen/{reihe}/import`
#[utoipa::path(
    post,
    path = "/api/v1/referenzreihen/{reihe}/import",
    tag = "referenzreihen",
    params(("reihe" = String, Path, description = "Referenzreihe name"), ImportQuery),
    request_body(content = String, content_type = "application/octet-stream", description = "The published file itself, in the `format` the query names"),
    responses(
        (status = 200, description = "OK", body = serde_json::Value),
        (status = 401, description = "No valid credentials", body = ApiError),
        (status = 403, description = "Cedar refused the action", body = ApiError),
        (status = 422, description = "The request was refused; `code` names the rule", body = ApiError),
    )
)]
pub async fn post_import(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(pool): Extension<PgPool>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(reihe): Path<String>,
    Query(q): Query<ImportQuery>,
    body: Bytes,
) -> ApiResult<Json<ImportAntwort>> {
    enforcer
        .check(&claims.principal(), "import-referenzreihen", &tenant)
        .map_err(|e| ApiError::forbidden(e.to_string()))?;
    let reihe = reihe_of(&reihe)?;
    importiere(&pool, reihe, &q, &body).await.map(Json)
}
