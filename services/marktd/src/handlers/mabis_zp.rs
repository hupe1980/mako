//! `GET|PUT /api/v1/bilanzierungsgebiete/{eic}/mabis-zp` and
//! `GET /api/v1/mabis-zp` — Bilanzierungsgebiet → MaBiS-Zählpunkt assignments.
//!
//! MSCONS Summenzeitreihen (PIDs 13003/13023) carry three distinct SG6 `LOC`
//! qualifiers: `172` the **Meldepunkt** (the MaBiS-Zählpunkt), `107` the
//! Bilanzierungsgebiet, and `237` the Bilanzkreis. Both of the first two are
//! free text at the MIG level, so filing a Summenzeitreihe under the wrong
//! Meldepunkt yields a message that parses, validates, and is indistinguishable
//! to the BIKO from a correct one.
//!
//! `mabis-syncd` reads this before every submission and **refuses** to submit a
//! territory with no assignment rather than substituting the Bilanzierungsgebiet
//! EIC. Holding it as master data instead of service configuration is what makes
//! that refusal possible across deployments.
//!
//! ## Access control
//!
//! - `GET` — any authenticated caller in the same tenant (`mabis-syncd`, ERP)
//! - `PUT` — NB role only
//!
//! ## Strom only
//!
//! MaBiS is the *Marktregeln für die Durchführung der Bilanzkreisabrechnung
//! **Strom***. Gas balancing runs under GaBi Gas and has no MaBiS-Zählpunkt, so
//! there is no `sparte` on this resource — an earlier one accepted `GAS` and
//! recorded an assignment that cannot exist.

use std::sync::Arc;

use axum::{Extension, Json, extract::Path, http::StatusCode, response::IntoResponse};
use mako_markt::repository::{MabisZpRecord, MabisZpRepository};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tracing::info;

use mako_service::cedar::CedarEnforcer;

use crate::handlers::{Claims, MdmErrorResponse, Tenant};
use crate::pg::PgMabisZpRepository;

/// Extension alias — concrete type so AFIT dispatches statically.
pub type MabisZpRepoExt = Arc<PgMabisZpRepository>;

// ── Request / response DTOs ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PutMabisZpBody {
    /// The Meldepunkt filed as `LOC+172`.
    pub mabis_zp_id: String,
    /// Origin of this assignment: `"manual"` | `"erp"` | import name.
    #[serde(default = "default_source")]
    pub source: String,
}

fn default_source() -> String {
    "manual".to_owned()
}

#[derive(Debug, Serialize)]
pub struct MabisZpResponse {
    pub bilanzierungsgebiet: String,
    pub mabis_zp_id: String,
    pub source: String,
    pub updated_at: String,
}

impl From<MabisZpRecord> for MabisZpResponse {
    fn from(r: MabisZpRecord) -> Self {
        use time::format_description::well_known::Rfc3339;
        Self {
            bilanzierungsgebiet: r.bilanzierungsgebiet,
            mabis_zp_id: r.mabis_zp_id,
            source: r.source,
            updated_at: r.updated_at.format(&Rfc3339).unwrap_or_default(),
        }
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `GET /api/v1/bilanzierungsgebiete/{eic}/mabis-zp`
pub async fn get_mabis_zp(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MabisZpRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(eic): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-mabis-zp", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match repo.find(&eic, &tenant).await {
        Ok(Some(rec)) => Json(MabisZpResponse::from(rec)).into_response(),
        // 404 is the signal `mabis-syncd` turns into a refused submission — it
        // must never be read as "use the Bilanzierungsgebiet EIC instead".
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": format!(
                    "no MaBiS-Zählpunkt assigned to Bilanzierungsgebiet {eic}"
                )
            })),
        )
            .into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `GET /api/v1/mabis-zp` — every assignment for the tenant.
pub async fn list_mabis_zp(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MabisZpRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "read-mabis-zp", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    match repo.list(&tenant).await {
        Ok(recs) => Json(
            recs.into_iter()
                .map(MabisZpResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}

/// `PUT /api/v1/bilanzierungsgebiete/{eic}/mabis-zp` — upsert the assignment.
///
/// Requires the `write-mabis-zp` action in the Cedar policy. Idempotent.
pub async fn put_mabis_zp(
    claims: Claims,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(repo): Extension<MabisZpRepoExt>,
    Extension(Tenant(tenant)): Extension<Tenant>,
    Path(eic): Path<String>,
    Json(body): Json<PutMabisZpBody>,
) -> impl IntoResponse {
    if let Err(e) = enforcer.check(&claims.principal(), "write-mabis-zp", &tenant) {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }

    let mabis_zp_id = body.mabis_zp_id.trim().to_owned();
    if mabis_zp_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "mabis_zp_id must not be empty" })),
        )
            .into_response();
    }

    // Rejected here as well as by the table CHECK: this is the exact
    // substitution the assignment exists to prevent, and a 400 naming it is more
    // useful than a constraint violation surfaced as a 500.
    if mabis_zp_id == eic {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "the MaBiS-Zählpunkt must not be the Bilanzierungsgebiet EIC — \
                          they are different identifiers (LOC+172 vs LOC+107), and \
                          substituting one for the other is invisible on the wire"
            })),
        )
            .into_response();
    }

    // Not this territory's EIC, and not any other's either. A
    // Zählpunktbezeichnung is 33 characters and a Bilanzierungsgebiet EIC is 16,
    // so the length separates them. Territory A's EIC assigned as territory B's
    // Meldepunkt passes the inequality above and would sit in master data
    // reading as valid until a submission run refused it.
    let len = mabis_zp_id.chars().count();
    if len != 33 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!(
                    "the MaBiS-Zählpunkt must be a 33-character Zählpunktbezeichnung,                      got {len} characters — a 16-character value is a                      Bilanzierungsgebiet EIC and must not be filed as the Meldepunkt"
                )
            })),
        )
            .into_response();
    }

    let rec = MabisZpRecord {
        bilanzierungsgebiet: eic.clone(),
        mabis_zp_id,
        source: body.source,
        tenant: tenant.clone(),
        updated_at: OffsetDateTime::now_utc(),
    };

    match repo.upsert(rec).await {
        Ok(()) => {
            info!(bilanzierungsgebiet = %eic, tenant = %tenant, "mabis-zp assignment upserted");
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => MdmErrorResponse(e).into_response(),
    }
}
