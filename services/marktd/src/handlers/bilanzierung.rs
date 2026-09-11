//! `GET|PUT /api/v1/malos/{malo_id}/bilanzierung[/history]` — the first-class,
//! temporal BO4E `Bilanzierung` resource (BO #3).
//!
//! The PUT body is a `Bo4e<Bilanzierung>`, so [the gate](mako_markt::bo4e::decode)
//! runs as `serde` deserialises it. The typed columns are derived from the
//! **typed** object and the **canonical round-trip** is persisted as JSONB —
//! never the request body, so a column cannot disagree with the document it
//! shadows. `?at=` resolves the Bilanzierung effective at a point in time.
//!
//! ## `aggregationszustaendigkeit` is a fourth column, not a fifth spelling
//!
//! `Aggregationsverantwortung` has two members — `UENB` and `VNB` — and in
//! e-mobility Modell 2 the Aggregationsverantwortung *ruht* (BDEW
//! Anwendungshilfe to BK6-20-160 § 1.6.2). Its wire encoding is an **absent**
//! field, which is indistinguishable from a payload that simply does not say.
//! So the state is read from the **pair** with `abwicklungsmodell`, by
//! `rubo4e`'s `Bilanzierung::aggregationszustaendigkeit()`, and stored beside
//! the raw value rather than instead of it.
//!
//! ## Access control
//! - `GET` — any authenticated caller in the same tenant
//! - `PUT` — NB/BKV role (`write-bilanzierung` Cedar action)

use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::bo4e::Bo4e;
use mako_markt::repository::{BilanzierungRecord, BilanzierungRepository};
use mako_service::{ApiError, ApiResult, Json};
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
///
/// The body is a `Bo4e<Bilanzierung>`, so the gate — `_typ`, schema, strict
/// enums with their JSON-paths, BO4E rules — runs as `serde` deserialises it.
/// Every typed column is then derived from the **typed** object by
/// [`BilanzierungRecord::from_bo4e`], and what is stored is the canonical
/// round-trip rather than the request body.
pub async fn put_bilanzierung(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<BilanzierungRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id): Path<String>,
    Json(bo): Json<Bo4e<rubo4e::current::Bilanzierung>>,
) -> ApiResult<StatusCode> {
    enforcer
        .check(&claims.principal(), "write-bilanzierung", &tenant)
        .map_err(|_| ApiError::Forbidden)?;

    let rec = BilanzierungRecord::from_bo4e(&tenant, &malo_id, &bo)
        .map_err(|e| ApiError::unprocessable(e.to_string()))?;

    info!(
        %malo_id,
        beginn = %rec.bilanzierungsbeginn,
        zustaendigkeit = rec.aggregationszustaendigkeit.as_deref().unwrap_or("-"),
        "marktd: upserting BO4E Bilanzierung"
    );
    repo.upsert(&rec)
        .await
        .map_err(|e| ApiError::Internal(anyhow::Error::new(e)))?;
    Ok(StatusCode::NO_CONTENT)
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
