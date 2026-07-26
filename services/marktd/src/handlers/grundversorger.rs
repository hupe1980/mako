//! `GET|PUT /api/v1/grundversorger/{nb_mp_id}` — the §36 Abs. 2 EnWG
//! Grundversorger determination per Netzgebiet.
//!
//! The supplier with the most Haushaltskunden in the Netzgebiet, festgestellt
//! by the NB every three years (zum 1. Juli, published by 30. September).
//! Maintained by the operator; read by the `processd` EoG gap-closure
//! automation to address the UTILMD 55013/44013 Zuordnung.
//!
//! ## Access control
//!
//! - `GET` — any authenticated caller in the same tenant (processd, obsd, ERP)
//! - `PUT` — NB role only (`write-grundversorger` Cedar action)

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    domain::Sparte,
    repository::{GrundversorgerRecord, GrundversorgerRepository},
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::info;

use mako_service::cedar::CedarEnforcer;

use crate::handlers::{Claims, MdmErrorResponse, TenantGln};
use crate::pg::PgGrundversorgerRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type GrundversorgerRepoExt = Arc<PgGrundversorgerRepository>;

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct GrundversorgerQuery {
    /// Commodity: `STROM` (default) or `GAS`.
    pub sparte: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PutGrundversorgerBody {
    /// Commodity: `STROM` or `GAS`.
    pub sparte: String,
    /// MP-ID of the Grundversorger.
    pub gv_mp_id: String,
    /// Date of the §36 Abs. 2 Feststellung (ISO date).
    pub festgestellt_am: Option<String>,
    /// Pre-deposited default Bilanzkreis for EoG-ohne-Antwort (GPKE Teil 4).
    #[serde(default)]
    pub default_bilanzkreis: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GrundversorgerResponse {
    pub nb_mp_id: String,
    pub sparte: String,
    pub gv_mp_id: String,
    pub festgestellt_am: Option<String>,
    pub updated_at: String,
}

impl From<GrundversorgerRecord> for GrundversorgerResponse {
    fn from(r: GrundversorgerRecord) -> Self {
        use time::format_description::well_known::Rfc3339;
        Self {
            nb_mp_id: r.nb_mp_id,
            sparte: r.sparte.to_string(),
            gv_mp_id: r.gv_mp_id,
            festgestellt_am: r.festgestellt_am.map(|d| d.to_string()),
            updated_at: r.updated_at.format(&Rfc3339).unwrap_or_default(),
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/v1/grundversorger/{nb_mp_id}?sparte=STROM`
pub async fn get_grundversorger(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<GrundversorgerRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(nb_mp_id): Path<String>,
    Query(q): Query<GrundversorgerQuery>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-grundversorger", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let sparte: Sparte = match q.sparte.as_deref().unwrap_or("STROM").parse() {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid sparte: {e}") })),
            )
                .into_response();
        }
    };

    match repo.find(&tenant, &nb_mp_id, sparte).await {
        Ok(Some(rec)) => Json(GrundversorgerResponse::from(rec)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "no Grundversorger recorded for this Netzbetreiber/Sparte"
            })),
        )
            .into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `PUT /api/v1/grundversorger/{nb_mp_id}` — upsert the Feststellung.
pub async fn put_grundversorger(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<GrundversorgerRepoExt>,
    Extension(TenantGln(tenant)): Extension<TenantGln>,
    Path(nb_mp_id): Path<String>,
    Json(body): Json<PutGrundversorgerBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-grundversorger", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let sparte: Sparte = match body.sparte.parse() {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("invalid sparte: {e}") })),
            )
                .into_response();
        }
    };
    let festgestellt_am = match body
        .festgestellt_am
        .as_deref()
        .map(|s| {
            time::Date::parse(s, &time::format_description::well_known::Iso8601::DEFAULT)
                .map_err(|e| format!("invalid festgestellt_am: {e}"))
        })
        .transpose()
    {
        Ok(d) => d,
        Err(reason) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": reason })),
            )
                .into_response();
        }
    };

    let rec = GrundversorgerRecord {
        nb_mp_id,
        sparte,
        gv_mp_id: body.gv_mp_id,
        festgestellt_am,
        default_bilanzkreis: body.default_bilanzkreis,
        updated_at: OffsetDateTime::now_utc(),
        tenant,
    };

    info!(
        nb_mp_id = %rec.nb_mp_id,
        gv_mp_id = %rec.gv_mp_id,
        sparte = %rec.sparte,
        "marktd: upserting Grundversorger Feststellung (§36 Abs. 2 EnWG)"
    );

    match repo.upsert(&rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}
