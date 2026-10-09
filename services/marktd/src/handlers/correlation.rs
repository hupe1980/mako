//! Process correlation REST handlers.
//!
//! Routes:
//!   GET /api/v1/correlations/:process_id
//!   GET /api/v1/correlations            (query by erp_order_id, malo_id, status)

use crate::state::AppState;
use std::sync::Arc;

use axum::{
    Extension,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use mako_markt::{
    domain::{MaloId, ProcessStatus},
    error::MdmError,
    repository::{CorrelationFilter, CorrelationIndex},
};
use serde::Deserialize;
use uuid::Uuid;

use mako_service::cedar::CedarEnforcer;

use super::{Claims, IntoMdmResponse as _};

#[derive(Debug, Deserialize)]
pub struct CorrelationQuery {
    pub erp_order_id: Option<String>,
    pub malo_id: Option<String>,
    pub status: Option<String>, // parsed to ProcessStatus below
}

/// `GET /api/v1/correlations/:process_id`
pub async fn get_correlation(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-correlation", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "read-correlation denied",
        }
        .into_response();
    }
    match state.correlation_index.find_by_process_id(id).await {
        Ok(Some(e)) => axum::Json(e).into_response(),
        Ok(None) => mako_markt::error::MdmError::NotFound {
            resource_type: "resource",
            id: id.to_string(),
        }
        .into_response(),
        Err(e) => e.into_response(),
    }
}

/// `GET /api/v1/correlations`
pub async fn list_correlations(
    State(state): State<Arc<AppState>>,
    Extension(enforcer): Extension<Arc<CedarEnforcer>>,
    claims: Claims,
    Query(q): Query<CorrelationQuery>,
) -> impl IntoResponse {
    if enforcer
        .check(&claims.principal(), "read-correlation", &state.tenant)
        .is_err()
    {
        return MdmError::Forbidden {
            reason: "read-correlation denied",
        }
        .into_response();
    }
    if q.erp_order_id.is_none() && q.malo_id.is_none() && q.status.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            axum::Json(serde_json::json!({
                "error": "UNPROCESSABLE",
                "message": "At least one query parameter (erp_order_id, malo_id, status) is required"
            })),
        )
            .into_response();
    }

    let malo_id_filter = match q.malo_id {
        Some(s) => match s.parse::<MaloId>() {
            Ok(id) => Some(id),
            Err(e) => {
                return MdmError::InvalidMaloId {
                    id: s,
                    reason: e.to_string(),
                }
                .into_response();
            }
        },
        None => None,
    };
    let filter = CorrelationFilter {
        erp_order_id: q.erp_order_id,
        malo_id: malo_id_filter,
        status: q
            .status
            .as_deref()
            .and_then(|s| s.parse::<ProcessStatus>().ok()),
    };

    match state.correlation_index.list(filter).await {
        Ok(entries) => axum::Json(entries).into_response(),
        Err(e) => e.into_response(),
    }
}
