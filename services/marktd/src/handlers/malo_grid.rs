//! `GET|PUT /api/v1/malos/{malo_id}/grid` — NB grid topology for a MaLo.
//!
//! Source: the NB's own **NIS/GIS** (Network/Geographic Information System).
//! Provisioned through this API by the operator's own NIS/GIS export; mako
//! ships no importer, because no standard export format exists.
//! Read by `processd` NB module for Anmeldung STP decisions via `mako-pruefung`.
//!
//! NOTE: This is NOT MaStR data. MaStR (BNetzA Marktstammdatenregister) covers
//! generation/consumption units — not NB grid topology or Bilanzierungsgebiet.
//!
//! ## Access control
//!
//! - `GET` — any authenticated caller in the same tenant (ERP, processd)
//! - `PUT` — NB role only (`mako_roles` contains `"NB"`)

use mako_service::ApiError;
use std::sync::Arc;

use axum::{Extension, extract::Path, http::StatusCode, response::IntoResponse};
use mako_markt::{
    domain::{MaloId, Sparte},
    repository::{MaloGridRecord, MaloGridRepository},
};
use mako_service::Json;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::info;

use mako_service::cedar::CedarEnforcer;

use crate::handlers::{Claims, Tenant, mdm_error};
use crate::pg::PgMaloGridRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type MaloGridRepoExt = Arc<PgMaloGridRepository>;

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutMaloGridBody {
    pub nb_mp_id: String,
    pub bilanzierungsgebiet: Option<String>,
    pub netzgebiet: Option<String>,
    pub sparte: String,
    /// Origin of this record: `"mastr"` | `"nis"` | `"manual"` (default: `"manual"`).
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "manual".to_owned()
}

#[derive(Debug, Serialize)]
pub struct MaloGridResponse {
    pub malo_id: String,
    pub nb_mp_id: String,
    pub bilanzierungsgebiet: Option<String>,
    pub netzgebiet: Option<String>,
    pub sparte: String,
    pub source: String,
    pub updated_at: String,
}

impl From<MaloGridRecord> for MaloGridResponse {
    fn from(r: MaloGridRecord) -> Self {
        use time::format_description::well_known::Rfc3339;
        Self {
            malo_id: r.malo_id.to_string(),
            nb_mp_id: r.nb_mp_id,
            bilanzierungsgebiet: r.bilanzierungsgebiet,
            netzgebiet: r.netzgebiet,
            sparte: r.sparte.to_string(),
            source: r.source,
            updated_at: r.updated_at.format(&Rfc3339).unwrap_or_default(),
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/v1/malos/{malo_id}/grid` — fetch the grid topology for a MaLo.
pub async fn get_malo_grid(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MaloGridRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id_str): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-malo-grid", &tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let malo_id: MaloId = match malo_id_str.parse() {
        Ok(id) => id,
        Err(e) => {
            return ApiError::bad_request(format!("invalid malo_id: {e}")).into_response();
        }
    };

    match repo.find(&malo_id, &tenant).await {
        Ok(Some(rec)) => Json(MaloGridResponse::from(rec)).into_response(),
        Ok(None) => ApiError::NotFound.into_response(),
        Err(e) => mdm_error(e).into_response(),
    }
}

/// `PUT /api/v1/malos/{malo_id}/grid` — upsert the grid topology for a MaLo.
///
/// Requires the `write-malo-grid` action in the Cedar policy.
/// Idempotent — safe to call repeatedly from `mastr-syncd`.
pub async fn put_malo_grid(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MaloGridRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(malo_id_str): Path<String>,
    Json(body): Json<PutMaloGridBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-malo-grid", &tenant) {
        return ApiError::forbidden(e.to_string()).into_response();
    }

    let malo_id: MaloId = match malo_id_str.parse() {
        Ok(id) => id,
        Err(e) => {
            return ApiError::bad_request(format!("invalid malo_id: {e}")).into_response();
        }
    };

    let sparte: Sparte = match body.sparte.parse() {
        Ok(s) => s,
        Err(e) => {
            return ApiError::bad_request(format!("invalid sparte: {e}")).into_response();
        }
    };

    let rec = MaloGridRecord {
        malo_id,
        nb_mp_id: body.nb_mp_id,
        bilanzierungsgebiet: body.bilanzierungsgebiet,
        netzgebiet: body.netzgebiet,
        sparte,
        source: body.source,
        updated_at: OffsetDateTime::now_utc(),
        tenant,
    };

    info!(malo_id = %rec.malo_id, nb_mp_id = %rec.nb_mp_id, "marktd: upserting MaLo grid record");

    match repo.upsert(rec).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => mdm_error(e).into_response(),
    }
}
