//! Handlers for the `Lokationszuordnung` location-graph endpoints (B5).
//!
//! Routes:
//!   GET    /api/v1/malos/{id}/lokationen               — recursive graph from a MaLo
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
use mako_markt::repository::{Lokationsbuendel, LokationszuordnungRepository};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Lokationstyp;
use serde::{Deserialize, Serialize};

use crate::pg::PgLokationszuordnungRepository;

use super::{Claims, TenantGln};

pub type LzRepoExt = Arc<PgLokationszuordnungRepository>;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// Request body for `PUT /api/v1/lokationszuordnungen`.
#[derive(Debug, Deserialize)]
pub struct UpsertEdgeRequest {
    /// Source node ID (e.g. MaLo-ID).
    pub von_id: String,
    /// Source node type — BO4E [`Lokationstyp`] (`MALO`/`MELO`/`NELO`/`SR`/`TR`).
    pub von_typ: Lokationstyp,
    /// Target node ID.
    pub nach_id: String,
    /// Target node type — BO4E [`Lokationstyp`].
    pub nach_typ: Lokationstyp,
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

/// `GET /api/v1/malos/{id}/lokationen`
///
/// Recursively traverses the MaKo location graph starting at the given `MaLo-ID`.
/// Returns all reachable edges (MaLo → MeLo → NeLo → SR/TR) ordered by depth.
/// Pass `?at=YYYY-MM-DD` to filter to edges valid on a specific date.
pub async fn get_malo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(malo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant_gln)
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

/// Response for `GET /api/v1/malos/{id}/buendel` — the projected Lokationsbündel
/// plus its structural-integrity status.
#[derive(Debug, Serialize)]
pub struct BuendelResponse {
    /// The projected bundle.
    #[serde(flatten)]
    pub buendel: Lokationsbuendel,
    /// `true` when the bundle carries at least one Messlokation.
    pub valid: bool,
    /// Human-readable integrity violation, when `valid` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation_error: Option<String>,
}

/// `GET /api/v1/malos/{id}/buendel`
///
/// Returns the first-class [`Lokationsbuendel`] rooted at the given `MaLo-ID`,
/// projected from the typed location graph, together with its structural
/// integrity status (a bundle can be transiently incomplete mid-Einzug, so this
/// reports `valid: false` rather than failing the request).
pub async fn get_malo_buendel(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(malo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let at_date = q.at.as_deref().and_then(parse_date);
    match repo.load_buendel(&tenant_gln, &malo_id, at_date).await {
        Ok(buendel) => {
            let (valid, validation_error) = match buendel.validate() {
                Ok(()) => (true, None),
                Err(e) => (false, Some(e.to_string())),
            };
            Json(BuendelResponse {
                buendel,
                valid,
                validation_error,
            })
            .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/melos/{id}/lokationen`
///
/// Recursively traverses the location graph starting at the given `MeLo-ID`.
/// Returns all reachable edges ordered by depth.
pub async fn get_melo_lokationen(
    Extension(repo): Extension<LzRepoExt>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path(melo_id): Path<String>,
    Query(q): Query<GraphQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-lokationszuordnung", &tenant_gln)
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
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Json(req): Json<UpsertEdgeRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-lokationszuordnung", &tenant_gln)
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
            req.von_typ,
            &req.nach_id,
            req.nach_typ,
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
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(enforcer): Extension<CedarEnforcer>,
    Path((von_id, nach_id)): Path<(String, String)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-lokationszuordnung", &tenant_gln)
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
