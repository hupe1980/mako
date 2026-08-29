//! Gas MSB-Rahmenvertrag registry API (GeLi Gas 3.0, Tenor Ziff. 13–16).
//!
//! Tracks per-(GNB, MSB) conclusion state of the Messstellenbetreiberrahmen-
//! vertrag Gas (§9 Abs. 1 Nr. 3 i.V.m. Abs. 4 MsbG). From 01.10.2026 the
//! contract text is the market-developed KoV XV Anlage 8 in its jeweils
//! gültige Fassung; legacy BK7-17-026 conclusions carry
//! `status = anpassung_erforderlich` until migrated. Every change emits
//! `de.markt.msb-rahmenvertrag-gas.updated`.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{cloudevents::MarktEvent, error::MdmError};
use mako_service::cedar::CedarEnforcer;
use serde::Deserialize;
use uuid::Uuid;

use crate::pg::msb_rahmenvertrag_gas::{MsbRahmenvertragGas, MsbRvGasStatus};

use super::{Claims, IntoMdmResponse as _, Tenant};

/// Injected `Arc<PgMsbRahmenvertragGasRepository>`.
pub type MsbRvGasRepoExt = Arc<crate::pg::PgMsbRahmenvertragGasRepository>;
async fn emit(
    pool: &sqlx::PgPool,
    notify: &tokio::sync::Notify,
    tenant: &str,
    subject: String,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let evt = MarktEvent::new(
        tenant,
        mako_events::markt::MSB_RAHMENVERTRAG_GAS_UPDATED,
        subject,
        data,
    );
    crate::outbox::enqueue(pool, &evt, notify).await
}

/// `PUT /api/v1/msb-rahmenvertraege-gas` — upsert a conclusion record.
///
/// Idempotent on the natural key `(gnb_mp_id, msb_mp_id, valid_from)`; the
/// record id stays stable across re-submits. A non-zero `version` in the body
/// acts as an optimistic-locking guard (version-conflict problem response on
/// mismatch).
#[utoipa::path(
    put,
    path = "/api/v1/msb-rahmenvertraege-gas",
    tag = "msb-rahmenvertraege-gas",
    request_body = MsbRahmenvertragGas,
    responses(
        (status = 200, description = "Upserted; returns stable id and new version"),
        (status = 400, description = "Missing gnb_mp_id / msb_mp_id"),
        (status = 403, description = "Missing write-msb-rv-gas scope"),
        (status = 412, description = "Supplied version does not match the stored version"),
        (status = 422, description = "valid_to is before valid_from"),
    )
)]
pub async fn upsert_msb_rv_gas(
    claims: Claims,
    Extension(repo): Extension<MsbRvGasRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Json(mut rec): Json<MsbRahmenvertragGas>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-msb-rv-gas", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-msb-rv-gas");
        return StatusCode::FORBIDDEN.into_response();
    }
    rec.tenant = tenant.clone();
    if rec.gnb_mp_id.trim().is_empty() || rec.msb_mp_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "gnb_mp_id and msb_mp_id are required" })),
        )
            .into_response();
    }
    if rec
        .valid_to
        .is_some_and(|valid_to| valid_to < rec.valid_from)
    {
        return MdmError::Unprocessable {
            reason: format!(
                "valid_to {} is before valid_from {}",
                rec.valid_to.expect("checked above"),
                rec.valid_from
            ),
        }
        .into_response();
    }
    match repo.upsert(&rec).await {
        Ok((id, version)) => {
            if let Err(e) = emit(
                &pool,
                &notify,
                &tenant,
                id.to_string(),
                serde_json::json!({
                    "id": id,
                    "gnb_mp_id": rec.gnb_mp_id,
                    "msb_mp_id": rec.msb_mp_id,
                    "fassung": rec.fassung,
                    "status": rec.status,
                    "version": version,
                    "valid_from": rec.valid_from,
                    "valid_to": rec.valid_to,
                    "signed_at": rec.signed_at.and_then(|t| {
                        t.format(&time::format_description::well_known::Rfc3339).ok()
                    }),
                }),
            )
            .await
            {
                tracing::error!(error = %e, "msb_rv_gas: durable enqueue failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({ "id": id, "version": version })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub msb_mp_id: Option<String>,
    #[serde(default)]
    pub status: Option<MsbRvGasStatus>,
}

/// `GET /api/v1/msb-rahmenvertraege-gas` — list conclusion records.
#[utoipa::path(
    get,
    path = "/api/v1/msb-rahmenvertraege-gas",
    tag = "msb-rahmenvertraege-gas",
    params(
        ("msb_mp_id" = Option<String>, Query, description = "Filter by MSB MP-ID"),
        ("status" = Option<String>, Query, description = "Filter by lifecycle status"),
    ),
    responses(
        (status = 200, description = "Conclusion records, newest valid_from first", body = [MsbRahmenvertragGas]),
        (status = 403, description = "Missing read-msb-rv-gas scope"),
    )
)]
pub async fn list_msb_rv_gas(
    claims: Claims,
    Extension(repo): Extension<MsbRvGasRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-msb-rv-gas", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-msb-rv-gas");
        return StatusCode::FORBIDDEN.into_response();
    }
    match repo.list(&tenant, q.msb_mp_id.as_deref(), q.status).await {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "data": rows }))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/msb-rahmenvertraege-gas/{id}` — fetch one record.
#[utoipa::path(
    get,
    path = "/api/v1/msb-rahmenvertraege-gas/{id}",
    tag = "msb-rahmenvertraege-gas",
    params(("id" = Uuid, Path, description = "Record id")),
    responses(
        (status = 200, description = "The record", body = MsbRahmenvertragGas),
        (status = 403, description = "Missing read-msb-rv-gas scope"),
        (status = 404, description = "Unknown id"),
    )
)]
pub async fn get_msb_rv_gas(
    claims: Claims,
    Extension(repo): Extension<MsbRvGasRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-msb-rv-gas", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-msb-rv-gas");
        return StatusCode::FORBIDDEN.into_response();
    }
    match repo.get(&tenant, id).await {
        Ok(Some(rec)) => (StatusCode::OK, Json(serde_json::json!({ "data": rec }))).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}
