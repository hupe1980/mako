//! Partner REST handlers.
//!
//! Routes:
//!   PUT  /api/v1/partners/:gln
//!   GET  /api/v1/partners/:gln
//!   GET  /api/v1/partners

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use mako_mdm::{
    domain::Gln,
    error::MdmError,
    repository::{
        AppState, ContractRepository, CorrelationIndex, MaloRepository, MeloRepository,
        PartnerRecord, PartnerRepository, SubscriptionRepository,
    },
};
use serde::Deserialize;

use super::{Claims, IntoMdmResponse as _};

#[derive(Debug, Deserialize)]
pub struct PartnerQuery {
    pub marktrolle: Option<String>,
    pub sparte: Option<String>,
}

/// `PUT /api/v1/partners/:gln`
pub async fn put_partner<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(gln_str): Path<String>,
    Json(mut record): Json<PartnerRecord>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    // Validate and parse the path-parameter GLN; override the body's gln field.
    let gln = match gln_str.parse::<Gln>() {
        Ok(g) => g,
        Err(e) => {
            return MdmError::InvalidGln {
                gln: gln_str,
                reason: e.to_string(),
            }
            .into_response();
        }
    };
    record.gln = gln;

    match state.partner_repo.upsert(record).await {
        Ok(version) => Json(serde_json::json!({ "version": version })).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/partners/:gln`
pub async fn get_partner<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Path(gln_str): Path<String>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    let gln = match gln_str.parse::<Gln>() {
        Ok(g) => g,
        Err(e) => {
            return MdmError::InvalidGln {
                gln: gln_str,
                reason: e.to_string(),
            }
            .into_response();
        }
    };

    match state.partner_repo.find(&gln).await {
        Ok(Some(p)) => Json(p).into_response(),
        Ok(None) => mako_mdm::error::MdmError::NotFound {
            resource_type: "resource",
            id: gln_str,
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/partners`
pub async fn list_partners<Ma, Me, Co, Su, Ci, Pa>(
    State(state): State<Arc<AppState<Ma, Me, Co, Su, Ci, Pa>>>,
    _claims: Claims,
    Query(q): Query<PartnerQuery>,
) -> impl IntoResponse
where
    Ma: MaloRepository + Clone,
    Me: MeloRepository + Clone,
    Co: ContractRepository + Clone,
    Su: SubscriptionRepository + Clone,
    Ci: CorrelationIndex + Clone,
    Pa: PartnerRepository + Clone,
{
    match state.partner_repo.list().await {
        Ok(partners) => {
            // Filter in Rust after fetching all (typical deployments have < 1000 partners)
            let filtered: Vec<_> = partners
                .into_iter()
                .filter(|p| {
                    let role_ok = q.marktrolle.as_deref().is_none_or(|r| {
                        p.marktrolle
                            .as_deref()
                            .is_some_and(|mr| mr.eq_ignore_ascii_case(r))
                    });
                    let sparte_ok = q.sparte.as_deref().is_none_or(|s| {
                        p.sparte
                            .is_some_and(|ps| ps.to_string().eq_ignore_ascii_case(s))
                    });
                    role_ok && sparte_ok
                })
                .collect();
            Json(filtered).into_response()
        }
        Err(e) => e.into_response(),
    }
}
