//! NeLo (Netz-Element-Lokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/nelo/:id           — upsert a NeLo (schema-validated `Netzlokation` BO4E)
//!   GET  /api/v1/nelo/:id           — get a single NeLo — returns typed `Netzlokation`
//!   GET  /api/v1/nelo               — list NeLos (?nb_mp_id=… filters by Netzbetreiber)
//!
//! NeLos are network element locations used in BDEW Redispatch 2.0 processes.
//! The `nelo_id` is typically a 16-char EIC code (ENTSO-E) or a 13-digit BDEW
//! Codenummer.
//!
//! ## Hard cut — typed API (same pattern as Marktlokation)
//!
//! PUT body: `rubo4e::current::Netzlokation` JSON (camelCase).
//! GET returns: `NetzlokationResponse` with `data: rubo4e::current::Netzlokation`.
//!
//! Validation on PUT:
//!   1. Auto-inject `_typ: "NETZLOKATION"` when absent.
//!   2. Reject 422 if `_typ` is present but does not equal `NETZLOKATION`.
//!   3. Deserialise as `rubo4e::current::Netzlokation` — invalid enum fields
//!      (`sparte`, `eigenschaftMsbLokation`, …) return 422.
//!   4. Re-serialise to canonical BO4E camelCase form; extract typed SQL columns.

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    domain::Sparte,
    error::MdmError,
    repository::{NeLoRecord, NeLoRepository, PageResult},
};
use mako_service::cedar::CedarEnforcer;
use rubo4e::current::Netzlokation;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::pg::PgNeLoRepository;

use super::{Claims, IntoMdmResponse as _, TenantGln, etag, parse_if_match};

/// Extension alias — concrete type so AFIT dispatches statically.
pub type NeLoRepoExt = Arc<PgNeLoRepository>;

// ── BO4E validation helpers ───────────────────────────────────────────────────

/// Validate and normalise a `Netzlokation` payload.
///
/// 1. Auto-inject `_typ: "NETZLOKATION"` when absent.
/// 2. Reject 422 if `_typ` is present but is not `NETZLOKATION`.
/// 3. Deserialise as `rubo4e::current::Netzlokation` to validate all enum fields.
/// 4. Re-serialise to canonical BO4E camelCase form.
fn normalize_netzlokation(
    mut data: serde_json::Value,
) -> Result<(Netzlokation, serde_json::Value), (StatusCode, serde_json::Value)> {
    if let Some(obj) = data.as_object_mut() {
        obj.entry("_typ")
            .or_insert_with(|| serde_json::json!("NETZLOKATION"));
    }
    if let Some(typ) = data.get("_typ").and_then(|v| v.as_str())
        && typ.to_uppercase() != "NETZLOKATION"
    {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("expected _typ NETZLOKATION, got '{typ}'") }),
        ));
    }
    let nelo: Netzlokation = serde_json::from_value(data).map_err(|e| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            serde_json::json!({ "error": format!("invalid Netzlokation payload: {e}") }),
        )
    })?;
    let canonical = serde_json::to_value(&nelo).unwrap_or_default();
    Ok((nelo, canonical))
}

