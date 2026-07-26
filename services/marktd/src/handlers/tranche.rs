//! Tranche REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/tranche/{id}   — upsert a Tranche
//!   GET  /api/v1/tranche/{id}   — get a single Tranche
//!   GET  /api/v1/tranche        — list Tranchen (?malo_id=… filters by parent MaLo)
//!
//! A Tranche is a share of a Marktlokation's energy assigned to a distinct
//! balancing responsibility (BO4E `Tranche`; GPKE Teil 4 „Daten der Tranche").
//! Writes require the NB role (same policy as NeLo — network/balancing topology).

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, Query},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    error::MdmError,
    repository::{PageResult, TrancheRecord, TrancheRepository},
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use crate::pg::PgTrancheRepository;

use super::{Claims, IntoMdmResponse as _, TenantGln, etag, parse_if_match};

/// Extension alias — concrete type so AFIT dispatches statically.
pub type TrancheRepoExt = Arc<PgTrancheRepository>;

/// PUT body for a Tranche upsert. The typed columns are indexed/patchable; the
/// full BO4E `Tranche` payload goes in `data`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TrancheUpsertRequest {
    /// Parent Marktlokation this Tranche belongs to.
    #[serde(default)]
    pub malo_id: Option<String>,
    /// Bilanzierungsgebiet EIC (`LOC+237`).
    #[serde(default)]
    pub bilanzierungsgebiet: Option<String>,
    /// Netzebene.
    #[serde(default)]
    pub netzebene: Option<String>,
    /// Energierichtung (`EINSPEISUNG` / `ENTNAHME`).
    #[serde(default)]
    pub energierichtung: Option<String>,
    /// Full BO4E `Tranche` payload (open-ended JSON object).
    #[serde(default)]
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
}

/// GET response.
#[derive(Debug, Serialize, ToSchema)]
pub struct TrancheResponse {
    pub tranche_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub malo_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bilanzierungsgebiet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub netzebene: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub energierichtung: Option<String>,
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    pub version: i64,
    pub updated_at: String,
}

impl TrancheResponse {
    fn from_record(rec: TrancheRecord) -> Self {
        Self {
            tranche_id: rec.tranche_id,
            malo_id: rec.malo_id,
            bilanzierungsgebiet: rec.bilanzierungsgebiet,
            netzebene: rec.netzebene,
            energierichtung: rec.energierichtung,
            data: rec.data,
            version: rec.version,
            updated_at: rec.updated_at.to_string(),
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct TrancheListResponse {
    pub items: Vec<TrancheResponse>,
    pub total: u64,
    pub page: u32,
    pub size: u32,
}

impl From<PageResult<TrancheRecord>> for TrancheListResponse {
    fn from(p: PageResult<TrancheRecord>) -> Self {
        Self {
            items: p
                .items
                .into_iter()
                .map(TrancheResponse::from_record)
                .collect(),
            total: p.total,
            page: p.page,
            size: p.size,
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TrancheListQuery {
    /// Filter by parent Marktlokation.
    #[serde(default)]
    pub malo_id: Option<String>,
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

/// PUT /api/v1/tranche/{id} — insert or update a Tranche (NB role).
pub async fn put_tranche(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<TrancheRepoExt>,
    Path(tranche_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<TrancheUpsertRequest>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "write-tranche", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

    let if_match = parse_if_match(&headers);
    let rec = TrancheRecord {
        tranche_id,
        tenant: tenant_gln,
        malo_id: body.malo_id,
        bilanzierungsgebiet: body.bilanzierungsgebiet,
        netzebene: body.netzebene,
        energierichtung: body.energierichtung,
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

/// GET /api/v1/tranche/{id}
pub async fn get_tranche(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<TrancheRepoExt>,
    Path(tranche_id): Path<String>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-tranche", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    match repo.find(&tranche_id, &tenant_gln).await {
        Ok(Some(rec)) => {
            let version = rec.version;
            let mut resp_headers = HeaderMap::new();
            resp_headers.insert("ETag", etag(version).parse().unwrap());
            (
                StatusCode::OK,
                resp_headers,
                Json(TrancheResponse::from_record(rec)),
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => e.into_response(),
    }
}

/// GET /api/v1/tranche — list Tranchen (`?malo_id=` filters by parent MaLo).
pub async fn list_tranchen(
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Extension(TenantGln(tenant_gln)): Extension<TenantGln>,
    Extension(repo): Extension<TrancheRepoExt>,
    Query(query): Query<TrancheListQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-tranche", &tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    let Some(malo_id) = query.malo_id.as_deref() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "malo_id query parameter is required" })),
        )
            .into_response();
    };
    match repo
        .list_by_malo(malo_id, &tenant_gln, query.page, query.size)
        .await
    {
        Ok(page) => Json(TrancheListResponse::from(page)).into_response(),
        Err(e) => e.into_response(),
    }
}
