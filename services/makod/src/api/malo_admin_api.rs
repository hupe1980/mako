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

use mako_service::{ApiError, ApiResult};
use std::sync::Arc;

use axum::{
    Router,
    extract::{Path, State},
    http::HeaderMap,
    routing::{delete, get, put},
};
use energy_api::models::electricity::MaloIdentResultPositive;
use mako_service::Json;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::info;
use utoipa::ToSchema;

use crate::cedar_authz::{CedarAuthorizer, MakoAction, MaloResource};
use crate::malo_cache::{MaloCacheStats, SlateDbMaloCache};

// ── Identifier validation ──────────────────────────────────────────────────────

/// Reject an invalid MaLo-ID path parameter before any business logic runs.
///
/// A `422` when `s` is not a valid 11-digit Marktlokations-ID (BDEW
/// alternating-weight checksum).
#[inline]
fn check_malo_id(s: &str) -> ApiResult<()> {
    s.parse::<rubo4e::identifiers::MaloId>()
        .map(drop)
        .map_err(|_| {
            ApiError::unprocessable(format!("{s:?} is not a valid 11-digit Marktlokations-ID"))
        })
}

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the admin API.
pub struct MaloAdminState {
    pub cache: SlateDbMaloCache,
    /// Cedar-based authorization engine — authenticates callers and evaluates
    /// ABAC policies for each MaLo admin operation.
    pub cedar: Arc<CedarAuthorizer>,
    /// Operator tenant ID (MP-ID). All cache operations are scoped to this tenant.
    ///
    /// When a caller provides `X-Tenant-Id`, it must match this value exactly;
    /// mismatches are rejected with `403 Forbidden` to prevent cross-tenant data access.
    pub tenant_id: String,
}

// ── Request / response types ──────────────────────────────────────────────────

/// Request body for `PUT /admin/malo/{malo_id}`.
///
/// The server scopes all operations to its configured operator MP-ID;
/// the request body must not include a `tenant_id` — the tenant is
/// always the operator that holds the bearer token.
#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
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
    #[schema(example = "10001234558")]
    malo_id: String,
    #[schema(example = "2026-10-01T08:00:00Z")]
    updated_at: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeleteResponse {
    #[schema(example = "10001234558")]
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
        .route("/admin/malo/{malo_id}", get(handle_get))
        .route("/admin/malo/{malo_id}", put(handle_put))
        .route("/admin/malo/{malo_id}", delete(handle_delete))
        .with_state(state)
}

// ── Auth helper ───────────────────────────────────────────────────────────────

/// A cache failure: logged under an `error_id`, answered without its text.
fn internal_error(e: impl std::fmt::Display) -> ApiError {
    ApiError::internal(anyhow::anyhow!("malo-admin-api: {e}"))
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
        (status = 400, description = "X-Tenant-Id sent more than once"),
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
) -> ApiResult<Json<MaloIdentResultPositive>> {
    check_malo_id(&malo_id)?;
    let identity = state
        .cedar
        .authenticate(&headers)
        .ok_or(ApiError::Unauthorized)?;
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloRead,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return Err(ApiError::Forbidden);
    }
    let tenant_id = resolve_tenant(&headers, &state.tenant_id)?;
    state
        .cache
        .get(&tenant_id, &malo_id)
        .await
        .map_err(internal_error)?
        .map(Json)
        .ok_or(ApiError::NotFound)
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
) -> ApiResult<Json<UpsertResponse>> {
    check_malo_id(&malo_id)?;
    let identity = state
        .cedar
        .authenticate(&headers)
        .ok_or(ApiError::Unauthorized)?;
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloWrite,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return Err(ApiError::Forbidden);
    }
    // Validate that the path parameter matches the payload MaLo-ID.
    if body.result.data_market_location.malo_id.as_ref() != malo_id {
        return Err(ApiError::unprocessable(format!(
            "path malo_id {malo_id:?} does not match payload maloId {:?}",
            body.result.data_market_location.malo_id.as_ref(),
        )));
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

    state
        .cache
        .upsert(&tenant_id, &result)
        .await
        .map_err(internal_error)?;
    Ok(Json(UpsertResponse {
        malo_id,
        updated_at: iso_now(),
    }))
}

/// `DELETE /admin/malo/{malo_id}` — remove a MaLo record.
#[utoipa::path(
    delete,
    path = "/admin/malo/{malo_id}",
    tag = "admin",
    params(("malo_id" = String, Path, description = "11-digit Marktlokations-ID")),
    responses(
        (status = 200, description = "Deletion result", body = DeleteResponse),
        (status = 400, description = "X-Tenant-Id sent more than once"),
        (status = 401, description = "Missing or invalid bearer token"),
        (status = 403, description = "Tenant mismatch"),
    ),
    security((), ("bearer_token" = []))
)]
pub(crate) async fn handle_delete(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> ApiResult<Json<DeleteResponse>> {
    check_malo_id(&malo_id)?;
    let identity = state
        .cedar
        .authenticate(&headers)
        .ok_or(ApiError::Unauthorized)?;
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloDelete,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: Some(&malo_id),
        },
    ) {
        return Err(ApiError::Forbidden);
    }
    let tenant_id = resolve_tenant(&headers, &state.tenant_id)?;
    let deleted = state
        .cache
        .remove(&tenant_id, &malo_id)
        .await
        .map_err(internal_error)?;
    Ok(Json(DeleteResponse { malo_id, deleted }))
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
) -> ApiResult<Json<StatsResponse>> {
    let identity = state
        .cedar
        .authenticate(&headers)
        .ok_or(ApiError::Unauthorized)?;
    if !state.cedar.authorize_malo(
        &identity,
        MakoAction::AdminMaloStats,
        &MaloResource {
            tenant: &state.tenant_id,
            malo_id: None,
        },
    ) {
        return Err(ApiError::Forbidden);
    }
    let tenant_ids = state.cache.list_tenants().await.map_err(internal_error)?;
    let mut tenant_stats: Vec<TenantStats> = Vec::new();
    for tid in tenant_ids {
        let s = state.cache.stats(&tid).await.map_err(internal_error)?;
        tenant_stats.push(stats_to_json(s));
    }
    Ok(Json(StatsResponse {
        tenants: tenant_stats,
    }))
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Resolve the tenant a request acts for.
///
/// `X-Tenant-Id` may be omitted, in which case the operator's own tenant is
/// used; when present it must name that same tenant, because accepting an
/// arbitrary tenant ID from a header is cross-tenant data access. A repeated
/// header is refused rather than resolved first-wins: an intermediary that
/// authorizes on the second value would disagree with what this reads.
fn resolve_tenant(headers: &HeaderMap, operator_tenant: &str) -> ApiResult<String> {
    let header = mako_service::headers::single_str(headers, "x-tenant-id")
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    match header {
        Some(id) if id != operator_tenant => Err(ApiError::forbidden(
            "X-Tenant-Id does not match operator tenant",
        )),
        Some(id) => Ok(id.to_owned()),
        None => Ok(operator_tenant.to_owned()),
    }
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
