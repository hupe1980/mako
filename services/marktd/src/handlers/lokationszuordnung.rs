//! Handlers for the `Lokationszuordnung` location-graph endpoints (B5).
//!
//! Routes:
//!   GET    /api/v1/malo/{id}/lokationen                — recursive graph from a MaLo
//!   GET    /api/v1/melos/{id}/lokationen               — recursive graph from a MeLo
//!   PUT    /api/v1/lokationszuordnungen                — upsert a directed edge
//!   DELETE /api/v1/lokationszuordnungen/{von_id}/{nach_id} — hard-delete an edge pair

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::repository::LokationszuordnungRepository;
use mako_service::cedar::CedarEnforcer;
use serde::Deserialize;

use crate::pg::PgLokationszuordnungRepository;

use super::{Claims, TenantGln};

pub type LzRepoExt = Arc<PgLokationszuordnungRepository>;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/lokationszuordnungen`.
#[derive(Debug, Deserialize)]
pub struct UpsertEdgeRequest {
    /// Source node ID (e.g. MaLo-ID).
    pub von_id: String,
    /// Source node type: `"malo"` | `"melo"` | `"nelo"` | `"sr"` | `"tr"`.
    pub von_typ: String,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type.
    pub nach_typ: String,
    /// Start of validity (`YYYY-MM-DD`). `null` = from epoch.
    pub valid_from: Option<String>,
    /// End of validity (`YYYY-MM-DD`). `null` = open-ended.
    pub valid_to: Option<String>,
    /// Full BO4E `Lokationszuordnung` payload (may be `{}`).
    #[serde(default = "empty_object")]
    pub data: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

fn parse_date(s: &str) -> Option<time::Date> {
    use time::format_description::well_known::Iso8601;
    time::Date::parse(s, &Iso8601::DEFAULT).ok()
}

/// Query parameters for graph endpoints.
#[derive(Debug, Deserialize, Default)]
pub struct GraphQuery {
    /// Point-in-time filter (`YYYY-MM-DD`). Omit for all edges regardless of validity.
    pub at: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/malo/{id}/lokationen`
///
/// Recursively traverses the MaKo location graph starting at the given `MaLo-ID`.
/// Returns all reachable edges (MaLo → MeLo → NeLo → SR/TR) ordered by depth.
/// Pass `?at=YYYY-MM-DD` to filter to edges valid on a specific date.
pub async fn get_malo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(malo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-malo", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let at_date = q.at.as_deref().and_then(parse_date);
    match repo.find_graph(&tenant_gln, &malo_id, at_date).await {
        Ok(edges) => Json(edges).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/melos/{id}/lokationen`
///
/// Recursively traverses the location graph starting at the given `MeLo-ID`.
/// Returns all reachable edges ordered by depth.
pub async fn get_melo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(melo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-melo", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let at_date = q.at.as_deref().and_then(parse_date);
    match repo.find_graph(&tenant_gln, &melo_id, at_date).await {
        Ok(edges) => Json(edges).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `PUT /api/v1/lokationszuordnungen`
///
/// Upserts a directed edge in the location graph.  Idempotent.
pub async fn put_lokationszuordnung(
    Extension(repo): Extension<LzRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Json(req): Json<UpsertEdgeRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-malo", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let valid_from = req.valid_from.as_deref().and_then(parse_date);
    let valid_to = req.valid_to.as_deref().and_then(parse_date);

    match repo
        .upsert_edge(
            &tenant_gln,
            &req.von_id,
            &req.von_typ,
            &req.nach_id,
            &req.nach_typ,
            valid_from,
            valid_to,
            req.data,
        )
        .await
    {
        Ok(id) => (StatusCode::OK, Json(serde_json::json!({ "id": id }))).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `DELETE /api/v1/lokationszuordnungen/{von_id}/{nach_id}`
///
/// Hard-deletes all temporal variants of an edge pair.
pub async fn delete_lokationszuordnung(
    Extension(repo): Extension<LzRepoExt>,
    Extension(claims): Extension<Claims>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path((von_id, nach_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-malo", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.delete_edge(&tenant_gln, &von_id, &nach_id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