/// Deserialise stored JSONB as `Netzlokation`. Returns `None` on schema drift.
fn deserialize_stored_nelo(data: serde_json::Value, nelo_id: &str) -> Option<Netzlokation> {
    serde_json::from_value::<Netzlokation>(data)
        .map_err(|e| {
            tracing::error!(
                nelo_id,
                error = %e,
                "schema drift: stored NeLo data is not a valid Netzlokation — \
                 re-PUT with a valid BO4E payload"
            );
        })
        .ok()
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

/// PUT body — accepts a `rubo4e::current::Netzlokation` JSON object.
///
/// Top-level convenience fields (`nb_mp_id`, `sparte`) are REQUIRED separately
/// since they are indexed SQL columns.  The full BO4E payload goes in `data`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct NeLoUpsertRequest {
    /// Owning Netzbetreiber MP-ID (indexed column, required for filtering).
    pub nb_mp_id: String,
    /// `STROM` or `GAS` (indexed column — must also match `data.sparte` when present).
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Full `rubo4e::current::Netzlokation` payload (BO4E camelCase JSON).
    ///
    /// `_typ` is auto-injected as `NETZLOKATION` if absent.
    /// Unknown `_typ` values are rejected with 422.
    /// Invalid enum fields (`eigenschaftMsbLokation`, …) are rejected with 422.
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// GET response — returns typed `Netzlokation` BO4E payload.
#[derive(Debug, Serialize, ToSchema)]
pub struct NetzlokationResponse {
    /// 16-char EIC code or 13-digit BDEW Codenummer.
    pub nelo_id: String,
    /// Owning Netzbetreiber MP-ID.
    pub nb_mp_id: String,
    /// Sparte extracted from the `Netzlokation` payload.
    #[schema(value_type = String, example = "STROM")]
    pub sparte: String,
    /// `true` if this NeLo has a Steuerkanal (Redispatch 2.0 remote-control).
    pub steuerkanal: Option<bool>,
    /// gMSB Marktrolle — `eigenschaftMsbLokation` in BO4E.
    pub eigenschaft_msb_lokation: Option<String>,
    /// gMSB MP-ID — `grundzustaendigerMsbCodenr` in BO4E.
    pub grundzustaendiger_msb_codenr: Option<String>,
    /// Full validated `rubo4e::current::Netzlokation` — canonical BO4E camelCase.
    #[schema(value_type = Object)]
    pub data: Netzlokation,
    pub version: i64,
    pub updated_at: String,
}

impl NetzlokationResponse {
    fn from_record(rec: NeLoRecord) -> Self {
        let nelo_id = rec.nelo_id.clone();
        let nelo = deserialize_stored_nelo(rec.data.clone(), &nelo_id).unwrap_or_else(|| {
            // Schema drift — return a minimal valid Netzlokation.
            // The operator must re-PUT to fix the stored data.
            Netzlokation::default()
        });
        Self {
            nelo_id,
            nb_mp_id: rec.nb_mp_id,
            sparte: rec.sparte.to_string(),
            steuerkanal: rec.steuerkanal,
            eigenschaft_msb_lokation: rec.eigenschaft_msb_lokation,
            grundzustaendiger_msb_codenr: rec.grundzustaendiger_msb_codenr,
            data: nelo,
            version: rec.version,
            updated_at: rec.updated_at.to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NetzlokationListResponse {
    pub items: Vec<NetzlokationResponse>,
    pub total: u64,
    pub page: u32,
    pub size: u32,
}

impl From<PageResult<NeLoRecord>> for NetzlokationListResponse {
    fn from(p: PageResult<NeLoRecord>) -> Self {
        Self {
            items: p
                .items
                .into_iter()
                .map(NetzlokationResponse::from_record)
                .collect(),
            total: p.total,
            page: p.page,
            size: p.size,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct NeLoListQuery {
    /// Filter by owning Netzbetreiber GLN.
    #[serde(default)]
    pub nb_mp_id: Option<String>,
    #[param(example = 0)]
    #[serde(default)]
    pub page: u32,
    #[param(example = 50)]
    #[serde(default = "default_size")]
    pub size: u32,
}

fn default_size() -> u32 {
    50
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// PUT /api/v1/nelo/:id
///
/// Insert or update a Netz-Element-Lokation.
///
/// Body must be a valid `rubo4e::current::Netzlokation` JSON object (camelCase).
/// Returns 422 on wrong `_typ` or invalid enum values.
/// Supply `If-Match` header for optimistic concurrency; omit for unconditional upsert.
pub async fn put_nelo(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Path(nelo_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<NeLoUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

    // Validate and canonicalise the BO4E Netzlokation payload.
    let (typed_nelo, canonical_data) = match normalize_netzlokation(body.data) {
        Ok(v) => v,
        Err((status, json)) => return (status, Json(json)).into_response(),
    };

    // Extract typed SQL columns from the validated BO4E struct.
    let steuerkanal = typed_nelo.steuerkanal;
    let eigenschaft_msb_lokation = typed_nelo
        .eigenschaft_msb_lokation
        .as_ref()
        .map(|r| format!("{r:?}"));
    let grundzustaendiger_msb_codenr = typed_nelo
        .grundzustaendiger_msb_codenr
        .as_ref()
        .map(|id| id.to_string());

    let if_match = parse_if_match(&headers);
    let rec = NeLoRecord {
        nelo_id,
        tenant: tenant_gln,
        name: None,
        sparte: body.sparte,
        netzebene: None,
        nb_mp_id: body.nb_mp_id,
        steuerkanal,
        eigenschaft_msb_lokation,
        grundzustaendiger_msb_codenr,
        data: canonical_data,
        version: 0,
        updated_at: time::OffsetDateTime::now_utc(),
    };

    match repo.upsert(rec, if_match).await {
        Ok(new_version) => {
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(new_version).parse().unwrap());
            (StatusCode::OK, resp_headers).into_response()
        }
        Err(MdmError::VersionConflict { .. }) => StatusCode::PRECONDITION_FAILED.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/nelo/:id
///
/// Retrieve a single NeLo. Returns a typed `NetzlokationResponse` with the
/// full `rubo4e::current::Netzlokation` BO4E payload in the `data` field.
pub async fn get_nelo(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Path(nelo_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    match repo.find(&nelo_id, &tenant_gln).await {
        Ok(Some(rec)) => {
            let version = rec.version;
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(version).parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(NetzlokationResponse::from_record(rec)),
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/nelo
///
/// List NeLos for this tenant.  Pass `?nb_mp_id=<GLN>` to filter by Netzbetreiber.
pub async fn list_nelos(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<NeLoRepoExt>,
    Query(query): Query<NeLoListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-nelo", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let page_result = if let Some(nb_mp_id) = &query.nb_mp_id {
        repo.list_by_nb(nb_mp_id, &tenant_gln, query.page, query.size)
            .await
    } else {
        repo.list_by_tenant(&tenant_gln, query.page, query.size)
            .await
    };
    match page_result {
        Ok(page) => Json(NetzlokationListResponse::from(page)).into_response(),
        Err(e) => e.into_response(),
    }
}
