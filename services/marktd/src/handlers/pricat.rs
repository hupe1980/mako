//! Handlers for PRICAT 27003 version history and manual re-dispatch.
//!
//! Routes:
//! - `GET  /api/v1/pricat/{nb_mp_id}/history` — list all PRICAT versions for an NB
//! - `GET  /api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}` — dispatch audit log
//! - `POST /api/v1/pricat/{nb_mp_id}/dispatch` — trigger immediate (re-)dispatch

use std::sync::Arc;

use axum::{Extension, Json, extract::Path, http::StatusCode, response::IntoResponse};
use mako_markt::repository::{PriCatRepository, PriCatVersion};
use mako_service::cedar::CedarEnforcer;
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use super::{Claims, TenantGln};

// ── Type alias ───────────────────────────────────────────────────────────────

/// The PRICAT repo extension type injected via Axum `Extension`.
/// Re-exported from `preisblatt` as a shared alias — both handlers use the same
/// `Arc<PgPriCatRepository>` extension.
pub use super::preisblatt::PriCatRepoExt;

// ── DTOs ─────────────────────────────────────────────────────────────────────

/// One entry in the PRICAT version history list.
#[derive(Debug, Serialize, ToSchema)]
pub struct PriCatVersionSummary {
    pub id: Uuid,
    pub nb_mp_id: String,
    /// Start of validity period (ISO 8601 date).
    pub valid_from: String,
    /// End of validity period, or `null` for open-ended.
    pub valid_to: Option<String>,
    pub bo4e_version: String,
    pub source: String,
    /// Dispatch state: `pending` | `queued` | `done` | `error`.
    pub dispatch_state: String,
    pub dispatch_error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: time::OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: time::OffsetDateTime,
}

impl From<&PriCatVersion> for PriCatVersionSummary {
    fn from(v: &PriCatVersion) -> Self {
        let fmt_date =
            |d: time::Date| format!("{:04}-{:02}-{:02}", d.year(), d.month() as u8, d.day());
        Self {
            id: v.id,
            nb_mp_id: v.nb_mp_id.clone(),
            valid_from: fmt_date(v.valid_from),
            valid_to: v.valid_to.map(fmt_date),
            bo4e_version: v.bo4e_version.clone(),
            source: v.source.to_string(),
            dispatch_state: v.dispatch_state.to_string(),
            dispatch_error: v.dispatch_error.clone(),
            created_at: v.created_at,
            updated_at: v.updated_at,
        }
    }
}

/// One dispatch audit entry.
#[derive(Debug, Serialize, ToSchema)]
pub struct DispatchLogEntry {
    pub id: Uuid,
    pub lf_mp_id: String,
    pub process_id: Option<Uuid>,
    #[serde(with = "time::serde::rfc3339")]
    pub dispatched_at: time::OffsetDateTime,
    pub outcome: String,
    pub error_detail: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/pricat/{nb_mp_id}/history`
///
/// Returns all versioned PRICAT snapshots for the given NB GLN, newest first.
/// Each entry includes dispatch state so operators can monitor the pipeline.
#[utoipa::path(
    get,
    path = "/api/v1/pricat/{nb_mp_id}/history",
    params(
        ("nb_mp_id" = String, Path, description = "NB GLN (13-digit BDEW/DVGW code)"),
    ),
    responses(
        (status = 200, description = "PRICAT version history", body = Vec<PriCatVersionSummary>),
        (status = 403, description = "Forbidden"),
    ),
)]
pub async fn get_pricat_history(
    Extension(repo): Extension<PriCatRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.list_versions(&nb_mp_id, &tenant_gln).await {
        Ok(versions) => {
            let summaries: Vec<PriCatVersionSummary> =
                versions.iter().map(PriCatVersionSummary::from).collect();
            Json(summaries).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `GET /api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}`
///
/// Returns the dispatch audit log for a specific PRICAT version — one entry per
/// LF partner that was reached (or attempted).
#[utoipa::path(
    get,
    path = "/api/v1/pricat/{nb_mp_id}/dispatch-log/{version_id}",
    params(
        ("nb_mp_id" = String, Path, description = "NB GLN"),
        ("version_id" = Uuid, Path, description = "PRICAT version UUID"),
    ),
    responses(
        (status = 200, description = "Dispatch log entries", body = Vec<DispatchLogEntry>),
        (status = 403, description = "Forbidden"),
    ),
)]
pub async fn get_dispatch_log(
    Extension(repo): Extension<PriCatRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path((_nb_gln, version_id)): Path<(String, Uuid)>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    match repo.dispatch_log(version_id).await {
        Ok(entries) => {
            let out: Vec<DispatchLogEntry> = entries
                .iter()
                .map(|e| DispatchLogEntry {
                    id: e.id,
                    lf_mp_id: e.lf_mp_id.clone(),
                    process_id: e.process_id,
                    dispatched_at: e.dispatched_at,
                    outcome: e.outcome.clone(),
                    error_detail: e.error_detail.clone(),
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

/// `POST /api/v1/pricat/{nb_mp_id}/dispatch`
///
/// Enqueue immediate (re-)dispatch of the latest PRICAT version for the given NB
/// to all currently active LF partners.
///
/// The actual dispatch happens asynchronously in the background. Returns `202
/// Accepted` with the version ID that was queued.
///
/// Use this endpoint to manually trigger dispatch after an AS4 connectivity
/// incident, or to force re-distribution to newly on-boarded LF partners without
/// waiting for the next scheduled scan.
#[utoipa::path(
    post,
    path = "/api/v1/pricat/{nb_mp_id}/dispatch",
    params(
        ("nb_mp_id" = String, Path, description = "NB GLN to dispatch PRICAT for"),
    ),
    responses(
        (status = 202, description = "Dispatch enqueued"),
        (status = 404, description = "No PRICAT version found for this NB"),
        (status = 403, description = "Forbidden"),
    ),
)]
pub async fn post_pricat_dispatch(
    Extension(repo): Extension<PriCatRepoExt>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    claims: Claims,
    Path(nb_mp_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-preisblatt", &tenant_gln)
        .is_err()
    {
        return (StatusCode::FORBIDDEN, "access denied").into_response();
    }

    let latest = match repo.find_latest(&nb_mp_id, &tenant_gln).await {
        Ok(Some(v)) => v,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                format!("No PRICAT version found for NB GLN {nb_mp_id}"),
            )
                .into_response();
        }
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };

    // Reset dispatch state so the background scan picks it up.
    if let Err(e) = repo.mark_queued(latest.id).await {
        return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response();
    }

    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({ "version_id": latest.id, "status": "queued" })),
    )
        .into_response()
}
