//! Contract REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/contracts/:id
//!   GET  /api/v1/contracts/:id

use std::sync::Arc;

use axum::{
    Extension, Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use mako_markt::{
    domain::{MaloId, Sparte},
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRepository, SubscriptionRepository,
    },
};
use mako_service::cedar::CedarEnforcer;
use serde::{Deserialize, Serialize};
use time::macros::format_description;
use utoipa::ToSchema;

use super::{Claims, IntoMdmResponse as _, etag, parse_if_match};

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, ToSchema)]
pub struct ContractUpsertRequest {
    pub malo_id: Option<String>,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    pub vertragsart: String,
    pub data: serde_json::Value,
    /// Start of the contract validity period (ISO 8601, e.g. `"2026-01-01"`).
    ///
    /// Used by the Wechselprozess auto-responder to detect overlapping active
    /// contracts.  Pass `null` for pre-existing records without a known start date.
    #[serde(default)]
    pub valid_from: Option<String>,
    /// End of the contract validity period (ISO 8601).
    ///
    /// `null` means the contract is currently active with no known end date.
    #[serde(default)]
    pub valid_to: Option<String>,
    /// BO4E schema version of `data` (e.g. `"v202607.0.0"`). Defaults to current.
    #[serde(default = "default_bo4e_version")]
    pub bo4e_version: String,
}

fn default_bo4e_version() -> String {
    "v202607.0.0".to_owned()
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ContractResponse {
    pub contract_id: String,
    pub malo_id: Option<String>,
    #[schema(value_type = String, example = "STROM")]
    pub sparte: Sparte,
    pub vertragsart: String,
    pub version: i64,
    pub data: serde_json::Value,
    /// Contract validity start date (ISO 8601) or `null` if not set.
    pub valid_from: Option<String>,
    /// Contract validity end date (ISO 8601) or `null` if open-ended.
    pub valid_to: Option<String>,
}

// ── Handlers ──────────────────────────────────────────────────────────────────

/// `PUT /api/v1/contracts/:id`
#[utoipa::path(
    put,
    path = "/api/v1/contracts/{id}",
    tag = "contracts",
    params(("id" = String, Path, description = "Contract ID (ERP reference)")),
    request_body = ContractUpsertRequest,
    responses(
        (status = 200, description = "Updated"),
        (status = 201, description = "Created"),
        (status = 409, description = "Version conflict"),
    )
)]
pub async fn put_contract<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    headers: HeaderMap,
    claims: Claims,
    Path(id): Path<String>,
    Json(req): Json<ContractUpsertRequest>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    if enforcer
        .check(&claims.principal(), "write-contract", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }

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
    let exists = state.contract_repo.find(&id).await.ok().flatten().is_some();

    // Parse optional ISO-8601 date strings supplied by the caller.
    let date_fmt = format_description!("[year]-[month]-[day]");
    let valid_from = req
        .valid_from
        .as_deref()
        .and_then(|s| time::Date::parse(s, &date_fmt).ok());
    let valid_to = req
        .valid_to
        .as_deref()
        .and_then(|s| time::Date::parse(s, &date_fmt).ok());

    match state
        .contract_repo
        .upsert(
            &id,
            malo_id.as_ref(),
            req.sparte,
            &req.vertragsart,
            req.data,
            valid_from,
            valid_to,
            if_match,
            &req.bo4e_version,
        )
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

/// `GET /api/v1/contracts/:id`
#[utoipa::path(
    get,
    path = "/api/v1/contracts/{id}",
    tag = "contracts",
    params(("id" = String, Path, description = "Contract ID")),
    responses(
        (status = 200, description = "Found", body = ContractResponse),
        (status = 404, description = "Not found"),
    )
)]
pub async fn get_contract<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
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
    if enforcer
        .check(&claims.principal(), "read-contract", &state.tenant_gln)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "access denied",
        }
        .into_response();
    }
    match state.contract_repo.find(&id).await {
        Ok(Some(r)) => {
            let resp = ContractResponse {
                contract_id: r.contract_id,
                malo_id: r.malo_id.map(|id| id.to_string()),
                sparte: r.sparte,
                vertragsart: r.vertragsart,
                version: r.version,
                data: r.data,
                valid_from: r.valid_from.map(|d| d.to_string()),
                valid_to: r.valid_to.map(|d| d.to_string()),
            };
            (
                StatusCode::OK,
                [(axum::http::header::ETAG, etag(r.version))],
                axum::Json(resp),
            )
                .into_response()
        }
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "resource",
            id,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}
