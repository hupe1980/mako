//! `GET|PUT /api/v1/melos/{melo_id}/msb` — the per-Messlokation dated MSB
//! timeline (WiM Teil 2 UC 4.1.1 historical Werteanfrage routing).
//!
//! A historical Werteanfrage must be addressed to the MSB that served the MeLo
//! **at the requested period**. MaLo-level MSB data (rollenzuordnungen,
//! versorgungsstatus) cannot answer this when a MaLo bundles MeLos with
//! divergent MSB history, so this per-MeLo timeline is the authoritative source.
//!
//! ## Access control
//!
//! - `GET` — any authenticated caller in the same tenant
//! - `PUT` — MSB/NB role (`write-melo-msb` Cedar action)

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::repository::MeloMsbRepository;
use serde::Deserialize;
use time::Date;
use time::format_description::well_known::Iso8601;
use tracing::info;

use mako_service::cedar::CedarEnforcer;

use crate::handlers::{Claims, MdmErrorResponse, Tenant};
use crate::pg::PgMeloMsbRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type MeloMsbRepoExt = Arc<PgMeloMsbRepository>;

#[derive(Debug, Deserialize)]
pub struct MsbAtQuery {
    /// Point-in-time date (ISO `YYYY-MM-DD`). Defaults to today.
    pub at: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PutMeloMsbBody {
    /// GLN of the Messstellenbetreiber.
    pub msb_mp_id: String,
    /// Assignment start (ISO `YYYY-MM-DD`).
    pub valid_from: String,
}

fn parse_iso_date(s: &str) -> Result<Date, String> {
    Date::parse(s, &Iso8601::DEFAULT).map_err(|e| format!("invalid date {s:?}: {e}"))
}

/// `GET /api/v1/melos/{melo_id}/msb?at=YYYY-MM-DD` — the MSB responsible on a date.
pub async fn get_melo_msb_at(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MeloMsbRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(melo_id): Path<String>,
    Query(q): Query<MsbAtQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-melo-msb", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let at = match q.at.as_deref() {
        Some(s) => match parse_iso_date(s) {
            Ok(d) => d,
            Err(reason) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": reason })),
                )
                    .into_response();
            }
        },
        None => mako_fristen::heute(),
    };
    match repo.find_msb_at(&tenant, &melo_id, at).await {
        Ok(Some(msb_mp_id)) => Json(serde_json::json!({
            "melo_id": melo_id,
            "at": at.to_string(),
            "msb_mp_id": msb_mp_id,
        }))
        .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no MSB assignment covers this date",
                "melo_id": melo_id,
                "at": at.to_string(),
            })),
        )
            .into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `GET /api/v1/melos/{melo_id}/msb/history` — full dated MSB timeline.
pub async fn get_melo_msb_history(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MeloMsbRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(melo_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-melo-msb", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    match repo.history(&tenant, &melo_id).await {
        Ok(rows) => {
            Json(serde_json::json!({ "melo_id": melo_id, "history": rows })).into_response()
        }
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `PUT /api/v1/melos/{melo_id}/msb` — record a new dated MSB assignment.
pub async fn put_melo_msb(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MeloMsbRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(melo_id): Path<String>,
    Json(body): Json<PutMeloMsbBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-melo-msb", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    let valid_from = match parse_iso_date(&body.valid_from) {
        Ok(d) => d,
        Err(reason) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": reason })),
            )
                .into_response();
        }
    };
    info!(%melo_id, msb_mp_id = %body.msb_mp_id, %valid_from, "marktd: assigning MSB to MeLo");
    match repo
        .assign_msb(&tenant, &melo_id, &body.msb_mp_id, valid_from)
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}
