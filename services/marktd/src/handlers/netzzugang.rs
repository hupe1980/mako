//! §20b EnWG Netzzugangsplattform — request registry API.
//!
//! Projection of §20b requests (Zählpunktanordnung, Verrechnungskonzept,
//! §42c-Vereinbarungs-Registrierung) submitted through the makod `netzzugang`
//! adapter. makod upserts the record when a command is accepted and advances
//! its status after outbox delivery; the operator (or, once one exists, the
//! platform's answer channel) sets the final `bestaetigt`/`abgelehnt` state.
//!
//! Every write increments the record's optimistic-locking `version` (returned
//! in all responses); the status PATCH accepts an optional `expected_version`
//! and fails with a version-conflict problem response on mismatch.
//!
//! Every state change emits `de.markt.netzzugang.antrag.updated`.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    cloudevents::MarktEvent,
    repository::{NetzzugangAntrag, NetzzugangStatus},
};
use mako_service::cedar::CedarEnforcer;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{Claims, IntoMdmResponse as _, Tenant};

/// Injected `Arc<PgNetzzugangRepository>`.
pub type NetzzugangRepoExt = Arc<crate::pg::PgNetzzugangRepository>;
async fn emit(
    pool: &sqlx::PgPool,
    notify: &tokio::sync::Notify,
    tenant: &str,
    subject: String,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let evt = MarktEvent::new(
        tenant,
        mako_events::markt::NETZZUGANG_ANTRAG_UPDATED,
        subject,
        data,
    );
    crate::outbox::enqueue(pool, &evt, notify).await
}

fn antrag_event(rec: &NetzzugangAntrag, version: i64) -> serde_json::Value {
    serde_json::json!({
        "antrag_id": rec.id,
        "antrag_typ": rec.antrag_typ,
        "aktion": rec.aktion,
        "netzanschluss_id": rec.netzanschluss_id,
        "nb_mp_id": rec.nb_mp_id,
        "status": rec.status,
        "platform_ref": rec.platform_ref,
        "version": version,
    })
}

/// `PUT /api/v1/netzzugang/antraege` — upsert a request (makod adapter).
#[utoipa::path(
    put,
    path = "/api/v1/netzzugang/antraege",
    tag = "netzzugang",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Upserted; returns id and version"),
        (status = 400, description = "Missing netzanschluss_id / nb_mp_id"),
        (status = 403, description = "Missing write-netzzugang scope"),
    )
)]
pub async fn upsert_antrag(
    claims: Claims,
    Extension(repo): Extension<NetzzugangRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Json(mut rec): Json<NetzzugangAntrag>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-netzzugang", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-netzzugang");
        return StatusCode::FORBIDDEN.into_response();
    }
    rec.tenant = tenant.clone();
    if rec.netzanschluss_id.trim().is_empty() || rec.nb_mp_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "netzanschluss_id and nb_mp_id are required"
            })),
        )
            .into_response();
    }
    match repo.upsert(&rec).await {
        Ok((id, version)) => {
            rec.id = id;
            if let Err(e) = emit(
                &pool,
                &notify,
                &tenant,
                id.to_string(),
                antrag_event(&rec, version),
            )
            .await
            {
                tracing::error!(error = %e, "netzzugang: durable enqueue failed");
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
    pub status: Option<NetzzugangStatus>,
    #[serde(default)]
    pub netzanschluss_id: Option<String>,
}

/// `GET /api/v1/netzzugang/antraege` — list requests.
#[utoipa::path(
    get,
    path = "/api/v1/netzzugang/antraege",
    tag = "netzzugang",
    params(
        ("status" = Option<String>, Query, description = "Filter by lifecycle status"),
        ("netzanschluss_id" = Option<String>, Query, description = "Filter by Netzanschluss"),
    ),
    responses(
        (status = 200, description = "Requests incl. version, newest first"),
        (status = 403, description = "Missing read-netzzugang scope"),
    )
)]
pub async fn list_antraege(
    claims: Claims,
    Extension(repo): Extension<NetzzugangRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Query(q): Query<ListQuery>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-netzzugang", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-netzzugang");
        return StatusCode::FORBIDDEN.into_response();
    }
    match repo
        .list(&tenant, q.status, q.netzanschluss_id.as_deref())
        .await
    {
        Ok(rows) => (StatusCode::OK, Json(serde_json::json!({ "data": rows }))).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/netzzugang/antraege/{id}` — fetch a request.
#[utoipa::path(
    get,
    path = "/api/v1/netzzugang/antraege/{id}",
    tag = "netzzugang",
    params(("id" = Uuid, Path, description = "Request id")),
    responses(
        (status = 200, description = "The request incl. version"),
        (status = 403, description = "Missing read-netzzugang scope"),
        (status = 404, description = "Unknown id"),
    )
)]
pub async fn get_antrag(
    claims: Claims,
    Extension(repo): Extension<NetzzugangRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "read-netzzugang", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied read-netzzugang");
        return StatusCode::FORBIDDEN.into_response();
    }
    match repo.get(&tenant, id).await {
        Ok(Some(rec)) => (StatusCode::OK, Json(serde_json::json!({ "data": rec }))).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct StatusBody {
    /// Target lifecycle state.
    #[schema(value_type = String)]
    pub status: NetzzugangStatus,
    /// Platform reference, once known.
    #[serde(default)]
    pub platform_ref: Option<String>,
    /// Optimistic-locking guard: the version the caller last read. When
    /// supplied and stale, the update fails with a version-conflict problem
    /// response instead of overwriting.
    #[serde(default)]
    pub expected_version: Option<i64>,
}

/// `PATCH /api/v1/netzzugang/antraege/{id}/status` — advance the lifecycle.
///
/// Used by the makod sender (`uebermittelt`/`fehlgeschlagen`) and by the
/// operator or answer channel (`bestaetigt`/`abgelehnt`).
#[utoipa::path(
    patch,
    path = "/api/v1/netzzugang/antraege/{id}/status",
    tag = "netzzugang",
    params(("id" = Uuid, Path, description = "Request id")),
    request_body = StatusBody,
    responses(
        (status = 200, description = "Updated request incl. new version"),
        (status = 403, description = "Missing write-netzzugang scope"),
        (status = 404, description = "Unknown id"),
        (status = 412, description = "expected_version does not match the stored version"),
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn set_antrag_status(
    claims: Claims,
    Extension(repo): Extension<NetzzugangRepoExt>,
    Extension(cedar): Extension<Arc<CedarEnforcer>>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Extension(pool): Extension<sqlx::PgPool>,
    Extension(notify): Extension<Arc<tokio::sync::Notify>>,
    Path(id): Path<Uuid>,
    Json(body): Json<StatusBody>,
) -> impl IntoResponse {
    if let Err(e) = cedar.check(&claims.principal(), "write-netzzugang", &tenant) {
        tracing::warn!(error = %e, "marktd: Cedar denied write-netzzugang");
        return StatusCode::FORBIDDEN.into_response();
    }
    match repo
        .set_status(
            &tenant,
            id,
            body.status,
            body.platform_ref,
            body.expected_version,
        )
        .await
    {
        Ok(Some(rec)) => {
            if let Err(e) = emit(
                &pool,
                &notify,
                &tenant,
                id.to_string(),
                antrag_event(&rec.antrag, rec.version),
            )
            .await
            {
                tracing::error!(error = %e, "netzzugang: durable enqueue failed");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
            (StatusCode::OK, Json(serde_json::json!({ "data": rec }))).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}
