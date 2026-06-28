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
use secrecy::{ExposeSecret as _, SecretString};
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use tracing::info;

use crate::malo_cache::{MaloCacheStats, SlateDbMaloCache};

// ── State ─────────────────────────────────────────────────────────────────────

/// Shared state for the admin API.
pub struct MaloAdminState {
    pub cache: SlateDbMaloCache,
    pub optional_token: Option<SecretString>,
}

// ── Request / response types ──────────────────────────────────────────────────

/// Request body for `PUT /admin/malo/{malo_id}`.
#[derive(Debug, Deserialize)]
pub struct UpsertRequest {
    pub tenant_id: String,
    pub result: MaloIdentResultPositive,
    /// Optional ISO 8601 effective-from date (informational; not enforced).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    /// Source tag for audit purposes (e.g. `"erp-sync"`, `"manual"`).
    #[serde(default)]
    pub source: String,
}

#[derive(Serialize)]
struct UpsertResponse {
    malo_id: String,
    updated_at: String,
}

#[derive(Serialize)]
struct DeleteResponse {
    malo_id: String,
    deleted: bool,
}

#[derive(Serialize)]
struct StatsResponse {
    tenants: Vec<TenantStats>,
}

#[derive(Serialize)]
struct TenantStats {
    tenant_id: String,
    malo_count: u64,
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

fn check_auth(headers: &HeaderMap, optional_token: &Option<SecretString>) -> bool {
    let Some(expected) = optional_token else {
        return true;
    };
    let provided = headers
        .get("authorization")
        .or_else(|| headers.get("Authorization"))
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    provided
        .as_bytes()
        .ct_eq(expected.expose_secret().as_bytes())
        .into()
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
}

fn internal_error(msg: impl std::fmt::Display) -> Response {
    (StatusCode::INTERNAL_SERVER_ERROR, msg.to_string()).into_response()
}

fn iso_now() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /admin/malo/{malo_id}` — retrieve a cached MaLo record.
async fn handle_get(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    // Tenant is derived from the token's implied identity. For now, a single
    // default tenant per deployment. Multi-tenant: parse from a header or JWT.
    let tenant_id = tenant_from_headers(&headers);
    match state.cache.get(&tenant_id, &malo_id).await {
        Ok(Some(result)) => Json(result).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal_error(e),
    }
}

/// `PUT /admin/malo/{malo_id}` — upsert a MaLo record.
async fn handle_put(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
    Json(body): Json<UpsertRequest>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    // Validate that the path parameter matches the payload MaLo-ID.
    if body.result.data_market_location.malo_id.0 != malo_id {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!(
                "path malo_id {malo_id:?} does not match payload maloId {:?}",
                body.result.data_market_location.malo_id.0,
            ),
        )
            .into_response();
    }
    let UpsertRequest {
        tenant_id,
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
async fn handle_delete(
    State(state): State<Arc<MaloAdminState>>,
    headers: HeaderMap,
    Path(malo_id): Path<String>,
) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    let tenant_id = tenant_from_headers(&headers);
    match state.cache.remove(&tenant_id, &malo_id).await {
        Ok(deleted) => Json(DeleteResponse { malo_id, deleted }).into_response(),
        Err(e) => internal_error(e),
    }
}

/// `GET /admin/malo/stats` — per-tenant cache statistics.
async fn handle_stats(State(state): State<Arc<MaloAdminState>>, headers: HeaderMap) -> Response {
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
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
    if !check_auth(&headers, &state.optional_token) {
        return unauthorized();
    }
    (
        StatusCode::NOT_IMPLEMENTED,
        "Bulk upsert is not yet implemented. Use PUT /admin/malo/{malo_id} for individual records.",
    )
        .into_response()
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Derive tenant ID from request context.
///
/// In a single-tenant deployment the tenant is the operator's own GLN,
/// configured at startup. For now, use the `X-Tenant-Id` header as a
/// passthrough; default to `"default"` when absent.
///
/// TODO: replace with proper multi-tenant routing (mTLS cert CN or JWT sub).
fn tenant_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-tenant-id")
        .or_else(|| headers.get("X-Tenant-Id"))
        .and_then(|v| v.to_str().ok())
        .unwrap_or("default")
        .to_owned()
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
