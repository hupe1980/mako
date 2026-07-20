//! Admin REST API for runtime MaLo master-data management.
//!
//! Mounted on the **existing `--http-addr` port** under `/admin/malo/`.
//! Protected by the same bearer token as `POST /edifact`.
//!
//! # ⚠️ Security
//!
//! This router **must never** be mounted on the public BDEW API-Webdienste
//! port (`--api-webdienste-addr`). Only the admin port is allowed. Restrict
//! network access to trusted ERP hosts via firewall rules or VPC subnets.
//!
//! # Endpoints
//!
//! | Method | Path | Description |
//! |--------|------|-------------|
//! | `GET` | `/admin/malo/{malo_id}` | Retrieve a cached record |
//! | `PUT` | `/admin/malo/{malo_id}` | Upsert a record |
//! | `DELETE` | `/admin/malo/{malo_id}` | Remove a record |
//! | `GET` | `/admin/malo/stats` | Per-tenant statistics |
//! | `POST` | `/admin/malo/bulk` | Batch upsert (planned; returns 501) |

use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
    routing::{delete, get, post, put},
};
use energy_api::models::electricity::MaloIdentResultPositive;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::info;
use utoipa::ToSchema;

use crate::cedar_authz::{CedarAuthorizer, MakoAction, MaloResource};
use crate::malo_cache::{MaloCacheStats, SlateDbMaloCache};

// ── Identifier validation ──────────────────────────────────────────────────────

/// Reject an invalid MaLo-ID path parameter before any business logic runs.
///
/// Returns `Some(422 response)` when `s` is not a valid 11-digit
/// Marktlokations-ID (BDEW alternating-weight checksum); `None` when valid.
#[inline]
fn invalid_malo_id_response(s: &str) -> Option<Response> {
    if s.parse::<rubo4e::identifiers::MaloId>().is_err() {
        Some(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "invalid_malo_id",
                    "detail": format!("{s:?} is not a valid 11-digit Marktlokations-ID"),
                })),
            )
                .into_response(),
        )
    } else {
        None
    }
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the admin API.
pub struct MaloAdminState {
    pub cache: SlateDbMaloCache,
    /// Cedar-based authorization engine — authenticates callers and evaluates
    /// ABAC policies for each MaLo admin operation.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant ID (GLN). All cache operations are scoped to this tenant.
    ///
    /// When a caller provides `X-Tenant-Id`, it must match this value exactly;
    /// mismatches are rejected with `403 Forbidden` to prevent cross-tenant data access.
    pub tenant_id: String,
}

// ── Request / response types ──────────────────────────────────────────────────

