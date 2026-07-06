//! MeLo (Messlokation) REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/melo/:id
//!   GET  /api/v1/melo/:id

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_mdm::{
    domain::{MaloId, MeloId},
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRepository, SubscriptionRepository,
    },
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct MeloUpsertRequest {
    /// Associated MaLo-ID (optional).
    pub malo_id: Option<String>,
    /// Full BO4E MESSLOKATION payload.
    pub data: serde_json::Value,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct MeloResponse {
    pub melo_id: String,
    pub malo_id: Option<String>,
    pub version: i64,
    pub data: serde_json::Value,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/melo/:id`
#[utoipa::path(
    put,
    path = "/api/v1/melo/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID (DE + 31 chars)")),
    request_body = MeloUpsertRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 201, description = "Created"),
        (status = 409, description = "Version conflict"),
    )
)]
pub async fn put_melo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    headers: HeaderMap,
    _claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<MeloUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let melo_id = match id.parse::<MeloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMeloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    let malo_id = match req
        .malo_id
        .as_deref()
        .map(|s| {
            s.parse::<MaloId>().map_err(|e| MdmError::InvalidMaloId {
                id: s.to_owned(),
                reason: e.to_string(),
            })
        })
        .transpose()
    {
        Ok(id) => id,
        Err(e) => return e.into_response(),
    };

    let if_match = parse_if_match(&headers);
    let exists = state
        .melo_repo
        .find(&melo_id)
        .await
        .ok()
        .flatten()
        .is_some();

    match state
        .melo_repo
        .upsert(&melo_id, malo_id.as_ref(), req.data, if_match)
        .await
    {
        Ok(version) => {
            let status = if exists {
                StatusCode::OK
            } else {
                StatusCode::CREATED
            };
            (
                status,
                [(axum::http::header::ETAG, etag(version))],
                axum::Json(serde_json::json!({ "version": version })),
            )
                .into_response()
        }
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/melo/:id`
#[utoipa::path(
    get,
    path = "/api/v1/melo/{id}",
    tag = "melo",
    params(("id" = String, Path, description = "MeLo-ID")),
    responses(
        (status = 200, description = "Found", body = MeloResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_melo<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(id): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let melo_id = match id.parse::<MeloId>() {
        Ok(id) => id,
        Err(e) => {
            return MdmError::InvalidMeloId {
                id,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    match state.melo_repo.find(&melo_id).await {
        Ok(Some(r)) => {
            let resp = MeloResponse {
                melo_id: r.melo_id.to_string(),
                malo_id: r.malo_id.map(|id| id.to_string()),
                version: r.version,
                data: r.data,
            };
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag(r.version))],
                axum::Json(resp),
            )
                .into_response()
        }
        Ok(None) => mako_mdm::error::MdmError::NotFound {
            resource_type: "resource",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}
