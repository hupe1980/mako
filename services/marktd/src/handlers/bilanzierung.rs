//! `GET|PUT /api/v1/malos/{malo_id}/bilanzierung[/history]` — the first-class,
//! temporal BO4E `Bilanzierung` resource (BO #3).
//!
//! The PUT body is a full BO4E `Bilanzierung`; it is **type-validated** by
//! round-trip deserialization into `rubo4e::current::Bilanzierung` (like the
//! MaLo/MeLo envelope), the typed columns + validity are extracted, and the raw
//! BO is persisted as JSONB. `?at=` resolves the Bilanzierung effective at a
//! point in time.
//!
//! ## Access control
//! - `GET` — any authenticated caller in the same tenant
//! - `PUT` — NB/BKV role (`write-bilanzierung` Cedar action)

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::{Iso8601, Rfc3339};
use tracing::info;

use mako_service::cedar::CedarEnforcer;

use crate::handlers::{Claims, MdmErrorResponse, Tenant};
use crate::pg::PgBilanzierungRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type BilanzierungRepoExt = Arc<PgBilanzierungRepository>;

#[derive(Debug, Deserialize)]
pub struct AtQuery {
    /// Point-in-time instant (RFC 3339) or date (`YYYY-MM-DD`). Defaults to now.
    pub at: Option<String>,
}

/// Extract a BO4E enum/newtype field as its wire string.
fn as_wire_str(v: Option<&serde_json::Value>) -> Option<String> {
    v.and_then(|v| v.as_str()).map(str::to_owned)
}

/// Parse an `?at=` value as an instant: RFC 3339 first, then a bare date
/// (interpreted at 00:00 UTC).
fn parse_at(s: &str) -> Result<OffsetDateTime, String> {
    if let Ok(dt) = OffsetDateTime::parse(s, &Rfc3339) {
        return Ok(dt);
    }
    time::Date::parse(s, &Iso8601::DEFAULT)
        .map(|d| d.midnight().assume_utc())
        .map_err(|e| format!("invalid `at` {s:?}: expected RFC 3339 or YYYY-MM-DD ({e})"))
}

/// `PUT /api/v1/malos/{malo_id}/bilanzierung` — upsert a BO4E Bilanzierung.
pub async fn put_bilanzierung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<BilanzierungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-bilanzierung", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    // The BO4E gate: `_typ`, schema, strict enums (serde decodes an unknown
    // wire value to `Unknown`, so this is what rejects typos, legacy codes and
    // values from a newer schema, with their JSON-paths), BO4E rules.
    if let Err(e) = mako_markt::bo4e::decode::<rubo4e::current::Bilanzierung>(body.clone()) {
        return (StatusCode::UNPROCESSABLE_ENTITY, Json(e.to_json())).into_response();
    }

    // Validity start is mandatory for the temporal key.
    let Some(beginn) = body
        .get("bilanzierungsbeginn")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok())
    else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "bilanzierungsbeginn (RFC 3339) is required — it is the temporal key"
            })),
        )
            .into_response();
    };
    let ende = body
        .get("bilanzierungsende")
        .and_then(|v| v.as_str())
        .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());

    let bo4e_version = body.get("_version").and_then(|v| v.as_str()).map_or_else(
        || mako_markt::bo4e::schema_version().to_owned(),
        str::to_owned,
    );

    let rec = BilanzierungRecord {
        malo_id: malo_id.clone(),
        bilanzierungsbeginn: beginn,
        bilanzierungsende: ende,
        bilanzkreis: as_wire_str(body.get("bilanzkreis")),
        aggregationsverantwortung: as_wire_str(body.get("aggregationsverantwortung")),
        prognosegrundlage: as_wire_str(body.get("prognosegrundlage")),
        fallgruppenzuordnung: as_wire_str(body.get("fallgruppenzuordnung")),
        data: body,
        bo4e_version,
        tenant: tenant.clone(),
        updated_at: OffsetDateTime::now_utc(),
    };

    info!(%malo_id, %beginn, "marktd: upserting BO4E Bilanzierung");
    match repo.upsert(&rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `GET /api/v1/malos/{malo_id}/bilanzierung?at=<rfc3339|date>` — point-in-time.
pub async fn get_bilanzierung_at(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<BilanzierungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id): Path<String>,
    Query(q): Query<AtQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-bilanzierung", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let at = match q.at.as_deref() {
        Some(s) => match parse_at(s) {
            Ok(dt) => dt,
            Err(reason) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": reason })),
                )
                    .into_response();
            }
        },
        None => OffsetDateTime::now_utc(),
    };
    match repo.find_at(&tenant, &malo_id, at).await {
        Ok(Some(rec)) => Json(rec).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no Bilanzierung effective at this instant",
                "malo_id": malo_id,
                "at": at.format(&Rfc3339).unwrap_or_default(),
            })),
        )
            .into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `GET /api/v1/malos/{malo_id}/bilanzierung/history` — full temporal history.
pub async fn get_bilanzierung_history(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<BilanzierungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-bilanzierung", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    match repo.history(&tenant, &malo_id).await {
        Ok(rows) => {
            Json(serde_json::json!({ "malo_id": malo_id, "history": rows })).into_response()
        }
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}