/// Request body for `PUT /admin/malo/{malo_id}`.
///
/// The server scopes all operations to its configured operator GLN;
/// the request body must not include a `tenant_id` — the tenant is
/// always the operator that holds the bearer token.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertRequest {
    /// Full positive MaLo identification result from the UTILMD query process.
    #[schema(value_type = Object)]
    pub result: MaloIdentResultPositive,
    /// Optional ISO 8601 effective-from date (informational; not enforced).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = "2026-10-01")]
    pub valid_from: Option<String>,
    /// Source tag for audit trail (e.g. `"erp-sync"`, `"manual"`, `"partin"`).
    #[serde(default)]
    #[schema(example = "erp-sync")]
    pub source: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct UpsertResponse {
    #[schema(example = "10001234567")]
    malo_id: String,
    #[schema(example = "2026-10-01T08:00:00Z")]
    updated_at: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeleteResponse {
    #[schema(example = "10001234567")]
    malo_id: String,
    deleted: bool,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct StatsResponse {
    tenants: Vec<TenantStats>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct TenantStats {
    #[schema(example = "4012345000009")]
    tenant_id: String,
    #[schema(example = 1234)]
    malo_count: u64,
    #[schema(example = "2026-10-01T08:00:00Z")]
    last_upsert: Option<String>,
}

// ── Router ────────────────────────────────────────────────────────────────────

/// Build the axum [`Router`] for the MaLo admin API.
///
/// Mount this on the admin port (same as `--http-addr`) under no additional
/// prefix — the `/admin/malo/` prefix is already part of the route definitions.
pub fn router(state: Arc<MaloAdminState>) -> Router {
    Router::new()
        .route("/admin/malo/stats", get(handle_stats))
        .route("/admin/malo/bulk", post(handle_bulk_not_implemented))
        .route("/admin/malo/{malo_id}", get(handle_get))
        .route("/admin/malo/{malo_id}", put(handle_put))
        .route("/admin/malo/{malo_id}", delete(handle_delete))
        .with_state(state)
}

// ── Auth helper ───────────────────────────────────────────────────────────────

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

fn forbidden() -> Response {
    (StatusCode::FORBIDDEN, "Forbidden").into_response()
}

fn internal_error(e: impl std::fmt::Display) -> Response {
    tracing::error!("malo-admin-api internal error: {e}");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
}

fn iso_now() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /admin/malo/{malo_id}` — retrieve a cached MaLo record.
#[utoipa::path(
    get,
    path = "/admin/malo/{malo_id}",
    tag = "admin",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID")),
    responses(
        (status = 200, description = "MaLo record found"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Tenant mismatch"),
        (status = 404, description = "MaLo not found"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_get(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Some(err) = invalid_malo_id_response(&malo_id) {
        return err;
    }
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloRead,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return forbidden();
    }
    let tenant_id = if let Some(id) = tenant_from_header(&headers) {
        if id != state.tenant_id {
            return (
                StatusCode::FORBIDDEN,
                "X-Tenant-Id does not match operator tenant",
            )
                .into_response();
        }
        id
    } else {
        state.tenant_id.clone()
    };
    match state.cache.get(&tenant_id, &malo_id).await {
        Ok(Some(result)) => Json(result).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal_error(e),
    }
}

/// `PUT /admin/malo/{malo_id}` — upsert a MaLo record.
#[utoipa::path(
    put,
    path = "/admin/malo/{malo_id}",
    tag = "admin",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID")),
    request_body(content = UpsertRequest, description = "MaLo record to upsert", content_type = "application/json"),
    responses(
        (status = 200, description = "Upserted", body = UpsertResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Tenant mismatch"),
        (status = 422, description = "malo_id in path does not match maloId in body"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_put(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(body): Json<UpsertRequest>,
) -> Response {
    if let Some(err) = invalid_malo_id_response(&malo_id) {
        return err;
    }
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloWrite,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return forbidden();
    }
    // Validate that the path parameter matches the payload MaLo-ID.
    if body.result.data_market_location.malo_id.as_ref() != malo_id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "path malo_id {malo_id:?} does not match payload maloId {:?}",
                body.result.data_market_location.malo_id.as_ref(),
            ),
        )
            .into_response();
    }
    let tenant_id = state.tenant_id.clone();
    let UpsertRequest {
        result,
        valid_from,
        source,
    } = body;

    info!(
        malo_id = %malo_id,
        tenant_id = %tenant_id,
        source = %source,
        valid_from = valid_from.as_deref().unwrap_or(""),
        "admin malo upsert request",
    );

    match state.cache.upsert(&tenant_id, &result).await {
        Ok(()) => Json(UpsertResponse {
            malo_id,
            updated_at: iso_now(),
        })
        .into_response(),
        Err(e) => internal_error(e),
    }
}

/// `DELETE /admin/malo/{malo_id}` — remove a MaLo record.
#[utoipa::path(
    delete,
    path = "/admin/malo/{malo_id}",
    tag = "admin",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID")),
    responses(
        (status = 200, description = "Deletion result", body = DeleteResponse),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Tenant mismatch"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_delete(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if let Some(err) = invalid_malo_id_response(&malo_id) {
        return err;
    }
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloDelete,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return forbidden();
    }
    let tenant_id = if let Some(id) = tenant_from_header(&headers) {
        if id != state.tenant_id {
            return (
                StatusCode::FORBIDDEN,
                "X-Tenant-Id does not match operator tenant",
            )
                .into_response();
        }
        id
    } else {
        state.tenant_id.clone()
    };
    match state.cache.remove(&tenant_id, &malo_id).await {
        Ok(deleted) => Json(DeleteResponse { malo_id, deleted }).into_response(),
        Err(e) => internal_error(e),
    }
}

/// `GET /admin/malo/stats` — per-tenant cache statistics.
#[utoipa::path(
    get,
    path = "/admin/malo/stats",
    tag = "admin",
    responses(
        (status = 200, description = "Cache statistics", body = StatsResponse),
        (status = 401, description = "Missing or invalid bearer token"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_stats(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloStats,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: None,
        },
    ) {
        return forbidden();
    }
    let tenants_res = state.cache.list_tenants().await;
    let tenant_ids = match tenants_res {
        Ok(t) => t,
        Err(e) => return internal_error(e),
    };
    let mut tenant_stats: Vec<TenantStats> = Vec::new();
    for tid in tenant_ids {
        match state.cache.stats(&tid).await {
            Ok(s) => tenant_stats.push(stats_to_json(s)),
            Err(e) => return internal_error(e),
        }
    }
    Json(StatsResponse {
        tenants: tenant_stats,
    })
    .into_response()
}

/// `POST /admin/malo/bulk` — batch upsert (not yet implemented).
async fn handle_bulk_not_implemented(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
) -> Response {
    let identity = match state.cedar.authenticate(&headers) {
        Some(id) => id,
        None => return unauthorized(),
    };
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloWrite,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: None,
        },
    ) {
        return forbidden();
    }
    (
        StatusCode::NOT_IMPLEMENTED,
        "Bulk upsert is not yet implemented. Use PUT /admin/malo/{malo_id} for individual records.",
    )
        .into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract `X-Tenant-Id` from request headers.
///
/// Returns `None` when the header is absent or non-UTF8.
/// When present, callers must validate the value matches the operator's configured
/// `tenant_id` before use — accepting arbitrary tenant IDs from headers enables
/// cross-tenant data access.
fn tenant_from_header(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-tenant-id")
        .or_else(|| headers.get("X-Tenant-Id"))
        .and_then(|v| v.to_str().ok())
        .map(ToOwned::to_owned)
}

fn stats_to_json(s: MaloCacheStats) -> TenantStats {
    TenantStats {
        tenant_id: s.tenant_id,
        malo_count: s.count,
        last_upsert: s.last_upsert.map(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        }),
    }
}
