//! NeLo (Netz-Element-Lokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/nelo/:id           — upsert a NeLo
//!   GET  /api/v1/nelo/:id           — get a single NeLo by ID
//!   GET  /api/v1/nelo               — list NeLos (?nb_mp_id=… filters by Netzbetreiber)
//!
//! NeLos are network element locations used in BDEW Redispatch 2.0 processes.
//! The `nelo_id` is typically a 16-char EIC code (ENTSO-E) or a 13-digit BDEW
//! Codenummer.

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
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::pg::PgNeLoRepository;

use super::{Claims, IntoMdmResponse as _, TenantGln, etag, parse_if_match};

/// Extension alias — concrete type so AFIT dispatches statically.
pub type NeLoRepoExt = Arc<PgNeLoRepository>;

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct NeLoUpsertRequest {
    /// Human-readable Bezeichnung (optional).
    pub name: Option<String>,
    /// `STROM` or `GAS`.
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    /// Voltage / pressure level: `NS`, `MS`, `MSP`, `HSP`, `HS`, `HöS`, or `HöS/HS`.
    pub netzebene: Option<String>,
    /// Owning Netzbetreiber GLN.
    pub nb_mp_id: String,
    /// `true` if this NeLo can be remote-controlled (Redispatch 2.0 B6).
    #[serde(default)]
    pub steuerkanal: Option<bool>,
    /// gMSB Marktrolle (`"NB"`, `"MSB"`, …) — `eigenschaftMsbLokation` in BO4E.
    #[serde(default)]
    pub eigenschaft_msb_lokation: Option<String>,
    /// gMSB MP-ID — `grundzustaendigerMsbCodenr` in BO4E.
    #[serde(default)]
    pub grundzustaendiger_msb_codenr: Option<String>,
    /// Additional Redispatch 2.0 attributes (arbitrary JSON object).
    #[serde(default = "empty_object")]
    pub data: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NeLoResponse {
    pub nelo_id: String,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: String,
    pub name: Option<String>,
    pub netzebene: Option<String>,
    pub nb_mp_id: String,
    /// Redispatch 2.0 remote-control flag (B6).
    pub steuerkanal: Option<bool>,
    /// gMSB Marktrolle (B6).
    pub eigenschaft_msb_lokation: Option<String>,
    /// gMSB MP-ID (B6).
    pub grundzustaendiger_msb_codenr: Option<String>,
    pub data: serde_json::Value,
    pub version: i64,
    pub updated_at: String,
}

impl From<NeLoRecord> for NeLoResponse {
    fn from(r: NeLoRecord) -> Self {
        Self {
            nelo_id: r.nelo_id,
            sparte: r.sparte.to_string(),
            name: r.name,
            netzebene: r.netzebene,
            nb_mp_id: r.nb_mp_id,
            steuerkanal: r.steuerkanal,
            eigenschaft_msb_lokation: r.eigenschaft_msb_lokation,
            grundzustaendiger_msb_codenr: r.grundzustaendiger_msb_codenr,
            data: r.data,
            version: r.version,
            updated_at: r.updated_at.to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct NeLoListResponse {
    pub items: Vec<NeLoResponse>,
    pub total: u64,
    pub page: u32,
    pub size: u32,
}

impl From<PageResult<NeLoRecord>> for NeLoListResponse {
    fn from(p: PageResult<NeLoRecord>) -> Self {
        Self {
            items: p.items.into_iter().map(NeLoResponse::from).collect(),
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
/// Supply an `If-Match` header with the current ETag version for optimistic
/// concurrency; omit for unconditional upsert (first write).
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
    let if_match = parse_if_match(&headers);
    let rec = NeLoRecord {
        nelo_id,
        tenant: tenant_gln,
        name: body.name,
        sparte: body.sparte,
        netzebene: body.netzebene,
        nb_mp_id: body.nb_mp_id,
        steuerkanal: body.steuerkanal,
        eigenschaft_msb_lokation: body.eigenschaft_msb_lokation,
        grundzustaendiger_msb_codenr: body.grundzustaendiger_msb_codenr,
        data: body.data,
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
/// Retrieve a single NeLo by its EIC or BDEW Codenummer identifier.
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
            (StatusCode::OK, resp_headers, Json(NeLoResponse::from(rec))).into_response()
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
        Ok(page) => Json(NeLoListResponse::from(page)).into_response(),
        Err(e) => e.into_response(),
    }
}
